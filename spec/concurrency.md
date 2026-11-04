# Concurrent task blocks — 2026-11-03

This amendment adds effectful structured concurrency to the language. It
extends `parallelism.md`, whose `parallel for` maps remain pure, and
`evolution.md`, whose `cpl concurrent` scope documents schedule whole
functions from the toolchain. Existing source remains valid: `concurrent`
is contextual.

## Syntax

```cpl
@read_only("get")
interface Store {
    fn get(key: Text) -> Int
    fn put(key: Text, value: Int) -> Unit
}

mutate fn profile(id: Text, using users: Store, using settings: Store) -> Int
    effects { users.get, settings.get }
{
    concurrent {
        let user = users.get(id)
        let theme = settings.get(id)
    }
    user + theme
}
```

`concurrent { lines }` is a statement (grammar 3.11). Each line is a
*task*: a `let` (a name or `_`) or an expression statement of type `Unit`.
Another statement as a task is `concurrent_task_invalid`. Every task
resolves in the scope before the block: a task cannot read a binding of a
sibling task (`unresolved_name`, with a note naming the sibling), and two
tasks cannot bind one name (`shadowed_binding`). After the block, every
task's binding is in scope. Formatting, structural comparison, the
semantic index, facets, holes, and coverage include the statement; the
AST, HIR, and dumps have a distinct `Concurrent` variant.

## Independence rules

The checker makes the result of a block independent of the order of its
tasks, so that a runtime may overlap them:

| Rule | Diagnostic |
| - | - |
| a task assigns a variable declared outside it, contains `return`, or uses `?` | `concurrent_unsafe` |
| two tasks' footprints share a `using` handle with an operation that is not read-only | `concurrent_conflict` |
| an `@read_only` argument names no operation of its capability | `read_only_unknown_operation` |

A task's *footprint* is the set of handle operations it may perform: its
operation calls `h.op(...)` and, for each call `g(..., using h1, ...)`, the
declared effect bound of `g` substituted to the caller's handles (S28), or
its inferred effects when `g` declares no bound. Contract clauses perform
no effects (4.4). Handles are the enclosing function's `using` parameters,
the only places handles live (S27).

Two operations of one handle *commute* when the capability declares both
read-only with `@read_only("op", ...)`, a declaration annotation on the
capability or interface whose text arguments name its operations. Every
other pair, including two calls of the same operation that is not
declared read-only, conflicts. `concurrent_conflict` names both tasks
(one-based) and each conflicting pair, and spans the later task. Order
conflicting work explicitly: write the conflicting tasks as consecutive
statements, or in separate `concurrent` blocks.

`@read_only` is the program's claim about its host, as `effects` clauses
are: an operation declared read-only must not change what any other
operation of the resource observes. Effect declarations alone do not make
two resources distinct; see aliasing below.

## Execution

The value of a block is that of running its tasks in source order, each
binding its result, then making every binding visible. Because the
footprints of distinct tasks commute, every interleaving of tasks whose
actual handles are distinct produces the same results and the same
per-resource sequence of operations.

* The native engine runs tasks in source order. A fault in a task escapes
  after the earlier tasks ran; later tasks do not start.
* The interpreter overlaps tasks on worker threads when at least two of
  them perform effects, every handle they use *forks*, and no two of them
  reach one resource with an operation that is not read-only on both sides.
  Otherwise it runs them in source order, as the native engine does.
  Overlapped tasks each run to their end; the fault of the lowest-index
  faulting task escapes, so the reported fault is the one sequential
  execution reports, but tasks after it may already have performed
  effects. Effects already performed are never undone.
* Visible bindings in fault reports and hole reports follow name
  resolution: a fault inside a task lists the bindings before the block,
  never a sibling's.
* Aliasing: two distinct parameters can be bound to one host resource,
  which the checker cannot see. A host reports a handle's canonical
  resource identity (`CapabilityImpl::resource`); handles with one identity
  are one resource, and tasks reaching it with an operation that is not
  read-only on both sides run in source order.
* Hosts opt in to overlap: `CapabilityImpl::fork` returns a thread-safe
  handle to the same resource for a worker, and `CapabilityImpl::join`
  takes each fork back in task order. The scope executor's hosts
  (`EffectHost: Send + Sync`) fork; a fork records its calls in its own
  buffer, joined in task order.
* Recorded effects (`EffectEvent`s, `cpl concurrent --receipt`) keep task
  order, then call order within a task, whatever schedule ran, so traces
  replay deterministically with `cpl concurrent --replay`. Replay matches
  each call to a recorded event by resource, operation, and arguments, so
  overlapped tasks may reach it in any order, then compares the complete
  trace.

A block in a pure function, an initializer, or a machine has empty
footprints: its tasks are pure computations, as in `parallel for`, and are
mainly a way to state independence. `concurrent` does not grant authority:
each task still needs its effects in the enclosing function's bound.

## Relation to scope documents

`cpl concurrent SCOPE.json` (evolution amendment) schedules whole
functions with host-assigned resource identities and explicit `after`
dependencies, and records and replays their effects. Functions it runs may
contain `concurrent` blocks; their effects join the task's trace in block
order.
