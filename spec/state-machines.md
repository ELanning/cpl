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
new root. Calling a reducer or initializer computes a candidate snapshot;
it cannot itself install it in a running context. A host may own several
contexts and decide their event delivery order. Determinism is per
context and ordered input history, not a promise about concurrent
external systems.

`examples/state_machine` demonstrates composition, typed commands,
explicit effect handling, and rejection of stale asynchronous replies
using request IDs. IDs must not be reused while an old reply can still
arrive; the example leaves ID allocation to its host.

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
program. `snapshot()` returns detached JSON. There is no setter or
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
`constructor_invariant_false`. Context codes: `machine_program_invalid`,
`usage_not_a_machine`, `machine_value_invalid`, `machine_result_invalid`,
`machine_sequence_exhausted`, and `machine_replay_mismatch`.

## Design sources

The [shared design conversation](https://chatgpt.com/share/6ae7df1d-89e4-83e8-ad38-bdd82d2eb24d)
motivated typed state payloads, deterministic transitions with command
values, scoped machine composition, replay, explicit event policies,
capability boundaries, and contextual holes. CPL's existing stable
identities, semantic facets, contracts, and structured faults supply the
corresponding tooling foundation. The conversation's durable execution
and distributed protocol ideas are recorded above as explicit host
contracts rather than implied guarantees of this in-memory runtime.
