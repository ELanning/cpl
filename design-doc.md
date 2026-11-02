**CPL means Churn Programming Language.** CPL is designed for AI agents that write, review, and integrate code concurrently, while keeping source and decisions readable to people. Its central objective is to make correct changes with minimal hidden context.

The design combines a small typed language, explicit contracts and capabilities, a queryable program graph, Git-backed change management, and scoped agent tasks. The initial interoperability targets are **Python, JavaScript and TypeScript through Node.js, and C**. Existing applications should be able to adopt CPL one module at a time.

**Status: design draft, version 0.1.** The name and initial interoperability targets are settled. Syntax, runtime behavior, integration rules, and the implementation sequence below are the proposed design. There is no implemented CPL compiler or scheduler yet, and the examples have not been compiled or mechanically proved.

CPL source uses `.cpl`; development coordination uses `.work`; the command-line tool is `cpl`. The proposed project files are `cpl.toml` for configuration and `cpl.lock` for resolved dependency and toolchain inputs.

# Goals and scope

CPL should reduce the context an agent needs, make its edit permissions explicit, produce actionable failure evidence, and support parallel work without losing a consistent shared program. Human readers should be able to inspect the same source, contracts, and semantic changes that agents use.

| Design choice | Purpose |
| - | - |
| Explicit public types and contracts | Establish stable boundaries for independent implementations |
| Pure functions and immutable values by default | Make local reasoning and repeatable checks easier |
| Capabilities for external operations | Describe access and effects in interfaces |
| Typed holes and structured diagnostics | Turn unfinished or incorrect code into precise tasks |
| Stable symbol identities and semantic patches | Make changes robust to renames, moves, and unrelated text edits |
| Git as the source of record | Preserve familiar collaboration, history, and recovery |
| Isolated task execution and checked integration | Support swarms with bounded write authority |
| Python, Node.js, and C bindings | Reuse existing libraries and enable gradual adoption |

Initial scope excludes a new package registry, full C++ or Rust source-level interoperability, JVM and .NET backends, browser execution, unrestricted macros, and automatic proof of arbitrary programs. CPL must not require a model call to execute ordinary compiled code. Agent orchestration is a development toolchain feature; a program that intentionally calls a model does so through a runtime capability.

# Architecture

The toolchain separates five responsibilities:

| Component | Responsibility |
| - | - |
| Compiler and formatter | Parse source, resolve identities, typecheck, check effects, and emit canonical text and executable code |
| Verifier and test runner | Establish supported contracts, report counterexamples or unknown results, and run reproducible tests |
| Semantic index | Answer questions about definitions, callers, contracts, effects, and dependency versions |
| Runtime and interop adapters | Manage values, capability handles, foreign calls, and host-language bindings |
| Work coordinator and integrator | Assign isolated tasks, enforce edit scopes, validate candidates, and advance Git refs atomically |

The first compilation target is native executables and shared libraries. Whether the initial backend emits C or uses LLVM directly remains an implementation decision. Python and Node bindings wrap the same native CPL core; they do not require separate CPL language implementations.

The semantic index and caches are derived from a Git checkout plus locked build inputs. Compiled binaries, caches, and active task leases are disposable execution state. Durable task results and review evidence must retain their source revisions and provenance.

# Core syntax

Use braces for blocks, newlines for statements, and a block's final expression as its value. Semicolons are not part of the surface syntax. Expressions continue within parentheses or brackets; function clauses and an immediately following body form one declaration. Long contract expressions use explicit parentheses. Comments start with `//`, and documentation comments start with `///`. The canonical formatter preserves declaration order and formats one statement per line.

Names use snake_case for functions and values and PascalCase for types. Identifiers are ASCII in the initial version; strings are Unicode. Operators have fixed precedence and cannot be overloaded by user code.

```text
module reader.pages

use std.text.{trim}

pub type PageId(Text)
pub type Positive = Int where self > 0

pub struct Page {
    id: PageId
    text: Text
}

pub enum StoreError {
    Missing(PageId)
    Unavailable
}

fn clean(page: Page) -> Page {
    Page { ..page, text: trim(page.text) }
}
```

A module is a logical namespace and may span files. Imports are explicit. Declarations are private unless prefixed with pub. Types and capabilities exposed in a public signature must also be public. Public struct fields and enum variants are accessible through their exported type in this initial design. The prelude supplies Int, Bool, Text, Bytes, Unit, List, Option, Result, and their constructors.

Int is an arbitrary-precision integer. Bounded machine integers are distinct types with explicit, checked conversions. Records and collections are immutable values. A record update creates a value; it does not mutate the input. Local bindings may be reassigned with var, but mutable references cannot escape a function in the initial version.

PageId is a nominal wrapper, constructed as PageId("page-1"); an ordinary Text is not a PageId. Positive is a refinement of Int. A literal such as 2 can satisfy Positive through a compile-time proof. A dynamic integer can be validated with check\<Positive>(n), which returns Result\<Positive, ConstraintError>. Failure to prove a refinement never becomes an implicit unchecked conversion.

# Expressions and errors

```text
fn label(result: Result<Page, StoreError>) -> Text {
    match result {
        Ok(page) => page.text
        Err(Missing(_)) => "Missing page"
        Err(Unavailable) => "Storage unavailable"
    }
}

fn positive_size(text: Text) -> Result<Positive, ConstraintError> {
    let n = parse_size(text)?
    check<Positive>(n)
}
```

The second example assumes parse_size returns Result\<Int, ConstraintError>. A postfix ? unwraps Ok or returns Err from the enclosing function. Error types must match unless an explicit conversion is supplied. Prefix ?name denotes an unfinished expression and is a separate grammatical form.

match is exhaustive. if is an expression when both branches produce compatible values. return exits a function early. Option\<T> represents missing values; there is no null. Recoverable failure uses Result\<T, E>. Contract violations and resource exhaustion are execution faults rather than ordinary success values.

List\<T> supports len, is_empty(), take(n), drop(n), drop_last(), and push(value). push returns a new list. take and drop accept a nonnegative integer and clamp at the list length. drop_last() returns an empty list for an empty input. ++ concatenates lists, and flatten concatenates a list of lists. These operations have library specifications available to the verifier.

Boolean operators are `not`, `and`, and `or`. From highest to lowest, precedence is postfix access/calls/`?`, prefix operators, multiplication/division/remainder, addition/subtraction/concatenation, comparisons, `and`, then `or`. Comparisons do not chain: write `a <= b and b <= c`. Quantifiers use `forall x in collection: predicate`. Runtime-checkable quantifiers range over finite collections. Ghost mathematical assertions may extend this in the verified subset; unsupported proof obligations remain unknown.

# Functions and contracts

```text
@id("pgn_7c2d")
@prove
pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>
    ensures flatten(result) == items
    ensures forall page in result: page.len > 0 and page.len <= size
    ensures forall page in result.drop_last(): page.len == size
{
    ?build_pages
}
```

This is a development declaration with an unresolved body. The contract preserves ordering and multiplicity, forbids empty pages, and requires all but the final page to be full. An empty input therefore produces an empty result.

A `requires` clause declares a precondition; `ensures` declares a condition on the returned value, named `result`. Multiple clauses are conjoined. Contract expressions must be pure and total under their preceding assumptions. A `Result` return is checked as a whole, including error variants; a postcondition about a successful value must explicitly match `Ok`.

```text
fn divide_exactly(a: Int, b: Int) -> Int
    requires b != 0
    requires a % b == 0
    ensures result * b == a
{
    a / b
}
```

A caller must prove or check a precondition before entering the body. Sequential precondition checking makes the remainder expression safe after the nonzero check.

Functions are externally pure unless they declare effects. Local var reassignment does not make a function effectful. Public function signatures, effect bounds, and contracts are explicit; local value types may be inferred.

Default contract mode is `checked`: executable preconditions and postconditions run at their boundaries, with statically established checks eligible for elimination. `@prove` additionally requires all declared contract and termination obligations to be discharged in the supported verification subset. Unsupported expressions or a solver timeout yield `unknown`; they do not weaken the requirement.

Proof results record their assumptions. A caller relying on an unchecked or foreign callee's specification has a conditional proof until that dependency is discharged or explicitly admitted by project policy. A strict integration requirement rejects unresolved assumptions about CPL implementations; external adapter assumptions must remain named and visible. Tests and runtime checks never silently become a proof.

@id is generated by tooling for top-level definitions. The short value shown is illustrative; production identifiers would be opaque, collision-resistant, and unique within the resolved dependency graph. Renames and moves retain identity. Copying a definition creates a new identity. Names remain readable references in source.

# An implementation with loop invariants

```text
@id("pgn_7c2d")
@prove
pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>
    ensures flatten(result) == items
    ensures forall page in result: page.len > 0 and page.len <= size
    ensures forall page in result.drop_last(): page.len == size
{
    var pages: List<List<T>> = []
    var rest = items

    while rest.len > size
        invariant flatten(pages) ++ rest == items
        invariant forall page in pages: page.len == size
        decreases rest.len
    {
        pages = pages.push(rest.take(size))
        rest = rest.drop(size)
    }

    if not rest.is_empty() {
        pages = pages.push(rest)
    }

    pages
}
```

This is the completed version of the preceding declaration, not a second definition to include alongside it. The loop emits only full pages; after the loop, it appends any remaining page. The decreasing nonnegative measure states why the loop terminates.

Loop invariants must hold before the first iteration and after every iteration. A decreases clause must decrease strictly on every back edge and remain in a well-founded domain. A recursive verified function similarly declares a decreasing measure. Mathematical integer and sequence semantics keep this example independent of machine overflow; allocation failures remain outside the functional-correctness claim.

The example expresses the intended proof obligations. The verifier must use the specifications of `take`, `drop`, `push`, and `flatten` when checking it. Correctness remains relative to the language semantics, verifier, and trusted runtime.

# Capabilities and effects

```text
pub capability PageRead {
    fn get(id: PageId) -> Result<Page, StoreError>
}

pub capability PageWrite {
    fn put(page: Page) -> Result<Unit, StoreError>
}

pub fn load_page(
    id: PageId,
    using db: PageRead,
) -> Result<Page, StoreError>
    effects { db.get }
{
    db.get(id)
}

pub fn load_text(
    id: PageId,
    using db: PageRead,
) -> Result<Text, StoreError>
    effects { db.get }
{
    let page = load_page(id, using db)?
    Ok(page.text)
}
```

using introduces a capability parameter. Capabilities are runtime-issued handles; code cannot manufacture one from a string or a record. Calling a capability operation creates a tracked effect. effects is an upper bound checked through the whole call graph, including aliases and capability forwarding.

A PageRead handle grants no write operation. The host binds it to a specific authorized service and scope. An effects declaration describes potential behavior; it does not grant credentials or access. Separate handles for read and write make that distinction explicit.

Time, randomness, filesystem access, networking, and model calls also require capabilities. Code has no ambient environment access. A model response is external data that must be parsed and validated. Type validation does not establish that its contents are factually correct.

Adapters are a trusted boundary. Their contracts are assumptions unless independently verified. A recording adapter may replay captured inputs for debugging, but it does not replay writes or undo external events. Transactional behavior is supplied by a particular database capability and its implementation, not implied by a keyword on an arbitrary block.

# Holes and structured diagnostics

A prefix hole is legal during development:

```text
fn summarize(page: Page) -> Text {
    ?summary
}
```

The compiler reports the hole's expected type, visible bindings, available capability operations, relevant contracts, and the revision of each dependency. It can inspect and typecheck callers against the declaration while marking the implementation unresolved. Verification that depends on a hole's unproved contract is conditional. Release builds reject holes in the linked program and require the configured verification obligations to be closed.

A failure response is structured data, for example:

```json
{
  "code": "contract_counterexample",
  "symbol": "pgn_7c2d",
  "facet": "body",
  "clause": "flatten(result) == items",
  "input": { "items": [1, 2, 3], "size": 2 },
  "actual": [[1, 2]],
  "status": "failed"
}
```

The statuses proved, failed, and unknown are distinct from tested. A concrete executable counterexample should be distinguished from an abstract solver model. The diagnostic above describes a concrete example for a buggy candidate, not the intended implementation shown earlier.

# Agent query interface

Agents access the same typed program graph used by the compiler. Queries can retrieve a declaration's signature, contract, effects, implementation, callers, dependencies, unresolved obligations, relevant tests, and recent semantic changes. Every response identifies the source revision and selected symbol versions so context from different snapshots is not silently combined.

The default response includes the target and its interface dependencies. Callers and implementations can be expanded explicitly. A concise summary never replaces the actual contract or version information needed to validate a patch.

The proposed command interface includes:

```sh
cpl inspect reader.pages.paginate --include signature,contract,effects,callers --json
cpl check --json
cpl prove reader.pages.paginate --json
cpl test --json
cpl diff main...HEAD --semantic
cpl work run work/pagination.work
cpl work integrate pagination_update
```

These are proposed commands, not existing tools. The structured API and CLI must return the same underlying results. A verification receipt includes source and dependency digests, compiler and verifier versions, check configuration, status, and named assumptions. Test receipts additionally include seeds, fixtures, and environment inputs.

# Tests and properties

```text
test "pagination preserves a short tail" {
    assert paginate([1, 2, 3], 2) == [[1, 2], [3]]
}

test "pagination handles empty input" {
    let items: List<Int> = []
    assert paginate(items, 2) == []
}

property "pagination preserves input"
    for items in gen.list(gen.int)
    for size in gen.positive_int
{
    assert flatten(paginate(items, size)) == items
}
```

A test is an executable example. A property is a test with generated inputs. The runner records seeds and shrinks failures. These properties supplement contracts and proofs; finite sampling does not prove a universal statement.

Tests have stable identities and live in namespaces like ordinary declarations. The examples can be placed in module tests.pagination. A task allowed to add its own tests cannot change the integration policy or unrelated acceptance tests.

# Semantic source and Git

Git stores the canonical text files, contracts, locked dependencies, compiler configuration, and work declarations. Each checkout can regenerate the semantic index. No proprietary database is required to recover the code.

Each declaration has separately addressable facets: identity, name, signature, contract, effects, body, docs, and verification_policy. Its contract includes requires and ensures clauses; verification_policy includes @prove. A .definition selector covers all applicable facets. Stable identity and content versions are distinct: identity survives a rename, while content versions change as the corresponding contents change.

The compiler can resolve source-level selectors such as paginate.body to stable identities. Its dependency graph records references to types, public contracts, implementations used by inlining or proofs, external specifications, build inputs, and toolchain versions. Proof reuse follows those dependencies. Tests that execute a function depend on its implementation, even if they compile against its interface.

Ordinary Git diffs remain usable. A semantic diff adds statements such as contract changed, effect bound widened, or identity renamed. Merging different facets still requires checking the combined program; separate edits are not automatically semantically independent.

# Semantic merge conflicts

A textual merge can succeed while the combined program violates an assumption. For example, one approved change relaxes a function's result contract to permit an empty list while another adds a caller that assumes the result is nonempty. The merged caller must be rechecked against the changed contract.

The integrator uses dependency invalidation and verification to detect represented conflicts. It does not infer every business requirement or promise conflict-free merges. Changes to interfaces, effects, adapters, lockfiles, and build configuration can invalidate many tasks even when their function bodies did not change.

Stable identity supports moves and renames; normalized syntax supports deterministic comparisons. Content hashes identify source and configuration versions, not arbitrary semantic equivalence between two different programs.

# Changes and agent tasks

A .work file uses the same names, literals, comments, and braces, with declarative coordination constructs:

```text
use reader.pages.{paginate, Positive}

change pagination_update {
    base git("main")
    target git("main")
    workers 3

    task implementation {
        write { paginate.body }
        depend { paginate.contract, Positive.definition }
        preserve {
            paginate.signature,
            paginate.contract,
            paginate.effects,
            paginate.verification_policy,
        }
        goal "Implement pagination and discharge its proof obligations."
    }

    task examples {
        add { tests.pagination.* }
        goal "Add boundary cases for empty input and partial final pages."
    }

    task audit after implementation, examples {
        write {}
        goal "Review both patches against the frozen pagination contract."
    }

    integrate {
        require typecheck
        require prove(paginate)
        require test(tests.pagination.*)
        require review(audit)
        require no_holes
    }
}
```

This schedules development work when explicitly submitted to the toolchain. It does not launch agents when an application runs. workers is a concurrency ceiling. Independent tasks may run in parallel in isolated worktrees.

base resolves the Git ref to a full commit object ID when the change is opened. All initial tasks use that snapshot. target names the branch to advance, and its actual head is checked at integration. The work declaration, toolchain configuration, and acceptance policy are frozen for that run.

write permits changes to specified existing facets. add permits creation in a specified namespace; it does not permit editing existing acceptance tests. Everything outside the granted write and add sets is read-only. preserve additionally records explicit equality requirements against the task's baseline. A task cannot authorize itself by editing these declarations or build tools.

depend names important assumptions; compiler-derived transitive dependencies are tracked regardless of what the agent declares. Namespaces and selectors are resolved by the language tooling, not interpreted as filesystem globs. A definition's stable identity cannot be changed to bypass an ownership boundary.

after creates a dependency edge between tasks. The scheduler prepares the dependent task's candidate snapshot from the named predecessors. It resolves conflicts and runs basic consistency checks before starting that task; otherwise the dependent task is blocked. An audit observes exact predecessor outputs and emits a review record, not a source edit. If a reviewed input changes, that review is stale.

Overlapping write sets are rejected among unordered tasks by default. A later task can edit the output of an explicit predecessor. Competing implementations use separate candidate changes rather than silently merging two bodies. Leases carry scheduler-issued fencing tokens: a result from an expired or superseded assignment cannot overwrite current work.

Task declarations request capabilities within the submitter's authorization. The scheduler must enforce filesystem, process, network, and integration boundaries. Merely parsing a declaration does not sandbox an agent.

# Swarm authority and lifecycle

A project can assign different authority to implementers, test authors, reviewers, specification authors, and integrators. Implementation tasks ordinarily modify bodies. Specification changes require their own explicit scope. Review tasks produce records tied to the exact candidate they inspected. Integration authority belongs to the coordinator or an authorized actor, not to every worker.

The coordinator computes granted permissions as the intersection of project policy, the submitter's authority, and task requests. Natural-language goals, comments, and library documentation cannot expand these grants. Read-only source includes manifests, acceptance policy, existing tests, and build tools unless a task explicitly has authority over them.

| Task state | Meaning |
| - | - |
| Ready | Dependencies are available and scope can be assigned |
| Running | An agent holds the current assignment lease |
| Submitted | A candidate and provenance are available for validation |
| Blocked | Dependencies, a conflict, or a required decision prevent progress |
| Accepted | Required task-level validation passed for the candidate |
| Integrated | An accepted candidate is included in a checked target commit |
| Failed or cancelled | The attempt ended without integration |

Lease renewal and fencing prevent stale submissions. Repeated submission of the same task attempt and candidate must be idempotent. A crashed worker leaves an isolated worktree that can be inspected or reassigned; it does not partially update the target branch.

Dependent tasks see explicit candidate snapshots assembled from their predecessors. Independent speculative alternatives remain separate candidates until one is selected. The scheduler rejects cycles and unordered overlapping edits; it does not resolve those conflicts by silently choosing the last writer.

Coordinator budgets for concurrency, execution time, and external calls are execution policy. They are enforced by the host and adapters. Agent task concurrency and application runtime concurrency remain separate concepts.

# Semantic patch syntax

A candidate from the implementation task can contain:

```text
patch paginate.body
    expect base(paginate.body)
{
    var pages: List<List<T>> = []
    var rest = items

    while rest.len > size
        invariant flatten(pages) ++ rest == items
        invariant forall page in pages: page.len == size
        decreases rest.len
    {
        pages = pages.push(rest.take(size))
        rest = rest.drop(size)
    }

    if not rest.is_empty() {
        pages = pages.push(rest)
    }

    pages
}
```

patch is a development operation, not an application statement. Its body is parsed in the target function's lexical and generic scope. base refers to the immutable baseline assigned to the submitting task, including predecessor outputs for dependent tasks. expect performs an exact facet-version check. This permits a patch to tolerate unrelated file movement while refusing a stale body replacement.

The submitted artifact also carries task identity, baseline revision, fencing token, generated dependency versions, and check evidence. Those fields are produced and checked by the toolchain; they need not clutter the authored source. A patch is rejected if it escapes the task's write set, changes a preserved facet, or has stale dependencies that have not been revalidated.

Renames and moves are structural edits that preserve symbol identity. If they rewrite callers, the change must also authorize those resulting edits. Identity tracking never grants additional write permission.

# Integration semantics

The integrator builds a candidate from the current target head and the accepted task outputs, checks facet conflicts and relevant assumptions, and reruns or safely reuses the required checks. A required test selector must match at least one test; an empty selection is a configuration failure. It advances the target reference atomically only if the head still equals the revision just checked.

If the target moves, integration retries against the new head. The integrator never treats a passed branch check as evidence that a newly combined revision passes. Proof and test evidence is bound to the actual source and dependencies, toolchain, external assumptions, and check configuration. Pure work may be reused when all these inputs still match.

Code that typechecks can still contain behavioral conflicts. Detection is limited to the represented types, contracts, policies, and tests. Compilation and proof do not establish that the product requirements themselves are correct.

Each integration creates an ordinary Git commit with provenance linking task outputs and check records. The evidence location is project policy, but enough source and configuration must be committed to rerun checks. Reverting a commit changes source; it does not reverse production side effects.

Application concurrency is separate from agent concurrency. The initial language may expose structured task groups through capability-backed standard-library interfaces. A detailed runtime task and cancellation syntax is deferred; .work task declarations have only development-time meaning.

# Interoperability scope

The first ecosystem targets are **Python, JavaScript and TypeScript through Node.js, and C**. Each needs support in both directions.

| Target | CPL calling existing code | Existing code calling CPL |
| - | - | - |
| Python | A CPython adapter in process or a managed Python worker | Generated CPython extension and Python package |
| JavaScript and TypeScript | Node-hosted callbacks or a Node worker for standalone CPL | Generated Node-API addon, JavaScript wrapper, and TypeScript declarations |
| C | Generated or reviewed bindings to C-compatible functions | Native shared library and generated C headers |

CPL will use existing ecosystem package managers and resolved lockfiles. It will not assume that arbitrary packages can be imported without adapters, native build dependencies, or a compatible runtime. The first release supports a documented set of runtime and platform versions; exact versions are selected during implementation and tested in CI.

## Python

CPython already supports [embedding its interpreter and loading native extension modules](https://docs.python.org/3/extending/). CPL uses these mechanisms to expose compiled CPL functions to Python and to call Python through explicit adapters.

Generated packages include `.pyi` type stubs and wrappers that validate arguments and map errors. Python hints are inputs to binding generation, not proof of behavior or complete descriptions of dynamic APIs. Missing or ambiguous types require an explicit adapter specification.

In-process calls must respect interpreter object lifetimes, thread-state rules, and the supported runtime's concurrency model. A GIL-free Python build does not automatically make every extension safe for concurrent calls. Worker mode provides a separate process and a place to apply host resource and access policy; process separation alone is not a complete sandbox.

Initial array and byte transfer copies data for clear ownership. Buffer borrowing and DLPack-style tensor exchange are later optimizations requiring explicit lifetime, device, mutation, and synchronization rules.

## JavaScript and TypeScript

The initial JavaScript host is Node.js. [Node-API](https://nodejs.org/api/n-api.html) provides an ABI-stable interface for native addons across supported Node versions. CPL's generated package contains a native addon, a JavaScript wrapper, and [TypeScript declarations](https://www.typescriptlang.org/docs/handbook/declaration-files/introduction.html). Native binaries remain specific to supported operating systems and architectures.

A Node application can import a CPL package. CPL code hosted in Node may call registered JavaScript adapters on the correct event loop. A standalone CPL executable calls npm packages through a managed Node worker; a C ABI alone does not execute JavaScript.

The default generated Node wrapper exposes calls as Promises and schedules native work away from the event loop. Callbacks into JavaScript must be marshalled back to its owning runtime thread. Cancellation is a request; it does not undo an external action that has already happened. A synchronous fast path may be added for explicitly selected short, pure calls.

This host-level scheduling does not add application-level `async` syntax to the initial CPL language. Browser support, a JavaScript code-generation backend, and support for other JavaScript runtimes are deferred.

## C

CPL supports the target platform's C calling convention for imports and exports. A versioned CPL C API supplies contexts, value construction, validation, opaque resources, and explicit release operations. The internal representation of CPL values is not a public ABI.

C imports begin with fixed-width scalars, byte spans, explicit structs, and opaque handles. Header parsing can generate signatures but cannot establish pointer ownership, mutation, callback behavior, or semantic contracts. Those require adapter annotations or reviewed wrappers. Variadic APIs, unusual layouts, and complex callbacks are outside automatic binding support initially.

C exports use generated signatures and a defined status/error channel. Complex values and arbitrary-precision integers use managed handles unless the API explicitly selects a checked conversion to a machine type. An allocation must be released through the API that owns it; callers do not free CPL allocations with an unrelated allocator. Exceptions and language-runtime unwinding must not cross the C boundary.

Rust and C++ libraries may participate through C-compatible wrappers. Direct source-level interoperability with their generics, templates, and runtime object models is outside the initial scope.

# Foreign boundary semantics

Foreign code is exposed to ordinary CPL source through typed capabilities. A capability's operation name makes the foreign call visible to effect checking, while the adapter and host determine what the foreign implementation can actually access.

| Concern | Initial CPL rule |
| - | - |
| Type information | Generate bindings where possible; validate dynamic inputs and outputs |
| Ownership | Copy ordinary values initially; keep foreign objects behind opaque handles |
| Errors | Map exceptions and foreign error codes to typed CPL errors |
| Contracts | Check supported boundary predicates; record remaining foreign behavior as assumptions |
| Effects | Do not infer purity or access restrictions from a foreign signature |
| Isolation | Use reviewed in-process adapters or workers with actual host-enforced policy |
| Reproducibility | Lock package artifacts, runtime versions, adapters, native dependencies, and ABI targets |

A capability declaration cannot prevent an unrestricted native library from reading files or corrupting process memory. Foreign code executing in process is part of the trusted boundary. Worker execution can contain failures and enforce access only to the extent provided by its configured OS sandbox and brokered resources.

A foreign exception becomes a `ForeignError` with structured classification and diagnostic details. A worker crash is a transport or execution failure, not an ordinary successful return. The runtime must not automatically retry externally visible operations unless an adapter explicitly defines safe retry and idempotency behavior.

## Value mappings

| CPL value | Python | JavaScript and TypeScript | C interface |
| - | - | - | - |
| `Int` | `int` | `bigint` | Managed integer handle or checked machine integer |
| `Bool` | `bool` | `boolean` | Defined integer representation |
| `Text` | `str` | `string` | UTF-8 byte span with length |
| `Bytes` | `bytes` | `Uint8Array` | Byte span with explicit lifetime |
| `List<T>` | List | Array | Generated sequence wrapper or managed handle |
| Struct | Generated record wrapper | Validated typed object | Generated struct or opaque handle |
| Enum or `Option<T>` | Tagged generated value | Discriminated union | Tagged representation |
| `Result<T, E>` | Value or typed exception at the wrapper | Fulfilled or rejected Promise | Status plus result/error outputs |

The mappings apply only to supported instantiated types. CPL generic functions require concrete export wrappers. `Option<T>` uses explicit tags so nested options and foreign null-like values remain distinguishable. Foreign object graphs with cycles or identity-sensitive behavior use handles rather than recursive copying.

CPL Text represents Unicode scalar values. Adapters reject unpaired surrogates or require an explicit replacement policy, and do not apply implicit Unicode normalization. JavaScript Number is not the default representation of arbitrary-precision Int; numeric conversions must reject overflow and loss of precision.

Float types and their NaN and signed-zero contract semantics must be specified before exposing floating-point APIs. They are not assumed to behave like mathematical integers.

## Adapter example

An image adapter illustrates the CPL-facing boundary:

```text
module reader.images

use reader.pages.{Positive}
use std.foreign.{ForeignError}

pub struct Dimensions {
    width: Positive
    height: Positive
}

pub capability ImageCodec {
    fn dimensions(image: Bytes) -> Result<Dimensions, ForeignError>
}

pub fn image_width(
    image: Bytes,
    using codec: ImageCodec,
) -> Result<Positive, ForeignError>
    effects { codec.dimensions }
{
    let dimensions = codec.dimensions(image)?
    Ok(dimensions.width)
}
```

The host may bind ImageCodec to a Python image library through a reviewed adapter. The adapter validates returned dimensions before constructing Positive values. That establishes shape and range, not that the library reported the correct image dimensions.

A proposed project binding in `cpl.toml` selects the adapter:

```toml
[project]
name = "reader"
edition = "0.1"

[interop]
targets = ["python", "node", "c"]

[bindings.image_codec]
capability = "reader.images.ImageCodec"
backend = "python"
adapter = "reader_adapters.images"
isolation = "worker"
```

The adapter module is project-supplied, not a CPL built-in. The manifest requests the binding; host policy grants its actual privileges. Importing the adapter, including module initialization, must occur inside the selected execution boundary.

## Export example

A monomorphic wrapper makes the earlier generic pagination API usable from all three ecosystems:

```text
pub fn paginate_ints(
    items: List<Int>,
    size: Int,
) -> Result<List<List<Int>>, ConstraintError>
{
    let checked_size = check<Positive>(size)?
    Ok(paginate(items, checked_size))
}
```

Assuming the generated package is named reader_cpl, Python usage would be:

```python
from reader_cpl import paginate_ints

pages = paginate_ints([1, 2, 3], 2)
# [[1, 2], [3]]
```

JavaScript and TypeScript usage would be:

```typescript
import { paginate_ints } from "reader-cpl";

const pages = await paginate_ints([1n, 2n, 3n], 2n);
// [[1n, 2n], [3n]]
```

These are expected usages of proposed generated packages. Invalid size values become a typed exception in Python, a rejected Promise in Node, or a defined error status in C. The wrapper checks foreign inputs even if an internal CPL caller could establish the refinement statically.

# Builds and package reproducibility

The repository contains source, work declarations, project configuration, lockfiles, adapter specifications, and generator inputs. The lock record includes the CPL compiler and language edition, verification configuration, foreign package versions and artifact hashes, adapter versions, runtime requirements, and target ABI.

Existing Python and npm lockfiles may remain authoritative for their ecosystems, referenced by the CPL lock record. Native dependencies require pinned artifacts or reproducible build recipes. Native headers alone do not identify the linked library version. Build scripts execute as controlled build steps rather than during ordinary semantic queries.

Generated bindings are reproducible outputs. Projects may commit them for distribution, but their source interfaces and generator versions remain the editing authority. An agent edits those inputs and regenerates bindings through an authorized task rather than creating inconsistent independent copies.

A foreign dependency or adapter change invalidates the relevant type, contract, effect-assumption, and runtime compatibility evidence. Integration checks the updated combined lock state. Reproducible build inputs do not make external services deterministic; service-dependent tests must declare fixtures or environment assumptions.

# Minimal grammar outline

The outline below fixes the principal declaration boundaries. It is an abbreviated grammar; expression productions, lexical escaping, and all work selectors would need a complete specification before a parser is built.

```ebnf
function     = annotation*, ["pub"], "fn", identifier,
               [generics], "(", parameters, ")", "->", type,
               [effects], contract*, block ;

effects      = "effects", "{", [effect_ref, {",", effect_ref}], "}" ;
contract     = ("requires" | "ensures"), boolean_expression, newline ;
annotation   = "@", identifier, ["(", arguments, ")"], newline ;

hole         = "?", identifier ;
propagate    = postfix_expression, "?" ;

while_loop   = "while", boolean_expression,
               {"invariant", boolean_expression, newline},
               ["decreases", expression, newline], block ;

change       = "change", identifier, "{", change_item*, "}" ;
task         = "task", identifier,
               ["after", identifier, {",", identifier}],
               "{", task_item*, "}" ;
patch        = "patch", facet_ref,
               "expect", "base", "(", facet_ref, ")", block ;
```

Function and capability declarations require parameter and return types. Public functions require explicit effect bounds if effectful. Contracts are ordered requires clauses followed by ensures clauses in canonical output. The formatter keeps annotations and these clauses attached to their declaration.

No user-defined infix operators, implicit coercions, runtime metaprogramming, ambient I/O, or hidden mutable globals are included in the initial design. These restrictions make dependency tracking and local reasoning tractable. Future extensions must preserve explicit boundaries for effects, assumptions, and semantic edits.

# Implementation order

Implementation proceeds in vertical milestones so useful interop arrives before the full swarm coordinator.

| Milestone | Deliverable | Acceptance evidence |
| - | - | - |
| 1 Core execution | Parser, formatter, types, immutable values, Result, native execution, checked contracts, holes, and stable IDs | Formatter is idempotent; examples execute; invalid types, refinements, and release holes are rejected |
| 2 C boundary | Versioned C imports and exports, generated headers, ownership rules | A C host calls CPL and CPL calls a C library; ownership and failure paths are exercised |
| 3 Python package | Generated extension and stubs; Python adapters and worker transport | Python imports CPL; CPL calls a Python adapter; conversion, exception, and worker failure cases pass |
| 4 Node package | Node-API addon, JS wrapper, TypeScript declarations, Node adapter bridge | Node imports CPL; CPL calls a Node adapter; event-loop, BigInt, and error mapping cases pass |
| 5 Verification and edits | Supported pure proofs, structured diagnostics, dependency tracking, and scoped semantic patches | Proved, failed, and unknown remain distinct; stale and out-of-scope patches are rejected |
| 6 Swarm integration | Isolated tasks, dependency scheduling, reviews, leases, and atomic Git advancement | Concurrent changes integrate consistently; expired assignments and invalidated reviews cannot commit |

The first cross-ecosystem demonstration uses one CPL library from Python, TypeScript, and C, plus one adapter in each direction. The same CPL contract and implementation underpin all bindings.

A swarm demonstration assigns implementation and test additions independently, then reviews their combined result. Introduce a concurrent contract change deliberately: dependent work must be invalidated or rechecked before integration. Also test a target branch advancing after validation, an expired worker submitting late, and a coordinator restart.

# Open implementation decisions

| Decision | Constraint already established |
| - | - |
| Native backend | Preserve CPL semantics and expose the versioned C interface; choose C emission or direct LLVM lowering |
| Managed value representation | Keep the public ABI independent of internal memory layout |
| Verification engine | Start with a documented pure subset and explicit unknown results |
| Platform and runtime matrix | Select and continuously test supported CPython, Node, operating-system, and architecture versions |
| Callback and cancellation protocol | Define ownership, event-loop affinity, reentrancy, and cancellation without implying rollback |
| Runtime task syntax | Keep it separate from .work agent tasks; host-level async wrappers can ship first |
| Rich numeric and array types | Specify floating-point and tensor semantics before promising transparent numerical interoperability |

The main engineering risks are incomplete foreign specifications, unsound dependency reuse, expensive proof obligations, and coordinator isolation errors. Each has an explicit boundary in the design. A verified program may still implement the wrong requirement; specification changes must therefore be visible and reviewable.

# Design references

These existing mechanisms inform CPL; the CPL syntax and integration protocol above are a new proposal.

- [Dafny contracts and verification](https://dafny.org/latest/OnlineTutorial/guide) illustrate checking implementations against preconditions and postconditions.

- [Koka effect types](https://koka-lang.github.io/koka/doc/book.html) illustrate making effects part of function types.

- [Unison content-addressed definitions](https://www.unison-lang.org/docs/the-big-idea/) inform semantic code storage and tooling. CPL retains Git text as the source of record and distinguishes stable identity from content version.

- [CPython extension and embedding interfaces](https://docs.python.org/3/extending/) provide the Python host mechanisms.

- [Node-API](https://nodejs.org/api/n-api.html) and [TypeScript declarations](https://www.typescriptlang.org/docs/handbook/declaration-files/introduction.html) provide the initial JS/TS host interfaces.

# Language amendment: state-machine application contexts (2026-11-01)

CPL applications are organized around one immutable root snapshot per
application context. A checked, pure state machine computes its next
snapshot from an event, optionally returning typed command values for a
top-level effect handler. Smaller machines own scoped state and compose
recursively into the root. State-specific payloads use enums; event
handling is exhaustive and deterministic for an ordered input history.

Functions may be explicitly designated `pure` or `mutate`. Pure code,
including machines, initializers, and contracts, cannot transitively
reach a mutator. Mutation belongs near the application entry point.
There is no inheritance or object model. Structs may enforce pure
constructor invariants, and explicit operation interfaces remain
available. Validation should move to compile time wherever it can be
proved soundly; unproved dynamic and foreign inputs retain mandatory
checks. Faults retain source spans, language stacks, and transition
context, and failed transitions leave the committed snapshot unchanged.

The normative details, compatibility rules, executable example, replay
protocol, and limits are in [spec/state-machines.md](spec/state-machines.md).
This amendment takes precedence over conflicting earlier design text.
Earlier sections and verbatim acceptance examples are preserved so their
existing specification references remain stable.



## Structured concurrency and performance amendment (2026-11-01)

Application code now expresses independent pure work with `parallel for item in items: expression`. Task scopes use bounded workers, return ordered values, inherit evaluation limits, cancel later work after a failure, and join before returning. The checker rejects effects and escaping control flow in task expressions. Application state still has one owner: bounded mailboxes serialize events through the top-level machine and return receipts after atomic commit.

Optimized native lowering performs bounded scalar folding, safe branch/identity reductions, small-integer fast paths with bigint fallback, and ownership-aware list growth. Shared snapshots and runtime contracts retain their meaning. The interpreter also reuses unshared list capacity. See `spec/parallelism.md` for the normative rules and `spec/performance-audit.md` for measurements and validation. These rules supersede the earlier deferral of application task syntax.
