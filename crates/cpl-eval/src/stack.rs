//! The evaluation thread (semantics S38; `spec/toolchain.md` 1.7.1).
//!
//! The interpreter is a tree walker: every CPL activation costs several
//! host stack frames. The default call-depth limit
//! (`[run] max_call_depth = 10000`, S38) is far beyond what a default
//! thread stack holds, and S38 requires exceeding the limit to be the
//! fault `resource_exhaustion` (`resource: "stack"`), never a crash. So
//! [`crate::run_function`], [`crate::run_tests`], and [`crate::run_test`]
//! evaluate on a dedicated thread with a large stack ([`on_eval_stack`];
//! the reservation is virtual memory, committed only as it is touched).
//!
//! Values are different: a loop builds a value nested deeper than any
//! stack without a single nested call, and a result leaves the evaluation
//! thread (a [`crate::Value`] is not `Send`, so the caller cannot hand it
//! to a large stack). So what a host does with a result does not recurse
//! with its nesting depth: dropping a value, copying a result back from
//! the evaluation thread, value JSON and value text, and structural
//! equality run on explicit work lists, and a result of any depth is safe
//! on the caller's stack. The `serde_json` trees of value JSON (a fault's
//! `input` and `actual`, a test record's payloads) are another matter:
//! `serde_json`'s own destructor and serializer recurse with their depth.
//! They are `Send`, so a host that may meet deep ones serializes and drops
//! them inside [`on_eval_stack`].
//!
//! On that thread a second guard backs up the explicit depth counter: an
//! activation that would start within [`GUARD_MARGIN`] bytes of the end of
//! the stack is refused with the same `resource_exhaustion` fault, so even
//! a `--max-call-depth` larger than the stack can hold ends in a fault.
//! [`crate::Interpreter`] methods called directly on any other thread run
//! on that thread's stack with the depth counter as their only guard.

use std::cell::Cell;
use std::sync::Mutex;

/// The stack reserved for the evaluation thread: 1 GiB (with smaller
/// fallbacks when the host refuses the reservation), the size the native
/// engine's evaluation thread reserves too (`cpl_rtspec::limits`, T70;
/// `spec/cabi.md` 3.1).
pub const EVAL_STACK_SIZE: usize = cpl_rtspec::limits::EVAL_STACK_SIZE;

/// Bytes kept free below the guard for the work between two guard checks
/// (one activation's frames, fault construction, value encoding): 64 MiB
/// (`cpl_rtspec::limits`, T70).
pub const GUARD_MARGIN: usize = cpl_rtspec::limits::GUARD_MARGIN;

/// The smaller reservations tried when the host refuses a larger one; the
/// last resort is the current thread.
const FALLBACKS: [usize; 2] = [256 << 20, 64 << 20];

thread_local! {
    /// The lowest stack address an activation may start at on this thread;
    /// 0 when the thread is not an evaluation thread (no guard).
    static FLOOR: Cell<usize> = const { Cell::new(0) };
}

/// An address inside the current stack frame.
#[inline(always)]
fn here() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}

/// True when the current evaluation thread is within [`GUARD_MARGIN`] (or
/// an eighth of a smaller stack) of the end of its stack. Always false on a
/// thread that [`on_eval_stack`] did not start, and on a platform whose
/// stack grows upward (the comparison never holds there).
#[inline]
pub(crate) fn nearly_exhausted() -> bool {
    let floor = FLOOR.with(Cell::get);
    floor != 0 && here() < floor
}

/// Run `f` on a fresh thread whose stack is [`EVAL_STACK_SIZE`] bytes and
/// return its result; a panic in `f` resumes on the calling thread. When
/// the host refuses every reservation (a hard address-space limit), `f`
/// runs on the calling thread instead. On a thread that is already an
/// evaluation thread, `f` runs in place under that thread's guard.
pub fn on_eval_stack<R, F>(f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    // Already on an evaluation stack (a tool evaluating in a loop, a nested run): its guard
    // protects this evaluation too, so a new thread and stack would only add their cost.
    if on_evaluation_thread() {
        return f();
    }
    on_eval_stack_sized(EVAL_STACK_SIZE, f)
}

/// Whether the current thread is an evaluation thread with its stack guard set.
pub fn on_evaluation_thread() -> bool {
    FLOOR.with(|floor| floor.get() != 0)
}

/// [`on_eval_stack`] with a stack of `size` bytes (at least 1 MiB), for
/// hosts that bound their memory: the guard then ends deep recursion
/// within that stack, whatever the configured call-depth limit.
pub fn on_eval_stack_sized<R, F>(size: usize, f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    let size = size.max(1 << 20);
    let slot = Mutex::new(Some(f));
    for size in stack_sizes(size) {
        let outcome = std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new()
                .name("cpl-eval".to_string())
                .stack_size(size)
                .spawn_scoped(scope, || {
                    set_guard(size);
                    let f = slot
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take()
                        .expect("the evaluation closure runs once");
                    f()
                });
            spawned.ok().map(|handle| handle.join())
        });
        match outcome {
            Some(Ok(result)) => return result,
            Some(Err(panic)) => std::panic::resume_unwind(panic),
            None => continue,
        }
    }
    let f = slot
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .expect("the evaluation closure was not consumed");
    f()
}

/// The stack sizes tried for an evaluation thread of `size` bytes: `size`,
/// then the smaller fallbacks.
fn stack_sizes(size: usize) -> impl Iterator<Item = usize> {
    std::iter::once(size).chain(FALLBACKS.into_iter().filter(move |&s| s < size))
}

/// Arm the guard of the current thread, an evaluation thread whose stack is
/// `size` bytes and which is at its outermost frame.
pub(crate) fn set_guard(size: usize) {
    let margin = GUARD_MARGIN.min(size / 8);
    FLOOR.with(|c| c.set(here().saturating_sub(size - margin)));
}

/// [`on_eval_stack`] for an evaluation that needs things only the calling
/// thread may touch (values that are not `Send`, such as the capability
/// hosts of `cpl run`, `crate::run::run_function_with_hosts`): `f` runs on
/// the evaluation thread and calls the function it is given with a request;
/// the calling thread answers every request with `serve`, in order, while
/// it waits for `f` to finish. When no evaluation thread can be started,
/// `f` runs on the calling thread and calls `serve` directly.
///
/// A panic in `f` resumes on the calling thread; a panic in `serve` ends
/// the evaluation thread (its pending request is never answered) and then
/// resumes on the calling thread.
pub(crate) fn on_eval_stack_serving<R, Q, A, F>(f: F, serve: &mut dyn FnMut(Q) -> A) -> R
where
    F: FnOnce(&mut dyn FnMut(Q) -> A) -> R + Send,
    R: Send,
    Q: Send,
    A: Send,
{
    if on_evaluation_thread() {
        // The hosts already live on this evaluation thread: serve each request in place.
        return f(&mut |request| serve(request));
    }
    let slot = Mutex::new(Some(f));
    let slot = &slot;
    for size in stack_sizes(EVAL_STACK_SIZE) {
        let serve: &mut dyn FnMut(Q) -> A = &mut *serve;
        let (requests, received) = std::sync::mpsc::channel::<Q>();
        let (answers, answered) = std::sync::mpsc::channel::<A>();
        let outcome = std::thread::scope(move |scope| {
            let spawned = std::thread::Builder::new()
                .name("cpl-eval".to_string())
                .stack_size(size)
                .spawn_scoped(scope, move || {
                    set_guard(size);
                    let f = slot
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take()
                        .expect("the evaluation closure runs once");
                    let mut ask = |request: Q| -> A {
                        requests
                            .send(request)
                            .expect("the calling thread serves the evaluation's requests");
                        answered
                            .recv()
                            .expect("the calling thread answers every request")
                    };
                    f(&mut ask)
                });
            let handle = spawned.ok()?;
            // Ends when the evaluation thread drops `requests`: when `f` has
            // returned, or when it panicked.
            for request in received.iter() {
                if answers.send(serve(request)).is_err() {
                    break;
                }
            }
            Some(handle.join())
        });
        match outcome {
            Some(Ok(result)) => return result,
            Some(Err(panic)) => std::panic::resume_unwind(panic),
            None => continue,
        }
    }
    let f = slot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .expect("the evaluation closure was not consumed");
    f(serve)
}
