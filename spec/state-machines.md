# State machines and immutable application contexts

This is the 2026-11-01 language amendment. It supersedes conflicting rules
in the earlier edition 0.1 documents for the features below. Existing
edition 0.1 source remains accepted; new programs should use explicit
function roles and application contexts. The implementation is shared by
the reference evaluator and emitted C runtime.

## Declarations and purity

```cpl
pure fn total(items: List<Int>) -> Int { /* pure computation */ ?total }
mutate fn perform(command: Command, using http: Http) -> Event
    effects { http.get }
{ /* external orchestration */ ?perform }
initial fn start(settings: Settings) -> App { App { settings: settings, count: 0 } }
machine reduce(state: App, event: Event) -> App { ?transition }
```

`pure fn`, `mutate fn`, `initial fn`, and `machine` are function roles.
`machine name(...)` omits `fn`. They may carry `pub`, documentation,
annotations, contracts, and stable IDs just as ordinary functions do.
These words are contextual declaration prefixes, so existing names such
as `fn pure()` remain valid.

A pure function cannot call a mutate function anywhere in its transitive
call graph, including through unannotated helpers or recursive cycles.
Machines and initializers are pure. Their declared effect bounds must be
empty. Constructor invariants and all other contracts also exclude
mutating calls. Diagnostics include a concrete call chain. Very long
witnesses retain a bounded prefix and the terminal mutator, with an
explicit truncation marker and `call_chain_truncated: true`; rendering
errors cannot expand into quadratic path output.

Mutation means externally observable mutation/effects, not rebinding a
private local variable during a calculation. Values are immutable; local
`var` and loops remain available inside pure computations. Pure does not
mean total: a pure function may fault, exceed a resource limit, or fail to
terminate. Capabilities remain explicit `using` parameters, and each
operation must remain in the caller's declared effect bound even in a
`mutate fn`.

For compatibility, bare `fn` keeps its existing effect-bound semantics:
an empty bound is pure and a nonempty bound is effectful. It cannot hide
an explicit mutator from a pure caller. A chain of three explicit mutate
functions raises `mutation_depth`; keeping effects near the host entry
point and extracting pure computations removes that warning. The
warning also applies to recursive mutation. It is architectural guidance,
not a fixed maximum depth or a call-stack proof.

`interface` is an alternative spelling of `capability`. It defines an
explicitly supplied operation interface; it does not add object methods,
inheritance, implicit receivers, virtual dispatch, or mutable object
fields. Structs remain value records.

## Application roots and scoped machines

```cpl
program App {
    user: User
    settings: Settings
    download: Download
}

enum Event {
    Start(Int, Text)
    Received(Int, Text)
    Cancel
}

struct Step<S, C> {
    state: S
    commands: List<C>
}
```

A `program` is a concrete nominal struct designated as an application
root. Each program type must have exactly one root `machine`. Multiple
independent program types may exist, but each running context owns just
one root snapshot. This is context ownership, not a process-wide mutable
singleton.

A machine has exactly two positional parameters `(state: S, event: E)`,
no generic parameters, and no `using` parameters. `S` is a concrete
struct or enum; `E` is an enum. The result is either `S` or a struct with
exactly two fields: `state: S` and `commands: List<C>`, where `C` is an
enum. Either field order is accepted. Generic data types such as
`Step<App, Command>` may be instantiated in this signature.

State-specific data belongs in enum payloads: for example,
`Fetching(request_id, url)`, `Done(body)`, and `Failed(message)`. Existing
exhaustive match checking applies to states and events. An ignored or
rejected event must have an explicit arm or wildcard policy; there is no
implicit transition table, handler ordering, or nondeterministic guard
selection. An explicit rejection can return the unchanged state with a
rejection command. Business failures should normally be event/command
variants rather than runtime faults.

Only the root machine and its `initial fn` declarations may use the
program's constructor or spread-update syntax. An initializer must
return that program type and accept no program-state snapshots, even
inside a wrapper, list, enum, refinement, or generic argument. It has no
generic or capability parameters. Initialization can receive ordinary
configuration data loaded by the host.

Pure helpers can inspect snapshots. Smaller machines accept ordinary
structs or enums as scoped state; the parent calls them and assembles the
new root. Only machines, tests, and properties call a machine; an
initializer computes a candidate snapshot that only a `context` statement
or a host context installs (next section). A host may own several
contexts and decide their event delivery order. Determinism is per
context and ordered input history, not a promise about concurrent
external systems.

`examples/state_machine` demonstrates composition, typed commands,
explicit effect handling, a `context` that runs one download, and
rejection of stale asynchronous replies using request IDs. IDs must not be
reused while an old reply can still arrive; the example leaves ID
allocation to its host.

## One state machine per call stack

Code and data stay apart: state is a value of a `program` type, behavior
is its machines, and a running application holds exactly one state
machine per call stack. A function starts it; the functions below it
receive its handle; nothing else keeps evolving state of its own.

```cpl
mutate fn invite_to_calendar(req: Request, using db: Db) -> Response
    effects { db.get_user, db.get_calendar }
{
    let user = db.get_user(req.user)
    let calendar = db.get_calendar(req.calendar)
    context app = start(user, calendar)     // the state machine of this call stack
    let sent = send_invites(req.invitees, using app)
    respond(app.state(), sent)
}

mutate fn send_invites(people: List<Text>, using app: Context<App>) -> Int
    effects { app.dispatch }
{
    var sent = 0
    var i = 0
    while i < people.len {
        sent = sent + app.dispatch(Invite(people.get(i).unwrap_or(""))).len
        i = i + 1
    }
    sent
}
```

`context NAME = EXPR` is a statement (`context` is contextual: a
statement that begins with `context`, a name, and `=`). `EXPR` has a
program type `S` (`context_not_program` otherwise): usually an `initial
fn` call, or a snapshot the host loaded. `NAME` is the *context handle*,
of type `Context<S>`, for the rest of the function; the state machine ends
when the function returns.

`Context<S>` is a handle type, like a capability (semantics S27): `S` must
be a program (`context_not_program`); a handle is passed only as a
`using app: Context<S>` parameter and a `using app` argument, and is never
stored, compared, returned, or held in data (`capability_as_value`,
`capability_equality`, `capability_in_value_position`). It has two
operations:

- `app.dispatch(event)`: `event` (of the root machine's event type) is
  evaluated, then `S`'s root machine runs on the current state and the
  event, and its new state is committed when it returns. The value is the
  machine's commands (`List<C>`) for a machine returning
  `{ state, commands }`, or `Unit` for one returning `S`. Commands are
  values, so a `dispatch` whose commands are not used is written
  `let _ = app.dispatch(event)`. A fault ends the call as any fault does.
- `app.state()`: the current state, an ordinary immutable value that
  pure helpers can read.

Through a handle it *received*, a function's `dispatch` and `state` are
tracked effects (`effects { app.dispatch, app.state }`), so such a
function is a `mutate fn` or an effectful `fn`. What a function does
through the context it started itself, directly or through a callee, is
none of its callers' business: a pure function may run a state machine to
completion and return a value from it. The usual purity rule still holds:
a pure function cannot call a `mutate fn`, so a function that passes its
handle to one is itself `mutate`. A function whose only handles are
contexts adds no level to `mutation_depth`: passing the state machine down
the stack is the design, not an effect to pull up.

At most one state machine is running on any call
stack, checked on the call graph. A function contains at most one
`context` statement, and none when it receives a handle (at most one
`Context` parameter); and no function that holds a context (started or
received) reaches a function that starts one, through calls, contract
calls, or `dispatch` (a call of the root machine), its own recursion
included (`context_nested`, with a `call_chain` witness). Separate call
chains each start their own context, one after another (one per request,
say). Machines and initializers start none (`context_in_reducer`); only
machines (composing child machines), tests, and properties call a
machine (`machine_call_outside_machine`), so application code changes
state only through its context; and a machine that is no program's root
and that no root machine reaches by composition is `machine_unrooted` (a
warning: no context ever runs it). A `concurrent` task cannot use a
context (`context_in_concurrent`), and a `parallel` body cannot capture
one (`parallel_unsafe`). Rationale: CPL has no function values, so the
call graph is exact and the rule is a static guarantee, not a convention;
each context grows by composition (child machines over parts of the
root's state) rather than by more machines beside it.

A context lives in the frame of the function that started it. Hosts start
none: no function taking a context handle is exported to C, Python, Rust,
Go, Java, or Kotlin (the default exports skip it, and naming it in
`[interop.c] exports` is `project_export_not_function` with `kind:
"context"`); the function whose `context` statement starts the machine is
the export. A host that keeps state itself uses the runtime context below,
which runs only a program's root machine (`usage_not_a_root_machine`);
analysis tools such as `cpl explore` may still examine a child machine.
The evaluator keeps a function's contexts in a table it truncates when the
function returns; the C engine holds the state in one slot of the starting
function, the handle being its address (a `cplrt_w *` parameter); the
hosts use a shared cell (`cpl::Context<S>`, `*cpl.Context[S]`,
`Context<S>`), internal to the generated code.

## Constructor invariants

```cpl
struct Settings {
    retries: Int
} where self.retries >= 0 and self.retries <= 5
```

A struct or program may have one Boolean `where` expression following
its closing brace (on that line or a continuation). The expression binds
`self` to the complete value. It is a pure contract; generic fields and
calls to pure helpers are allowed. Struct identity remains nominal.

The checker attempts bounded constant evaluation of each construction.
A proved true predicate marks that construction as proved, allowing its
runtime check to be omitted. A proved false predicate is
`constructor_invariant_false`. A known evaluation fault is
`constant_evaluation_fault`. Unknown inputs, unsupported operations, or
an exhausted proof budget retain a mandatory runtime check; they are
never treated as successful proofs. The compiler does not run arbitrary
user functions or capability operations during this proof attempt.

The proof evaluator limits each attempt to 10,000 expression steps,
1,000,000 charged aggregate allocation units, 16,384-bit integer operand
work, and 65,536-byte/element text or list operations. These are resource
guards, not a language-level integer or collection size limit. Existing
refinement proof attempts now use these bounds too; their previous
requirement for a successful proof is unchanged.

Dynamic construction, including spread updates, checks the completed
value before returning it. Host inputs and outputs check nested
invariants after validating their shape and children. These checks are
required even when optional function/loop contract checking is disabled.
A constructor rejection is `contract_violation` with kind `invariant`,
the declaration's symbol and predicate span, clause text, actual value,
and the ordinary language backtrace. Boundary rejection uses the
existing `refinement_violation` diagnostic with its value pointer.

Validation is fallible: constructor predicates may themselves fault or
exceed configured evaluation limits. Predicate cycles are not assumed
true. There is no general SMT solver or automatic proof of arbitrary
function preconditions/postconditions in this amendment.

## Runtime contexts, commands, and replay

`cpl_eval::machine::MachineContext` owns its state privately. Creation
validates the initial snapshot and selected machine against the checked
program; the machine must be its program's root machine
(`usage_not_a_root_machine`, which `cpl replay` reports too). `snapshot()` returns detached JSON. There is no setter or
mutable reference to the state. `dispatch(&mut self, engine, event)`:

1. Validates the event and its nested constraints.
2. Calls the pure machine with the current snapshot and event.
3. Validates the complete result, including commands.
4. Commits the new state and increments the sequence once.
5. Returns a transition receipt and its command data.

Any failure leaves both state and sequence unchanged. Diagnostics add
`machine`, `transition_sequence`, and `state_unchanged: true` to the
original structured error, retaining its source locations and stack.
The mutable context receiver serializes dispatch. Callers using the
interpreter drive this API on `on_eval_stack`, as with `RunEngine`.

Command execution belongs to the host after a successful commit. A
`mutate fn` may translate a command to a capability call and return an
event. The context and replay command never execute commands or bind
external capabilities. A host should serialize resulting events through
the same dispatch entry point.

A `TransitionRecord` contains exactly `program_digest`, `machine`
(stable ID), `sequence`, `event`, `before`, `after`, and `commands`.
`replay` checks the program identity, machine, sequence, and prior state;
recomputes the transition; compares all fields; and commits only a match.
Duplicate, stale, divergent, or differently versioned receipts fail with
`machine_replay_mismatch`. The digest covers the toolchain version, the
optional-contract checking policy, and
every formatted source module, including imports, annotations, and
standard-library source. Comments can conservatively change identity.

Receipts detect deterministic divergence; they are not signatures and
do not authenticate the source of events. An authenticated durable log,
schema migrations, atomic state/outbox persistence, command delivery,
idempotency keys, and supervision/restarts remain host responsibilities.
The in-memory context does not claim crash durability or exactly-once
effects. It retains no unbounded history; the host chooses what to store.

## CLI and toolchain

```text
cpl replay MACHINE --state JSON [--events JSON_ARRAY | --records JSON_ARRAY]
    [--engine eval|native|both] [--max-call-depth N] [--timeout-ms N] [--json]
```

With neither input flag, replay validates the starting snapshot and
returns an empty history. `--events` computes a history; `--records`
verifies one. JSON is read with the existing duplicate-key and nesting
checks. Unknown receipt fields are rejected. `--timeout-ms` is positive
and defaults to 5,000 milliseconds per machine call or boundary
predicate; hosts configuring the Rust API choose their own budgets.

Human output is the final snapshot. The JSON envelope adds `engine`,
`state`, `sequence`, and `records`. A failing event stops processing and
reports the last committed snapshot with the original diagnostic. Exit
codes are 0 for success, 1 for transition/replay/check failure, and 2 for
usage, engine divergence, or infrastructure failure. `both` compares
receipts, state, sequence, and normalized faults. Native shutdown must
report no live objects, handles, capabilities, or buffers.

Function roles participate in signature and effects facets. Program and
interface spellings participate in signature facets. Constructor
invariants have their own contract facet and dependency edges. The
semantic index reports constructor obligations as `proved` with bounded
constant-evaluation evidence or `unknown` with
`runtime_check_required`. Machine hole diagnostics include the function
role and state/event/command types, alongside existing local bindings,
contracts, and permitted capability operations. Release policy continues
to reject unfinished linked code.

New checker codes: `pure_calls_mutate`, `mutate_in_contract`,
`pure_declares_effects`, `mutation_depth` (warning),
`machine_signature`, `program_machine_count`, `program_generic`,
`state_construction_outside_machine`, `initial_signature`, and
`constructor_invariant_false`; for contexts, `context_not_program`,
`context_nested`, `context_in_reducer`, `context_in_concurrent`,
`machine_call_outside_machine`, and `machine_unrooted` (warning). Runtime
context codes: `machine_program_invalid`, `usage_not_a_machine`,
`usage_not_a_root_machine`, `machine_value_invalid`,
`machine_result_invalid`, `machine_sequence_exhausted`, and
`machine_replay_mismatch`.

## Design sources

The [shared design conversation](https://chatgpt.com/share/6ae7df1d-89e4-83e8-ad38-bdd82d2eb24d)
motivated typed state payloads, deterministic transitions with command
values, scoped machine composition, replay, explicit event policies,
capability boundaries, and contextual holes. CPL's existing stable
identities, semantic facets, contracts, and structured faults supply the
corresponding tooling foundation. The conversation's durable execution
and distributed protocol ideas are recorded above as explicit host
contracts rather than implied guarantees of this in-memory runtime.
