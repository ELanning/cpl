//! Structured parallel maps. Only flat owned values cross threads; capability hosts stay on their owner.
use super::{Binding, Env, Interpreter};
use crate::interp::eval::{Eval, Flow};
use crate::run::Portable;
use crate::{CapabilityImpl, Fault, HandleId, HostContext, Value};
use cpl_hir::{Expr, LocalId, Stmt, Task};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::sync::{Arc, OnceLock};

static LIMIT: OnceLock<usize> = OnceLock::new();
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

fn limit() -> usize {
    *LIMIT.get_or_init(|| {
        std::env::var("CPL_PARALLELISM")
            .ok()
            .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| (1..=64).contains(n))
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(8)))
    })
}

struct Permits(usize);
impl Permits {
    fn acquire(wanted: usize) -> Self {
        let max = limit().saturating_sub(1);
        let mut current = ACTIVE.load(Ordering::Relaxed);
        loop {
            let n = wanted.min(max.saturating_sub(current));
            match ACTIVE.compare_exchange_weak(
                current,
                current + n,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Self(n),
                Err(actual) => current = actual,
            }
        }
    }
}
impl Drop for Permits {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(self.0, Ordering::Release);
    }
}

impl<'p> Interpreter<'p> {
    pub(super) fn eval_parallel(
        &mut self,
        binder: LocalId,
        collection: &'p Expr,
        body: &'p Expr,
    ) -> Eval<Value> {
        let items = self.eval_expr(collection)?;
        let Value::List(items) = &items else {
            return self.confusion("a list", &items, collection.span);
        };
        let permits = Permits::acquire(if self.parallel_depth == 0 {
            items.len().saturating_sub(1)
        } else {
            0
        });
        if permits.0 == 0 {
            let mut values = Vec::with_capacity(items.len());
            self.parallel_depth += 1;
            for item in items.iter() {
                if let Err(flow) = self.check_limits(Some(body.span)) {
                    self.parallel_depth -= 1;
                    return Err(flow);
                }
                let mark = self.scope_mark();
                self.bind(binder, item.clone());
                let result = self.eval_expr(body);
                self.restore_scope(mark);
                match result {
                    Ok(value) => values.push(value),
                    Err(flow) => {
                        self.parallel_depth -= 1;
                        return Err(flow);
                    }
                }
            }
            self.parallel_depth -= 1;
            return Ok(Value::list(values));
        }
        let workers = permits.0 + 1;
        let chunk = items.len().div_ceil(workers);
        let inputs: Vec<Portable> = items
            .chunks(chunk)
            .map(|xs| Portable::from(&Value::list(xs.to_vec())))
            .collect();
        let env = self.env();
        let captures = Portable::from(&Value::list(
            env.values
                .iter()
                .map(|v| v.clone().unwrap_or(Value::Bool(false)))
                .collect(),
        ));
        let present: Vec<bool> = env.values.iter().map(Option::is_some).collect();
        let table = env.table;
        let scope = env.scope.clone();
        let program = self.program;
        let config = self.config.clone();
        let frames = self.frames.clone();
        let measures = self.measures.clone();
        let calls = self.calls;
        let deadline = self.deadline;
        let stop = Arc::new(AtomicUsize::new(usize::MAX));
        let steps = self.steps.clone();
        let run = |part: usize, input: &Portable| {
            let mut child = Interpreter::new(program, config.clone());
            child.steps = steps.clone();
            child.parallel_depth = 1;
            child.frames = frames.clone();
            child.measures = measures.clone();
            child.calls = calls;
            child.deadline = deadline;
            let values = Value::from(captures.clone());
            let Value::List(values) = &values else {
                unreachable!()
            };
            child.envs.push(Env {
                table,
                scope: scope.clone(),
                values: values
                    .iter()
                    .zip(&present)
                    .map(|(v, present)| present.then(|| v.clone()))
                    .collect(),
            });
            let input = Value::from(input.clone());
            let Value::List(input) = &input else {
                unreachable!()
            };
            let result: Result<Portable, Fault> = (|| {
                let mut output = Vec::with_capacity(input.len());
                for (offset, item) in input.iter().enumerate() {
                    let index = part * chunk + offset;
                    child.parallel_cancel = Some((stop.clone(), index));
                    if let Err(flow) = child.check_limits(Some(body.span)) {
                        stop.fetch_min(index, Ordering::AcqRel);
                        return Err(flow.into_fault());
                    }
                    let mark = child.scope_mark();
                    child.bind(binder, item.clone());
                    let result = child.eval_expr(body);
                    child.restore_scope(mark);
                    match result {
                        Ok(value) => output.push(value),
                        Err(flow) => {
                            stop.fetch_min(index, Ordering::AcqRel);
                            return Err(flow.into_fault());
                        }
                    }
                }
                Ok(Portable::from(&Value::list(output)))
            })();
            (result, child.attributed, child.violation)
        };
        let results = std::thread::scope(|s| {
            let mut jobs = Vec::new();
            for (part, input) in inputs[..inputs.len() - 1].iter().enumerate() {
                let run = &run;
                let handle = std::thread::Builder::new()
                    .name("cpl-parallel".into())
                    .stack_size(crate::stack::EVAL_STACK_SIZE)
                    .spawn_scoped(s, move || {
                        crate::stack::set_guard(crate::stack::EVAL_STACK_SIZE);
                        run(part, input)
                    });
                jobs.push(match handle {
                    Ok(h) => Ok(h),
                    Err(_) => Err(run(part, input)),
                });
            }
            let last = run(inputs.len() - 1, inputs.last().unwrap());
            let mut results = Vec::with_capacity(inputs.len());
            for job in jobs {
                results.push(match job {
                    Ok(h) => h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)),
                    Err(result) => result,
                });
            }
            results.push(last);
            results
        });
        let mut output = Vec::with_capacity(items.len());
        for (result, attributed, violation) in results {
            match result {
                Ok(value) => {
                    let value = Value::from(value);
                    let Value::List(xs) = &value else {
                        unreachable!()
                    };
                    output.extend(xs.iter().cloned());
                }
                Err(fault) => {
                    self.attributed = attributed;
                    self.violation = violation;
                    return Err(Flow::Fault(Box::new(fault)));
                }
            }
        }
        Ok(Value::list(output))
    }
}

/// A fork shared by the worker that calls it and the owner that joins it.
type Shared = Arc<Mutex<Option<Box<dyn CapabilityImpl + Send>>>>;

/// A worker's view of a forked handle, by its handle number.
struct ForkedHandle(Shared, u32);
impl CapabilityImpl for ForkedHandle {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_mut() {
            Some(imp) => imp.call(op, args, ctx),
            None => Err(unbound(op, self.1)),
        }
    }
}

/// A handle outside a task's footprint, which the task therefore never calls: in the worker it
/// is not bound.
struct Unforked(u32);
impl CapabilityImpl for Unforked {
    fn call(
        &mut self,
        op: &str,
        _args: Vec<Value>,
        _ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        Err(unbound(op, self.0))
    }
}

fn unbound(op: &str, handle: u32) -> Fault {
    Fault::internal(
        "unbound_capability",
        cpl_rtspec::messages::render(
            &cpl_rtspec::messages::DETAIL_UNBOUND_CAPABILITY,
            &[("operation", op), ("handle", &handle.to_string())],
        ),
    )
}

impl<'p> Interpreter<'p> {
    /// Overlap the tasks of a `concurrent` block on worker threads (`spec/concurrency.md`): when
    /// at least two of them perform effects, no two reach one resource with an operation that is
    /// not read-only on both sides, and every handle they use forks. `None`: run them in order.
    /// Every task runs to its end; their effect records and forks join in task order, and the
    /// lowest-index fault escapes, so the outcome is that of some schedule of independent tasks.
    pub(super) fn exec_tasks_parallel(&mut self, tasks: &'p [Task]) -> Option<Eval<()>> {
        if tasks.len() < 2 || self.parallel_depth > 0 || limit() < 2 {
            return None;
        }
        let p = self.program;
        // Each task's footprint as resources: a host identity when the host reports one.
        let mut identities: Vec<Option<String>> = vec![None; self.handles.len()];
        let mut asked = vec![false; self.handles.len()];
        let mut prints: Vec<Vec<(HandleId, String, bool)>> = Vec::with_capacity(tasks.len());
        for task in tasks {
            let mut print = Vec::new();
            for effect in p.footprint(&task.stmt) {
                let handle = match self.env().values.get(effect.param.0 as usize) {
                    Some(Some(Value::Capability(h))) => *h,
                    _ => return None,
                };
                let index = handle.0 as usize;
                let binding = self.handles.get_mut(index)?;
                if !asked[index] {
                    asked[index] = true;
                    identities[index] = binding.imp.resource();
                }
                let key = identities[index]
                    .clone()
                    .unwrap_or_else(|| format!("handle {index}"));
                print.push((handle, key, p.read_only(binding.capability, &effect.op)));
            }
            prints.push(print);
        }
        if prints.iter().filter(|f| !f.is_empty()).count() < 2 {
            return None;
        }
        for (i, a) in prints.iter().enumerate() {
            for b in &prints[i + 1..] {
                if a.iter()
                    .any(|x| b.iter().any(|y| x.1 == y.1 && !(x.2 && y.2)))
                {
                    return None;
                }
            }
        }
        let permits = Permits::acquire(tasks.len() - 1);
        if permits.0 + 1 < tasks.len() {
            return None;
        }
        let mut forks: Vec<Vec<(HandleId, Shared)>> = Vec::with_capacity(tasks.len());
        for print in &prints {
            let mut handles: Vec<HandleId> = print.iter().map(|(h, _, _)| *h).collect();
            handles.sort_by_key(|h| h.0);
            handles.dedup();
            let mut task_forks = Vec::with_capacity(handles.len());
            for h in handles {
                let fork = self.handles[h.0 as usize].imp.fork()?;
                task_forks.push((h, Arc::new(Mutex::new(Some(fork)))));
            }
            forks.push(task_forks);
        }
        let env = self.env();
        let captures = Portable::from(&Value::list(
            env.values
                .iter()
                .map(|v| v.clone().unwrap_or(Value::Bool(false)))
                .collect(),
        ));
        let present: Vec<bool> = env.values.iter().map(Option::is_some).collect();
        let table = env.table;
        let scope = env.scope.clone();
        let config = self.config.clone();
        let frames = self.frames.clone();
        let measures = self.measures.clone();
        let calls = self.calls;
        let deadline = self.deadline;
        let bindings: Vec<(cpl_hir::DefId, String, bool)> = self
            .handles
            .iter()
            .map(|b| (b.capability, b.label.clone(), b.validate))
            .collect();
        let steps = self.steps.clone();
        let run = |task: &'p Task, forks: &[(HandleId, Shared)]| {
            let mut child = Interpreter::new(p, config.clone());
            child.steps = steps.clone();
            child.parallel_depth = 1;
            child.frames = frames.clone();
            child.measures = measures.clone();
            child.calls = calls;
            child.deadline = deadline;
            child.handles = bindings
                .iter()
                .enumerate()
                .map(|(i, (capability, label, validate))| Binding {
                    capability: *capability,
                    label: label.clone(),
                    validate: *validate,
                    imp: match forks.iter().find(|(h, _)| h.0 as usize == i) {
                        Some((_, shared)) => Box::new(ForkedHandle(shared.clone(), i as u32)),
                        None => Box::new(Unforked(i as u32)),
                    },
                })
                .collect();
            let values = Value::from(captures.clone());
            let Value::List(values) = &values else {
                unreachable!()
            };
            child.envs.push(Env {
                table,
                scope: scope.clone(),
                values: values
                    .iter()
                    .zip(&present)
                    .map(|(v, present)| present.then(|| v.clone()))
                    .collect(),
            });
            let result = match &task.stmt {
                Stmt::Let { value, .. } => child.eval_expr(value).map(|v| Some(Portable::from(&v))),
                other => child.exec_stmt(other).map(|()| None),
            }
            .map_err(Flow::into_fault);
            let effects = std::mem::take(&mut child.effects);
            (result, child.attributed, child.violation, effects)
        };
        let results = std::thread::scope(|s| {
            let jobs: Vec<_> = tasks
                .iter()
                .zip(&forks)
                .map(|(task, forks)| {
                    let run = &run;
                    std::thread::Builder::new()
                        .name("cpl-task".into())
                        .stack_size(crate::stack::EVAL_STACK_SIZE)
                        .spawn_scoped(s, move || {
                            crate::stack::set_guard(crate::stack::EVAL_STACK_SIZE);
                            run(task, forks)
                        })
                })
                .collect();
            jobs.into_iter()
                .zip(tasks.iter().zip(&forks))
                .map(|(job, (task, forks))| match job {
                    Ok(handle) => handle
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
                    Err(_) => run(task, forks),
                })
                .collect::<Vec<_>>()
        });
        drop(permits);
        let mut fault = None;
        let mut bound = Vec::new();
        for ((result, attributed, violation, effects), (task, task_forks)) in
            results.into_iter().zip(tasks.iter().zip(&forks))
        {
            self.effects.extend(effects);
            for (h, shared) in task_forks {
                if let Some(f) = shared.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    self.handles[h.0 as usize].imp.join(f);
                }
            }
            match result {
                Ok(Some(value)) => {
                    if let Stmt::Let { local, .. } = &task.stmt {
                        bound.push((local, Value::from(value)));
                    }
                }
                Ok(None) => {}
                Err(f) => {
                    fault.get_or_insert((f, attributed, violation));
                }
            }
        }
        if let Some((f, attributed, violation)) = fault {
            self.attributed = attributed;
            self.violation = violation;
            return Some(Err(Flow::Fault(Box::new(f))));
        }
        for (local, value) in bound {
            self.bind(*local, value);
        }
        Some(Ok(()))
    }
}
