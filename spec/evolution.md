# Program evolution amendment, 2026-11-02

This amendment adds executable versions of the program-evolution ideas in
the shared CPL discussion. It supersedes the earlier closed annotation
vocabulary and extends tooling without changing ordinary CPL expression
syntax. The discussion's `transform`, `protocol`, and `migration` examples
were sketches; this implementation uses strict JSON change/scope documents
and checked source annotations. Existing `.work` and `.patch` files keep
their meanings. Automatic agent execution by `work run` remains separate.

The 2026-11-03 revision adds semantic merge with composed verification,
generated differential testing, `@preserve` acknowledgements, removal and
hole-filling edits, staged changes, permission queries, exploration
invariants and goals, migration paths, schema compatibility in
verification evidence, read grants, facet grafting and move-aware
references in merges, and protocol response analysis. The discussion's
in-language `concurrent { ... }` block is the `concurrent` statement of
`concurrency.md`; the scope documents below still schedule whole functions.

## Implementation and dependency boundary

`cpl-evolve` depends on the project loader, checker, index, evaluator, and
work primitives. The CLI delegates to it. No checker, evaluator, or index
acquires a dependency on orchestration or native emission. All APIs can be
used without a CLI process. `Snapshot` loads and checks an immutable
`VirtualTree`, includes embedded or overridden std, and fingerprints
source files, lock, std, and the evolution format/toolchain version.

| Idea | Implemented entry point |
| --- | --- |
| Semantic patches | `Edit`, `Operation` (replace, rename, add, remove, fill a hole), `cpl edit`, facet-hash preconditions, staged multi-edit changes |
| Semantic merge | `merge::merge`, `cpl merge`/`cpl conflicts`: facet-level three-way merge by stable identity, rename-following references, import set merge |
| Change contracts | `ChangeContract`, structural preservation, `preserve_behavior` proved by symbolic equivalence (`verification.md`) with execution-confirmed counterexamples, measured `improve` requirements, behavior samples, generated differential testing |
| Agent permissions | `Policy`, per-actor grants, forbids, add/remove namespaces, read grants, region ownership, `writable by ACTOR` |
| Semantic conflict detection | composed-candidate checking and testing: structural conflicts, check failures, test-detected regressions, unknown interactions |
| Transactional changes | `prepare`, journaled `apply`/`recover`, atomic Git `publish`, two-parent merge publication |
| State-space exploration | `explore`, breadth-first search, shortest failing traces, state invariants, reachability witnesses, derived alphabets |
| Protocol types | `@protocol`, `@transition`, `@message`, nominal endpoint and graph checks, affine states, `@role` conformance, `protocol NAME` graphs |
| Controlled effectful concurrency | `concurrent { ... }` task blocks with checked effect footprints (`concurrency.md`); `concurrent::plan/execute/replay`, canonical resources, explicit conflict ordering |
| Data migrations | `@schema`, `@migration`, `pack/migrate/migrate_to`, schema fingerprints, migration paths, compatibility in verification |
| Proof-carrying changes | snapshot-bound `Evidence` with a per-kind summary and verifier statuses (`verification.md`), independent rechecking, evidence stored in published commits |
| Incremental verification | `verify --changed`, reverse dependency closure across both revisions, schema compatibility, scope counts |
| Programming API | `query`, source AST, typed expression locations, resolved references, typed holes, structured edits |
| Self-description | `@intent`, `@preserve`, inspect docs, query metadata, docs-facet preservation, `@preserve` acknowledgements |

### Performance model

Snapshots are immutable, so each computes its index data (facet versions,
module hashes, receipt inputs), digest, and reverse reference graph once,
on first use. Facet versions and module content hashes are memoized
process-wide by source text, so snapshots that share most declarations (a
base and its candidate, the sides of a merge) hash each unchanged
declaration once. Verification runs affected tests on a pool of evaluation
threads (`CPL_PARALLELISM`, else the available processors up to eight);
records are kept in test order, so evidence is identical to a sequential
run. The affected functions' proof obligations are decided on the same
number of threads, in order, each function's verifier report computed
once per snapshot. Exploration validates events once and steps from stored state values
without rebuilding machine contexts. Differential testing and other
repeated evaluations run on one evaluation stack:
`cpl_eval::on_eval_stack` evaluates in place on a thread that already is
one. `crates/cpl-evolve/examples/evolve_bench.rs` times every operation on
a generated project.

## Source annotations

Every argument is nonempty text. Repeated annotations, wrong arity, and
invalid placement are checker errors. Formatting preserves the annotations.

```cpl
@intent("Compute the price of a quantity of items")
@preserve("Nonnegative quantities retain their price")
pure fn price(quantity: Int) -> Int { quantity * 10 }
```

Intent and preservation text are documentation, never assumed facts.
They participate in `docs` facet hashes, appear in `inspect` docs and
query metadata, and can be protected by preserving or denying writes to
that facet. A change whose `replace_body` or `replace_declaration` edit
targets a declaration carrying `@preserve` must either revise that docs
metadata in the same change or list the statement in the contract's
`acknowledge` (`{symbol, statement}`, or `--acknowledge TEXT` on `cpl
edit`); otherwise preparation fails with `change_preserve_unacknowledged`
and the unacknowledged statements. The report's `attestations` record
each acknowledgement as `attested` by the actor, or `revised` with the
new text. An attestation is the actor's claim, not verified evidence.
An acknowledgement that no edit requires is `change_acknowledgement_invalid`.
Renames, hole fills, additions, and removals need none. Protocol/schema/transition/migration/message annotations
participate in signature hashes. Canonical annotations are ordered by
metadata name, with JSON-escaped text arguments.

### Protocols

```cpl
@protocol("connection", "closed", "initial_terminal")
type Closed(Int)
@protocol("connection", "open", "state")
type Open(Int)
@transition("connection", "closed", "open")
@message("Client", "Server")
pure fn connect(state: Closed) -> Open {
    match state { Closed(session) => Open(session) }
}
@transition("connection", "open", "closed")
pure fn close(state: Open) -> Closed {
    match state { Open(session) => Closed(session) }
}
```

Protocol states annotate nominal wrappers, structs, or enums. State names
are unique within a protocol. Roles are `initial`, `state`, `terminal`, or
`initial_terminal`. A protocol has exactly one initial state and at least
one terminal state. Every state must be reachable from the initial state
and have a path to a terminal state. Cycles with an exit are legal.

A transition is a monomorphic function whose first argument is exactly the
source nominal type and whose return type is exactly the destination
nominal type. Ordinary argument checking rejects an operation on the wrong
state, on both engines. Additional arguments and capabilities are allowed.
`@message(sender, receiver)` labels a checked transition with its participants.

Protocol states are **affine**: a value whose type is a state, or contains
one (a struct field, an enum payload, a wrapper, `Option`, `Result`), is used
at most once, so after a transition consumes a state the old value cannot be
used again (`crates/cpl-sema/src/linear.rs`). Every read consumes (a call
argument, an operand, a `match` scrutinee, a field access, a returned value);
assigning a `var` makes it usable again. The analysis is path-sensitive: a
value consumed on one branch of an `if` or `match` is consumed after it, and
loop, `forall`, and `parallel` bodies are analyzed twice, so a state consumed
in an iteration and not reassigned is a reuse in the next. Contract clauses
consume nothing.

| Rule | Diagnostic |
| - | - |
| a local holding a state is read after a read consumed it (a note names the consuming read) | `protocol_state_reused` |
| a list element type contains a state (lists share their elements) | `protocol_state_in_list` |
| a generic function that may use a parameter twice is instantiated with a state | `protocol_state_duplicated` |

`@role(protocol, role)` on a function declares it part of the implementation
of one participant: its calls may perform only transitions whose `@message`
sender is that role, and the functions marked with the role must together
perform every such transition of the protocol (calling it, or being it).

| Rule | Diagnostic |
| - | - |
| the role is not a sender or receiver of the protocol's messages | `protocol_role_unknown` |
| the function calls a transition that another role sends | `protocol_role_violation` |
| a transition the role sends is performed by none of the role's functions | `protocol_role_incomplete` |

The checks do not authenticate a distributed peer or prove fairness, actual
network delivery, or absence of every distributed deadlock. The graph checks
detect unreachable states and states with no completion path; participant
labels describe the interaction without assuming delivery.

### Schemas and migrations

`@schema(family, version)` names a nominal value schema; `@migration(family,
from, to)` annotates a monomorphic pure function with one value argument,
no capabilities, and the exact source/destination types. Endpoints must
exist, differ, and have one migration per directed pair. Persisted values
and runnable migration endpoints are monomorphic.

`schema TYPE --value JSON` validates shapes and all nested invariants before
returning `{schema, version, schema_digest, value}`. `migrate FUNCTION
--input FILE` checks those fields against the declared source, runs checked
CPL code, and validates the new value. `migrate --input FILE --to VERSION`
follows the shortest chain of declared migrations from the stored version
(`migrate_to`), validating every intermediate value, and reports the
functions applied. Input data is never overwritten.

The fingerprint includes the type and transitive type/predicate dependencies
and std. A transaction defaults to `require_migrations: true`: existing
schema versions must remain available and unchanged. Adding a new version
to an existing family requires a migration path from every old version.
Retain the old nominal type and stable ID and introduce a new version/type;
`@schema` supplies the family identity connecting them. This prevents an
in-place schema edit from invalidating persisted data unnoticed. Ordinary
file editors can bypass change policy; it is enforced at the change boundary.

## Query grammar

`cpl query EXPRESSION [--since REV] --json` accepts these exact forms:

```text
symbols
symbol NAME_OR_ID
callers of NAME_OR_ID
dependencies of NAME_OR_ID
affected by NAME_OR_ID
tests of NAME_OR_ID
functions with EFFECT effects
unproven contracts
holes
protocols
protocol NAME
schemas
migrations
changed
writable by ACTOR
```

`changed` requires `--since`. `holes` lists every typed hole as a
structured synthesis request: the enclosing symbol, location, expected
type, visible bindings, capabilities, enclosing clauses, and dependency
versions; `fill_hole` answers one. `protocol NAME` returns the checked
state graph: states with roles and nominal types, the initial and
terminal states, and transitions with their functions and `@message`
participants, plus an informational `unanswered` list: messages after
which the receiver never sends from the resulting non-terminal state, a
likely missing response (one-way notifications are legitimate, so this is
not a checker error). `writable by ACTOR` reads the authority policy (`--policy`
or `.cpl/authority.json`) and lists, per declaration, the facets the actor
may write and whether it may remove the declaration, plus the namespaces
it may add to and the declarations it may read (or `unrestricted`),
applying the same grants, forbids, regions, and read lists as preparation. Targets use the existing index's stable ID,
qualified-path, and unambiguous bare-name resolution. An effect matches an
operation name, a parameter.operation, a qualified capability, or a
qualified capability.operation. Queries require a checked snapshot;
`inspect` remains available for partially valid programs.

`symbol` includes the serializable source AST, expression types with source
locations, resolved references with stable target identities and reference
kinds, metadata, facet hashes, and verification obligations. It does not
serialize Rust-internal pointers or invent natural-language query meanings.

## Change document

`cpl edit replace-body --symbol ID --input BODY --patch CHANGE.json` creates
a complete, independently reviewable change. `rename` takes `--name`;
`replace-declaration` takes `--input`; `fill-hole` takes `--hole NAME
--expression EXPR`; `remove` takes only the symbol. All are previews
unless `--apply` is given. Source targets require explicit stable IDs.
With `--append`, the edit is added to the existing `--patch` change of
the same actor, and the whole staged change is prepared again, so a
multi-edit transaction can be built one edit at a time and previewed as
a unit before one atomic `cpl transaction FILE --apply`.

```json
{
  "name": "optimize_price",
  "actor": "optimizer",
  "edits": [{
    "symbol": "fn_price",
    "expect": {"body": "HASH_FROM_QUERY_OR_INSPECT"},
    "operation": {"kind": "replace_body", "body": "{ 10 * quantity }"}
  }],
  "reads": [],
  "contract": {
    "preserve": [{"symbol": "fn_price", "facets": ["signature", "contract", "effects", "docs"]}],
    "behavior_samples": [{"symbol": "fn_price", "arguments": [[0], [1], [100]]}],
    "differential": [{"symbol": "fn_price", "cases": 500}],
    "acknowledge": [{"symbol": "fn_price", "statement": "Nonnegative quantities retain their price"}]
  },
  "verification": {"seed": 1, "cases": 100, "timeout_ms": 5000}
}
```

Fields are strict: unknown fields and enum variants fail. Operations:

* `replace_body {body}` expects `body`; it parses exactly one block and
  supports functions, tests, and properties.
* `rename {name}` expects `name`; it edits the declaration's identifier,
  resolved name references, and direct imports. Full checking and authority
  enforcement cover every resulting change, including callers. Enum
  variant and field names remain distinct from the containing type's name.
* `replace_declaration {source}` expects every existing facet; exactly one
  declaration, retaining its stable identity, is accepted.
* `add_declaration {file, module, source}` expects an empty hash map and
  a new explicit stable identity matching `symbol`. The destination is a
  project-relative CPL source path in the selected module. Multiple new
  declarations can be submitted together before typechecking.
* `remove_declaration` expects every existing facet and deletes the
  declaration with its doc comments, annotations, and leading comments.
  The candidate must still check, so a referenced declaration cannot be
  removed alone.
* `fill_hole {hole, expression}` expects the facet containing the hole
  (`body`, or `contract`) and replaces the uniquely named typed hole of
  the target with one parsed expression, parenthesized unless it is a
  primary expression so the surrounding precedence is kept.

All edits refer to one snapshot. Overlapping byte ranges or repeated
overwritten facets fail before any write. A body edit follows its stable
identity through a rename, while another body edit invalidates its hash.
`reads` entries `{symbol, facet, expect}` pin assumptions about dependencies.
Preserve selectors must resolve; empty or nonexistent promises are errors.

`preserve_behavior: true` requests universal equivalence on the base
version's domain. Its `behavior` obligation lists every function the change
modifies (own executable facets or resolved callees differ) with the
equivalence verdict of `verification.md`: identical definitions, together
with everything they call, are shared by both sides, and every other callee
is executed in place. Candidate inputs the prover could not rule out are run
on both versions (with the step budget of differential testing, a version
out of budget showing no difference); a difference becomes `failed` with
`{"by": "execution", "inputs", "before", "after"}`. A removed public function
is `failed` (`removed`), a changed type `unknown` (`type_changed`). The
obligation is `proved` when every function is (`partial` evidence marks a
proof that assumes both versions terminate).

`improve` entries are a measure name for every function the change modifies
(`"time_complexity"`) or `{"symbol", "measure"}` for one. Each is measured:
both versions run on inputs of sizes 1 to 64 (integers of that magnitude,
texts, bytes, and lists of that length, structs of such parts; three inputs
per size), counting interpreter steps (function activations and loop
iterations, `Interpreter::steps`) under a budget of two million steps per
run; a version out of budget is slower there than one within it, and a size
at which both run out is not compared. Outputs must agree. The status is `measured` when the new version is
never slower and its advantage grows with the input (at the largest size it
is at least twice as fast, and at least twice its advantage at the smallest),
`failed` otherwise (`no_asymptotic_improvement`, `slower at size N`,
`behavior_changed`), and `unknown` for a function that cannot be run
(generic, capability parameters). `steps` asks only for fewer steps at the
largest size. Evidence lists the steps per size and the empirical growth
exponents between the two largest sizes. Improvement requirements accept `measured`; every other required
obligation must be `proved`, and an unknown one prevents acceptance. Nonempty `behavior_samples` compare the old
and new function on specified JSON arguments and record concrete mismatches;
success is `tested`, with equivalence still `unknown`.

`differential` entries (`{symbol, cases?, filter?}`) compare the old and
new versions of a monomorphic, capability-free function on generated
inputs. Inputs are drawn per parameter type from a deterministic stream
seeded by `verification.seed`, the symbol, and the case number, with sizes
growing from zero: integers (including values beyond 64 bits), booleans,
text, bytes, lists, options, results, structs, enums (payload-free
variants at depth, so recursive types terminate), wrappers, and
refinements (drawn at their base type). An input is outside the compared
domain, and discarded, when it violates an argument refinement or struct
invariant, the old function's own `requires` (a nested call's
precondition failure is behavior, not domain), or an optional `filter`: a
`Bool` function of the base program taking the same parameters, such as
"existing positive behavior". Values are compared exactly; faults are
compared by diagnostic code. Each run has a budget of 1,000,000 steps
besides the time budget; a run that exhausts either, or the call depth,
has no outcome to compare, and its case counts as `inconclusive` (a loop
bounded by an input never finishes on the generated integers far beyond
64 bits, which is no difference from a version that does). Up to ten
attempts per requested case are made (`cases` defaults to
`verification.cases`). The first disagreement
is shrunk on its value JSON (integers toward zero, shorter text and
lists, recursively inside fields and payloads, each candidate rerun) and
reported with the generated and shrunk arguments and both outcomes. A
comparison in which no input fell inside the domain is `untested` and
prevents acceptance. Agreement is `tested` evidence with equivalence
`unknown`; it never discharges `preserve_behavior`.

### Authority

A separate policy file contains `grants` and `regions`. The default is
`.cpl/authority.json` when present; `--policy PATH` selects another explicit
policy. The change cannot supply its own grants. Each grant contains
`actor`, `write` selectors, optional `forbid` selectors, and optional `add`
and `remove` namespace strings. A removal needs a `remove` namespace
covering the declaration's module and no forbid on any of its facets.

An optional `read` list bounds what the actor's changes may depend on:
each entry is a stable identity or name, `module.*`, or
`dependencies(NAME)`, the transitive dependency closure of NAME (NAME
included). When any of the actor's grants has a `read` list, every
reference from a declaration the change writes or adds must target a
readable declaration, a declaration the change itself writes, or the
standard library (`change_read_denied`, with the `declaration` and its
`target`), and every `reads` guard must name a readable declaration. A
selector that names nothing is `change_read_selector_empty`. Without read
lists, reads are unrestricted. Like the rest of the policy this governs
changes at the change boundary; it does not hide files from a process. Selectors contain `symbol` (identity, name, or `module.*`)
and a set of `facets`. Denials win. Regions contain `namespace` and `owner`;
a write into any owned region must come from its owner as well as satisfy
the grant. Unmatched writes fail. Namespace matching respects segment
boundaries (`auth` does not cover `authentication`).

Policy-controlled transactions require `--actor` matching the document.
This identifies the caller supplied by the orchestration layer; it is not
OS authentication or a sandbox. Filesystem access control and host actor
authentication remain the embedding application's responsibilities.

### Validation, checkout, and publication

`transaction FILE` prepares an immutable candidate, checks the whole
program, enforces permissions and contracts, runs affected tests/properties,
and reports independent evidence. No partial edit reaches source files.
`--apply` installs only accepted candidates. An OS lock serializes
cooperating writers. Complete input fingerprints are compared before and
after installation; a concurrent change is refused. A durable journal is
written before files. I/O failure rolls back; process death leaves a journal
for `transaction --recover`. Recovery never overwrites an independent edit.
Symlink traversal and non-normal source paths are rejected on writes.

A multi-file checkout is not an instantaneous filesystem snapshot for
uncooperating readers. Use immutable publication when readers require an
atomic accepted-program boundary: `--publish refs/cpl/changes/NAME` writes
a Git commit with evidence, then advances the reference using Git CAS.
Publication requires project source inputs matching the committed `HEAD`
baseline, so the commit's parent reconstructs the change's exact starting
program; unrelated staged files are allowed. It preserves the user's index,
working files, and current branch. A stale
head, source snapshot, or target reference fails. Git author configuration
is required, as with ordinary commits. A rejected CAS can leave unreachable
objects, which Git can later collect.

## Verification and semantic conflicts

`verify` checks all tests and properties. `verify --changed --since REV`
uses the semantic diff as roots, computes a reverse closure over **all**
reference kinds in both the old and new checked programs, and selects tests
and obligations in that closure. This includes contracts, constructors,
field uses, and type references. Module/import, manifest, lock, or std
changes select the whole project. Typechecking always covers the whole
candidate. This is incremental execution/obligation selection, not an
incremental compiler. Obligation statuses are the verifier's
(`verification.md`).

Evidence records configuration (seed, cases, shrink/depth/time limits,
proof policy), source/std/lock fingerprint, optional baseline fingerprint,
affected identities, test outcomes, obligations, holes, acceptance, and a
content digest. Holes and failing tests prevent acceptance. Unknown proof
obligations remain visible; `--require-proofs` makes them fail acceptance.
`--receipt PATH` saves evidence; `--evidence PATH` checks identity/digest and
independently reruns verification, comparing results apart from timing.
The digest is integrity metadata, not a cryptographic signature or proof.

Evidence also carries `schemas`, the persisted-data incompatibilities
with the baseline (a version removed or changed in place, or a new
version that an older version cannot reach by migration), and a
`summary` that keeps each kind of evidence separate: typecheck, passed
and failed tests, property cases, runtime contract checking, proved,
unknown, and failed obligations, holes, schema compatibility
(`compatible`, `incompatible`, or `not_checked` without a baseline), and
the scope (project declarations, affected declarations, full recheck).
With a baseline, schema issues prevent acceptance unless
`require_migrations` is false (`--allow-schema-changes`); they are
reported either way, so an agent that edits files directly is told about
an in-place schema edit at `verify --changed`, not only in transactions.
A change's contract `require_migrations` sets this for its verification.

## Semantic merge

`merge(base, left, right)` (`cpl merge LEFT RIGHT [--base BASE]`, and
`cpl conflicts` for the analysis alone) merges two descendants of `base`
by stable identity. Revisions are Git revisions or `worktree`; the base
defaults to the merge base of two revisions, otherwise `HEAD`.

Each declaration merges facet by facet: a facet changed on one side is
taken from that side, the same change on both sides is taken once, and
different changes of one facet are a structural `conflict`. File
placement merges the same way. A declaration removed on one side and
changed on the other, or added differently on both, conflicts. The merged
text of a declaration comes from one side, with the other side's body
spliced in when only the body (and, for named declarations, the name)
differs that way. Other combinations of facets changed on both sides
(such as a contract on one side and docs on the other) are *grafted*: the
syntax of each facet is taken from its side (docs and `@intent`/`@preserve`,
`@prove`, the signature with its metadata annotations, `requires`/`ensures`/
`decreases`, `effects`, a struct invariant, a body, a test's name), the
declaration is laid out canonically, and every resulting facet hash must
equal its origin's. A facet this cannot separate (a capability's operation
contracts, for example) is `both_sides_changed_declaration_level_facets`.

Text taken from either side is re-rendered against the merged names: each
resolved reference (calls, type uses, constructors, checks; never field or
capability-operation names) to a declaration whose merged name differs
from its name on that side is renamed. A rename on one side therefore
reaches callers the other side added, which a text merge breaks. A
reference to a declaration that moved to another module is requalified by
its new module path when it was qualified, or when it was an unqualified
reference within the old module. Imports merge as three-way sets of
resolved targets, so two sides adding imports to one file combine, and
imports follow renames and moves. Files that are not CPL
modules (`cpl.toml`, `cpl.lock`, `.work` files) merge as whole files.
Merged files keep their sides' layout; formatted inputs give formatted
output.

The composed program is checked and verified against the base like a
change: every test affected by either side runs on it. Results:

| Status | Meaning |
| --- | --- |
| `conflict` | structural conflicts; no tree is composed |
| `check_failed` | the composition does not check (for example, one side removed what the other side's new code calls); its diagnostics are reported |
| `regression` | a test that passes on each side containing it fails on the composition: a semantic conflict that touched no common text |
| `failed` | the composition fails verification for reasons that already fail on a side (`inherited_failures`), holes, proofs, or schemas |
| `tested` | the composition checks and passes its affected tests |

`interactions` lists declarations whose dependency closure, over the
reference graphs of all three programs, includes changes from both sides.
`compatibility` is `independent` when there are none and the composition
was accepted, `tested` when there are some and it was accepted, and
`unknown` otherwise; interactions remain `unknown`, since passing tests
are not equivalence. `decisions` records each changed declaration's facet
origins.

`--apply` installs an accepted composition over the worktree, which must
be the left side, through the journaled installation of transactions.
`--publish refs/cpl/changes/NAME` (two committed revisions) writes a
commit whose parents are the left and right commits, with the merge
report under `.cpl/evidence/`, and advances the reference by
compare-and-swap. A merge result is sealed like a candidate and installs
only over its own left side.

## Bounded exploration

`explore MACHINE [--state JSON] [--events JSON] --depth N` explores a
finite event alphabet using breadth-first traversal, canonical JSON state
deduplication, and immutable machine contexts. Without `--state`, the
program's unique parameterless `initial fn` of the machine's state type
supplies the initial state (`initial_source: "initializer"`). Without
`--events`, the alphabet is every value of the event type when that type
is finite (payload-free variants and payloads built from `Bool`, finite
enums, structs, and wrappers, up to 256 events; `alphabet_source:
"derived"`); an `Int`, `Text`, `Bytes`, or `List` payload requires explicit
samples (`explore_alphabet_infinite`).

`--invariant FN` (repeatable) names a pure `Bool` function of the state
checked at every reachable state, the initial one included: a false
result or a fault is a counterexample with the shortest trace to that
state. This expresses properties that are not construction invariants,
such as `refunded <= charged`. `--goal FN` asks whether some state
satisfying a predicate is reachable: the result is `witness` with a
shortest trace, `unreachable` when the reachable set closed without one,
or a bounded result. It validates initial state,
events, transition contracts, returned state and commands. Commands are
data and are never executed. The shortest failing event trace includes
the initial state, last valid state, and diagnostic for reproduction using
`replay`. The original state is unchanged.

Results are `exhaustive` (the reachable state set closed), `bounded_pass`
(depth reached), `incomplete` (a configured resource cap),
`counterexample`, and with a goal `witness` or `unreachable`. Completeness always refers to the supplied event alphabet;
sampling payloads does not prove behavior for other payloads. Defaults:
depth 20, 10,000 states, 100,000 transitions, and a 5-second budget per
checked evaluation. State and transition limits bound search growth.

## Effectful concurrency and replay

A scope JSON contains tasks (`id`, function, JSON arguments, capability
parameter `using` bindings, `after` task IDs), resources, and verification
configuration. Resources have a host-assigned canonical `identity`, a
capability type, and per-operation `read` or `exclusive` classification.
Missing classifications are exclusive. All aliases of an actual resource
must use the same identity; the host is responsible for accurate identities
and classifications. Effect bounds alone do not establish independence.

The planner checks types, arguments, bindings, operations, dependencies,
and cycles. It derives footprints from declared and inferred effects,
coalesces aliased resources, and rejects an exclusive overlap without an
explicit dependency path. Read/read operations can share a wave. Tasks
run in deterministic topological waves with a bounded worker count, joining
every started task. A fault finishes the current wave and stops dependent
waves. External effects already performed are not rolled back.

Results and event sequence numbers follow deterministic wave/task order;
within a task, effects retain call order. They do not purport to encode
wall-clock completion order. Every record includes task, resource,
operation, arguments, and host response. Replay pins the snapshot and scope,
supplies recorded responses, checks each request, and compares final results
and the entire trace, without contacting live hosts. This reproduces observed
external values; it does not make independently changing external systems
deterministic.

`cpl concurrent FILE` plans only. `--responses FILE` executes against an
explicit fixture map of resource -> operation -> result. `--receipt` writes
an execution record; `--replay FILE` reruns it with no external effects.
Production hosts implement the thread-safe `EffectHost` Rust API. Execution
currently uses the interpreter; ordinary protocol and migration functions
also compile through the native backend. Host calls must terminate; an
interpreter budget cannot preempt arbitrary blocking host code.
