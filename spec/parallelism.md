# Structured parallelism and performance — 2026-11-01

This amendment extends `state-machines.md` and supersedes the older
statements that application task syntax is deferred and the native runtime
never starts threads. Existing source remains valid. It does not change
the public C API table or introduce shared mutable language values.

## Language syntax

```cpl
pure fn transform(items: List<Int>, offset: Int) -> List<Int> {
    parallel for item in items: expensive(item) + offset
}
```

`parallel for NAME in EXPRESSION: EXPRESSION` is an expression of type
`List<T>`, where the collection is a list and the task body produces `T`.
The collection evaluates once on the calling task. Results appear in
input order, including nested parallel maps. An empty input produces an
empty output. Expected result types flow into the body, including generic
types and empty list literals. As with `forall`, parenthesize a parallel
expression when using it as an operand of another expression.

`parallel` is contextual: `fn parallel()` and ordinary variables with
that name still work. The AST and HIR have a distinct `Parallel` variant;
tooling identifies its binder as `parallel`. Formatting, structural
comparison, reference traversal, semantic facets, and coverage include it.

Task bodies must be pure, including their transitive call graph. They
cannot call capabilities, pass capability handles, reach a mutator,
assign variables, or return/propagate out of the task expression. Use a
pure helper for private loops or rebinding. Each worker receives an
immutable capture snapshot. The checker reports `parallel_unsafe` for
violations and `parallel_range_not_list` for invalid collections. The
collection itself runs under the caller's ordinary effect rules.

Pure functions, initializers, and reducers can use parallel expressions.
Reducers still return one candidate state and command list; the owning
context commits only after the complete transition validates. Parallel
work does not grant authority to change an application root.

## Structured execution and cancellation

Both engines execute independent inputs on bounded native threads. The
interpreter transfers flat, owned value graphs between threads, preserving
sharing within each graph without sending `Rc` values or capability hosts.
The native engine shares atomically reference-counted immutable values and
writes disjoint result slots. Every worker finishes before captures or
results leave the group. There are no detached language tasks.

Configure `CPL_PARALLELISM=1..64` before starting the process. The default
uses available processors, capped at eight; invalid values use that
default. A value of one executes maps sequentially. The setting is read
once per runtime implementation/library copy. At most `N-1` additional
workers run across its active groups, in addition to existing host calling
threads. Nested groups execute on their existing worker, preventing
recursive thread growth and worker starvation. If thread creation fails,
that chunk runs on the caller. Small jobs may cost more to schedule than
they save; batching substantial work is preferable.

Task scopes inherit the caller's absolute deadline, active call depth,
contract policy, and recursive `decreases` context. A task cannot reset
these limits. Worker stacks have the normal evaluation guard. Native
workers also observe the host cancellation token.

The lowest failing input index determines the reported language fault.
Once a task fails, later inputs are cooperatively cancelled at function,
loop, or task checkpoints; lower-index inputs finish so scheduling cannot
change which fault wins. All workers join before a fault escapes. Source
locations, contract attribution, and language frames remain available.
Long primitive operations are not forcibly interrupted mid-operation.
An earlier nonterminating task still requires a configured deadline;
purity is not a termination proof. Time and memory exhaustion are resource
outcomes, not schedule-independent semantic results.

## Concurrent application contexts

`cpl_eval::concurrent::with_machine` provides a structured machine worker
with one private root snapshot and a bounded mailbox. Its callback receives
a cloneable `MachineMailbox`; multiple producers can submit events with
`try_dispatch`. A full mailbox returns `SubmitError::Full` with the original
event, allowing explicit backpressure. A closed mailbox likewise returns
the event. Capacity must be positive.

An accepted event has a `TransitionReply` with blocking `wait` and
nonblocking `try_recv`. A successful reply contains the committed
`TransitionRecord`; an invalid or faulting event leaves state and sequence
unchanged. Receive order fixes the committed sequence. Dropping a reply
does not cancel an accepted event. The scope drains events preceding its
stop message and joins its worker on normal or panic exit. Escaped mailbox
clones become closed when the scope ends. Configure an evaluation deadline
when shutdown must have a time bound.

This API currently hosts the reference evaluator. Native hosts can drive
independent contexts on their existing threads. Command handlers remain
host work: dispatch resulting events through the owning mailbox. There
is no language `async`/`await`, asynchronous I/O scheduler, detached spawn,
or shared-memory channel value in this amendment. Concurrent pure work
and serialized application state have explicit first-class language and
embedding support; effect delivery, persistence, and outboxes remain host
responsibilities.

## Optimization passes and preserved behavior

Optimized native builds enable the following lowering passes:

1. Bounded constant folding of integer, Boolean, and text operations.
2. Short-circuit folding and constant-branch elimination.
3. Algebraic identity reduction that preserves evaluation of all
   potentially faulting operands (`x * 0` still evaluates `x`).
4. Elimination of concatenation with literal empty text/lists.
5. Inline small-integer addition, subtraction, multiplication, division,
   remainder, and comparisons, with arbitrary-precision fallback on overflow
   and the ordinary fault path for a zero divisor.
6. Ownership-aware `xs = xs.push(item)`: geometric buffer growth when
   uniquely owned and copying when another snapshot shares the list.
7. Reuse of list, text, and byte capacity for `x = x ++ rhs`, preserving
   snapshots and left-to-right evaluation when the right operand mutates locals.

Folding uses a 256-node work budget per attempt, a 4,096-bit integer
operand budget, and a 65,536-byte text bound. Division by zero remains a
runtime fault; folding does not execute user functions or discharge
constructor/contract obligations. Contracts, constructor invariants,
foreign boundary checks, source spans, cancellation, and loop budgets
remain enabled. The coverage emitter retains every original HIR node.

`EmitConfig::optimize` controls these passes. Package/driver builders set
it for the optimized profile, including sanitizer builds; coverage disables
it. `cpl build --profile debug` retains the straightforward lowering.
Native `run`, `test`, and `replay` now use cached optimized drivers; the
reference evaluator remains the default engine.

The evaluator also reuses uniquely owned local list, text, and byte capacity.
Its eight-entry Unicode-length cache uses weak buffer identities, retains no
string contents, and carries known counts forward through text appends.
Shared snapshots remain immutable. The public payloads are `Rc<Vec<Value>>`
for `Value::List`, `Rc<String>` for `Value::Text`, and `Rc<Vec<u8>>` for
`Value::Bytes`; embedders should use the `Value` constructors and accessors
rather than depending on the previous slice representations. Deep value
teardown remains iterative.

Machine results now pass the existing iterative structural/boundary walk
directly, avoiding a JSON encode/decode cycle before validation. Receipts
still serialize full snapshots. Compiler AST lookup uses binary search in
the documented FileId-sorted table, and parallel type checking avoids an
unnecessary body clone. These implementation improvements apply in both
build profiles.

See [the example](../examples/parallel/README.md) and
[the performance and validation report](performance-audit.md).
