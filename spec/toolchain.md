# CPL toolchain architecture, edition 0.1

This document specifies the Rust toolchain that implements CPL: its Cargo
workspace, the public API of every crate, the intermediate representations,
the stable-identity and facet model, the semantic index, the command-line
contract, receipts, semantic diff, the work-coordinator data model, project
files, the testing strategy, and the milestone map.

It is written for implementers who will work on different crates
concurrently without talking to each other. The Rust signatures in Section 1
are the contracts those implementers code against: a crate may add private
items freely, but every public item named here must exist with the stated
shape, and no public item may be renamed without updating this document.

| Document | Role |
| - | - |
| `design-doc.md` | authoritative intent; this document never contradicts it knowingly |
| `spec/grammar.md` | lexical and syntactic grammar, decisions `D1`..`D73`; the AST in Section 2 mirrors its nonterminals |
| `spec/stdlib.md` | the standard library: prelude types, members, `std.*` modules, generator vocabulary; Section 3.6 fixes only how its files are loaded |
| `spec/semantics.md` | typing, effect, contract, and evaluation rules; this document fixes the data structures those rules read and write and the diagnostic codes they emit |
| `spec/cabi.md` | the native backend and the C boundary of Milestone 2, decisions `C1`..`C39`: the runtime model, the native code-generation contract, the C API with its `interop_*` codes, the driver protocol, and the import and export rules; this document fixes the crates, commands, manifest keys, lock entries, codes, and tests around them and cites it for those rules |
| this document | everything in the first paragraph |

Conventions. Where the design is explicit this document follows it. Where
the design is silent a rule is chosen and marked with a line beginning
`Decision:`, numbered `T1`, `T2`, ... (the grammar's decisions are `D`
numbers) with the rule, the rationale, and the rejected alternative. Where
the design is internally inconsistent the chosen reading is marked
`Design conflict:`. Every `Decision:` and `Design conflict:` line also
carries a project-wide stable identifier `[D-nnn]`, assigned in document
order and indexed in `spec/decisions.md`; the local `T<n>` numbers remain
the citation form. Rules owned by another specification are marked
`Semantics:` or `Library:` and stated only as far as this document needs
them. Rust code blocks are normative signatures; their bodies are omitted
or shown as `...`. JSON blocks are normative schemas written as examples;
every key shown is required unless marked optional in the accompanying
table. All names in JSON are `snake_case`.

Fixed decisions inherited from the project: the toolchain is a Rust Cargo
workspace; the C API and the Python and Node packages wrap the same native
core, the C runtime of `cpl-rt` and the C that `cpl-emit-c` emits from the
checked program (T68 reopened the earlier "same Rust core" wording,
2026-10-29); `cpl.toml` is at the project root, sources live under the
`[build] sources` directories (default `["src"]`), the `module a.b.c`
header is authoritative for the module path and `src/a/b/c.cpl` is only
the conventional location, work declarations live in `work/*.work`. No
other fixed decision is reopened here.

Decision: [D-264] T68. the C API and the Python and Node packages of
Milestones 3-4 wrap one native core: the C11 runtime of `cpl-rt` (Section
1.12, `spec/cabi.md` Section 2) and the C that `cpl-emit-c` emits from the
checked program (Section 1.13, cabi.md Section 1). The Rust crates are the
toolchain and the reference evaluator, which stays the oracle that native
execution is tested against (semantics S75, Section 12.6). This amends the
fixed decision stated before 2026-10-29 in this introduction ("the C API,
Python (PyO3) and Node (Node-API) packages wrap the same Rust core") and in
`spec/README.md` ("the C API and the Python and Node packages of later
milestones wrap the same Rust core"), which was planning text: the design
says "Python and Node bindings wrap the same native CPL core" (line 38)
and names no implementation language. The Milestone 3 and 4 rows of Section 13 change
with it (T79). The project owner confirmed this reading on 2026-10-30,
before Milestone 3 was planned, so the Milestone 3-4 packages are a
generated CPython extension and a generated Node-API addon in C over the C
API (`spec/python.md`, `spec/node.md`).
Rationale: a C runtime needs only the C compiler that the C boundary needs
anyway, so every generated package (a C library, a CPython extension, a
Node-API addon) builds and cross-compiles with that compiler alone; the
sanitizers instrument the runtime together with the emitted code;
allocation failure can be a fault (semantics S76) rather than an abort; and
two independent implementations compared on every case also find
evaluator bugs that shared code would hide. Rejected alternatives: a Rust
core shared by the evaluator and the packages (a prebuilt static library
per target and per host kind, or cargo at install time; no sanitizer
coverage of the runtime on stable Rust; an abort on allocation failure; an
oracle that shares the bugs it should find); PyO3 and napi-rs packages over
`cpl-eval` (Python and Node would run a different core from C hosts,
against line 38).

Design conflict: [D-132] the design's Milestone 1 row lists "native execution"
among its deliverables (design line 644) and names native executables as
the first compilation target (line 38) while leaving the backend open ("C
emission or direct LLVM lowering", line 659). Milestone 1 of this toolchain
executes CPL with a tree-walking interpreter over a typed IR, and every
code-emission path (`cpl-emit-c`, `cpl build`) arrives in Milestone 2
(Section 13). This is the deviation `spec/semantics.md` S51 records; it is
adopted here, not re-decided.

Decision: [D-133] T41. the native backend is deferred to Milestone 2 and the
reference evaluator is Milestone 1's only execution engine; the design's
"examples execute" criterion is met by `cpl run` and `cpl test` over the
corpus (Section 12.3), and the evaluator remains the test oracle for the
backend. Rationale: a backend can only be validated against semantics that
are already fixed, and an interpreter with no code generation fixes them
for the whole corpus first (semantics S51). Rejected alternative: a native
path in Milestone 1 (every semantic question would be decided by emitted
code with no independent oracle).

Milestone 2 adds the native backend and the C boundary (decisions T68 to
T82 here, semantics S75-S81, stdlib S45-S47, and `spec/cabi.md` C1-C39).
This document owns their crates and Rust APIs (Sections 1.1-1.3 and
1.11-1.14, with additions to 1.4, 1.6, and 1.7), the command `cpl build`
and the engine selector `--engine` (Section 7.2), the manifest keys and
lock entries of the C target (Sections 11.1 and 11.2), the codes of the
`build` category (Appendix B), and the differential and C host test
suites with the Milestone 2 acceptance checklist (Sections 12.6-12.9);
`spec/cabi.md` owns the runtime, the code-generation contract, the C API,
the driver protocol, and the import and export rules, and semantics S75
states when native execution equals the evaluator. Every Milestone 1
struct that a test builds literally keeps its fields, every Milestone 1
signature keeps its shape, and every new behaviour is opt-in or additive
(T77, T80).

## 0. Scope and terms

| Term | Meaning in this document |
| - | - |
| project | a directory containing `cpl.toml`; the *project root* |
| repository | the Git repository that contains the project root; found with `git rev-parse --show-toplevel` from the project root. The project root need not be the repository root |
| revision | one state of the project's source set: a commit, or the working tree (Section 6.3) |
| source set | exactly: `cpl.toml`, every `*.cpl` file under the configured source directories, and every `*.work` and `*.patch` file directly under `work/` (symbolic links per T54; a file named just `.cpl` is not a source file). This is the one definition every digest, receipt, lock, and scope rule in this document uses, except that from Milestone 2 the C entries of the lock also pin the build inputs, which are no members (Section 11.2). `cpl.lock` is read when present (Section 11.2) but is *not* a member, so that the lock can record the digest of the set without digesting itself; std is never a member |
| std | the standard-library declaration stubs shipped inside the `cpl` binary (Section 3.6) |
| declaration | a top-level item of a `.cpl` file: `type`, `struct`, `enum`, `capability`, `fn`, `test`, `property` |
| symbol path | the fully qualified name of a declaration (Section 4.4) |
| stable id | the `@id("...")` value of a declaration, or its provisional identity (Section 4) |
| facet | one of `identity`, `name`, `signature`, `contract`, `effects`, `body`, `docs`, `verification_policy`; `.definition` selects them all (Section 5) |
| HIR | the typed, resolved intermediate representation, defined in `cpl-hir` (T50), produced by `cpl-sema`, and consumed by `cpl-eval`, `cpl-index`, and, from Milestone 2, `cpl-emit-c` (Section 3) |
| build input | a file that the native build reads and that is not a member of the source set: the adapter file and the vendored sources of a backend-`c` binding (Section 11.1); pinned by digest in `cpl.lock` (Section 11.2), never part of a revision's digest, though a revision's state and every run that binds the binding report their edits (T82) |
| engine | what evaluates a checked program for `cpl run` and `cpl test`: `eval`, the reference evaluator of `cpl-eval` (the default and the oracle), or `native`, the program compiled to C and run in a driver process (Milestone 2; Sections 1.13-1.14, `spec/cabi.md` Section 3) |

Decision: [D-134] T1. `cpl-eval` interprets the typed HIR produced by `cpl-sema`;
there is no separate evaluation IR and the interpreter never reads the AST.
Rationale: the HIR already carries resolved definitions, types, and the
property/method distinction, and contract checking needs the types of
parameters to encode counterexamples; a lower IR would be a second
representation to keep in sync before there is a compiler backend to need
it. Rejected alternative: an untyped AST interpreter (cheaper to start,
but holes, refinement checks, and `?` on `Result` all need types).

Decision: [D-244] T54. a symbolic link to a file is read as that file,
both by `load_project` from a directory and by `tree_at` from a commit
(which follows the link inside the commit's tree with `git cat-file
--batch --follow-symlinks`), so it is a member of the source set under its
own path with the bytes of its target; a link to a directory is never
followed, so nothing reached only through it is a member; and a link that
leads to no file (dangling, a loop, a link to a directory, a path through
a file, or a target outside the repository) is not a member. From a
directory, "the repository" is the git work tree that contains the
project (the nearest ancestor of its resolved root with a `.git` entry),
else the project root itself, and a file is a member only when its fully
resolved path lies inside it: a checked-out repository may then not make
`cpl check` read, and print in a diagnostic, a file the user can read
elsewhere (`/proc/self/environ`, a key in the home directory), and the
directory reading agrees with the commit's, which holds no such file.
A `[build] sources` directory or `work/` that is itself a link, or is
reached through one, is therefore absent, exactly as in the commit's tree
(`git ls-tree` lists nothing under a linked `src`): a sources directory is
then `project_source_dir_missing`. A link whose target cannot be inspected
for lack of permission is `LoadError::Io`, not a non-member.
`cpl.toml`, `cpl.lock`, and the `work/` files follow the same rule, and a
submodule is never part of a tree. Rationale: Section 5.4 requires a facet
map, and Section 6.3 a digest, to be the same whether a revision is read
from a checkout or from its commit, and git records a link, not the tree
behind it, so only rules both readers can apply keep the two equal;
following a file link gives the text an editor and the checker see in a
checkout. Rejected alternatives: ignoring every link (a checkout would
compile files its digest leaves out); following directory links (a git
tree cannot follow them, and they can form cycles); rejecting links
(projects that share a file by linking it would not load).

## 1. Cargo workspace

### 1.1 Layout

```text
/mnt/c/code/rpl
├── Cargo.toml                  workspace manifest (resolver 2, shared [workspace.dependencies])
├── Cargo.lock
├── design-doc.md
├── spec/                       grammar.md, stdlib.md, semantics.md, toolchain.md (this file), cabi.md
├── examples/                   acceptance corpus: reader/, holes/ (each a CPL project)
├── std/                        standard-library declaration stubs (*.cpl), embedded at build time by cpl-project's build.rs
├── crates/
│   ├── cpl-diag/               SourceMap, FileId, Span, Diagnostic, every shared JSON wire type
│   ├── cpl-syntax/             lexer, tokens, AST with spans, parsers for .cpl/.work/.patch, formatter, doc comments
│   ├── cpl-hir/                the typed HIR and `Program` with its accessors (Section 3.3); between cpl-syntax and cpl-sema (T50)
│   ├── cpl-project/            project loading: cpl.toml, cpl.lock, the source set, parsing; embeds std/ (build.rs); re-exported as cpl_sema::project (T52)
│   ├── cpl-sema/               module graph, resolution, types, effects, contract well-formedness, holes, building the HIR (re-exports cpl_hir as cpl_sema::hir and cpl_project as cpl_sema::project)
│   ├── cpl-eval/               values, interpreter with checked contracts, faults, test/property runner, capability handles
│   ├── cpl-index/              stable ids, facets, content hashes, semantic index, semantic diff, receipts
│   ├── cpl-work/               .work model, scheduler state machine, leases, candidates, patch validation/application, git
│   ├── cpl-rtspec/             the one source of the tables both engines share: message templates, White_Space, builtin specs, limits, versions, div_rem, ForeignError orders, interop codes (Section 1.11; Milestone 2)
│   ├── cpl-rt/                 the C11 runtime and C API 0.1 as embedded source (c/, c/include/cpl.h), its generated table header, the runtime digest (Section 1.12)
│   ├── cpl-emit-c/             C emission from the typed HIR, generated headers and adapter wrappers, compiler discovery, compile and link, build.json, drivers, the lock's C entries (Section 1.13)
│   ├── cpl-cabi/               the native engine (driver client, NativeSession), fault reconstruction, CAdapter, outcome comparison; its tests hold the differential harness and the C host suites (Section 1.14)
│   └── cpl-cli/                the `cpl` binary
└── target -> (build output; may be a symlink to a Linux-side cache)
```

Each crate has `src/lib.rs` (or `src/main.rs` for `cpl-cli`), a `tests/`
directory for integration tests (Section 12), and a `tests/corpus/`
directory for file-based test inputs. The `std/` directory is a sibling of
`crates/` so that the library specification can edit it without touching
Rust code; `crates/cpl-project/build.rs` embeds it (Section 3.6, T52).
Likewise `crates/cpl-rt/c/` holds the C runtime and its public headers
(`c/include/cpl.h`, `c/include/cpl_adapter.h`; `spec/cabi.md` Section 2),
which `crates/cpl-rt/build.rs` embeds by the same mechanism (Section
1.12); no build script of the workspace compiles C.

Workspace settings: Rust edition 2021 (as in the existing manifest),
`resolver = "2"`, `version = "0.1.0"` shared by every crate through
`[workspace.package]`. The toolchain version string reported everywhere is
that workspace version (Section 1.4, `cpl_diag::version`). The workspace
manifest sets one lint for every crate, which each crate inherits with
`[lints] workspace = true`: `[workspace.lints.clippy] result_large_err =
"allow"`, because the crate APIs of Sections 1.6-1.9 return
`Result<_, Diagnostic>` (and `Fault`, `RunError`) by value and `Diagnostic`
is larger than clippy's 128-byte default for an error variant.

### 1.2 Crate dependency DAG

```text
cpl-diag
  ▲
  │
cpl-syntax ◄─────── cpl-project
  ▲                    ▲
  │                    │
cpl-hir                │
  ▲         ▲          │
  │         │          │
cpl-sema  cpl-eval     │
  ▲   └────────────────┤
  │                    │
cpl-index ─────────────┤
  ▲                    │
  │                    │
cpl-work ──────────────┘

cpl-cli ──► every crate, the Milestone 2 crates below included
```

The four crates of Milestone 2 (T69):

```text
cpl-rtspec    leaf: no internal dependency; read by cpl-sema and cpl-eval (data only), cpl-rt, cpl-emit-c, cpl-cabi
cpl-rt      ──► cpl-rtspec, cpl-diag
cpl-emit-c  ──► cpl-rt, cpl-rtspec, cpl-eval, cpl-sema, cpl-hir, cpl-project, cpl-syntax, cpl-diag
cpl-cabi    ──► cpl-emit-c, cpl-eval, cpl-hir, cpl-project, cpl-rtspec, cpl-diag      (dev: cpl-sema, cpl-syntax, cpl-rt)
```

| Crate | Depends on (internal) | Must not depend on |
| - | - | - |
| `cpl-diag` | nothing | any other crate |
| `cpl-syntax` | `cpl-diag` | `cpl-project`, `cpl-hir` and up |
| `cpl-project` | `cpl-syntax`, `cpl-diag` (T52) | `cpl-hir`, `cpl-sema`, `cpl-eval`, `cpl-index`, `cpl-work`, `cpl-rt`, `cpl-emit-c`, `cpl-cabi` |
| `cpl-hir` | `cpl-syntax`, `cpl-diag` | `cpl-project`, `cpl-sema`, `cpl-eval`, `cpl-index`, `cpl-work`, `cpl-rt`, `cpl-emit-c`, `cpl-cabi` |
| `cpl-sema` | `cpl-hir`, `cpl-project`, `cpl-syntax`, `cpl-diag`; from Milestone 2 `cpl-rtspec` (the builtin ids and the one division, T70); `cpl-prove` (the verification status of every `@prove` function, `spec/verification.md`) | `cpl-eval`, `cpl-index`, `cpl-work`, `cpl-rt`, `cpl-emit-c`, `cpl-cabi` |
| `cpl-prove` | `cpl-hir`, `cpl-syntax`, `cpl-diag` (the verifier reads the typed HIR alone); `cpl-sema` and `cpl-project` only as dev-dependencies (its tests check the programs they verify) | `cpl-project`, `cpl-sema`, `cpl-eval`, `cpl-index`, `cpl-work`, `cpl-evolve`, `cpl-rt`, `cpl-emit-c`, `cpl-cabi` |
| `cpl-eval` | `cpl-hir`, `cpl-syntax`, `cpl-diag`; from Milestone 2 `cpl-rtspec` (templates, White_Space, builtin specifications, limits, the one division, T70); `cpl-sema` only as a dev-dependency (its tests check the programs they run) | `cpl-sema` as a normal dependency, `cpl-index`, `cpl-work`, `cpl-rt`, `cpl-emit-c`, `cpl-cabi` |
| `cpl-index` | `cpl-project`, `cpl-syntax`, `cpl-diag`; `cpl-sema` (the HIR through `cpl_sema::hir`) for the `Index` of Section 6, the one part that needs a checked `Program` (identities, facets, revisions, semantic diff, and receipts are syntactic and use `cpl-project` alone); `cpl-prove` for obligation statuses (Section 8.1) | `cpl-eval`, `cpl-work`, `cpl-rt`, `cpl-emit-c`, `cpl-cabi` |
| `cpl-work` | `cpl-index`, `cpl-project`, `cpl-syntax`, `cpl-diag`; `cpl-sema` for `validate_work_file` and `resolve_patches`, which need a checked `Program` (the model, patch validation and application, enforcement, the scheduler, and the git wrappers do not) | `cpl-eval`, `cpl-rt`, `cpl-emit-c`, `cpl-cabi` |
| `cpl-rtspec` | nothing (Milestone 2) | any other crate |
| `cpl-rt` | `cpl-rtspec`, `cpl-diag` | every other crate |
| `cpl-emit-c` | `cpl-rt`, `cpl-rtspec`, `cpl-eval` (its pure naming and stub helpers only, Section 1.7.4), `cpl-sema` (the plan types of `cpl_sema::interop` only, Section 1.6), `cpl-hir`, `cpl-project`, `cpl-syntax` (the operators of the HIR's `Unary` and `Binary`, which are `cpl_syntax::ast` types, and the facet texts of the generated headers), `cpl-diag` | `cpl-cabi` |
| `cpl-cabi` | `cpl-emit-c`, `cpl-eval`, `cpl-hir`, `cpl-project`, `cpl-rtspec`, `cpl-diag`; `cpl-sema`, `cpl-syntax`, and `cpl-rt` only as dev-dependencies (its tests check the programs they compare, name the operators of the coverage gate, and find the C compiler and the embedded runtime through `cpl_rt::testing`) | nothing |
| `cpl-cli` | every other crate (twelve) | nothing |

In one line: `cpl-diag <- cpl-syntax <- {cpl-project, cpl-hir}; cpl-hir <- {cpl-sema, cpl-eval, cpl-prove}; cpl-prove <- {cpl-sema, cpl-index, cpl-evolve}; cpl-project <- {cpl-sema, cpl-index, cpl-work}; cpl-sema <- cpl-index <- cpl-work; cpl-cli <- all` (`cpl-sema` is a dev-dependency of `cpl-eval`), and from Milestone 2 `cpl-rtspec <- {cpl-sema, cpl-eval, cpl-rt, cpl-emit-c, cpl-cabi}; {cpl-rt, cpl-eval, cpl-sema} <- cpl-emit-c <- cpl-cabi`. An edge to `cpl-sema` from `cpl-index` or `cpl-work` is added with the first item of that crate that needs a checked `Program`; until then they build without the checker.

The forbidden edges of the table are enforced by
`crates/cpl-cli/tests/workspace_dag.rs`, which reads every
`crates/*/Cargo.toml` (normal and dev dependencies; `toml` is a
dev-dependency of `cpl-cli` for it) and fails on any of them: no Milestone
1 crate depends on `cpl-rt`, `cpl-emit-c`, or `cpl-cabi`, so the checker
and the evaluator never come to depend on code emission, and the only new
edges into `cpl-sema` and `cpl-eval` go to the data-only `cpl-rtspec`;
`cpl-rtspec` depends on no internal crate and `cpl-rt` on none but
`cpl-rtspec` and `cpl-diag`; `cpl-emit-c` does not depend on `cpl-cabi`;
and every Milestone 1 edge stays as it is. That `cpl-emit-c` uses
`cpl-sema` only for the plan types and `cpl-eval` only for the pure helpers
of Section 1.7.4, never the checker's passes or the `Interpreter`, is a
review rule: the test cannot see calls.

Decision: [D-135] T2. `cpl-work` does not depend on `cpl-eval`; when the Milestone 6
scheduler needs to run checks (`typecheck`, `test(...)`, `prove(...)`) it
calls a `CheckRunner` trait object (Section 1.9) that `cpl-cli` implements
over `cpl-sema`, `cpl-eval`, and the verifier. Rationale: the scheduler's
state machine is tested with fake runners that return scripted results
(expired leases, flapping targets), which must not require an interpreter;
and the verifier of Milestone 5 would otherwise force a third dependency
edge. Rejected alternative: `cpl-work -> cpl-eval` (simpler call graph,
untestable scheduler).

Decision: [D-136] T3. every JSON document any command emits is a `serde` type
defined in `cpl-diag::json` (Section 1.4) or composed of such types. Crates
produce those wire types directly rather than private types converted at
the CLI. Rationale: the design requires that "the structured API and CLI
return the same underlying results"; one definition per document makes the
library API and the CLI identical by construction, and the only crate every
crate already depends on is `cpl-diag`. Rejected alternative: per-crate
result types with a conversion layer in `cpl-cli` (two definitions to keep
in step).

Decision: [D-265] T69. Milestone 2 adds exactly four crates, with the
dependencies and forbidden edges of the table above: `cpl-rtspec`, the one
source of every specification-owned text and table that both engines need
(Section 1.11, T70); `cpl-rt`, the C runtime of `spec/cabi.md` Section 2
as embedded source (Section 1.12); `cpl-emit-c`, emission from the typed
HIR and every native build (Section 1.13); and `cpl-cabi`, the toolchain's
side of the C boundary, a process boundary to the driver (Section 1.14).
Rationale: each crate is one independently testable concern, and the
forbidden edges keep the accepted checker and evaluator independent of the
backend that is validated against them: the evaluator gains a dependency
on data, never on code generation, and the toolchain never loads C into
its own process, so no `unsafe` FFI enters it. Rejected alternatives: a
lowered-IR crate between the HIR and C (a second representation of every
body to keep in sync with the HIR, which T1 rejects, with an ownership
verifier that would re-prove what the slot discipline of cabi.md Section 1
makes structural); a build-driver crate of its own (one more crate for
code whose only consumer is the emitter); one `cpl-cabi` that implements
the C API in Rust over `cpl-eval` (the earlier Milestone 2 row of Section
13: an embedded interpreter, not native execution, which would put the
parser and the checker into every package); moving the emitter's naming
and stub helpers into `cpl-hir` (it changes the crate T50 keeps minimal,
while calling `cpl-eval`'s own functions guarantees the same answers).

Decision: [D-240] T50. the typed HIR of Section 3.3 and `Program` with
every accessor method of Section 1.6 live in a crate of their own,
`cpl-hir`, which depends only on `cpl-diag` and `cpl-syntax` (of the
internal crates) and sits between `cpl-syntax` and `cpl-sema`.
`cpl-sema` builds the HIR and re-exports the crate as `cpl_sema::hir`
(`pub use cpl_hir as hir;`), so every `cpl_sema::hir` path in the
specifications stays valid and names the `cpl-hir` item. The one `Program`
operation that needs name resolution, selector resolution, is the free
function `cpl_sema::resolve_selector(&Program, ...)` instead of a method.
`cpl-eval` depends on `cpl-hir` rather than `cpl-sema`, and has `cpl-sema`
only as a dev-dependency, for its tests to check the programs they run;
`cpl-index` and `cpl-work` reach the HIR through `cpl-sema` (they load
projects through `cpl-project`, T52, and take the `cpl-sema` edge with
their first item that needs a checked `Program`). Rationale:
the evaluator and the index can then be built and tested while the
checker's internals are under construction, because the HIR's shape is
fixed by Section 3.3 and changes rarely: `cpl-eval` never compiles against
the checker, and tests of either crate can construct HIR values directly.
Rejected alternative: the HIR inside `cpl-sema` (an earlier revision of
this section), where any compile error in a checker pass blocks every
dependent crate.

Decision: [D-242] T52. project loading (the `cpl_sema::project` block of
Section 1.6 and the manifest and lock types of Section 11) is a crate of
its own, `cpl-project`, which depends only on `cpl-diag` and `cpl-syntax`
(of the internal crates) and also embeds the standard-library stubs (its
`build.rs`, Section 3.6). `cpl-sema` re-exports it (`pub use cpl_project
as project;`) and keeps the crate-root re-exports of loading
(`cpl_sema::load_project`, `cpl_sema::Project`, ...), so every
`cpl_sema::project` path in the specifications stays valid and names the
`cpl-project` item; `cpl-index` and `cpl-work` depend on `cpl-project`
directly. Rationale: the index and work crates need a loaded project
(identities, facet maps, revisions, and patch application are syntactic)
but must not depend on the checker's passes while those are under
construction, the reasoning of T50 for `cpl-hir`; the std embedding
belongs with loading because loading is what registers the std files.
Rejected alternative: loading inside `cpl-sema` (an earlier revision of
Sections 1.1, 1.6, and 3.6), where any compile error in a checker pass
blocks every crate that only needs to read a project.

### 1.3 External crates

Versions are "latest stable at implementation time"; the workspace manifest
pins the major version shown and `Cargo.lock` pins the exact release.
Adding a dependency not in this table requires a `Decision:` line in a
later revision of this document.

| Crate | Used by | Purpose | Why this crate; rejected alternatives |
| - | - | - | - |
| `num-bigint` 0.5, `num-traits` 0.2 | `cpl-hir` (`Lit::Int`, `PatternKind::Int`), `cpl-sema` (literal folding, refinement proofs on literals), `cpl-eval` (`Value::Int`), `cpl-rtspec` (`arith::div_rem`, T70), `cpl-emit-c` (the `Int` literals of emitted code), `cpl-cabi` (`Value::Int` in the node-list codec, Section 1.14) | arbitrary-precision `Int` | pure Rust, no C build dependency, `serde` support behind a feature, well-maintained; rejected `rug` (GMP, native build dependency on every platform), `i128` (not arbitrary precision; the design says Int is) |
| `serde` 1 (derive), `serde_json` 1 | `serde`: every crate but `cpl-sema`, `cpl-rtspec`, and `cpl-rt`, which define no serialized type; `serde_json`: every crate but `cpl-syntax`, `cpl-hir`, `cpl-rtspec`, and `cpl-rt` | every JSON wire type, `--json` output, `--arg` parsing, receipts, `build.json`, the driver protocol (Milestone 2) | the ecosystem standard; `serde_json` is built with the `arbitrary_precision` feature so `Int` round-trips as a JSON number of any length (Section 7.3), never with `preserve_order`, so that a `serde_json::Map` iterates in the byte order of its keys (T4), the order the C writers of `spec/cabi.md` 3.3.2 reproduce, and from Milestone 2 with `unbounded_depth`, used by `cpl-cabi`'s protocol reader alone (T71); rejected `miniserde` (no arbitrary precision, no derive flexibility) |
| `clap` 4 (derive) | `cpl-cli` | argument parsing, `--help`, usage errors | derive gives one struct per command that doubles as the documented contract; exit code 2 on usage error matches Section 7; rejected `argh`/`pico-args` (no value validation, weaker help) |
| `toml` 1 | `cpl-project`; `cpl-cli` as a dev-dependency (the DAG test of Section 1.2) | `cpl.toml`, `cpl.lock`, the declarative bindings (`.cbind.toml`, Milestone 2) | spec-complete TOML 1.x parser with `serde` integration and spanned errors; rejected a hand-written parser (manifests gain keys every milestone) |
| `blake3` 1 | `cpl-project`, `cpl-index`, `cpl-eval`, `cpl-rt`, `cpl-emit-c`; `cpl-cabi` as a dev-dependency (the seeds of its suites) | file, manifest, lock, and std digests, and the digests of the C build inputs (`cpl-project`); facet content hashes and module content hashes (`cpl-index`); the property seed of semantics S44 (`cpl-eval`); the runtime digest (`cpl-rt`); the program, export, compiler, flag, artifact, and native-dependency digests and the cache keys of objects and drivers (`cpl-emit-c`) | fast, 256-bit, stable output across platforms, no configuration; rejected `sha2` (slower, no advantage for non-cryptographic-protocol use), `xxhash`/`fxhash` (not collision-resistant; hashes are identities in receipts) |
| `rand` 0.10 (the infallible generator trait `rand::Rng`), `rand_chacha` 0.10 | `cpl-eval` (property runner, `ChaCha20Rng` through `rand_chacha::rand_core`), `cpl-index` (`generate_id`, `plan_id_assignments`, taking `&mut dyn rand::Rng`; `rand_chacha` only as a dev-dependency, its tests seeding them), `cpl-cli` (OS randomness for ids and run seeds: `rand::rng()` or `rand::rand_core::UnwrapErr(rand::rngs::SysRng)`; rand 0.10 exports neither `RngCore` at its root nor `OsRng`), `cpl-cabi` as a dev-dependency (the input sweeps D5 and the runtime fuzzing D7, Section 12.6) | seeded property generators; the random source of `cpl id assign` | `ChaCha20Rng::from_seed` is a fully specified stream, so a seed reproduces a case sequence independent of `rand`'s internal seeding helpers; rejected `StdRng` (algorithm may change between `rand` releases), `SmallRng` (not portable across word sizes) |
| in-house shrinking (no crate) | `cpl-eval` | shrinking failed property inputs | shrinking must respect CPL types and refinement predicates (`Positive` shrinks toward 1, not 0) and operate on `Value`; rejected `proptest` and `quickcheck` (strategies are Rust-type-directed and cannot be generated from CPL `for` clauses at run time) |
| `git` CLI via `std::process::Command` (no crate) | `cpl-index` (revision, trees), `cpl-work` (refs, worktrees, commits) | all Git operations in Milestone 1 | uses the user's installed Git, credential helpers, hooks, and configuration unchanged; no native build dependency; the command set is small (Section 9.3); rejected `git2`/libgit2 (native dependency, separate configuration surface, worktree support lags the CLI) and `gitoxide` (API still moving) |
| the system C compiler via `std::process::Command` (no crate; Milestone 2) | `cpl-emit-c` (compiler discovery, compilation, linking), the tests of `cpl-rt`, `cpl-emit-c`, and `cpl-cabi` | compiling the runtime, the emitted program, adapters, and vendored sources; linking packages, drivers, and executables (`spec/cabi.md` Section 1) | the C compiler is the one native tool the C boundary needs anyway (T68); it is selected explicitly and recorded (T72) and pinned by digest in the lock (Section 11.2); rejected the `cc` crate (build-script oriented, with defaults taken from `CC` and `CFLAGS`, hidden inputs T72 rejects) and LLVM or Cranelift lowering (cabi.md Section 1) |
| `thiserror` 2 | `cpl-project`, `cpl-eval`, `cpl-index`, `cpl-work`, `cpl-cabi`: the crates with library-level error enums | error enums for library-level errors (`LoadError`, `GitError`, ...) | derive for `std::error::Error`; rejected `anyhow` (type-erased errors hide which failures are usage errors versus internal errors, which Section 7 distinguishes by exit code) |

`cpl-work` uses neither `rand` nor `blake3`: a `ChangeRunId` is built from
10 random bytes its caller draws, and a `CandidateId` is hashed through
`cpl_index::facet_hash` (Section 10.1).

Decision: [D-137] T4. no other runtime dependencies in Milestone 1: no `tracing`,
no `rayon`, no `indexmap`, no interning crate. `std::collections::BTreeMap`
is used wherever iteration order matters (every JSON map, every facet map)
so output is deterministic without a sort step. Rationale: the toolchain's
outputs are compared byte-for-byte in tests and committed in receipts;
determinism is cheaper to get from ordered maps than from sorting at every
emission point. Rejected alternative: `HashMap` plus sorting at emission.

Decision: [D-267] T71. Milestone 2 adds no external crate: its crates use
crates of the table above (`num-bigint` in `cpl-rtspec`, `cpl-emit-c`, and
`cpl-cabi`; `blake3` in `cpl-rt` and `cpl-emit-c`; `serde` and
`serde_json` in `cpl-emit-c` and
`cpl-cabi`; `thiserror` in `cpl-cabi`; the system C compiler through
`std::process::Command`), and the workspace's `serde_json` gains the
feature `unbounded_depth`. Only
`cpl-cabi`'s protocol reader uses it: it calls
`Deserializer::disable_recursion_limit()` explicitly, on the evaluation
thread, to read the payload of a native fault record, which can be as deep
as the values it holds (`spec/cabi.md` Section 3); every other reader keeps
`serde_json`'s 128-level limit, which the feature does not change, so the
`--arg` reader of Section 7.2 refuses what it refused before. Rationale:
the C boundary needs nothing that the C compiler and the standard library
do not already provide, and every candidate crate would bring hidden
inputs, native tooling, or C loaded into the toolchain's own process.
Rejected alternatives: the `cc`
crate (build-script oriented, environment-driven defaults, `CC` and
`CFLAGS`, which T72 rejects as hidden inputs); `libloading` and `libc` (the
toolchain loads no C, Section 1.14, and stack bounds are the runtime's
business); `bindgen` (it needs libclang, and generated headers are
reviewed contracts, cabi.md Section 6); `libffi` (dynamic calls with no
compile-time check against the real headers); `llvm-sys`, `inkwell`, and
`cranelift-*` (cabi.md Section 1).

Minimum supported Rust version: the latest stable at implementation time;
CI builds with that version only.

### 1.4 `cpl-diag` public API

`cpl-diag` owns the source model, the diagnostic model, and every shared
wire type. It has no CPL-specific logic beyond the facet vocabulary.

```rust
// ---- cpl_diag::source -------------------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct FileId(pub u32);

/// Byte offsets into the file's text, half-open, `start <= end`. `Ord` (by file, then start, then end) lets spans key
/// sorted collections, such as the HIR's references "sorted by (from, span)" (Section 3.3).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Span { pub file: FileId, pub start: u32, pub end: u32 }
impl Span {
    pub fn new(file: FileId, start: u32, end: u32) -> Span;
    pub fn point(file: FileId, at: u32) -> Span;   // the empty span at `at`
    pub fn join(self, other: Span) -> Span;        // the covering span: same file (the result keeps `self.file`); min start, max end
    pub fn len(self) -> u32;                       // 0 for an empty span
    pub fn is_empty(self) -> bool;
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]               // "project", "std", {"revision": "<40 hex>"}, "virtual"
pub enum FileOrigin {
    Project,                 // read from the project root (or a VirtualTree of it)
    Std,                     // embedded standard-library stub
    Revision(String),        // materialized from a git commit (40-hex id) for `cpl diff` / baselines
    Virtual,                 // text supplied by a command (a selector argument, a patch body under test)
}

pub struct SourceFile {
    pub id: FileId,
    /// '/'-separated. Project files: relative to the project root ("src/reader/pages.cpl").
    /// Std files: "std/<name>.cpl". Virtual files: "<virtual:NAME>".
    pub path: String,
    pub origin: FileOrigin,
    text: std::sync::Arc<str>,
    line_starts: Vec<u32>,                         // LF, CRLF, and a lone CR each end a line; line_starts[0] is 0, or the length of a leading BOM
}
impl SourceFile {
    pub fn text(&self) -> &str;
    pub fn shared_text(&self) -> std::sync::Arc<str>;
    pub fn line_count(&self) -> u32;              // a final terminator starts one more (empty) line; an empty file has one line
    pub fn line_text(&self, line: u32) -> &str;   // 1-based, without the terminator ("" out of range)
    pub fn line_col(&self, offset: u32) -> LineCol;   // offsets clamp to the text and back onto a character boundary
    pub fn location(&self, span: Span) -> Location;
    pub fn slice(&self, start: u32, end: u32) -> &str;
}

#[derive(Clone, Debug, Default)]
pub struct SourceMap { files: Vec<SourceFile> }
impl SourceMap {
    pub fn new() -> Self;
    pub fn add(&mut self, path: String, origin: FileOrigin, text: String) -> FileId;
    /// A file read as bytes. Invalid UTF-8 is the parse diagnostic `invalid_utf8` (grammar 1.1, D70), returned here:
    /// the file is still registered, its bytes decoded lossily (each invalid sequence becomes U+FFFD), so that it has a
    /// FileId and the diagnostic a span (the first replacement character); the caller treats the file as failed and
    /// does not lex it. The message is `invalid UTF-8 at byte <offset>`.
    pub fn add_bytes(&mut self, path: String, origin: FileOrigin, bytes: &[u8]) -> (FileId, Option<Diagnostic>);
    pub fn file(&self, id: FileId) -> &SourceFile;              // panics on an id this map did not issue
    pub fn get(&self, id: FileId) -> Option<&SourceFile>;
    pub fn files(&self) -> impl Iterator<Item = &SourceFile>;  // id order
    pub fn find(&self, path: &str) -> Option<FileId>;           // the first file registered under `path`
    pub fn text(&self, span: Span) -> &str;
    pub fn line_col(&self, file: FileId, offset: u32) -> LineCol;
    pub fn location(&self, span: Span) -> Location;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}

/// 1-based line; 1-based column counted in Unicode scalar values from the line start. Lines end at LF, CRLF, or a
/// lone CR (the lexer still rejects a lone CR, `invalid_line_terminator`, but every offset keeps a defined location);
/// a leading byte-order mark is not a column: column 1 of line 1 is the first scalar value after it, and an offset
/// inside the mark maps to 1:1.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct LineCol { pub line: u32, pub col: u32 }

/// The JSON form of a span: `end_col` exclusive; a point span has `end_line == line` and `end_col == col` (semantics S49).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Location { pub file: String, pub line: u32, pub col: u32, pub end_line: u32, pub end_col: u32 }

// ---- cpl_diag::diag ---------------------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity { Error, Warning, Info }        // the derived order is the severity key of Diagnostics::sorted; as_str, Display

/// A snake_case code: spec/semantics.md 9.4 for the language categories, Appendix B for the toolchain categories.
/// Codes are stable strings: tests and agents match on them. A DiagCode dereferences to `str`, compares equal to `str`,
/// `&str`, and `String`, displays as the bare code, and serializes as a plain string.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct DiagCode(pub &'static str);
impl DiagCode { pub const fn as_str(self) -> &'static str; }

/// semantics S49 categories plus the toolchain-owned ones (Appendix B).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category { Parse, Resolve, Type, Effect, Contract, Hole, Test, ReleasePolicy, Runtime, Internal,
                    Project, Index, Git, Work, Patch, Usage,
                    Build, Interop }              // Milestone 2 (T76): "build" (Appendix B), "interop" (codes in spec/cabi.md Section 4)
impl Category { pub const ALL: [Category; 18]; pub fn as_str(self) -> &'static str; pub fn parse(s: &str) -> Option<Category>; }

/// The fixed keys of the diagnostic object (T5): `with_data` asserts in debug builds that a payload key is none of
/// them, and `to_json` drops a payload key that is.
pub const RESERVED_KEYS: [&str; 10] = ["code", "category", "severity", "message", "span", "symbol", "qualified_name",
                                       "facet", "notes", "backtrace"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: DiagCode,
    pub category: Category,
    pub severity: Severity,
    pub message: String,                          // one sentence; never the only carrier of a fact a field carries
    pub span: Option<Span>,                       // primary location
    pub symbol: Option<StableId>,                 // blamed declaration, when known
    pub qualified_name: Option<String>,           // always set together with `symbol`
    pub facet: Option<Facet>,                     // blamed facet, when known
    pub notes: Vec<Note>,                         // related locations and explanations
    pub backtrace: Vec<BacktraceFrame>,           // faults only; innermost first
    /// Code-specific payload (`clause`, `input`, `actual`, `expected`, `status`, ...); flattened into the JSON object.
    pub data: serde_json::Map<String, serde_json::Value>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note { pub message: String, pub span: Option<Span> }
impl Note { pub fn new(message: impl Into<String>) -> Note; pub fn at(message: impl Into<String>, span: Span) -> Note; }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktraceFrame { pub qualified_name: String, pub span: Option<Span> }   // BacktraceFrame::new(qualified_name, span)

impl Diagnostic {
    pub fn new(code: DiagCode, category: Category, severity: Severity, message: impl Into<String>) -> Self;
    pub fn error(code: DiagCode, category: Category, message: impl Into<String>) -> Self;
    pub fn warning(code: DiagCode, category: Category, message: impl Into<String>) -> Self;
    pub fn info(code: DiagCode, category: Category, message: impl Into<String>) -> Self;
    pub fn with_span(self, span: Span) -> Self;
    pub fn with_note(self, message: impl Into<String>, span: Option<Span>) -> Self;
    pub fn with_symbol(self, id: StableId, qualified_name: impl Into<String>, facet: Option<Facet>) -> Self;
    pub fn with_facet(self, facet: Facet) -> Self;
    pub fn with_backtrace(self, frames: Vec<BacktraceFrame>) -> Self;
    pub fn with_data(self, key: &str, value: serde_json::Value) -> Self;
    pub fn is_error(&self) -> bool;
    /// Every span resolved through `sm`; a span whose file `sm` does not know is omitted, and a payload key that is
    /// one of RESERVED_KEYS is dropped.
    pub fn to_json(&self, sm: &SourceMap) -> json::DiagnosticJson;
    pub fn render(&self, sm: &SourceMap) -> String;   // human form, Section 1.4.1
}   // Display writes line 1 of the human form, `<severity>[<code>]: <message>`

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Diagnostics(Vec<Diagnostic>);           // From<Vec<Diagnostic>>, Into<Vec<Diagnostic>>, FromIterator<Diagnostic>
impl Diagnostics {
    pub fn new() -> Self;
    pub fn push(&mut self, d: Diagnostic);
    pub fn extend(&mut self, other: impl IntoIterator<Item = Diagnostic>);
    pub fn has_errors(&self) -> bool;
    pub fn error_count(&self) -> usize;
    pub fn warning_count(&self) -> usize;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    /// Stable order: (file path, span.start, severity, code); diagnostics without a span (or whose file `sm` does not
    /// know) last, in insertion order (a stable sort, T46).
    pub fn sorted(self, sm: &SourceMap) -> Vec<Diagnostic>;
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic>;
    pub fn as_slice(&self) -> &[Diagnostic];
    pub fn into_vec(self) -> Vec<Diagnostic>;
}
/// The same operations on a plain slice: the sorted order in place, any error, and every rendering concatenated.
pub fn sort_diagnostics(diagnostics: &mut [Diagnostic], sm: &SourceMap);
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool;
pub fn render_all(diagnostics: &[Diagnostic], sm: &SourceMap) -> String;

// ---- cpl_diag::facet --------------------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Facet { Identity, Name, Signature, Contract, Effects, Body, Docs, VerificationPolicy }
impl Facet {
    /// Canonical order used by every map and by the `.definition` hash.
    pub const ALL: [Facet; 8] = [Facet::Identity, Facet::Name, Facet::Signature, Facet::Contract,
                                 Facet::Effects, Facet::Body, Facet::Docs, Facet::VerificationPolicy];
    pub fn as_str(self) -> &'static str;          // "verification_policy", ...
    pub fn parse(s: &str) -> Option<Facet>;       // `definition` is not a facet (FacetSelector::parse); Display, FromStr
}
/// Serialized as a plain string, the facet name or "definition" (the `targets` of `cpl work check`, Section 7.2), by
/// hand-written Serialize and Deserialize impls; deserializing any other string is an error.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum FacetSelector { Facet(Facet), Definition }
impl FacetSelector {
    pub fn as_str(self) -> &'static str;          // the facet name, or "definition"
    pub fn parse(s: &str) -> Option<FacetSelector>;
    pub fn facets(self) -> Vec<Facet>;            // in canonical order; all eight for Definition
    pub fn includes(self, facet: Facet) -> bool;
}   // From<Facet>, Display, FromStr

// ---- cpl_diag::id -----------------------------------------------------------
/// An `@id("...")` value or a provisional identity (Section 4). Validated on construction, deserialization included
/// (Deserialize is written by hand and calls `parse`); a provisional identity is stored in its canonical
/// `SymbolPath::display` spelling.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize)]
#[serde(transparent)]
pub struct StableId(String);
impl StableId {
    /// Either form: a string containing `.` must parse as a symbol path (Section 4.4) and is stored canonically; any
    /// other string must have the accepted `@id` syntax of Section 4.1.
    pub fn parse(s: &str) -> Result<StableId, IdError>;
    pub fn from_annotation(s: &str) -> Result<StableId, IdError>;   // the @id form only: what an `@id` argument must match
    pub fn as_str(&self) -> &str;
    pub fn is_provisional(&self) -> bool;                 // a qualified name: contains '.', which no accepted @id value does
    pub fn kind_prefix(&self) -> &str;                    // "fn", "ty", "cap", "test", "prop", or "" for nonstandard and provisional ids
    /// The qualified name `module.path.name` (semantics S2), Section 4.3.
    pub fn provisional(module_path: &str, name: &DeclName) -> StableId;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdError {
    Length(usize),                                 // an @id value with fewer than 4 or more than 64 characters (the length found)
    Syntax(String),                                // an @id value not matching ^[a-z][a-z0-9]*(_[a-z0-9]+)+$
    Path(String),                                  // a provisional identity (it contains `.`) that is not a valid symbol path: the SymbolPath::parse message
}   // Display, std::error::Error
/// A TEXT literal in canonical form (formatter rule 10, Section 2.4), quotes included: the spelling of a test or
/// property name in a symbol path and in `name` canonical text.
pub fn quote_text(value: &str) -> String;
// `SymbolPath` and `DeclName` (Section 4.4) are defined in this module as well.

// ---- cpl_diag::version ------------------------------------------------------
pub mod version {
    pub const TOOLCHAIN: &str = env!("CARGO_PKG_VERSION");   // workspace version, e.g. "0.1.0"
    pub const EDITION: &str = "0.1";
    pub const JSON_SCHEMA: u32 = 1;                           // bumped when any wire type changes incompatibly
}
```

The `StableId` newtype lives here, not in `cpl-index`, because `cpl-sema`
attaches identities to definitions before `cpl-index` exists in the DAG;
generation, assignment, and provisional derivation are `cpl-index`
functions (Section 1.8).

#### 1.4.1 Human rendering

`Diagnostic::render` produces exactly this layout (one trailing newline):

```text
error[type_mismatch]: expected `Text`, found `Int`
  --> src/reader/pages.cpl:12:5
   |
12 |     page.len
   |     ^^^^^^^^
   |
   = note: the tail expression is the function's return value
```

Line 1 is `<severity>[<code>]: <message>` and line 2 is `  --> <file>:<line>:<col>`
(omitted when the diagnostic has no span). These two lines are a stable
contract (tests and agents may grep them); the gutter block and notes are
not. Below the gutter block, notes without a span render as `= note:
<message>` lines and a backtrace as `= at <qualified_name> (<file>:<line>)`
lines (`= at <qualified_name>` for a frame without a span); notes with a
span follow after them as `note: <message>` blocks, each with its own
`-->` line and gutter block. The caret run carries no label (the message is
line 1), covers the span's scalar values on the first line of the span
only, and has at least one caret; a tab displays as four columns.
`warning` and `info` use the same shape (`info[hole]: ...`). Other
non-ASCII text is printed unchanged. `Display` for `Diagnostic` writes line
1 alone.

#### 1.4.2 `cpl_diag::json`: shared wire types

```rust
pub mod json {
    use super::*;
    /// A JSON object (a sorted map, T4), and the object a serializable payload produces, for flattening into
    /// `Diagnostic::data` (a hole report, a test record); `to_data` panics when `value` is not an object.
    pub type JsonMap = serde_json::Map<String, serde_json::Value>;
    pub fn to_data<T: Serialize + ?Sized>(value: &T) -> JsonMap;

    /// Common envelope of every `--json` output (Section 7.1).
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Envelope<T> {
        pub command: String,                       // "check", "test", ...
        pub toolchain: ToolchainInfo,
        pub revision: Option<Revision>,            // None exactly when no project is involved (T60): none is located or it does not load, the form reads none (`fmt --stdin`, loose files, `work run`/`work integrate`), clap refused the command line, or a panic is reported
        pub status: Status,                        // "ok" | "unsuccessful" | "error"  (exit code 0 | 1 | 2); never a proof or test status (T44)
        pub diagnostics: Vec<DiagnosticJson>,      // in `Diagnostics::sorted` order (Section 7.1, T46)
        #[serde(flatten)] pub result: T,
    }
    /// The result of a command whose envelope carries no keys of its own (a usage error, a load failure).
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct NoOutput {}
    #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct ToolchainInfo { pub version: String, pub edition: String, pub json_schema: u32 }   // ToolchainInfo::current(): cpl_diag::version
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
    pub enum Status { Ok, Unsuccessful, Error }   // exit_code(): 0, 1, 2; as_str()

    /// Exactly the object of spec/semantics.md S49. Optional fields are omitted when absent, never null.
    #[derive(Serialize, Deserialize)]
    pub struct DiagnosticJson {
        pub code: String,
        pub category: Category,
        pub severity: Severity,
        pub message: String,
        #[serde(skip_serializing_if = "Option::is_none")] pub span: Option<Location>,
        #[serde(skip_serializing_if = "Option::is_none")] pub symbol: Option<StableId>,
        #[serde(skip_serializing_if = "Option::is_none")] pub qualified_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")] pub facet: Option<Facet>,
        #[serde(skip_serializing_if = "Vec::is_empty")] pub notes: Vec<NoteJson>,   // { "message": String, "span"?: Location }; omitted when empty (S49)
        #[serde(skip_serializing_if = "Vec::is_empty")] pub backtrace: Vec<BacktraceJson>,   // { "qualified_name": String, "span"?: Location }
        #[serde(flatten)] pub data: serde_json::Map<String, serde_json::Value>,
    }

    /// Section 6.3. `commit` is None (`null`) when the project is not in a git repository or has no commits.
    #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Revision { pub commit: Option<String>, pub state: RevisionState, pub digest: Digest }
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
    pub enum RevisionState { Clean, Worktree }

    /// blake3-256 of some bytes, 64 lowercase hex characters.
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] #[serde(transparent)]
    pub struct Digest(pub String);                 // as_str(), Display
    /// blake3-256 of a facet's canonical text, truncated to 32 lowercase hex characters (Section 5.3).
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] #[serde(transparent)]
    pub struct FacetHash(pub String);              // as_str(), Display

    /// One symbol's facet versions, as embedded in inspect responses, receipts, hole reports, and candidates.
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct SymbolVersion {
        pub id: StableId,
        pub path: String,                          // display symbol path, Section 4.4
        pub provisional: bool,
        pub facets: BTreeMap<Facet, FacetHash>,    // only facets applicable to the declaration kind, in Facet::ALL order
        pub definition: FacetHash,                 // the `.definition` hash, Section 5.4
    }

    /// Vocabulary enums that wire types use but other sections own are defined here too, `cpl-diag` being the only
    /// crate below all of their users, and re-exported where those sections place them: ProofStatus as
    /// cpl_sema::check::ProofStatus (Section 1.6), BindingKind as cpl_sema::hir::BindingKind (1.6, 3.3), Include as
    /// cpl_index::query::Include (1.8), and ExecutionBoundary with its three access enums as cpl_work::scheduler items
    /// (10.8; the `[work.policy]` section of cpl_sema::project needs them, Section 11.1). Their definitions are in
    /// those sections; each also has `as_str`.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
    pub enum Relation { Call, TypeUse, Ctor, FieldAccess, CapabilityOp, Check, Direct, Transitive }   // SymbolBlock::relation (6.4): a RefKind in dependency lists, direct/transitive in `tests`
    // Further wire types are defined where they are used and listed in Appendix C, with the names this module gives
    // the objects the sections show without one: CheckOutput, FmtOutput, RunOutput, TestOutput, TestReport,
    // TestCaseRecord, InspectOutput, ProveOutput, VerificationReceipt, TestReceipt, DiffOutput, DiffStatement,
    // IdAssignOutput, WorkCheckOutput, HoleReportJson, and their parts. All live in this module. Milestone 2 adds
    // BuildOutput with BuildArtifact, BuildInput, BuildExport, BuildBinding, BuildLock, CompilerInfo, and RuntimeInfo
    // (`cpl build`), and EngineKind, RunBinding, EngineRunOutput, and EngineTestOutput (`--engine`), Section 7.2; it
    // changes no Milestone 1 wire type, so version::JSON_SCHEMA stays 1 (T73).
}
```

The design document's failure response is a `DiagnosticJson` with code
`contract_counterexample`, `symbol`, `facet`, and flattened `data` keys
`clause`, `input`, `actual`, `status` (emitted by the test runner, Section
1.7.3). `status` inside a diagnostic follows the vocabulary of semantics
S50 and is distinct from the envelope's `status`.

Decision: [D-138] T5. structured payload fields of a diagnostic are flattened into
the diagnostic object; the fixed keys (`code`, `category`, `severity`,
`message`, `span`, `symbol`, `qualified_name`, `facet`, `notes`,
`backtrace`) are reserved and a payload may not use them. Rationale: the design's example puts `clause`, `input`,
`actual`, and `status` at top level beside `code`, `symbol`, and `facet`.
Rejected alternative: a nested `data` object (cleaner schema, but the
design's example would not be a valid instance). This is also semantics
S49's conclusion; the two documents define one object.

Decision: [D-139] T44. the envelope's `status` vocabulary is `ok`, `unsuccessful`,
`error` (exit codes 0, 1, 2), and test records and receipts use semantics
S50's test vocabulary `tested` / `failed`; no document emitted by this
toolchain spells a command outcome or a test outcome with a word of the
verification vocabulary (`proved`, `failed`, `unknown`) other than the
shared `failed` of S50. Rationale: the design keeps `proved`, `failed`,
and `unknown` distinct from `tested` (line 241); an envelope that said
`status: failed` for `cpl prove` with `result: unknown` would collapse
`unknown` into `failed` for any consumer reading only the top level, and
`passed` would drift from S50's documented word. Rejected alternatives:
`status: failed` for exit code 1 (an earlier revision); `passed` for
tests.

### 1.5 `cpl-syntax` public API

`crates/cpl-syntax` implements this section over grammar Sections 1-5 and
the formatter of Section 2.4. Its public modules are `ast` (Section 2.2),
`codes`, `dump`, `formatter`, `lexer`, and `parser`; the crate root
re-exports the items marked `(root)`. Every diagnostic it produces is an
`cpl_diag::Diagnostic` (Section 1.4) of category `parse`, whose code is one
of the `codes` constants and whose message and data fields follow grammar
D72.

```rust
// ---- cpl_syntax::lexer (grammar Section 1) -------------------------------------
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenKind { /* exactly spec/grammar.md Appendix A, in that order */ }        // (root)
impl TokenKind {
    pub fn keyword(spelling: &str) -> Option<TokenKind>;  // the keyword token of one of the 33 reserved spellings
    pub fn is_keyword(self) -> bool;                      // true for the 33 reserved keyword kinds
    pub fn spelling(self) -> Option<&'static str>;        // the fixed spelling of a keyword, `_`, or punctuation token
    pub fn describe(self) -> String;                      // "end of line", "end of file", "`{`", ... (grammar D72)
}
/// The eight spellings reserved for a future edition (grammar D4), lexed as `TokenKind::ReservedWord`.
pub const RESERVED_FUTURE_WORDS: &[&str];
/// Keyword, `ReservedWord`, `Underscore`, `UpperIdent`, or `LowerIdent` for an identifier spelling (grammar 1.3, 1.4).
pub fn classify_identifier(spelling: &str) -> TokenKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {                                                                      // (root)
    pub kind: TokenKind,
    pub span: Span,
    /// Payload (grammar Appendix A): Int -> digit string without `_`; Text -> decoded value; keywords, reserved words,
    /// LowerIdent/UpperIdent -> spelling; Hole -> name without `?`; DocComment -> text after `///` with one leading
    /// space removed. Empty for punctuation, Newline, and Eof.
    pub text: String,
}
impl Token { pub fn describe(&self) -> String; }   // the `found` text of D72: "identifier `x`", "integer literal `1`", ...

/// Trivia, returned out of band in source order (Section 2.3): ordinary `//` comments and blank lines.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TriviaKind { Comment, BlankLine }                                              // (root)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TriviaToken {                                                                // (root)
    pub kind: TriviaKind,
    pub span: Span,                  // Comment: from `//` to the end of the line; BlankLine: the blank line's terminator
    pub line_start: u32,             // byte offset of the first character of the line the trivia is on
}
#[derive(Clone, Debug, Default)]
pub struct Lexed { pub tokens: Vec<Token>, pub trivia: Vec<TriviaToken>, pub diagnostics: Vec<Diagnostic> }   // (root)
/// Both never panic, keep lexing after an error, and end the tokens with an Eof token; `lex` drops the trivia.
pub fn lex(src: &str, file: FileId) -> (Vec<Token>, Vec<Diagnostic>);                  // (root)
pub fn lex_full(src: &str, file: FileId) -> Lexed;                                     // (root)

// ---- cpl_syntax::parser (grammar Sections 2-5) ---------------------------------
#[derive(Clone, Debug)]
pub struct ParseResult<T> {                                                             // (root)
    /// Some iff the file lexed and parsed without error. Otherwise (grammar D59, D70) `value` is None and
    /// `diagnostics` holds exactly one diagnostic: the earliest error by offset.
    pub value: Option<T>,
    pub diagnostics: Vec<Diagnostic>,              // a `parse` code of spec/semantics.md 9.4 (T6), category parse
}
impl<T> ParseResult<T> {
    pub fn has_errors(&self) -> bool;
    pub fn into_result(self) -> Result<T, Vec<Diagnostic>>;   // Ok(value), or Err(diagnostics) when the parse failed
}
pub fn parse_module(src: &str, file: FileId) -> ParseResult<ast::Module>;              // (root) `.cpl`: source_file
pub fn parse_work(src: &str, file: FileId) -> ParseResult<ast::WorkFile>;              // (root) `.work`: work_file
pub fn parse_patch(src: &str, file: FileId) -> ParseResult<ast::PatchFile>;            // (root) `.patch`: patch_file
/// Exactly one expression with nothing after it: the constant-expression arguments of `cpl run` (Section 7.2). The text
/// is read with the `expr` grammar of grammar 3.12 as an expression statement reads it (struct literals are allowed,
/// D41, and a line break outside brackets ends the expression); line breaks and ordinary comments before and after it
/// are allowed (a doc comment anywhere in the text is `doc_comment_position`, grammar 1.2), and any other token after it
/// is `syntax_error` (`expected end of input after the expression, found <token>`). No constant-expression restriction
/// is applied here (names, calls, holes, and `if` parse; the caller rejects them). The ParseResult contract is the file parsers': a value, or exactly one diagnostic, the earliest.
pub fn parse_expr(src: &str, file: FileId) -> ParseResult<ast::Expr>;                  // (root)
/// The implementation limits of grammar D73, at the minimum the grammar guarantees.
pub const MAX_DEPTH: u32 = 64;                     // nesting levels; beyond: `nesting is too deep (more than 64 levels)`
pub const MAX_EXPR_OPS: u32 = 1000;                // operators along a path of nested expressions; beyond: `an expression has too many operators ...`

// ---- cpl_syntax::formatter (Section 2.4) ---------------------------------------
pub const MAX_WIDTH: usize = 100;                  // rule 6 (iii): the column budget of a line, in scalar values
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FileKind { Cpl, Work, Patch }                                                  // (root)
impl FileKind {
    pub fn from_path(path: &std::path::Path) -> Option<FileKind>;   // by extension: `cpl`, `work`, `patch`
    pub fn extension(self) -> &'static str;                         // "cpl", "work", "patch"
}
pub fn format_module(m: &ast::Module) -> String;                                        // (root)
pub fn format_work(w: &ast::WorkFile) -> String;                                        // (root)
pub fn format_patch(p: &ast::PatchFile) -> String;                                      // (root)
/// Parse then format. Err carries the parse diagnostic when the file has an error: the formatter never rewrites a
/// file it could not fully parse (T7).
pub fn format_source(src: &str, kind: FileKind, file: FileId) -> Result<String, Vec<Diagnostic>>;   // (root)
/// One fragment in isolation at indentation level 0, with LF line ends and no trailing newline (below and Section 5.2).
pub fn format_fragment(f: Fragment<'_>, mode: FragmentMode) -> String;                 // (root)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FragmentMode { Canonical, Source }                                            // (root)
#[derive(Copy, Clone, Debug)]
pub enum Fragment<'a> {                                                                 // (root)
    Declaration(&'a ast::Declaration),             // the whole declaration including docs and annotations
    Signature(&'a ast::Declaration),               // Section 5.2 signature text (name replaced by `_`)
    Contract(&'a ast::Declaration),                // requires lines, ensures lines, decreases line
    Effects(&'a ast::EffectsClause),
    Block(&'a ast::Block),
    Docs(&'a ast::Docs),
    Annotations(&'a [ast::Annotation]),
    Expr(&'a ast::Expr),
    Type(&'a ast::Type),
    Selector(&'a ast::Selector),
}

// ---- cpl_syntax::dump (Section 12.1) -------------------------------------------
pub fn dump_module(module: &ast::Module) -> String;                                     // (root)
pub fn dump_work(work: &ast::WorkFile) -> String;                                       // (root)
pub fn dump_patch(patch: &ast::PatchFile) -> String;                                    // (root)

// ---- cpl_syntax::codes ---------------------------------------------------------
pub const CATEGORY: Category = Category::Parse;    // the category of every diagnostic of the crate
/// One constant per `parse` code of spec/semantics.md 9.4, named by the code in upper case: INVALID_UTF8 =
/// DiagCode("invalid_utf8") (raised when a file's bytes are read, grammar D70, never by the parser), INVALID_LINE_TERMINATOR,
/// NON_ASCII_OUTSIDE_TEXT, INVALID_CHARACTER, UNTERMINATED_TEXT_LITERAL, INVALID_ESCAPE, INVALID_INT_LITERAL,
/// FLOAT_LITERAL_UNSUPPORTED, LITERAL_ADJACENT_IDENTIFIER, PREFIXED_LITERAL_RESERVED, DOC_COMMENT_POSITION,
/// RESERVED_WORD, HOLE_NAME_CASE, HOLE_NAME_KEYWORD, SYNTAX_ERROR, DUPLICATE_CLAUSE, USE_AFTER_DECLARATION,
/// POSITIONAL_AFTER_USING, MISPLACED_ELSE, UNKNOWN_REQUIREMENT, DUPLICATE_WORK_ITEM.
pub const SYNTAX_ERROR: DiagCode = DiagCode("syntax_error");   // ... and likewise for the other twenty
pub const ALL: &[DiagCode];                        // the 21 codes, in the order of the 9.4 table
```

`format_fragment` produces the facet canonical texts of Section 5.2 that
`cpl-index` hashes, the rendering from which T56 derives the one-line
clause text of the HIR's `Clause::text` (Section 3.3), and the text
`cpl-work` splices into a file when it applies a patch (Section 10.5).
`FragmentMode::Canonical` skips every `Trivia` and every
`trailing_comments` list, so no ordinary comment and no blank line
appears; it ignores the tree's layout memory (`Group::multiline`,
`BinaryExpr::break_before_op`) and the width budget, so every comma group
prints flat and no line is broken for width or before an operator; blocks
stay multi-line (2.4 rule 8), one statement per line, and parentheses are
kept as written. `FragmentMode::Source` is exactly what the formatter
prints for the fragment at level 0, comments and blank lines included; for
`Signature`, `Contract`, and `Effects` it has the shape of the canonical
text plus the comments and the source's layout. The texts, with the
canonical forms of corpus declarations:

| Variant | Text |
| - | - |
| `Declaration` | the declaration as a formatted file prints it: docs, annotations, header, clauses, body (`pub type PageId(Text)`); in `Source` mode with its leading and trailing comments |
| `Signature` | the `signature` facet: `pub fn _<T>(items: List<T>, size: Positive) -> List<List<T>>` (parameters flat, `using` parameters included: `pub fn _(id: PageId, using db: PageRead) -> Result<Page, StoreError>`); `pub type _(Text)`, `pub type _ = Int` (the predicate is the contract), `pub type _<T>` for a std opaque type; `pub struct _ {` LF `    id: PageId` LF `    text: Text` LF `}`, enums and capabilities alike (operations without contracts), and an empty body as `pub struct _ {}`; a property's `for x in <gen>` lines; `""` for a test |
| `Contract` | `requires` lines in source order, then `ensures` lines, then the `decreases` line, unindented (`requires b != 0` LF `requires a % b == 0` LF `ensures result * b == a`); `where self > 0` for a refinement; for a capability, per operation with clauses, `fn name` LF and its `    requires` lines then its `    ensures` lines; `""` when there are none |
| `Effects` | `effects { codec.dimensions, db.get, db.put }`: the references sorted by their `param.op` string and deduplicated (T26), `""` for an empty set; in `Source` mode the author's order with any repetition (T13), and `effects {}` for an empty set |
| `Block` | `{` LF the statements one level deeper LF `}` (or `{}`), loop `invariant` and `decreases` clauses included, with no blank lines |
| `Docs` | the doc lines joined with LF, each without `///` and without trailing spaces and tabs, the horizontal whitespace of grammar 1.1 (other Unicode white space at a line's end, such as U+00A0, is text and stays; `Docs::text()` keeps the spaces and tabs too), so that formatting never changes the facet: `Arbitrary-precision integer.`; the same in both modes |
| `Annotations` | one per line in the given order: `@id("pgn_7c2d")` LF `@prove`; `@name()` keeps its parentheses (T14) |
| `Expr` | `flatten(result) == items`, `forall page in result: page.len > 0 and page.len <= size`, `b != 0`: one line unless the expression contains an `if` or a `match`, whose blocks are multi-line and whose arms take a line each (2.4 rule 8) |
| `Type` | `List<List<T>>`, the same in both modes |
| `Selector` | `paginate.body`, `tests.pagination.*` (in `Source` mode with its comments) |

The `name` facet of a test or property has no variant of its own: callers
render `Fragment::Expr` of a `Text` literal expression holding the name
(`"pagination preserves input"`).

Decision: [D-140] T6. a lexical or syntax error is fatal for its file, as grammar
D59 specifies: `ParseResult::value` is `None`, and the single diagnostic,
the earliest error by offset (the parser may resynchronise internally to
stay total and to find it, grammar D70, but reports nothing after it), has
one of the `parse` codes of `spec/semantics.md` 9.4 (`invalid_character`,
`unterminated_text_literal`, `syntax_error`, `unknown_requirement`, ...),
category `parse`, a message that begins with the grammar's quoted text
(grammar D72), the span of the offending token, and, on a message of the
generic `expected ..., found ...` shape, the `expected` and `found` fields;
the AST has no error-recovery nodes. The 9.4 table is the "finer code
catalogue" that D59 defers to the diagnostics specification, and it is
mandatory, not a choice: the two-code scheme `lexical_error` / `syntax_error` is never
emitted. Other files of the project are still parsed, so one run reports
one parse diagnostic per broken file; when any file failed to parse no
resolution or typing runs (semantics S52). Rationale: D59 is the grammar's
decision, and *reporting* the errors found after a resynchronisation in a
newline-terminated grammar produces cascades that cost agents more than a
second run, because resynchronisation routinely mis-aligns inside
bracketed groups; one mandatory code set is what lets a shared negative
corpus match codes exactly (Section 12.2). Rejected alternatives:
statement-level recovery with `Error` nodes and several diagnostics per
file; letting an implementation choose between D59's two class names as
codes and the 9.4 catalogue (an earlier revision of this decision; two
implementations would disagree on every parse diagnostic).

Decision: [D-141] T7. the formatter only ever receives a successfully parsed AST:
`format_source` returns the parse diagnostic as `Err` and writes nothing,
and `cpl fmt` leaves such a file untouched. Rationale: a formatter cannot
preserve text it did not parse, and a formatter that drops text is worse
than one that refuses. Rejected alternative: formatting the parsed prefix.

Decision: [D-261] T65. the lexer, the parser, and the formatter of
`cpl-syntax` run in time near-linear in their input: no construct makes
the time grow quadratically with the input's length or exponentially with
its nesting depth (width fitting renders a line unit once per group it
breaks and reuses the finished text of the units nested inside it; the
generic-call check of grammar D42 remembers what it found at each `<`; the
attachment of a trailing comment does constant work per element), and at
the minimum limits of grammar D73 they need less than 1 MiB of stack in a
debug build (as measured, nested struct literals, the deepest construct,
about 730 KB; nested `while` loops about 7 KiB per level). Rationale: D73
exists so that no input, however deep or long, can break the toolchain,
and an input that takes exponential time breaks it as surely as one that
overflows the stack; the coordinator parses `.work` and `.patch` files that
agents submit (grammar D36 anticipates malicious ones), and `cpl fmt` runs
on every save. Rejected alternative: no bound (an earlier state of the
crate, in which 24 nested long `if` headers took over ten seconds to format
and a trailing comment after a long one-line group made parsing
quadratic).

### 1.6 `cpl-sema` public API

`cpl_sema::project` is the crate `cpl-project` (T52): `cpl-sema` re-exports
it (`pub use cpl_project as project;`), and the crate roots of both export
`find_project_root`, `load_project`, `load_project_from_tree`,
`LoadOptions`, `StdSource`, `VirtualTree`, `Project`, `SourceEntry`,
`ParsedFile`, `LoadError`, `Manifest`, `Lock`, `parse_manifest`,
`parse_lock`, and `render_lock` (`cpl-project` also the section types,
`ManifestError`, `LockToolchain`, `LockError`, `STD_FILES`, `std_digest`,
and `check_bindings`, and from Milestone 2 the C section types,
`check_c_bindings`, `CBindingCheck`, and the module `cbind` of Section
11.1 and the C lock entry types and `c_input_digests` of Section 11.2).

```rust
// ---- cpl_sema::project = the crate cpl-project (T52) ------------------------
/// Nearest ancestor of `start` (inclusive) that contains `cpl.toml` as a file. A relative `start` is first made absolute
/// against the current directory (without resolving symlinks), so the result is absolute.
pub fn find_project_root(start: &std::path::Path) -> Option<std::path::PathBuf>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadOptions {
    pub std: StdSource,                            // default Embedded
    /// Parse work/*.work and work/*.patch into `Project::parsed`. Default false: `cpl run` and `cpl test` never read
    /// work files (spec/semantics.md 1.1); `cpl check` sets it to parse them for syntax only (semantics S53), as do
    /// `cpl fmt`, `cpl work check`, and the coordinator. The work files are members of the source set either way:
    /// they are read, registered in the source map (so FileIds do not depend on this option), and digested; without
    /// the option they are `Skipped` and none of their diagnostics, `invalid_utf8` included, is reported.
    pub include_work_files: bool,
}
/// Dir: the CPL_STD_DIR override (Section 3.6): every `*.cpl` file directly in that directory (not below it), in sorted
/// file-name order, registered as `std/<file name>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StdSource { #[default] Embedded, Dir(std::path::PathBuf) }

/// An in-memory source tree: '/'-separated project-relative paths to bytes. Built by cpl-index from a git commit
/// (Section 9.3) or by tests. Loading from a tree and from a directory produce identical Projects (apart from
/// `Project::root` and the files' origin). Paths are normalized (`src/a/b.cpl`, never `./src/a/b.cpl`); a directory
/// exists in the tree exactly when some file is under it, so an empty directory, which a git tree cannot record, is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtualTree { pub files: std::collections::BTreeMap<String, Vec<u8>> }

/// The source set (Section 0): `cpl.toml`, every `*.cpl` file under the `[build] sources` directories (recursively; a
/// file named just `.cpl` is not a source; symbolic links per T54), and every `*.work` and `*.patch` file directly in
/// `work/`; `cpl.lock` is read when present. Every project file has `FileOrigin::Project`.
pub fn load_project(root: &std::path::Path, opts: &LoadOptions) -> Result<Project, LoadError>;
/// `origin` is the FileOrigin of every file of the tree: `Revision(commit)` for a git tree (Section 9.3), `Virtual` in
/// tests. A tree without `cpl.toml` is `LoadError::NoManifest(".")`.
pub fn load_project_from_tree(tree: &VirtualTree, origin: FileOrigin, opts: &LoadOptions) -> Result<Project, LoadError>;

#[derive(Clone, Debug)]
pub struct Project {
    pub root: Option<std::path::PathBuf>,          // None for a VirtualTree
    pub manifest: Manifest,                        // Section 11.1
    pub lock: Option<Lock>,                        // Section 11.2; None when cpl.lock is absent
    pub manifest_digest: Digest,                   // blake3 of the bytes of cpl.toml: the one source-set member that is not a SourceEntry
    pub lock_digest: Option<Digest>,               // blake3 of the bytes of cpl.lock when present: every receipt's `inputs.lock` (Section 8); the lock is not a source-set member, so no other digest covers it
    pub source_map: std::sync::Arc<SourceMap>,     // project files, std files, nothing else (cpl.toml and cpl.lock are not registered); shared with Program
    pub files: Vec<SourceEntry>,                   // the .cpl, .work, and .patch files of the source set plus std: std first in sorted name order, then path order (T15); files[i].file == FileId(i)
    pub parsed: Vec<ParsedFile>,                   // one per SourceEntry (Failed for a file with a parse diagnostic, D59/D70)
    /// Manifest, lock, io, lexical, and syntax diagnostics and the module diagnostics of loading (Section 3.4), in stage
    /// order: the `project` diagnostics (manifest, lock, source discovery), then the `parse` diagnostics in FileId order,
    /// then the `resolve` ones (`unsupported_edition`, then `reserved_module_path` and `module_file_misplaced` in FileId
    /// order). `check` starts `CheckResult::diagnostics` with them.
    pub diagnostics: Vec<Diagnostic>,
}
impl Project {
    /// `std_digest` (Section 3.6) of the std files this project was loaded with (origin Std, by file name): with
    /// `StdSource::Embedded` it is `std_digest(STD_FILES)`.
    pub fn std_digest(&self) -> Digest;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEntry { pub file: FileId, pub kind: FileKind, pub digest: Digest, pub origin: FileOrigin }
/// Skipped: a .work/.patch file present in the source set but not parsed because `include_work_files` is false.
/// Failed: the file has a lexical or syntax error or is not valid UTF-8 (its one diagnostic is in `Project::diagnostics`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedFile { Module(std::sync::Arc<ast::Module>), Work(std::sync::Arc<ast::WorkFile>), Patch(std::sync::Arc<ast::PatchFile>), Skipped, Failed }

/// Why a project could not be loaded at all (exit 2). Paths are project-relative (`cpl.toml`, `src/reader/pages.cpl`).
#[derive(thiserror::Error, Debug)]
pub enum LoadError {
    #[error("no cpl.toml found at {0}")]           NoManifest(std::path::PathBuf),
    #[error("{path}: {message}")]                  Manifest { path: String, message: String, line: Option<u32> },
    #[error("{path}: {message}")]                  Lock { path: String, message: String, line: Option<u32> },
    #[error("{path}: {source}")]                   Io { path: String, #[source] source: std::io::Error },
    // Invalid UTF-8 in a source file is not a LoadError: it is the parse diagnostic `invalid_utf8`, raised by the
    // SourceMap when the bytes are read (grammar D70), and the file is `ParsedFile::Failed`.
    #[error("standard library unavailable: {0}")]  Std(String),   // a StdSource::Dir that is unreadable or holds no *.cpl file
}
// Manifest, Lock, parse_manifest, parse_lock, render_lock, check_bindings: Section 11.
/// The embedded std stubs: one (file name, text) pair per `std/*.cpl` file of the workspace, in sorted file-name order
/// (`("prelude.cpl", "...")`), generated by `crates/cpl-project/build.rs` (Section 3.6).
pub static STD_FILES: &[(&str, &str)];
pub fn std_digest(files: &[(&str, &str)]) -> Digest;   // Section 3.6: blake3 over `name LF content LF` per file, in the order given
/// The diagnostic codes of loading: the thirteen `project_*` codes of Appendix B (CATEGORY = Category::Project) and the
/// three `resolve` codes loading raises (UNSUPPORTED_EDITION, RESERVED_MODULE_PATH, MODULE_FILE_MISPLACED). From
/// Milestone 2 also every `project_*` code of the C target (Appendix B, T76), whichever crate raises it, each in the
/// `PROJECT` table, so that `cpl_sema::codes::category` and the corpus harness of Section 12.2 know it.
pub mod codes { /* one DiagCode constant per code, named by the code in upper case; PROJECT: &[DiagCode] */ }

// ---- cpl_sema::check --------------------------------------------------------
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct CheckConfig {
    pub release: bool,                             // Section 7.2: `hole_in_release` and `verification_unsatisfied` (semantics S47)
    pub warnings_as_errors: bool,
    pub lints: BTreeSet<String>,                   // enabled off-by-default lints (Section 7.2 `--lint`)
}
pub fn check(project: &Project) -> CheckResult;    // check_with(project, &CheckConfig::default())
pub fn check_with(project: &Project, config: &CheckConfig) -> CheckResult;

#[derive(Clone, Debug)]
pub struct CheckResult {
    /// Always present; it has no definitions when some file failed to parse (semantics S52). Definitions whose
    /// checking failed have `Def::status == DefStatus::HasErrors`.
    pub program: Program,
    /// Emitted in stage order (project: `Project::diagnostics` of that category, the release lock check, the binding
    /// checks, and from Milestone 2 the C binding checks and the export plan of Section 11.1; parse; resolve; type;
    /// effect; contract; hole; release_policy) and within a stage by file path then item order, and returned in
    /// `Diagnostics::sorted` order (Section 7.1, T46), which keeps the stage order among span-less diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    pub holes: Vec<HoleReport>,                    // one per hole that is not an error hole (semantics S64), its expected type possibly erroneous (S69); each is also the `hole` diagnostic in `diagnostics`, or `hole_in_release` under release
    pub verification: Vec<VerificationStatus>,     // one per `@prove` declaration; each is also a `verification_result` diagnostic (without `receipt`, which cpl-cli adds)
    /// Milestone 2 (T80): the C surface of the program (`interop::plan`, below) when `"c"` is in `[interop] targets` and
    /// every file parsed; None otherwise.
    pub interop: Option<interop::InteropPlan>,
    warnings_as_errors: bool,                      // CheckConfig::warnings_as_errors, for has_errors
}
/// True when some diagnostic is an error or, with `warnings_as_errors` (`--deny-warnings`), a warning; severities are unchanged.
impl CheckResult { pub fn has_errors(&self) -> bool; }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationStatus { pub def: DefId, pub status: ProofStatus, pub reason: Option<String> }   // the verifier's status (spec/verification.md)
/// Defined in cpl_diag::json (Section 1.4.2) and re-exported here; `as_str`. Conditional is never produced in Milestone 1.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum ProofStatus { Proved, Failed, Unknown, Conditional }

// ---- cpl_sema::hir = the crate cpl-hir (full definitions in Section 3.3; T50) ---
// `cpl-sema` re-exports the crate (`pub use cpl_hir as hir;`); every item of this block is a `cpl_hir` item.
#[derive(Clone, Debug)]
pub struct Program { /* Section 3.3 */ }
impl Program {
    pub fn empty(source_map: std::sync::Arc<SourceMap>) -> Program;  // no definitions: what `check` returns when a file failed to parse (S52)
    pub fn def(&self, id: DefId) -> &Def;                          // panics on an id out of range
    pub fn defs(&self) -> impl Iterator<Item = &Def>;              // DefId order
    pub fn module(&self, id: ModuleId) -> &ModuleDef;
    pub fn modules(&self) -> impl Iterator<Item = &ModuleDef>;     // ModuleId order
    pub fn module_by_path(&self, path: &[String]) -> Option<ModuleId>;
    /// The first definition of the path's module, in `ModuleDef::defs` order, with that name (after a
    /// `duplicate_declaration`, the earlier one). Capability operations and enum variants are members, not
    /// definitions, and are not found.
    pub fn lookup(&self, path: &SymbolPath) -> Option<DefId>;
    pub fn by_stable_id(&self, id: &StableId) -> Option<DefId>;
    pub fn std_defs(&self) -> &StdDefs;                            // DefIds of Unit, Option, Result, ConstraintError, ForeignError, std.gen.Positive
    pub fn references_from(&self, id: DefId) -> &[Reference];      // outgoing edges, Section 6.5, in span order (relies on the (from, span) sort)
    pub fn references_to(&self, id: DefId) -> Vec<&Reference>;     // incoming edges (callers), Section 6.5, in (from, span) order
    pub fn sort_references(&mut self);                             // establishes the (from, span) order of Program::references (a stable sort)
    /// CPL type syntax without a generic context: `Ty::Param(i)` renders as `$<i>` (`List<List<$0>>`); otherwise as
    /// `display_ty_in` (`Ty::Error` as `?`).
    pub fn display_ty(&self, ty: &Ty) -> String;
    /// CPL type syntax with `Ty::Param(i)` rendered as `generics[i]`, the enclosing definition's generic parameters
    /// (`List<List<T>>`); named types render by their declared name and a refinement by the declaration written at the
    /// use site (S55), and `Ty::Error` as `?` (semantics S69).
    pub fn display_ty_in(&self, ty: &Ty, generics: &[String]) -> String;
    pub fn display_path(&self, id: DefId) -> String;               // the qualified name, Section 4.4
    pub fn symbol_path(&self, id: DefId) -> SymbolPath;
    pub fn source_map(&self) -> &SourceMap;
    pub fn file_ast(&self, file: FileId) -> Option<&ast::Module>;
    pub fn ast(&self, id: DefId) -> &ast::Declaration;             // the declaration's AST (needed by cpl-index facets); panics when its file is not in `asts`
    pub fn refinement_class(&self, class: RefinementClass) -> &RefinementClassDef;
}
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum BindingKind { Parameter, Using, Let, Var, Pattern, Forall, For, Result, SelfValue }   // SelfValue serializes as "self"; LocalDef::kind, HoleBinding::kind
// BindingKind is defined in cpl_diag::json, where the wire type HoleBindingJson (Appendix C) uses it, and re-exported by cpl-hir.

// ---- cpl_sema::check, re-exported at the crate root: selector resolution ----
/// Resolve a selector in a scope (a .work file's `use` list, a module, or fully qualified only). A function of
/// cpl-sema rather than a `Program` method because it runs name resolution (T50). The first segment is looked up in
/// the scope's imports (an imported item, or a submodule alias that continues the path), then among the scope module's
/// declarations, then as the start of a fully qualified path; a selector naming no declaration is `Err` with
/// `unresolved_name` or `unresolved_module`, which callers map to their own code (`work_selector_empty`, after the
/// file selectors of T42). A declaration or facet selector takes the first of the three readings that names a
/// declaration (with `use tests.{app}`, `app.core.double.body` falls back to the fully qualified `app.core.double`). A
/// namespace `m.*` whose first segment a `use` item binds to a module path (a submodule alias) is always read through
/// it, even when no module exists under the expanded path, since an `add` namespace may be new (`app.core.*` is then
/// `tests.app.core.*`); `m.*` with no module under `m` resolves to no entry.
pub fn resolve_selector(program: &Program, sel: &ast::Selector, scope: &SelectorScope) -> Result<Vec<Selected>, Diagnostic>;
pub struct SelectorScope<'a> { pub uses: &'a [ast::UseDecl], pub module: Option<ModuleId> }
pub struct Selected { pub def: DefId, pub facet: FacetSelector }   // a namespace wildcard expands to one entry per declaration

// ---- cpl_sema::check, continued: hole reports -------------------------------
/// The hole report of spec/semantics.md S48, with resolved definitions instead of strings where the index needs them.
pub struct HoleReport {
    pub name: String,                              // "build_pages"
    pub span: Span,
    pub def: DefId,                                // enclosing declaration
    pub expected: Ty,                              // contains Ty::Error when the position's type comes from a declaration or form that failed (S69); rendered `?` there
    pub bindings: Vec<HoleBinding>,                // visible bindings, innermost first
    pub capabilities: Vec<HoleCapability>,         // one per `using` parameter in scope
    pub requires: Vec<String>, pub ensures: Vec<String>,   // enclosing fn clauses, canonical text, in order (empty for tests)
    pub invariants: Vec<String>,                   // enclosing `while` invariants, innermost first
    pub dependencies: Vec<DefId>,                  // every definition the enclosing declaration references, itself included when it is recursive (as `Index::dependencies`, Section 6.5), sorted by qualified name
}
pub struct HoleBinding { pub name: String, pub ty: Ty, pub kind: BindingKind, pub mutable: bool }
pub struct HoleCapability { pub parameter: String, pub capability: DefId, pub operations: Vec<HoleOperation> }
pub struct HoleOperation { pub name: String, pub signature: String, pub in_bound: bool }
// HoleReport, HoleBinding, HoleCapability, HoleOperation, VerificationStatus, Selected, SelectorScope derive Clone, Debug,
// PartialEq, Eq where their fields allow. `capabilities` follow `bindings` (innermost scope first, the most recently
// declared first) and `operations` the capability's declaration order.
// The `hole` diagnostic in `diagnostics` carries every S48 field except `dependencies`, which cpl-cli adds through
// cpl-index (`Index::complete_check_diagnostics`: the versions of `Index::dependencies` of the hole's declaration and
// the module content hashes, Section 7.2); likewise cpl-cli attaches the S59 `receipt` of `verification_result`
// (Section 7.2), which needs the Git revision and the module content hashes.
//
// ---- cpl_sema::codes ---------------------------------------------------------
// Every code cpl-sema emits, one DiagCode constant each, and `ALL: &[(DiagCode, Category, Severity)]` (semantics 9.4
// order, then this document's Appendix B), with `category(code)` and `default_severity(code)`; `BUILTIN_IDS`, the sorted
// registry ids of stdlib 10.6 that `std_builtin_missing` checks (cpl-sema cannot depend on cpl-eval, Section 3.6): from
// Milestone 2 `pub const BUILTIN_IDS: &[&str] = &cpl_rtspec::builtins::IDS;`, the same name, type, and contents (T70).
```

Milestone 2 adds the plan of a program's C surface (T74, T80). The check
driver computes it after name resolution, beside `check_bindings`, from
the manifest, the checked program, and the declarative bindings that
`cpl_project::check_c_bindings` read (Section 11.1), and reports its
diagnostics in the project stage, so loading never reports them. The C
names, mappings, and header layout it decides are `spec/cabi.md` Section
5's; this block fixes the data that `cpl-emit-c` reads.

```rust
// ---- cpl_sema::interop (Milestone 2) -------------------------------------------
/// The C surface of a checked program. A definition already marked `HasErrors` when the plan runs is left out without
/// a diagnostic (semantics S62). Every list has the order stated, so the plan is a deterministic function of the program
/// and the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteropPlan {
    pub prefix: String,                            // Manifest::c_prefix(): `[interop.c] prefix`, else `[project] name` (Section 11.1)
    pub abi: u16,                                  // `[interop.c] abi`, default 0
    pub exports: Vec<ExportPlan>,                  // the export set of T74 in export order: the `exports` entries in manifest order, else qualified-name order
    pub skipped: Vec<DefId>,                       // the `pub fn`s the default set leaves out because they are generic, in qualified-name order (`project_export_skipped_generic`, reported by `cpl build` only)
    pub types: Vec<TypePlan>,                      // the listed types of spec/cabi.md 5.3, which the package header declares, in the byte order of their texts
    pub capabilities: Vec<DefId>,                  // the listed capabilities of spec/cabi.md 5.3 (those of the exports' `using` parameters and those the backend-`c` bindings bind), in qualified-name order
    pub bindings: Vec<BindingPlan>,                // the backend-`c` bindings, in binding-name order
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPlan { pub function: DefId, pub symbol: String, pub int: IntMapping, pub entry: Option<usize> }   // entry: the index of its `[interop.c] exports` entry, None for the default set
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypePlan { pub ty: Ty, pub text: String, pub c_name: String }   // text: type_text(program, &ty); c_name: the <name> of `<prefix>_type_<name>`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingPlan {
    pub name: String, pub capability: DefId, pub form: AdapterForm,
    pub adapter: String,                           // the project-relative adapter file
    pub cbind: Option<CBindSpec>,                  // the parsed `.cbind.toml` of a declarative binding
}
impl InteropPlan {
    /// Nothing exported or bound: the plan of a driver for a project without the C target, `name` being the project's
    /// effective prefix (Manifest::c_prefix()); its `prefix` is the driver prefix that spec/cabi.md 1.6 derives from
    /// `name`.
    pub fn empty(name: &str) -> InteropPlan;
    pub fn binding_for(&self, capability: DefId) -> Option<&BindingPlan>;   // the backend-c binding of a capability (at most one, T74)
}
/// The qualified type text of spec/cabi.md 5.3: CPL type syntax with every declaration named as `$type` names it
/// (T30). It is the one renderer of that text: `plan` orders `types` by it, and cpl-emit-c writes every type text it
/// emits with it (the type table, `cpl_type_name`, the header's comments) and renders none itself;
/// crates/cpl-emit-c/tests/tables.rs checks that it names every declaration as `cpl_eval::types::type_name` does.
pub fn type_text(program: &Program, ty: &Ty) -> String;
/// The rules of Section 11.1 that need the checked program: the `project_export_*` codes, `project_type_name_unknown`,
/// and the semantic half of `project_cbind_invalid` (Appendix B). An `exports` target resolves by the rows of Section
/// 6.1 over `program` (T59), with the `project_export_*` codes in place of `index_unknown_symbol` and
/// `index_ambiguous_symbol`. `cbind`: the declarative bindings whose files `check_c_bindings` read and parsed.
pub fn plan(program: &Program, manifest: &Manifest, cbind: &BTreeMap<String, CBindSpec>) -> (InteropPlan, Vec<Diagnostic>);
// IntMapping, AdapterForm, and CBindSpec are cpl_project's (Section 11.1).
```

The package's type table (`spec/cabi.md` 5.3) is not part of the plan:
`cpl-emit-c` computes it from the program and the plan by the rule of
cabi.md 5.3, with `type_text`, so that a driver built from
`InteropPlan::empty` has one too.

Decision: [D-276] T80. the Milestone 2 additions to `cpl-project` and
`cpl-sema` keep every Milestone 1 struct and function shape: the C
manifest tables are the new `Manifest` fields `interop_c` and `binding_c`,
never fields of `InteropSection` or `BindingSection` (Section 11.1); the
lock's C entries live inside the existing `foreign`, `adapters`, and
`targets` tables of `Lock`, read and written through typed views (Section
11.2); `check_bindings` and `render_lock` keep their signatures and
behaviour (`check_bindings` still accepts `backend = "c"` with any
`adapter`), and the backend-`c` rules are the new `check_c_bindings`;
`CheckResult` gains `interop`; and `codes::BUILTIN_IDS` keeps its name,
type, and contents. Rationale: Milestone 1 tests build `InteropSection`,
`BindingSection`, and `Lock` literally (`crates/cpl-project/tests/manifest.rs`,
`lock.rs`; `crates/cpl-sema/tests/driver.rs`) and call `check_bindings` on
a backend-`c` binding with `adapter = "a"`, so a new field or a stricter
`check_bindings` would break accepted tests, while no test builds
`Manifest` or `CheckResult` literally (the latter has a private field).
Rejected alternatives: new fields on `InteropSection`, `BindingSection`,
or `Lock`; the backend-`c` checks inside `check_bindings`.

Semantics: typing, effect, contract, and policy rules, and the codes of
the `parse`, `resolve`, `type`, `effect`, `contract`, `hole`, `test`,
`release_policy`, `runtime`, and `internal` categories, are specified in
`spec/semantics.md` (Sections 1-9 there; code list in its 9.4). `cpl-sema`
emits exactly those codes; Section 3.4 states which stage produces each
category and Appendix B lists the toolchain-owned categories that
`spec/semantics.md` does not cover.

### 1.7 `cpl-eval` public API

`cpl-eval` depends on `cpl-hir`, not on `cpl-sema` (T50): `Program`,
`DefId`, `Ty`, and the other HIR names below are `cpl_hir` items, and the
evaluator never calls the checker; its tests use `cpl-sema` as a
dev-dependency to check the programs they run.

```rust
// ---- cpl_eval::value --------------------------------------------------------
use std::rc::Rc;
#[derive(Clone, Debug)]
pub enum Value {
    Int(num_bigint::BigInt),
    Bool(bool),
    Text(Rc<String>),
    Bytes(Rc<Vec<u8>>),
    List(ListRef), // a view of a shared buffer: Rc<Vec<Value>> and a range; derefs to [Value]
    Struct { def: DefId, fields: Rc<[Value]> },           // fields in declaration order
    Enum { def: DefId, variant: u32, payload: Rc<[Value]> },
    Wrapper { def: DefId, inner: Rc<Value> },             // nominal wrapper, e.g. PageId("x")
    Capability(HandleId),
}
impl Value {
    pub fn unit(p: &Program) -> Value;                    // Enum { def: std_defs.unit, variant: 0, payload: [] }
    pub fn some(p: &Program, v: Value) -> Value;  pub fn none(p: &Program) -> Value;
    pub fn ok(p: &Program, v: Value) -> Value;    pub fn err(p: &Program, e: Value) -> Value;
    pub fn as_result<'a>(&'a self, p: &Program) -> Option<Result<&'a Value, &'a Value>>;
    pub fn as_option<'a>(&'a self, p: &Program) -> Option<Option<&'a Value>>;
    /// Structural equality on the erased value (spec/semantics.md 6.5). Never called on capabilities (rejected
    /// statically). Iterative: it does not recurse with nesting depth and skips shared buffers. `PartialEq`/`Eq` are it.
    pub fn structurally_equal(&self, other: &Value) -> bool;
    pub fn concat(&self, other: &Value) -> Option<Value>; // `++` on List, Text, Bytes
    pub fn into_int(self) -> Result<BigInt, Value>;
    // Also: the constructors int, text, bytes, list, empty_list, new_struct, new_enum, variant(p, def, name, payload)
    // (by variant name), wrap; the accessors is_unit, as_int, as_bool, as_text, as_bytes, as_list, kind_name.
}
// Value implements Drop by hand, with an explicit work list, so that dropping a value of any nesting depth never
// recurses on the host stack (semantics S38, S72); a consequence is that a pattern cannot move a field out of an owned
// Value (E0509): match on a reference and clone the part (an Rc clone).
```

Text, byte, and list buffers retain capacity for uniquely owned local
append assignments. Other references remain immutable snapshots through
copy on write. A list is a view of a buffer, so `take`, `drop`, and
`drop_last` copy nothing and walking a list with `first()` and `drop(1)`
is linear; a uniquely owned view appends past its last element and drops
its dead prefix once that is longer than the list. Embedders should use the constructors and accessors above;
the earlier `Rc<str>` and `Rc<[T]>` payload representations have changed.

Refined values (`Positive`) have the representation of their base type; a
refinement is a static fact, not a runtime wrapper (semantics 6.1).
`Unit`, `Option`, and `Result` are the prelude's ordinary enums (Section
3.6).

Decision: [D-142] T8. values use `Rc`, and the interpreter is single-threaded;
`cpl test` runs tests sequentially in Milestone 1. Rationale: an
interpreter over immutable values shares aggressively (`push` copies a
list, `..base` shares field values), and `Rc` is the cheapest sharing
primitive; agent-level parallelism arrives in Milestone 6 as separate
processes in separate worktrees, not as threads. Rejected alternative:
`Arc` everywhere now (no present benefit).

```rust
// ---- cpl_eval::interp -------------------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize)]
pub struct HandleId(pub u32);
pub struct Handle { pub id: HandleId, pub capability: DefId, pub label: String }

/// A host-provided implementation of a capability. Milestone 1 ships none (semantics 6.9); the interop adapters
/// (Milestones 2-4) implement it, Milestone 2's being `cpl_cabi::adapter::CAdapter` (Section 1.14), bound through
/// `bind_capability_with` (Section 1.7.4). The evaluator calls operations only through this interface.
pub trait CapabilityImpl {
    fn call(&mut self, op: &str, args: Vec<Value>, ctx: &mut HostContext<'_>) -> Result<Value, Fault>;
}
pub struct HostContext<'a> { pub program: &'a Program }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalConfig {                            // Default: the values below; EvalConfig::DEFAULT_MAX_CALL_DEPTH = 10_000
    pub max_call_depth: u32,                       // default [run] max_call_depth = 10_000 (semantics S38); CPL function activations only (S72), enforced by an explicit counter
    /// Default None; `cpl test` sets it from [test] timeout_ms when non-zero. It applies to a top-level `call` or
    /// `satisfies_refinement`, and in the runner to exactly what Section 7.2 budgets: one test body, one property case,
    /// one shrink rerun (not a property's range setup, nor the runner's refinement checks of drawn values and shrink
    /// candidates). It is checked at every function entry and every loop iteration, not inside builtins or `forall`.
    pub time_budget: Option<std::time::Duration>,
    /// Default true; false is reserved for release builds whose obligations were proved (Milestone 5): then no
    /// `requires`, `ensures`, `invariant`, loop or header `decreases`, or capability-operation clause runs and `Refine`
    /// nodes are not re-checked. The argument checks of `run_function` still run (they are boundary checks, S39), and a
    /// builtin still checks its own preconditions (stdlib S39: they are part of its definition).
    pub check_contracts: bool,
    pub trace_effects: bool,                       // default false; true records EffectEvents
}
pub struct Interpreter<'p> { /* private */ }
impl<'p> Interpreter<'p> {
    pub fn new(program: &'p Program, config: EvalConfig) -> Self;
    pub fn program(&self) -> &'p Program;
    pub fn config(&self) -> &EvalConfig;
    pub fn bind_capability(&mut self, capability: DefId, imp: Box<dyn CapabilityImpl>, label: &str) -> Handle;
    pub fn handle(&self, id: HandleId) -> Option<Handle>;
    /// Call `f` with positional `args` and handles for its `using` parameters, checking its contract (semantics 5.2-5.4).
    /// `using` handles whose count or capabilities do not match `f`'s are refused (`internal_error`, kind `call_mismatch`).
    pub fn call(&mut self, f: DefId, args: Vec<Value>, using: Vec<Handle>) -> Result<Value, Fault>;
    /// Evaluate a refinement chain's predicates for `check<R>(x)` and for boundary checks (semantics 3.5 step 2 subset is
    /// not required here: the full evaluator runs).
    pub fn satisfies_refinement(&mut self, refined: DefId, value: &Value) -> Result<bool, Fault>;
    pub fn effects(&self) -> &[EffectEvent];
    pub fn current_def(&self) -> Option<DefId>;   // the innermost active declaration: what builtins report as `caller`
    pub fn frames(&self) -> &[Frame];             // the active declarations, outermost first (a backtrace reverses them)
    // Doc-hidden for the builtins and the runner: push_frame, pop_frame, call_handle (an unbound handle is
    // `internal_error`, kind `unbound_capability`).
}
pub struct EffectEvent { pub handle: HandleId, pub op: String, pub def: DefId, pub span: Span }

// ---- cpl_eval::stack (semantics S38) ----------------------------------------
/// Run `f` on a fresh thread whose stack is EVAL_STACK_SIZE (1 GiB, virtual memory committed only as it is used; smaller
/// fallbacks when the host refuses the reservation, the calling thread as the last resort); a panic in `f` resumes on
/// the caller. On that thread a guard refuses any activation that would start within GUARD_MARGIN (64 MiB, or an eighth
/// of a smaller stack) of its end with `resource_exhaustion` (`resource: "stack"`, plus a `depth` field), so even a
/// `max_call_depth` larger than the stack holds ends in a fault, never a crash. `run_function`, `run_tests`, and
/// `run_test` evaluate on it; a host driving an Interpreter itself should too.
pub fn on_eval_stack<R: Send, F: FnOnce() -> R + Send>(f: F) -> R;               // (root)
pub fn on_eval_stack_sized<R: Send, F: FnOnce() -> R + Send>(size: usize, f: F) -> R;   // at least 1 MiB
// A result leaves the evaluation thread (Value is not Send) as a flat node list, rebuilt without recursion, and every
// operation a host performs on a Value (drop, equality, value JSON, value text) is iterative, so a result of any
// nesting depth is safe on the caller's stack. The serde_json trees of value JSON (a fault's `input` and `actual`, a
// record's payloads) still drop and serialize recursively: a host that may meet deep ones handles them inside
// on_eval_stack (they are Send).

// ---- cpl_eval::fault --------------------------------------------------------
#[derive(Clone, Debug)]
pub struct Fault {
    pub kind: FaultKind,
    pub message: String,
    pub symbol: Option<StableId>,                  // blamed declaration (Section 1.7.3)
    pub facet: Option<Facet>,                      // `contract` for every contract violation and for a fault raised while evaluating a clause (semantics S34, S57); `body` otherwise
    pub span: Option<Span>,
    pub backtrace: Vec<Frame>,                     // innermost first
    pub data: serde_json::Map<String, serde_json::Value>,   // Section 1.7.3 payload
}
pub struct Frame { pub def: DefId, pub span: Option<Span> }
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FaultKind {
    ContractViolation(ClauseKind),
    AssertionFailed,
    UnresolvedHole,
    DivisionByZero,
    RefinementViolation,                           // boundary only: a `cpl run` argument (semantics S39); from Milestone 2 also a C export argument (S81) and the result of a validating binding (S77)
    ResourceExhaustion(Resource),
    Internal,                                      // includes an operation call on an unbound handle (kind "unbound_capability")
}
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)] #[serde(rename_all = "snake_case")]
pub enum ClauseKind { Requires, Ensures, Invariant, Decreases }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)] #[serde(rename_all = "snake_case")]
pub enum Resource { Stack, Memory, Time, Steps }   // Memory: the evaluator never raises it (safe Rust cannot observe allocation failure); from Milestone 2 a native fault rebuilt by cpl_cabi::fault_from_record carries it (semantics S76);
                                                   // Steps is reserved for the opt-in step budget semantics S38 allows, which no Milestone 1 flag defines
impl Fault { pub fn to_diagnostic(&self, p: &Program) -> Diagnostic; }   // codes in Section 1.7.3

// ---- cpl_eval::json ---------------------------------------------------------
pub fn value_to_json(p: &Program, v: &Value, ty: &Ty) -> serde_json::Value;          // value JSON, Section 7.3 (semantics S40)
pub fn value_from_json(p: &Program, j: &serde_json::Value, ty: &Ty) -> Result<Value, ArgError>;
pub fn value_text(p: &Program, v: &Value, ty: &Ty) -> String;                        // value text, Section 7.3 (semantics S40)
pub struct ArgError { pub pointer: String /* JSON pointer */, pub message: String }

// ---- cpl_eval::run ----------------------------------------------------------
#[derive(Clone, Debug, PartialEq)]
pub enum RunArg { ConstExpr(String), Json(serde_json::Value) }   // positional constant expression (Section 7.2, semantics S39) or `--arg JSON`
/// Resolve `path`, decode `args` against the parameters, check every refined position (`refinement_violation`, S39 (a)),
/// and call the function in checked mode with no calling declaration (S39 (c)), on the evaluation thread
/// (`on_eval_stack`). Refusals in this order: ProgramHasErrors, NotFound, NotAFunction (also a generator, a function whose
/// declared return type is a `Gen<T>`, T61), Generic, CannotRunEffectful, ArgCount, then the first argument that does
/// not decode (argument indices are 0-based).
pub fn run_function(p: &Program, path: &SymbolPath, args: &[RunArg], config: &EvalConfig) -> Result<RunOutcome, RunError>;
pub struct RunOutcome {
    pub value: Result<Value, Fault>, pub return_ty: Ty, pub effects: Vec<EffectEvent>,
    /// Every argument as decoded, in parameter order, each to be encoded at its parameter's type (`FnDef::params`):
    /// `RunOutput::arguments` (Section 7.2), whichever form spelled it; present on a fault too, a refinement violation
    /// of an argument included (the decoded value is what was checked).
    pub arguments: Vec<Value>,
}
/// The FileId constant-expression arguments are parsed as (FileId(u32::MAX)): the spans of an ArgNotConstant
/// diagnostic are byte offsets into the argument's own text, which no program's source map holds.
pub const ARGUMENT_FILE: FileId;
pub const USAGE_BAD_ARGUMENT: DiagCode;           // "usage_bad_argument" (Appendix B)
#[derive(thiserror::Error, Debug)]
pub enum RunError {
    #[error("program has errors")]                        ProgramHasErrors,   // the program has no definitions, or one has `HasErrors`
    #[error("no function {0}")]                           NotFound(String),
    #[error("{0} is not a function")]                     NotAFunction(String),
    #[error("{0} is generic; run a monomorphic wrapper")]  Generic(String),                 // code cannot_run_generic (semantics S39)
    #[error("{0} has using parameters")]                  CannotRunEffectful(String),     // code cannot_run_effectful (semantics S39)
    #[error("expected {expected} arguments, got {got}")]  ArgCount { expected: usize, got: usize },
    #[error("argument {index}: {error}")]                 Arg { index: usize, error: ArgError },   // ArgError::pointer: the RFC 6901 position in the argument's value JSON, `/1/id/$payload/0`
    #[error("argument {index} is not a constant expression")] ArgNotConstant { index: usize, diagnostic: Diagnostic },   // the parse diagnostic, or `usage_bad_argument` spanning the offending part in ARGUMENT_FILE
}

// ---- cpl_eval::test ---------------------------------------------------------
pub struct TestSelector {
    pub namespace: Option<Vec<String>>,            // module-path prefix (`tests.pagination`, `tests.pagination.*`); None = all
    pub name: Option<String>,                      // exact test/property name (the TEXT literal's value)
    pub id: Option<StableId>,
}
/// `parse` accepts a module path or a namespace wildcard (`a.b`, `a.b.*`, both meaning the namespace, T43); anything else
/// is Err, which `cpl test` reports as `usage_bad_argument` (exit 2).
impl TestSelector { pub fn all() -> Self; pub fn parse(arg: &str) -> Result<TestSelector, String>; }  // Section 7.2 `cpl test`

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestConfig {
    pub seed: u64,                                 // the run seed (semantics S44)
    pub cases: u32,                                // per property; default [test] cases = 100 (TestConfig::DEFAULT_CASES); 0 is accepted here (a property then runs no case) but refused by `cpl test` and `cpl.toml` (T57)
    pub max_shrinks: u32,                          // body evaluations spent shrinking per failure; default [test] max_shrinks = 1000 (DEFAULT_MAX_SHRINKS)
    pub fail_fast: bool,                           // default false
    pub eval: EvalConfig,                          // time_budget from [test] timeout_ms
}
impl TestConfig { pub fn with_seed(seed: u64) -> TestConfig; }   // the defaults (100 cases, 1000 shrinks, no fail-fast, EvalConfig::default()) with that run seed
pub fn select_tests(p: &Program, selector: &TestSelector) -> Vec<DefId>;            // order: qualified name (byte order of the display string)
pub fn run_tests(p: &Program, selector: &TestSelector, config: &TestConfig) -> TestReport;   // on the evaluation thread
pub fn run_test(p: &Program, test: DefId, config: &TestConfig) -> TestCaseRecord;
/// The `test_result` diagnostic of a record (semantics S46): `info` when tested, `error` when failed; `symbol` and
/// `qualified_name` the test's identity; no span (so the diagnostics stay in completion order under T46); every other
/// field of the record flattened into the payload.
pub fn test_result_diagnostic(record: &TestCaseRecord) -> Diagnostic;
pub fn seed_hex(seed: &[u8; 32]) -> String;       // a property seed as the 64 lowercase hex digits records carry (T45)
pub const TEST_RESULT: DiagCode;                  // "test_result"
pub const CONTRACT_COUNTEREXAMPLE: DiagCode;      // "contract_counterexample"
/// semantics S44 (adopted by spec/stdlib.md 8.2): `blake3(run_seed_le(8) ++ property_id_utf8)`, where `property_id` is
/// the property's identity (semantics S42: its `@id` value, else its qualified name as `SymbolPath::display` renders it).
/// The 32-byte `ChaCha20Rng::from_seed` key of the property's one stream; reported as 64 lowercase hex digits
/// (`property_seed` in records and receipts, T45).
pub fn property_seed(run_seed: u64, property_id: &str) -> [u8; 32];
```

`TestReport` and `TestCaseRecord` are wire types in `cpl_diag::json`
(Section 7.2 gives their JSON); `run_tests` returns them directly. A
`TestCaseRecord` is the payload of the `test_result` diagnostic of
semantics S46. Its `receipt` is an `Option<ReceiptCore>` that `run_tests`
leaves `None`: the caller (`cpl-cli`) fills it, since it needs the
revision and the module content hashes of `cpl-index`; its
`counterexamples` are the `contract_counterexample` diagnostics the
failure produced, which the caller also emits on their own. A report with
`selected == 0` is the caller's `empty_test_selection`. `run_tests` and
`run_test` honour `config.eval` as given, `check_contracts` included:
`cpl test` always passes `true` (semantics 7.3: tests run in checked mode),
and a host that passes `false` runs unchecked, which the test receipt
records (`configuration.check_contracts`, Section 8.2).

The crate also has a doc-hidden module `cpl_eval::testing` for tests: a
`ProgramBuilder` that loads the real `std/*.cpl` stubs and lowers HIR by
hand, and `testing::corpus` (`parse_cases`, `run_case`), the reader and
runner of the interpreter corpus format of Section 12.5. It is not part of
the contract of this section.

#### 1.7.1 Contract checking in the interpreter

Semantics Sections 5.2-5.4, 6.3, 6.7, and 7.1 own these rules; the table
records how the evaluator reports them.

| Event | Check | On failure |
| - | - | - |
| call of a CPL function `f` | each `requires` clause of `f`, in source order, with parameters bound; a later clause runs only if every earlier one held | `ContractViolation(Requires)`: `symbol` = `f`, `facet` = `contract`, `data.attributed_to = "caller"`, `data.caller` = the calling declaration (absent for `cpl run`'s top-level call), `data.input`, `span` = the clause (semantics 5.2) |
| call of a `@builtin` `f` | none by the interpreter, which never evaluates a stub's `requires` or `ensures` clauses: the builtin's Rust code checks each of its preconditions first, in source order and in every contract mode (stdlib S39, semantics S58) | the same `ContractViolation(Requires)` shape: `symbol` = the stub's qualified name, `clause` and `span` = the stub's clause (from its `requires` keyword), `data.input` with the receiver under `self`, `data.caller` = the innermost active declaration (the refinement declaration for a builtin called in a `where` predicate; absent at `cpl run`'s top level) |
| return from `f` (every exit: tail, `return`, a propagated `Err`) | each `ensures` clause, in source order, with `result` bound | `ContractViolation(Ensures)`: `symbol` = `f`, `facet` = `contract`, `data.attributed_to = "body"`, `span` = the clause; `data.input`, `data.actual` |
| `while` entry and after every iteration body | each `invariant`, in source order | `ContractViolation(Invariant)`: enclosing declaration, `facet` = `contract`, `data.attributed_to = "body"`; `data.iteration`, `data.actual = { bindings }` (semantics 5.3) |
| `while` with `decreases` | exactly semantics 5.3: the measure is evaluated before each body execution (after the condition held), must be `>= 0`, and must be strictly smaller than the previous iteration's value; it is never evaluated after the loop exits | `ContractViolation(Decreases)`: `facet` = `contract`, `data.attributed_to = "body"`, `data.actual = { bindings, measure, previous? }`, `data.iteration` |
| entry to a fn with a header `decreases` | semantics S35: after its `requires` clauses held, the measure is `>= 0` and strictly smaller than the measure of the nearest enclosing activation of the same fn; when that activation has not recorded a measure yet (it is still evaluating its `requires` clauses), there is no comparison | `ContractViolation(Decreases)`: `facet` = `contract`, `data.attributed_to = "body"`, `data.actual = { measure, previous? }`, `data.input` |
| capability operation call `c.op(args)` (no handle exists in Milestone 1; from Milestone 2 the handles of `cpl run`'s bindings, semantics S78) | each `requires` clause of the operation, evaluated by the caller before the handle is invoked | `ContractViolation(Requires)`: `symbol` = the operation's qualified name, `facet` = `contract`, `data.attributed_to = "caller"`, `data.caller` (semantics S31) |
| return of a capability operation | for a handle bound with `BindOptions { validate_results: true, .. }` (every Milestone 2 path, Section 1.7.4), first the validation of the result by semantics S77 (its shape, then its refined positions in S74 order); then each `ensures` clause with `result` bound to the handle's result | a failed validation is the outcome S77 assigns: an `Err(ForeignError { kind: InvalidResult, .. })` value when the return type carries `ForeignError`, else `RefinementViolation` (`attributed_to: "binding"`) or `Internal` (kind `binding_result_ill_typed`); a failed `ensures` is `ContractViolation(Ensures)`: `symbol` = the operation, `facet` = `contract`, `data.attributed_to = "binding"` (semantics S31, S57) |
| `assert e` | `e` is `true` | `AssertionFailed`: `data.clause` (text), `data.bindings` (visible bindings with values) |
| `?name` evaluated | never succeeds | `UnresolvedHole`: `data.hole`, `data.expected_type`, `data.bindings` (with values) |
| `/`, `%` with zero divisor | | `DivisionByZero`: `data.input = { a, b }` |
| call depth exceeds `max_call_depth` (CPL function activations only, semantics S72), or the evaluation thread's stack guard trips (`stack` module); wall clock exceeds `time_budget` (checked at every function entry and loop iteration) | | `ResourceExhaustion(Stack)` / `ResourceExhaustion(Time)`: `data.resource`, `data.limit` (for `time`, the budget in whole milliseconds, truncated); the stack guard adds `data.depth` |
| `check<R>(x)` | every predicate of `chain(R)` with `self` = `x`; no fault: `Ok(x)` or `Err(ConstraintError)` (Library: payload) | - |
| `Refine` node (Section 3.3) re-evaluated when `check_contracts` is on | predicate holds | `Internal` (`kind: "refinement_proof_unsound"`): a static proof was wrong |
| a contract expression itself faults | | the inner fault propagates with `facet = contract`, `data.clause` = the clause text, `symbol` = the clause's owner (semantics S34); with nested clauses the innermost one a fault escapes claims it (S70); in a property's `for` range, S71; an `Internal` fault is never rewritten (it keeps no symbol, facet, or clause, S34) |
| operation call on an unbound handle | | `Internal` (`kind: "unbound_capability"`) |

Decision: [D-143] T9. `cpl-eval` implements `Int` division and remainder exactly as
`spec/semantics.md` S37 specifies (quotient truncated toward zero, remainder
with the dividend's sign, `division_by_zero` on a zero divisor) in one
function, `cpl_eval::arith::div_rem`, and nowhere else. Rationale: the
semantics specification owns arithmetic; concentrating the rule in one
function keeps a future change (Milestone 5 may prefer an SMT-friendly
convention) a one-line edit. Rejected alternative: scattering `BigInt`
operators through the evaluator.

Amendment (T70, Milestone 2): the one function is
`cpl_rtspec::arith::div_rem` (Section 1.11), which
`cpl_eval::arith::div_rem` calls with its name and signature unchanged
(mapping a zero divisor to `division_by_zero`) and which `cpl-sema`'s
constant evaluator calls in place of its own copy; the C runtime's
division mirrors it (`spec/cabi.md` Section 2).

#### 1.7.2 Test and property runner

Selection: `select_tests` returns every `test` and `property` definition
whose module path starts with `namespace` segment by segment (all modules
when `None`), filtered by `name` and `id` when given, sorted by qualified
name: the byte order of the `SymbolPath::display` string, which is also
the run order (semantics 7.3), so `ord."a b"` and `ord."a!"` run before
`ord."a"`. A selector that matches nothing is `empty_test_selection`
(semantics S42); `cpl test` exits 1 (Section 7.2). With `fail_fast` the
run stops after the first failing record; `selected` still counts the
whole selection and `tested` counts the records that ran and passed.

The seed derivation is semantics S44 (adopted by `spec/stdlib.md` 8.2,
S31); distributions and shrink candidates are `spec/stdlib.md` 8.3 and 8.4;
this document restates none of them and fixes only what the runner records.
The run seed is `--seed N` (a decimal `u64`) or 64 random bits from the OS,
always reported as a decimal integer. Each property has one
`ChaCha20Rng::from_seed(property_seed(run_seed, property_id))` stream for
the whole run (`property_seed` above; `property_id` is the property's
identity: its `@id` value, else its qualified name); case `i` (0-based)
runs at size `min(i, 100)` and draws from that stream in case order, the
`for` ranges drawing in clause order with the distributions of stdlib 8.3,
so `--cases N` changes no case a smaller `N` would also have run; the
arguments of `gen.range(lo, hi)` are evaluated once per property before
case 0 (the *setup*, in clause order, `lo` before `hi`), and a fault there
fails the property with that fault and zero cases: a fault raised while
the bound expressions evaluate is reported per semantics S71 (the
property's contract expression), and the generator's own `requires lo <=
hi`, which the runner checks after both bounds, keeps the builtin shape
of `spec/stdlib.md` 12 (`symbol` `std.gen.range`, `clause` `lo <= hi`,
`caller` the property). A contract violation raised inside the setup by
a function a bound calls also yields its `contract_counterexample`
(semantics S46). The setup, like the runner's refinement checks of drawn
values and shrink candidates, runs without the time budget (Section 7.2
budgets only the evaluations it lists), so a bound expression that never
terminates hangs `cpl test` even under `--timeout-ms`. A plain `test`
draws nothing.

Outcome of one case (semantics 7.3): the body completes (pass) or raises
any fault (fail). A plain `test` with a fault is reported with that fault;
a property additionally shrinks.

Shrinking follows semantics S45 and stdlib 8.4 exactly: greedy,
coordinate-wise over the binders in clause order, with the candidate lists
of stdlib's table (an `Int` toward `0`, a `Positive` toward `1`, a list by
halves, then element removal, then element shrinking, ...), re-running the
body with one coordinate replaced, accepting the first candidate that fails
*with the same fault code* as the original failure (S45, stdlib S32) and
continuing with that binder from the accepted value, moving to the next
binder when none of its candidates is accepted, restarting from the first
binder after the last, and stopping after a full pass accepts nothing or
when `[test] max_shrinks` (default 1000) body evaluations have been spent.
Candidates are produced lazily and are not deduplicated (`1` has the three
candidates `0`, `0`, `0`; each rerun counts as one step).
`shrink_exhausted` is `true` exactly when another rerun was wanted but the
budget was spent: when the last allowed step was also the last candidate
of the final pass it is `false`, and with `--max-shrinks 0` it is `true`
whenever the failing value has any candidate. Every candidate is
additionally checked against every refinement predicate of the binder's
type (`satisfies_refinement`; a faulting predicate discards the
candidate), which is what keeps a `Positive` binder at `>= 1`. The record
reports `failing_case = { index, input: the original tuple }`, `shrunk = {
input: the final tuple, shrink_steps, shrink_exhausted }`, and `fault` =
the fault raised by the final tuple; `cases` is the failing index plus one,
as S46 specifies.

Decision: [D-144] T45. the property runner's random stream is the one semantics
S44 defines (one ChaCha20 stream per property keyed by the 32-byte blake3
digest of the run seed and the property's identity, which `spec/stdlib.md`
S31 adopts), every generator's distribution is `spec/stdlib.md` 8.3, and
shrinking is semantics S45 with the candidate orders of `spec/stdlib.md`
8.4; this document restates none of them and records the run seed as a
decimal `u64` and `property_seed` as the 64-lowercase-hex-digit digest, so
that a consumer can recompute every case from a record or receipt alone.
Rationale: three documents once had three incompatible derivations, which
made `failing_case`, `shrunk`, and the golden files
implementation-dependent; the semantics document owns the diagnostic fields
(S60) and the library owns the `gen` vocabulary the runner interprets, so
the toolchain adds nothing of its own. Rejected alternatives: a
toolchain-private derivation (an earlier revision of this section); the
per-case FNV-1a keys and any-failure shrinking of an earlier revision of
`spec/stdlib.md`.

From Milestone 2 the runner is generic over the engine
(`cpl_eval::test::TestEngine`, Section 1.7.4): selection, seeds,
generation, shrinking, the setup's `gen.range` precondition (now built by
the pure `builtins::check_gen_range_at` with the property as `caller`),
and the assembly of records stay in the runner for both engines, and only
body evaluation, refinement checks, range bounds, and the "as raised"
violation records come from the engine. `run_tests` and `run_test` are the
runner over a fresh `Interpreter` per test, with unchanged results.

#### 1.7.3 Faults as diagnostics

`Fault::to_diagnostic` maps kinds to the codes of semantics 6.7 and 9.4.
`input` is the declaration's positional parameters as value JSON (Section
7.3; a `using` parameter's handle is not listed, semantics 5.2),
`actual` the returned value or the bindings, `clause` the canonical clause
text (Section 5.2, T56); `backtrace` is `[{ "qualified_name", "span"? }]`
innermost first (every activation: CPL functions, test and property
bodies, refinement predicates, and `@builtin` callees, each with the span
of the call that entered it; omitted when empty, as for a
`refinement_violation` of a `cpl run` argument, raised before any
activation), and `symbol`/`qualified_name` name the blamed declaration.

| `FaultKind` | code | category | `symbol` | flattened payload keys |
| - | - | - | - | - |
| `ContractViolation(k)` | `contract_violation` | contract | the declaration whose clause failed | `clause_kind` (`requires`/`ensures`/`invariant`/`decreases`), `attributed_to` (`caller`/`body`/`binding`, semantics S57), `clause`, `input`, `actual`, `caller` (requires), `iteration` (loops), `status: "failed"`, `backtrace` |
| `AssertionFailed` | `assertion_failed` | test | enclosing declaration | `clause`, `bindings`, `backtrace` |
| `UnresolvedHole` | `unresolved_hole` | hole | enclosing declaration | `hole`, `expected_type`, `bindings`, `backtrace` |
| `DivisionByZero` | `division_by_zero` | runtime | enclosing declaration | `input`, `backtrace` |
| `RefinementViolation` | `refinement_violation` | runtime | the function whose argument was violated (a `cpl run` argument, semantics S39; from Milestone 2 also a C export's, S81), or, from Milestone 2, the operation whose result was violated (S77) | `expected_type` (the declared type at the failing position, `Positive`), `actual`, `clause` (the first predicate of the chain, root first, that is false), `parameter` (the argument's name; absent for a result), `pointer` (the RFC 6901 position of the failing value in the argument or the result), `attributed_to` (`binding`) and `binding` (a result only, S77), `backtrace`; `span` = the parameter's, or the operation call's for a result |
| `ResourceExhaustion(r)` | `resource_exhaustion` | internal | enclosing declaration (none for a refused top-level call, as under `max_call_depth = 0`, semantics S72) | `resource` (`stack`/`memory`/`time`/`steps`), `limit` (milliseconds for `time`; from Milestone 2 bytes for `memory`, raised natively only, semantics S76), `depth` (the stack guard, Section 1.7), `backtrace` (possibly empty for `memory`, S76) |
| `Internal` | `internal_error` | internal | - | `kind`, `detail`, `backtrace`; from Milestone 2, for the binding kinds of semantics S80, `operation` and `binding`, and `pointer` for `binding_result_ill_typed`, with `span` the operation call |

`facet` is `contract` for every `contract_violation` (the clause belongs
to the declaration's contract facet; blame is carried by `attributed_to`,
semantics S57) and for any fault raised while evaluating a clause (then
`clause` names it; semantics S34); it is `body` for every other fault that
has a `symbol`, a `cpl run` argument's `refinement_violation` included
(its `symbol` is the function, although the violated position is a
parameter). `facet` accompanies `symbol`, so `internal_error`, which
has none, has no `facet` either: `Fault::to_diagnostic` never renders
`symbol`, `qualified_name`, or `facet` for an `internal_error`, whatever
the fault carries, and the S34 rewrite never gives one a `clause`. The keys a kind owns (`clause_kind`,
`status: "failed"`, `attributed_to`, `resource`, `kind`) are always
present; payload keys that are reserved (T5) are dropped.

The design document's example (`"code": "contract_counterexample",
"symbol": "pgn_7c2d", "facet": "body", "clause": "flatten(result) ==
items", "input": {...}, "actual": [[1, 2]], "status": "failed"`) is not a
fault: it is the additional diagnostic the test runner emits (semantics
S46) when a test observes an `ensures`, `invariant`, or `decreases`
violation in a declaration other than the test itself; it carries that
declaration as `symbol`, `facet: "body"`, the clause, `input`, `actual`,
`status: "failed"`, and `found_by` (the test's identity), plus
`clause_kind` (and `iteration` for a loop clause), with `span` the
violated clause. It is built from the violation as originally raised, so
it stays correct after an S34 rewrite of the fault the test reports, and it
is not emitted for a capability operation's `ensures`, whose failure is the
host binding's, not a body's (S31). The verifier's
abstract models (Milestone 5) use `verification_result` with
`status: "failed"` and a `model` key, keeping concrete and abstract
evidence distinguishable as the design requires.

#### 1.7.4 Milestone 2 additions

Milestone 2 adds to `cpl-eval` exactly what the native engine and the
bindings of `cpl run` need (T77). Every item below is new or a
byte-identical refactor: no Milestone 1 signature, struct, or behaviour
that a test exercises changes.

```rust
// ---- cpl_eval::interp -------------------------------------------------------
/// `label` is the handle's binding name (semantics 6.9), which `boundary::validate_result`, every fault of semantics S77
/// and S80 that the handle's results cause (`binding`), and the `adapter` detail of stdlib S45 carry verbatim: the name
/// of the `[bindings.<name>]` table for a handle that `cpl run` binds (Section 1.14), the label a test gives its host
/// otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindOptions { pub label: String, pub validate_results: bool }
impl<'p> Interpreter<'p> {
    /// `bind_capability` with options. With `validate_results`, every result of `imp` passes semantics S77's validation
    /// (`boundary::validate_result`, at the operation call's span) before the operation's `ensures` clauses run; every
    /// Milestone 2 path binds this way.
    pub fn bind_capability_with(&mut self, capability: DefId, imp: Box<dyn CapabilityImpl>, options: BindOptions) -> Handle;
    // bind_capability(capability, imp, label) is unchanged: BindOptions { label, validate_results: false }, trusted
    // (semantics 6.9), so an ill-typed host result still surfaces as internal_error kind type_confusion where it is used.
}
/// The binding sites of `def` (semantics S41, `spec/cabi.md` 3.2): for every `assert`, `while` loop, and hole of its
/// body, in source order, the span of the statement or hole and the locals in scope there, outermost first, in the
/// order the interpreter brings them into scope: the positional parameters, then the `using` parameters, then every
/// `let`, `var`, pattern, `forall`, and `for` binder as its evaluation binds it, a block's binders leaving scope at its
/// end. The interpreter's environment at each site holds exactly these locals in this order (the `bindings` of a fault
/// list them innermost first), which crates/cpl-eval/tests/m2_hooks.rs checks at every site of every corpus program;
/// the binding-site tables of emitted code are this list (`spec/cabi.md` C13).
pub fn scope_at(p: &Program, def: DefId) -> Vec<(Span, Vec<LocalId>)>;
/// Was pub(crate); public for TestEngine and re-exported at the crate root. Fresh: a top-level evaluation with its own
/// time budget (a host's call, a test body, a property case, a shrink rerun, Section 7.2); None: the runner's
/// unbudgeted evaluations (a property's range setup, the refinement checks of drawn values and shrink candidates).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Budget { Fresh, None }

// ---- cpl_eval::boundary (new): semantics S77 and stdlib S45 in Rust ------------
/// The validation of `v`, the result of operation `op` (declaration order) of `capability`, called at `span` on a handle
/// whose binding name is `binding` (its `BindOptions::label`): Ok(v), or Ok(the `Err(ForeignError { kind: InvalidResult,
/// .. })` that replaces it), or the fault S77 assigns. It runs inside the current evaluation and starts none (its time
/// budget, clause attribution, and violation record are kept); each predicate is an activation of its refinement
/// entered through `span`, and every fault it builds spans `span`.
pub fn validate_result(it: &mut Interpreter<'_>, capability: DefId, op: u32, binding: &str, span: Span, v: Value)
    -> Result<Value, Fault>;
/// A failure that the binding named `binding` reported for operation `op` of `capability`, with its `ForeignErrorKind`
/// name, its message, and its details in the order it gave them: the `Err(ForeignError)` value of stdlib S45, the
/// runtime's details (`adapter`, `capability`, `operation`) first, when the operation's return type is a foreign-error
/// return type, else the fault `internal_error` kind `binding_error_unrepresentable` of semantics S80. `CAdapter` and
/// `ScriptHost` (Section 1.14) build every reported failure with it, as the C runtime builds it natively.
pub fn foreign_failure(p: &Program, capability: DefId, op: u32, binding: &str, kind: &str, message: &str,
                       details: &[(String, String)]) -> Result<Value, Fault>;

// ---- cpl_eval::test ---------------------------------------------------------
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RangeSide { Lo, Hi }
/// What the runner of Section 1.7.2 needs from an engine; each method is one top-level evaluation of the engine.
pub trait TestEngine<'p> {
    fn program(&self) -> &'p Program;
    /// A test or property body, the `for` binders bound in clause order; Ok when the body completes.
    fn run_body(&mut self, test: DefId, binders: &[(LocalId, Value)]) -> Result<(), Fault>;
    /// The last `ensures`, `invariant`, or `decreases` violation of the last evaluation, as raised (semantics S46).
    fn take_violation(&mut self) -> Option<(DefId, Fault)>;
    /// The first refinement of `chain(refined)`, root first, whose predicate is false for `value`; None when all hold.
    fn failing_refinement(&mut self, refined: DefId, value: &Value, budget: Budget) -> Result<Option<DefId>, Fault>;
    /// Bound `side` of the `gen.range` of binder `binder` (clause order) of `property`, reported per semantics S71 with
    /// `clause_text`, the range's clause text.
    fn eval_range_bound(&mut self, property: DefId, binder: usize, side: RangeSide, clause_text: &str) -> Result<Value, Fault>;
}
impl<'p> TestEngine<'p> for Interpreter<'p> { /* the evaluations run_tests already performs */ }
/// The runner over any engine; `engines` yields the engine of each selected test, in run order. `run_tests(p, s, c)`
/// is this over a fresh Interpreter per test, and `run_test` likewise over one.
pub fn run_tests_with<'p>(p: &'p Program, selector: &TestSelector, config: &TestConfig,
                          engines: &mut dyn FnMut() -> Box<dyn TestEngine<'p> + 'p>) -> TestReport;
pub fn run_test_with<'p>(p: &'p Program, test: DefId, config: &TestConfig, engine: &mut dyn TestEngine<'p>) -> TestCaseRecord;

// ---- cpl_eval::run ----------------------------------------------------------
/// What `run_function` settles before it evaluates: the target, its decoded arguments, and its return type.
pub struct PreparedRun { pub function: DefId, pub arguments: Vec<Value>, pub return_ty: Ty }
/// `run_function`'s refusals in its order, `CannotRunEffectful` only for a `using` parameter for which `has_host` is
/// false (semantics S78), then the decoding of the arguments. `has_host` has no side effect (`cpl run` answers it from
/// the interop plan), so that a refused run builds and starts nothing.
pub fn prepare_run(p: &Program, path: &SymbolPath, args: &[RunArg], has_host: &dyn Fn(&ParamDef) -> bool)
    -> Result<PreparedRun, RunError>;
/// The hosts of `cpl run`'s `using` parameters (semantics S78), in two steps, so that a refused run starts nothing.
pub trait RunHosts {
    /// Whether `param`'s capability has a host: the refusal check of S78. It has no side effect.
    fn available(&self, param: &ParamDef) -> bool;
    /// The host of `param` and its options, asked for once per `using` parameter, in parameter order, after the
    /// refusals and the decoding of the arguments; it may build and start a driver. Err: the diagnostics of a host
    /// that cannot be created or granted (`interop_bind_failed`, or a driver that cannot be built), which `cpl run`
    /// reports with the exit codes of Section 7.4.
    fn create(&mut self, param: &ParamDef) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>>;
}
/// What provides a `RunEngine`'s handle for a `using` parameter.
pub enum HostSource {
    Impl(Box<dyn CapabilityImpl>, BindOptions),   // a host implementation, which the evaluator binds with bind_capability_with
    Binding(String),                              // the backend-c binding of that name, which the native engine grants in its driver
}
/// What `cpl run` needs from an engine after `prepare_run`.
pub trait RunEngine {
    /// Bind the handle of `param`; called in `using`-parameter order, so the handles are numbered from 0. Err: the
    /// handle cannot be provided (a grant that failed, `interop_bind_failed`; a source the engine does not take,
    /// `internal_error` kind `call_mismatch`), and then nothing runs (semantics S78).
    fn bind(&mut self, param: &ParamDef, host: HostSource) -> Result<Handle, Vec<Diagnostic>>;
    /// Semantics S39 (a), the refined positions of the arguments, then the call with no calling declaration (S39 (c)).
    fn call_checked(&mut self, f: DefId, args: Vec<Value>, using: Vec<Handle>) -> Result<Value, Fault>;
}
impl<'p> RunEngine for Interpreter<'p> { /* HostSource::Impl, bound with bind_capability_with */ }
/// `run_function` with hosts: the refusals ask `hosts.available`; after the decoding of the arguments the host of every
/// `using` parameter is created (`hosts.create`) and bound, in parameter order and numbered from 0, before the refined
/// positions of the arguments are checked (semantics S78); a host that cannot be created is `RunError::Bind`, and
/// nothing runs. `run_function(p, path, args, config)` is this with hosts that have none.
pub fn run_function_with_hosts(p: &Program, path: &SymbolPath, args: &[RunArg], config: &EvalConfig,
                               hosts: &mut dyn RunHosts) -> Result<RunOutcome, RunError>;
// RunError gains one variant: #[error("a using parameter could not be bound")] Bind(Vec<Diagnostic>), the
// diagnostics RunHosts::create returned.

// ---- cpl_eval::builtins -----------------------------------------------------
/// The facts of requires clause `clause` (0-based) of builtin `id`: from the program's stub when it declares one, else
/// the cpl-rtspec fallback (Section 1.11). Extracted from the private `requires_violation`, which now uses it.
pub struct RequiresFacts { pub symbol: StableId, pub clause: String, pub span: Option<Span>, pub inputs: Vec<String> }
pub fn requires_spec(p: &Program, id: &str, clause: usize) -> RequiresFacts;
/// The precondition fault of stdlib 10.4 with `caller` given (None at the top level).
pub fn requires_violation_at(p: &Program, caller: Option<DefId>, id: &str, clause: usize, args: &[Value]) -> Fault;
pub fn check_gen_range_at(p: &Program, caller: Option<DefId>, lo: &BigInt, hi: &BigInt) -> Result<(), Fault>;
// check_gen_range(&Interpreter, lo, hi) stays, as check_gen_range_at(it.program(), it.current_def(), lo, hi).

// ---- cpl_eval::types --------------------------------------------------------
/// The qualified identity of operation `op` of `capability` (`reader.pages.PageRead.get`, semantics 1.4); it was the
/// private `Interpreter::operation_identity`.
pub fn operation_identity(p: &Program, capability: DefId, op: &str) -> StableId;
```

Refactors that must be byte-identical: every message the evaluator
formats whose text a specification fixes is rendered from its
`cpl_rtspec::messages` template; `is_white_space` (the `trim` family) and
the precondition fallbacks read `cpl-rtspec`; the registry's ids are
tested equal to `cpl_rtspec::builtins::IDS`; `arith::div_rem` delegates to
`cpl_rtspec::arith::div_rem` (T9 as amended); and `stack::EVAL_STACK_SIZE`,
`stack::GUARD_MARGIN`, and `EvalConfig::DEFAULT_MAX_CALL_DEPTH` keep their
names and values and are defined from `cpl_rtspec::limits`. `BuiltinFn`,
`Value` (still `Rc`, T8), `HostContext`, `CapabilityImpl`, `Fault`,
`FaultKind`, `EvalConfig`, and every `pub` signature of Sections 1.7-1.7.3
are unchanged; `RunError` gains the one variant `Bind`.

| Milestone 1 constraint (checked in the code) | How Milestone 2 keeps it |
| - | - |
| `crates/cpl-eval/tests/adversarial_pipeline_language.rs` binds an ill-typed host through `bind_capability` and asserts `internal_error` kind `type_confusion` | `bind_capability` stays trusted; validation is `bind_capability_with(.., BindOptions { validate_results: true, .. })` |
| literals of `InteropSection` and `BindingSection` (`crates/cpl-project/tests/manifest.rs`) and of `Lock` (`crates/cpl-project/tests/lock.rs`, `crates/cpl-sema/tests/driver.rs`); `check_bindings` accepting `backend = "c"` with `adapter = "a"` | the new `Manifest` fields, the typed lock views over the existing tables, and `check_c_bindings` (T80) |
| `render_lock` writing the values of a reserved table inline (`crates/cpl-project/tests/lock.rs`) | unchanged; the C entries are such values (T75) |
| literals of `RunOutput` and `TestOutput` (`crates/cpl-diag/src/tests.rs`), `TestReport` and `TestCaseRecord` (`crates/cpl-index/tests/`) | unchanged; the CLI serializes wrapper types whose new keys are omitted under the default engine (T73) |
| `check_gen_range(&it, ..)` (`crates/cpl-eval/tests/foundation/builtins.rs`); `BuiltinFn` called with `&mut Interpreter` (`foundation/support.rs`) | unchanged signatures; the generic runner uses the new `_at` functions |
| `Rc::ptr_eq` on value buffers (`foundation/value.rs`, `evaluator/depth.rs`); tests reading `HostContext::program` | `Value` stays `Rc`, `HostContext { program }` stays |
| the CLI goldens of `run` and `test` (`crates/cpl-cli/tests/golden/`) | no `engine` or `bindings` key under the default engine; no golden is a `cannot_run_effectful` envelope and no test asserts its note |
| the iteration over `Category::ALL` in `crates/cpl-diag/src/tests.rs` | no count is asserted; the two new variants serialize as their names |
| `RunError` matched in `crates/cpl-eval/tests/examples.rs`, `adversarial_pipeline_language.rs`, and `evaluator/run.rs` | the new variant `Bind` changes no outcome: every such `match` has a wildcard arm, and the others are `matches!` |

Decision: [D-273] T77. `cpl-eval` gains the additive API of this section
and nothing else: the validation of adapter results is opt-in per binding
(`bind_capability_with`) and the Milestone 1 `bind_capability` stays
trusted; the property runner becomes generic over `TestEngine`, its
`gen.range` precondition built by the pure `check_gen_range_at`; `RunError`
gains the variant `Bind`, for a host of semantics S78 that cannot be
created; and `cpl run`'s refusals and decoding move into `prepare_run`,
which both engines share. Rationale: the native engine must reuse the evaluator's runner,
refusals, decoding, and rendering, so that the two engines differ in
evaluation alone (semantics S75), and the accepted Milestone 1 suite
(Section 13) is the oracle's evidence, which a changed test would weaken;
the table above lists what the code requires. Rejected alternatives:
validating every `CapabilityImpl` result (it breaks the `type_confusion`
test); a Rust core shared by the engines, with `Arc` values, a reshaped
`BuiltinFn`, and a new `HostContext` (every one of these breaks accepted
tests and APIs, and T68 rejects the shared core); a runner that keeps
pushing a frame on an `Interpreter` to give a builtin its `caller` (the
native engine has no `Interpreter`).

### 1.8 `cpl-index` public API

Every item is also re-exported at the crate root (`cpl_index::facet_map`,
`cpl_index::plan_id_assignments`, ...). The identity, facet, revision,
diff, receipt, and history items need no checked `Program` and are
computed from a loaded `Project` alone (T52); the `Index` of `query` and
`evidence` reads a checked `Program` through `cpl-sema` (Section 1.2) and
supplies everything `cpl inspect`, `cpl prove`, and the receipts and hole
dependencies of `cpl check` and `cpl test` need, so that `cpl-cli` holds no
logic of its own (1.10, T3).

```rust
// ---- cpl_index::id (Section 4) ----------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum IdKind { Fn, Ty, Cap, Test, Prop }
impl IdKind {
    pub const ALL: [IdKind; 5];
    pub fn for_decl(kind: &ast::DeclKind) -> IdKind;           // type/struct/enum -> Ty, capability -> Cap, fn -> Fn, test -> Test, property -> Prop
    pub fn prefix(self) -> &'static str;                        // "fn", "ty", "cap", "test", "prop"; also Display
    pub fn from_prefix(prefix: &str) -> Option<IdKind>;
}
/// Section 4.1: 10 bytes drawn with `fill_bytes`, 16 base32 characters, most significant bit first. `rand::Rng` is rand
/// 0.10's infallible generator trait (Section 1.3): a seeded ChaCha20Rng in tests, OS randomness in the CLI.
pub fn generate_id(kind: IdKind, rng: &mut dyn rand::Rng) -> StableId;
pub fn is_generated_format(id: &StableId) -> bool;                                      // matches the generated-format regex of Section 4.1 (`^(fn|ty|cap|test|prop)_[a-z2-7]{16}$`), not the accepted syntax
pub use cpl_diag::StableId;                                                             // provisional derivation is StableId::provisional (Section 4.3)

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdAssignment {
    pub file: FileId,
    pub path: String,                              // display symbol path of the declaration
    pub kind: IdKind,
    pub id: StableId,
    pub insert_at: u32,                            // byte offset of the first annotation or, if none, of `pub`/the keyword (after the doc comments)
    pub indent: String,                            // the horizontal whitespace between the start of that line and `insert_at`: the declaration's indentation ("" in a formatted file)
}
impl IdAssignment {
    /// The JSON of Section 7.2; `line` is the 1-based line of `insert_at` (the first annotation's line, else the keyword's).
    pub fn to_json(&self, source_map: &SourceMap) -> IdAssignmentJson;
}
/// One assignment per top-level declaration without an `@id` annotation, in (file path, source order) order (a
/// malformed `@id` is left alone for the checker to report); a file that failed to parse is skipped (Section 4.2).
/// Deterministic given `rng`.
pub fn plan_id_assignments(project: &Project, rng: &mut dyn rand::Rng) -> Vec<IdAssignment>;
/// Insert `@id("...")` LF at column 0 of the declaration's line, at offset `insert_at - indent.len()`, so that the
/// declaration's own line keeps every byte; the result differs from `text` only by the inserted lines (Section 4.2).
/// Panics when `indent` is not the text that ends at `insert_at`, or an offset is not a character boundary.
pub fn apply_id_assignments(text: &str, assignments: &[IdAssignment]) -> String;

// ---- cpl_index::facet (Section 5) -------------------------------------------
pub fn applicable_facets(kind: &ast::DeclKind) -> &'static [Facet];   // Section 5.1, in canonical order
/// Canonical text of one facet of a declaration; None when the facet does not apply to that kind.
/// `module_path` is the file's `module` header, which the provisional `identity` text needs (Section 4.3).
pub fn facet_text(module_path: &[String], decl: &ast::Declaration, facet: Facet) -> Option<String>;
pub fn facet_hash(canonical_text: &str) -> FacetHash;                 // blake3, first 32 hex chars
pub fn definition_hash(facets: &BTreeMap<Facet, FacetHash>) -> FacetHash;  // Section 5.4
/// The stable identity (Section 4.3): the value of a well-formed `@id` (the first `@id` annotation, exactly one text
/// argument of the accepted syntax), else the provisional identity; a malformed `@id` identifies nothing.
pub fn declaration_identity(module_path: &[String], decl: &ast::Declaration) -> StableId;
pub fn symbol_path(module_path: &[String], decl: &ast::Declaration) -> SymbolPath;   // the qualified name, Section 4.4
/// The single producer of SymbolVersion: the facet map and the Index (through Program::ast) both use it.
pub fn symbol_version(module_path: &[String], decl: &ast::Declaration) -> SymbolVersion;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FacetEntry {
    pub version: SymbolVersion,                    // id, path, provisional, facets, definition
    pub kind: IdKind,
    pub module: String,                            // "reader.pages"
    pub name: String,                              // declaration name or test/property name text
    pub file: String,                              // project-relative path
    pub effects: BTreeSet<String>,                 // fn only: "db.get"; empty otherwise
}
impl FacetEntry {
    pub fn new(module_path: &[String], decl: &ast::Declaration, file: &str) -> FacetEntry;
    pub fn facet(&self, facet: Facet) -> Option<&FacetHash>;   // None when the facet does not apply
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FacetMap {
    pub revision: Revision,
    pub entries: BTreeMap<StableId, FacetEntry>,   // an id declared more than once keeps its first occurrence in (file path, declaration order)
    /// The ids declared more than once in this revision (Section 4.5), in id order, each with the file of every
    /// occurrence in (file path, declaration order): a file that declares an id twice is listed twice.
    pub duplicates: Vec<(StableId, Vec<String>)>,
}
impl FacetMap { pub fn get(&self, id: &StableId) -> Option<&FacetEntry>; }
/// Purely syntactic: computed from `Project::parsed`, so it works for revisions that do not typecheck. It covers the
/// `.cpl` files of the source set only: std, which `Project::parsed` also holds, is not part of a revision (Section 0),
/// and a file that failed to parse contributes no declarations.
pub fn facet_map(project: &Project, revision: Revision) -> FacetMap;
/// semantics S48 `content_hash`: blake3 (64 hex) of the `format_module` output of every parsed file of a module, in path
/// order, joined with LF. Milestone 1's stand-in for a dependency revision (Section 7.2 `cpl check`).
pub fn module_content_hash(project: &Project, files: &[FileId]) -> Digest;

// ---- cpl_index::revision (Section 6.3, 9.3) ---------------------------------
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GitRepo { pub toplevel: std::path::PathBuf, pub project_prefix: String /* "" or "cpl/", `git rev-parse --show-prefix` */ }
pub fn discover_repo(project_root: &std::path::Path) -> Result<Option<GitRepo>, GitError>;
pub fn source_digest(project: &Project) -> Digest;                         // Section 6.3
/// Section 6.3; with a repository but no runnable `git` it returns `{ commit: null, state: worktree }` silently.
pub fn current_revision(project: &Project, repo: Option<&GitRepo>) -> Result<Revision, GitError>;
/// `git rev-parse --verify <rev>^{commit}`: every failing exit status of that command is UnknownRevision (git exits 1
/// for a name that is no object and 128 for well-formed names naming nothing here, such as `HEAD@{5}` past the end of
/// the reflog or `@{upstream}` without an upstream), and so is an empty name or one starting with `-`, which is refused
/// without running git. The other errors are a git that cannot run, a repository that is gone, and a signal.
pub fn resolve_rev(repo: &GitRepo, rev: &str) -> Result<String, GitError>;
/// Resolves both names first (the first unknown one is reported); two commits without a common ancestor are
/// `Command` with git's status 1 and the stderr `<a> and <b> have no common ancestor` (git itself prints nothing).
pub fn merge_base(repo: &GitRepo, a: &str, b: &str) -> Result<String, GitError>;
/// Project files only, Section 9.3 (symbolic links per T54). A commit without `cpl.toml` at the project prefix yields
/// an empty tree, which loading reports as `LoadError::NoManifest`: the CLI's `git_no_project_at_revision`, since
/// GitError has no variant for it. A manifest that does not parse yields `cpl.toml` and `cpl.lock` alone, so that
/// loading reports the manifest error as for a working tree.
pub fn tree_at(repo: &GitRepo, commit: &str) -> Result<VirtualTree, GitError>;
#[derive(thiserror::Error, Debug)]
pub enum GitError {
    #[error("git is not installed or not on PATH")]        NotInstalled,
    #[error("not a git repository: {0}")]                   NotARepository(String),
    #[error("unknown revision {0}")]                        UnknownRevision(String),
    #[error("git {args:?} failed with status {status}: {stderr}")] Command { args: Vec<String>, status: i32, stderr: String },   // args after `-C <dir>`; status -1 when killed by a signal
    #[error("{0}")]                                         Io(#[from] std::io::Error),
}

// ---- cpl_index::query (Section 6) -------------------------------------------
/// The semantic index of one checked program at one revision, rebuilt for every command (Section 6.5). `build` cannot
/// carry the check's hole reports (S48 bindings and expected types are `CheckResult` data) nor the base `changes`
/// compares with, so the builder methods supply them; an index without hole reports refuses to list the holes of a
/// body that has some (`inspect`: `internal_error`, kind `hole_reports_missing`) rather than answer that it has none.
pub struct Index<'p> { /* private: program, per-def SymbolVersion, reverse reference table, module hashes, facet map, receipt inputs */ }
impl<'p> Index<'p> {
    /// The versions are `symbol_version` of each definition's syntax, except that the later occurrence of a
    /// duplicated `@id` has the provisional identity the `Program` gives it (Section 4.5).
    pub fn build(program: &'p Program, project: &Project, revision: Revision) -> Index<'p>;
    pub fn from_check(check: &'p CheckResult, project: &Project, revision: Revision) -> Index<'p>;   // build + with_holes(&check.holes)
    pub fn with_holes(self, holes: &'p [HoleReport]) -> Index<'p>;
    pub fn with_changes_base(self, base: FacetMap) -> Index<'p>;   // the facet map `changes` compares with (history::changes_base)
    pub fn program(&self) -> &'p Program;
    pub fn revision(&self) -> &Revision;                                        // every version of the index is of this revision
    pub fn facet_map(&self) -> &FacetMap;                                       // of the revision, Section 5.4
    pub fn receipt_inputs(&self) -> &ReceiptInputs;
    pub fn holes(&self) -> &'p [HoleReport];                                    // empty without with_holes
    pub fn module_hash(&self, module: ModuleId) -> Option<&Digest>;            // semantics S48; None for a std module
    pub fn version(&self, def: DefId) -> &SymbolVersion;
    /// The warning `index_partial` (Section 7.2): the program is empty, some definition has `HasErrors`, or (with
    /// `from_check`) the check reported an error; None otherwise.
    pub fn partial(&self) -> Option<Diagnostic>;
    pub fn resolve(&self, target: &Target) -> Result<DefId, Diagnostic>;       // Section 6.1: index_unknown_symbol, index_ambiguous_symbol
    pub fn callers(&self, def: DefId) -> Vec<DefId>;                            // Section 6.5; a recursive definition is its own caller
    pub fn dependencies(&self, def: DefId) -> Vec<Dependency>;                  // Section 6.5
    pub fn interface_dependencies(&self, def: DefId) -> Vec<Dependency>;        // the default inspect set
    pub fn tests_for(&self, def: DefId) -> Vec<DefId>;                          // Section 6.5: tests and properties whose call closure reaches `def`
    pub fn call_closure(&self, roots: &[DefId]) -> BTreeSet<DefId>;             // the roots and every definition they reach by Call and CapabilityOp edges
    pub fn obligations(&self, def: DefId) -> Vec<Obligation>;                   // Section 8.1, numbered by T62; empty for a non-function or a bodiless stub
    pub fn call_sites(&self, caller: DefId, def: DefId) -> Vec<Location>;       // Section 6.4
    pub fn is_recursive(&self, def: DefId) -> bool;                             // on a cycle of the call graph (semantics 5.4)
    pub fn inspect(&self, target: &Target, include: &IncludeSet) -> Result<InspectOutput, Diagnostic>;   // `changes` without a base: git_not_a_repository
}
/// `Name` is a bare name of Section 6.1, which neither a `SymbolPath` (it starts with a module path) nor a `StableId`
/// can hold. `parse` never fails: it reads the rows of 6.1 in order (Section 6.1); `Display` gives the spelling
/// diagnostics quote.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target { Path(SymbolPath), Id(StableId), Name(DeclName) }
impl Target { pub fn parse(arg: &str) -> Target; }
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IncludeSet { pub items: BTreeSet<Include> }
/// Defined in cpl_diag::json (Section 1.4.2) and re-exported here (`pub use`); the derived order is the order of the
/// `include` key of an inspect response; `as_str`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum Include { Signature, Contract, Effects, Implementation, Docs, Callers, Dependencies, Obligations, Tests, Changes,
                   InterfaceDependencies }     // implied by `default_set()`, never written on the command line (Section 6.2)
/// `parse` reads the grammar of Section 6.2 strictly: no spaces, no empty item, `interface_dependencies` refused, a
/// repeated item counted once; `Err` carries the message of the CLI's `usage_bad_argument`.
impl IncludeSet { pub fn parse(csv: &str) -> Result<IncludeSet, String>; pub fn default_set() -> IncludeSet; pub fn contains(&self, item: Include) -> bool; }
/// One dependency: the referenced definition with the kind and interface flag of its first reference (Section 6.5).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Dependency { pub def: DefId, pub kind: RefKind, pub interface: bool }

// ---- cpl_index::diff (Section 9) --------------------------------------------
pub fn semantic_diff(old: &FacetMap, new: &FacetMap) -> DiffOutput;

// ---- cpl_index::receipt (Section 8) -----------------------------------------
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReceiptInputs {
    pub toolchain: ToolchainInfo,
    pub revision: Revision,
    pub sources: BTreeMap<String, Digest>,         // every file of the source set, by project-relative path, `cpl.toml` included
    pub std_digest: Digest,                        // Project::std_digest(): the std the project was loaded with (Section 3.6)
    pub lock_digest: Option<Digest>,               // Project::lock_digest
    pub manifest_digest: Digest,
}
impl ReceiptInputs { pub fn to_json(&self) -> ReceiptInputsJson; }   // the `inputs` object of a receipt document
pub fn receipt_inputs(project: &Project, revision: Revision) -> ReceiptInputs;
// VerificationConfig, VerifierInfo, TestRunConfig, Fixture, Obligation, Assumption, ReceiptCore: wire types of Appendix C.
pub fn verification_receipt(inputs: &ReceiptInputs, core: ReceiptCore, target: SymbolVersion, dependencies: Vec<SymbolVersion>,
                            config: VerificationConfig, status: ProofStatus, obligations: Vec<Obligation>,
                            assumptions: Vec<Assumption>, verifier: VerifierInfo) -> VerificationReceipt;
pub fn test_receipt(inputs: &ReceiptInputs, core: ReceiptCore, report: &TestReport, tested: Vec<SymbolVersion>,
                    config: TestRunConfig, fixtures: Vec<Fixture>, assumptions: Vec<Assumption>,
                    environment: BTreeMap<String, String>) -> TestReceipt;
// Both builders set `produced_at` to the current UTC time (RFC 3339, whole seconds) and `duration_ms` to 0, which the
// caller overwrites once the check is timed (both are informational, outside the digest); `dependencies` and `tested`
// are sorted by display path and deduplicated by id; `tests` holds one TestReceiptEntry per record (Section 8.2).
/// blake3 of the receipt's canonical JSON (Section 8): only the top-level `produced_at` and `duration_ms` are removed
/// (keys of the same names deeper down are evidence and stay), every object's keys are sorted by their UTF-8 bytes, no
/// whitespace, strings and numbers as `serde_json` writes them; the receipt's name.
pub fn receipt_digest(receipt: &serde_json::Value) -> Digest;

// ---- cpl_index::evidence (Sections 7.2, 8; semantics S48, S59) --------------
pub struct Proof { pub output: ProveOutput, pub diagnostic: Diagnostic }   // `cpl prove`: the result and its one verification_result
impl Proof { pub fn status(&self) -> Status; }                             // ok only for proved or conditional (T29)
pub fn test_check_configuration(config: &TestRunConfig, max_call_depth: u32) -> serde_json::Value;   // { cases, max_shrinks, max_call_depth, seed }
pub fn test_environment() -> BTreeMap<String, String>;                     // { arch, os }
impl<'p> Index<'p> {
    pub fn modules_of(&self, defs: impl IntoIterator<Item = DefId>) -> Vec<ModuleHash>;      // their project modules, in module-path order; std is never listed
    pub fn versions_of(&self, defs: impl IntoIterator<Item = DefId>) -> Vec<SymbolVersion>;  // in display-path order, one per id
    pub fn hole_dependencies(&self, report: &HoleReport) -> HoleDependenciesJson;   // semantics S48 plus `symbols` (Section 7.2)
    pub fn hole_report_json(&self, report: &HoleReport) -> HoleReportJson;
    pub fn verification_receipt_core(&self, def: DefId, release: bool) -> ReceiptCore;   // modules of the call closure, { release }
    pub fn test_receipt_core(&self, tests: &[DefId], check_configuration: serde_json::Value) -> ReceiptCore;
    /// The check's diagnostics with every `hole`'s `dependencies` and every `verification_result`'s `receipt` added
    /// (a `hole` is matched to its report by span, else by its `symbol`); nothing else changes.
    pub fn complete_check_diagnostics(&self, diagnostics: &[Diagnostic], release: bool) -> Vec<Diagnostic>;
    pub fn check_output(&self, check: &CheckResult, release: bool) -> CheckOutput;
    /// Resolve (Section 6.1), refuse a target that is no function (`usage_not_provable`), enumerate the obligations,
    /// and build the verification receipt and its `verification_result` (Section 7.2 `cpl prove`).
    pub fn prove(&self, target: &Target) -> Result<Proof, Diagnostic>;
    /// Fill the `receipt` of every record (each covering its own test's call closure) and return the run's test
    /// receipt (Section 8.2), whose `core` and `tested` cover the whole selection (`fail_fast` notwithstanding).
    pub fn complete_test_report(&self, report: &mut TestReport, selected: &[DefId], config: &TestRunConfig,
                                max_call_depth: u32) -> TestReceipt;
}

// ---- cpl_index::history (Sections 6.2, 9.3; Appendix B) ---------------------
pub const DEFAULT_SINCE: &str = "HEAD~1";                                  // the `--since` of `changes` when absent
pub fn git_error_diagnostic(error: &GitError) -> Diagnostic;               // the git_* diagnostic of a GitError (Appendix B)
/// The facet map of the commit `rev` names (Section 9.3): `git_*` codes for git failures, `git_no_project_at_revision`
/// for a commit without `cpl.toml` at the project prefix, `project_manifest_invalid`/`project_lock_invalid` (with
/// `revision` and `commit`) for one that does not load.
pub fn facet_map_at(repo: &GitRepo, rev: &str) -> Result<FacetMap, Diagnostic>;
pub fn changes_base(repo: Option<&GitRepo>, since: &str) -> Result<FacetMap, Diagnostic>;   // without a repository: git_not_a_repository

// ---- cpl_index::codes ---------------------------------------------------------
// INDEX_UNKNOWN_SYMBOL, INDEX_AMBIGUOUS_SYMBOL, INDEX_PARTIAL, GIT_NOT_A_REPOSITORY, GIT_UNKNOWN_REVISION,
// GIT_NO_PROJECT_AT_REVISION, GIT_COMMAND_FAILED, the categories INDEX and GIT, and the codes the crate reports for a
// command (USAGE_NOT_PROVABLE, VERIFICATION_RESULT, INTERNAL_ERROR).
```

### 1.9 `cpl-work` public API

Implemented in Milestone 1: over a loaded `Project` and the syntactic
facet maps of `cpl-index` (no checker), `model`, `patch` (validation and
application), `enforce`, the `scheduler` state machine (`ChangeRun`, one
method per transition of Section 10.2, and the `Coordinator` store) as
functions over the records, and the `git` wrappers; over a checked
`Program`, through `cpl-sema` (Section 1.2), `validate_work_file`,
`validate_work_file_with_policy`, and `resolve_patches`. Grant computation
(10.3), candidate assembly (10.4), and the integration loop with its
commit trailers and evidence (10.6), which drive the state machine, are
Milestone 6. Every item except the code constants is re-exported at the
crate root.

```rust
// ---- cpl_work::model (Section 10.1) -----------------------------------------
pub struct WorkModel { pub changes: Vec<ChangeSpec> }
pub struct ChangeSpec {
    pub name: String, pub span: Span,
    pub base: String, pub target: String,          // the TEXT inside git("...")
    pub workers: u32,                              // default 1
    pub tasks: Vec<TaskSpec>,                      // source order; `after` edges form a DAG
    pub integrate: Vec<RequirementSpec>,
}
pub struct TaskSpec {
    pub name: String, pub span: Span,
    pub after: Vec<String>,
    pub write: Vec<ResolvedSelector>, pub add: Vec<ResolvedNamespace>,
    pub write_files: Vec<FileSelector>,            // file selectors found in the `write` set (Section 10.3, T42)
    pub depend: Vec<ResolvedSelector>, pub preserve: Vec<ResolvedSelector>,
    pub goal: Option<String>,
}
pub struct ResolvedSelector { pub text: String, pub span: Span, pub targets: Vec<(StableId, FacetSelector)> }
pub struct ResolvedNamespace { pub text: String, pub span: Span, pub module_prefix: Vec<String> }   // covers(module): segment-wise prefix (T43)
pub fn namespace_covers(prefix: &[String], module: &[String]) -> bool;
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileSelector { Manifest, Lock, Work(String) }   // `project.manifest`, `project.lock`, `work.<stem>` (Section 10.3); parse, spelling, path, for_path; serialized as its spelling
pub enum RequirementSpec { Typecheck, NoHoles, NoUnresolvedAssumptions, Prove(ResolvedSelector), Test(ResolvedNamespaceOrSelector), Review(String) }
pub enum ResolvedNamespaceOrSelector { Namespace(ResolvedNamespace), Selector(ResolvedSelector) }

// ---- cpl_work::validate (Section 7.2 `cpl work check`) ----------------------
pub struct WorkCheckResult {
    pub model: Option<WorkModel>,                  // Some only when the file has no errors (warnings allowed)
    pub diagnostics: Vec<Diagnostic>,              // the rules of Section 7.2, in Diagnostics::sorted order over the program's source map (T46)
    /// The `WorkCheckOutput` of `cpl work check --json` (Section 7.2), produced here (T3): every change, also when the
    /// file has errors, with what resolved (a selector that names nothing has empty `targets`; `base_commit` only when
    /// `base` resolved). `file` is the work file's path in the program's source map, "" when it is not registered
    /// there, in which case the caller sets it (and sorts the diagnostics by its own source map).
    pub output: WorkCheckOutput,
}
impl WorkCheckResult { pub fn has_errors(&self) -> bool; }   // some diagnostic is an error: `cpl work check` exits 1
/// `validate_work_file_with_policy` with `WorkPolicySection::default()`, under which no file selector is grantable.
pub fn validate_work_file(file: &ast::WorkFile, program: &Program, repo: Option<&GitRepo>) -> WorkCheckResult;
/// Section 7.2's rules with the project's `[work.policy]`, whose `files` decide `work_file_not_grantable`; `repo` is
/// where `base` and `target` resolve (None, or no runnable git: skipped with `git_unavailable`). `program` is the
/// checked project and `file` its parsed work file (so that the spans are in the program's source map); a program
/// without definitions (a file failed to parse, semantics S52) resolves no selector, so `cpl work check` reports the
/// check's errors instead of validating (T58).
pub fn validate_work_file_with_policy(file: &ast::WorkFile, program: &Program, policy: &WorkPolicySection,
                                      repo: Option<&GitRepo>) -> WorkCheckResult;

// ---- cpl_work::patch (Section 10.5) -----------------------------------------
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPatch {
    pub target: (StableId, Facet), pub expect: (StableId, Facet), pub body: ast::Block, pub span: Span,
}
impl ResolvedPatch {
    pub fn body_text(&self) -> String;            // format_fragment(Block, Source): the replacement text
    pub fn body_hash(&self) -> FacetHash;         // the `body` hash the patch produces (canonical text)
    pub fn summary(&self, baseline: &FacetMap) -> Option<PatchSummary>;   // None when the baseline has no body facet for the target
}
/// `scope` is the submitting task's `.work` file (its `use` lines, `module: None`); otherwise selectors are fully
/// qualified. Each selector must name one facet of one declaration of the program: one that names none is
/// `patch_unknown_symbol`, a namespace or a whole declaration (`paginate`, `paginate.definition`) is
/// `patch_facet_unsupported` (D39), and other facets pass to `validate_patch`. `Err` holds every diagnostic, in form
/// order (target before expected), each spanning the offending selector.
pub fn resolve_patches(file: &ast::PatchFile, program: &Program, scope: &SelectorScope) -> Result<Vec<ResolvedPatch>, Vec<Diagnostic>>;
/// Section 5.5 and 10.5, in this order: target and expect equal and naming `body` (`patch_facet_unsupported`, nothing
/// else checked); the id in the baseline (`patch_unknown_symbol`) with a `body` facet (`patch_facet_unsupported`); the
/// id in `current` (`patch_target_missing`); `current != expected` is `patch_stale` (payload `expected`, `actual`); with
/// a grant, `(id, body)` outside `grant.write` is `patch_out_of_scope` whether or not the body changes, and `(id, body)`
/// in `grant.preserve` with a patch body hashing differently from the baseline's is `patch_preserved_facet`. Every
/// diagnostic spans the patch form and blames the target facet: `symbol`, `qualified_name`, and `facet` together
/// (semantics S49 allows no facet without a symbol); a target that is in neither facet map and not provisional has no
/// qualified name to give, so its id is the payload field `target` and no facet is set.
pub fn validate_patch(patch: &ResolvedPatch, baseline: &FacetMap, current: &FacetMap, grant: Option<&Grant>) -> Vec<Diagnostic>;
/// Replace the target body in the tree's text with the formatted block; everything else byte-identical (Section 10.5).
/// `project` must be loaded from `tree` (reload between patches): a file text that differs from the tree's bytes is
/// `internal_error`. A target facet other than `body`, or a kind without one, is `patch_facet_unsupported` (blaming the
/// declaration found in the project with `symbol`, `qualified_name`, and `facet`, or carrying `target` when there is
/// none); an undeclared target or a bodiless `fn` is `patch_target_missing`.
pub fn apply_patch(tree: &VirtualTree, project: &Project, patch: &ResolvedPatch) -> Result<VirtualTree, Diagnostic>;
/// A submitted `.patch` file: Err is `patch_syntax`, whose `notes` carry the parse diagnostic (Section 10.5).
pub fn parse_submitted_patch(text: &str, file: FileId) -> Result<ast::PatchFile, Diagnostic>;

// ---- cpl_work::enforce (Section 10.5) ---------------------------------------
/// `enforce(candidate, grant)` of Section 10.5 over the two facet maps (`candidate` after its patches are applied) and
/// the TreeDiff of the full trees, non-source files included.
pub fn enforce(baseline: &FacetMap, candidate: &FacetMap, tree: &TreeDiff, grant: &Grant) -> Enforcement;
pub struct Enforcement {
    pub violations: Vec<Diagnostic>,              // in the order of the algorithm's steps; accepted() iff empty
    pub consumed: Vec<String>,                    // the work/*.patch files the candidate added or changed
    pub statements: Vec<DiffStatement>,           // semantic_diff(baseline, candidate): the candidate's facet_diff
}
/// Every path of either tree whose bytes differ, in path order, with the module headers of `.cpl` files on both sides,
/// and the rebound names of T55. `sources` are the `[build] sources` of the run's frozen manifest.
pub fn tree_diff(baseline: &VirtualTree, candidate: &VirtualTree, sources: &[String]) -> TreeDiff;
pub struct TreeDiff { pub sources: Vec<String>, pub changes: Vec<FileChange>, pub rebound: Vec<ReboundName> }
pub struct FileChange { pub path: String, pub status: FileStatus, pub old_module: Option<Vec<String>>, pub new_module: Option<Vec<String>> }
pub enum FileStatus { Added, Modified, Removed }
/// T55: a name the unchanged `signature`, `contract`, `effects`, or `body` of a declaration present in both trees uses,
/// and that imports something else in its candidate file than in its baseline file (`old`/`new`: what it imports).
pub struct ReboundName { pub path: String, pub id: StableId, pub facet: Facet, pub name: String, pub old: Vec<String>, pub new: Vec<String> }
pub fn module_header(bytes: &[u8]) -> Option<Vec<String>>;   // None for a file without a readable `module` header

// ---- cpl_work::scheduler (Section 10; Milestone 6) --------------------------
// `FencingToken`, `Lease`, `TaskId`, `Timestamp`, `Actor`, and the records are defined once, in Section 10.1.
pub enum TaskState { Ready, Running, Submitted, Blocked, Accepted, Integrated, Failed, Cancelled }   // is_terminal(): Integrated, Failed, Cancelled; as_str
pub struct Grant {
    pub write: BTreeSet<(StableId, Facet)>, pub add: Vec<Vec<String>>, pub preserve: BTreeSet<(StableId, Facet)>,
    pub files: BTreeSet<FileSelector>,             // project files the task may change (Section 10.3)
    pub boundary: ExecutionBoundary,               // the host boundary the task runs under (Section 10.8)
}   // may_write(id, facet), may_add(module), may_change_file(selector)
/// The `[verification]`, `[test]`, and `[work]` sections frozen when a change is opened (Section 10.1); every check of
/// the run reads them instead of the assembled tree's manifest.
pub struct RunPolicy { pub verification: VerificationSection, pub test: TestSection, pub work: WorkSection }   // RunPolicy::from_manifest
pub trait CheckRunner {
    fn typecheck(&mut self, tree: &VirtualTree, policy: &RunPolicy, release: bool) -> CheckOutcome;
    fn no_holes(&mut self, tree: &VirtualTree, policy: &RunPolicy) -> CheckOutcome;
    fn test(&mut self, tree: &VirtualTree, policy: &RunPolicy, selector: &TestSelectorSpec, seed: u64) -> TestOutcome;
    fn prove(&mut self, tree: &VirtualTree, policy: &RunPolicy, target: &StableId) -> ProveOutcome;
}
pub struct CheckOutcome { pub ok: bool, pub diagnostics: Vec<DiagnosticJson>, pub receipt: Option<serde_json::Value> }
pub struct TestOutcome { pub report: TestReport, pub receipt: TestReceipt }
pub struct ProveOutcome { pub receipt: VerificationReceipt }
pub trait Sandbox {                                // Section 10.8; implemented in cpl-cli over the host's mechanism
    fn enforceable(&self) -> ExecutionBoundary;
    fn launch(&mut self, lease: &Lease, boundary: &ExecutionBoundary) -> Result<ExecutionBoundary, SandboxError>;
}
pub struct SandboxError { pub message: String }
// ExecutionBoundary, FsAccess, ProcessAccess, NetworkAccess: cpl_diag::json items (Section 10.8), re-exported here;
// boundary_text(&ExecutionBoundary) -> "filesystem=worktree process=toolchain network=none" (the CPL-Boundary spelling).
/// One change run, the document `.cpl/work/<change run id>.json` (T33): the state machine of Section 10.2 with one
/// method per transition (open, unblock, propagate_failures, fail_blocked, assign, renew, expire_leases,
/// report_failure, submit, validation_passed, validation_failed, invalidate, invalidate_dependents, cancel,
/// cancel_change) and the integration steps (begin_integration, stale_against_target, finish_integration,
/// fail_integration). Every operation that records a time takes `now: &Timestamp`; a refused operation changes nothing.
pub struct ChangeRun { pub change: ChangeRecord, pub tasks: Vec<TaskRecord> }
pub struct Submission { pub attempt: u32, pub token: FencingToken, pub tree: String }
pub enum SubmitOutcome { Recorded(CandidateId), Unchanged(CandidateId) }   // Unchanged: the same candidate submitted again
/// Why an operation was refused; `code()` is the Appendix B code where one exists (`work_stale_submission`,
/// `work_boundary_unenforceable`, `work_not_authorized`, `work_unknown_task`, `work_cycle`, `work_overlapping_writes`,
/// `work_bad_workers`, `work_too_many_tasks`), and `to_diagnostic()` uses it (else `internal_error`), with `task` in the payload. Variants
/// include `StaleBaseline { task, missing, unexpected }`, `OverlappingWrites { first, second, overlap }`,
/// `BadWorkers { change, workers }` (`ChangeRun::open` of a change with `workers 0`), and `InvalidTimestamp`.
pub enum ScheduleError { /* ... */ }
/// Every change run of a project, persisted in `.cpl/work/<id>.json` with atomic writes: load, restart (void every
/// expired lease, keep worktrees), void_expired_leases, insert, save_run, save; CoordinatorError.
pub struct Coordinator { /* Section 10 */ }

// ---- cpl_work::git (Section 10.7) -------------------------------------------
pub fn git(repo: &GitRepo, args: &[&str]) -> Result<std::process::Output, GitError>;   // runs `git -C <toplevel> <args>`
pub fn update_ref_cas(repo: &GitRepo, reference: &str, new: &str, old: &str) -> Result<(), GitError>;  // git update-ref <ref> <new> <old>
pub fn worktree_add(repo: &GitRepo, path: &std::path::Path, commit: &str) -> Result<(), GitError>;   // git worktree add --detach
pub fn worktree_remove(repo: &GitRepo, path: &std::path::Path) -> Result<(), GitError>;
pub fn commit_tree(repo: &GitRepo, tree: &str, parents: &[&str], message: &str) -> Result<String, GitError>;
pub fn write_tree(repo: &GitRepo, worktree: &std::path::Path) -> Result<String, GitError>;    // git add -A && git write-tree in that worktree
// GitRepo and GitError are cpl-index's (re-exported). Every invocation has the environment of Section 9.3 (LC_ALL=C,
// GIT_TERMINAL_PROMPT=0, GIT_LITERAL_PATHSPECS=1); worktree paths follow `--` and a relative path is resolved against the top level; an empty or
// option-like revision is UnknownRevision without running git; an empty `old` (or the 40-zero id) means the ref must not
// exist yet; a compare-and-swap conflict is `GitError::Command` (git's status 128), GitError having no conflict variant.

// ---- cpl_work::codes ---------------------------------------------------------
// Every `work_*` and `patch_*` code of Appendix B, plus `git_unavailable`, `git_command_failed`, `duplicate_identity`
// (reused by enforcement, 10.5 step 4), and `internal_error`.
```

### 1.10 `cpl-cli` structure

```text
crates/cpl-cli/src/
├── main.rs            fn main() -> std::process::ExitCode: runs the whole command on the evaluation thread (cpl_eval::on_eval_stack); a command line clap refuses is usage_bad_argument and a panic internal_error (kind panic), both exit 2
├── cli.rs             the clap derive definitions (Section 1.3): one struct per command of Section 7.2 (from Milestone 2 `build`, and `--engine` on `run` and `test`), `--project DIR` and `--json` global (before or after the command); cli::usage (a command's usage line) and cli::guess (the command and output form of a line clap refused)
├── output.rs          Report and Failure, Output::emit (human, --json, --jsonl) and Output::stream (--jsonl progress), the T46 sort, the human renderer (equal to Diagnostic::render, 1.4.1); writes ignore a closed pipe
├── exit.rs            enum Exit { Ok = 0, Unsuccessful = 1, Error = 2 } and the Status <-> Exit mapping (T44)
├── project.rs         project discovery (usage_no_project), Session (root, project, repository, revision; Session::refuse, T60), load_revision (a commit's tree, Section 9.3), the LoadError and GitError mappings
├── json_arg.rs        the strict reader of `cpl run --arg JSON` (Section 7.2: duplicate keys, nesting beyond serde_json's limit)
├── codes.rs           the codes the command line raises (Appendix B, producer cli) and their constructors, and re-exports of the codes of other producers it reports (from Milestone 2 those of cpl_emit_c::codes and cpl_cabi::codes, and interop_bind_failed)
└── commands/          mod.rs (dispatch) and one module per command: check.rs fmt.rs run.rs test.rs inspect.rs prove.rs diff.rs id.rs (id assign) work.rs (work check, and the reserved work run and work integrate), and from Milestone 2 build.rs
```

`runner.rs`, the `impl cpl_work::CheckRunner` over `cpl-sema` and
`cpl-eval`, arrives with Milestone 6. Every command follows the same
skeleton: locate the project (Section 7.1), load it, run the crate
function, build the report (`Envelope` parts plus the human rendering of
its result), print, exit; a command that acts on the checked program
refuses one with check errors (T58). The CLI contains no logic that is
not also reachable through the crate APIs: hole `dependencies`, receipts,
facet versions, target resolution, inspect responses, and proofs come from
`cpl_index::Index`, semantic diffs from `cpl_index::semantic_diff`, and
work validation from `cpl_work::validate_work_file_with_policy`; this is
what makes "the structured API and CLI must return the same underlying
results" true by construction (T3). The whole command runs inside
`cpl_eval::on_eval_stack`, so that the `serde_json` trees of deep values
(a fault's `input`, a record's payloads), which drop and serialize
recursively, are dropped and serialized on its large stack (Section 1.7).

From Milestone 2 the same holds for the native commands: `cpl build` is
`cpl_emit_c::package::build` over the check's `CheckResult::interop`
(Section 7.2); `cpl run` binds the `using` parameters of its target through
`cpl_cabi::adapter::manifest_hosts` (semantics S78); and `--engine native`
and `both` evaluate through `cpl_cabi::NativeSession` and compare with
`cpl_cabi::compare`. Beyond the wrapper keys `engine` and `bindings` and
the receipt's `engine` and `native` keys (Section 7.2 `--engine`), the CLI
decides only what belongs to the command line: `cpl build`'s info
`project_export_skipped_generic` and its errors `build_target_not_enabled`
and, with the release check, `build_lock_missing` (Section 7.2
`cpl build`).

### 1.11 `cpl-rtspec` public API

`cpl-rtspec` (Milestone 2) holds, once, every text and table that a
specification fixes and that both engines need (T70): the evaluator
renders and decides through it, `cpl-rt` formats it as the generated C
header `cpl_rt_gen.h` (Section 1.12), and `cpl-sema` reads it instead of a
copy. It is a leaf crate of data and pure functions, with `num-bigint` its
one external dependency.

```rust
// ---- cpl_rtspec::messages ---------------------------------------------------
/// A message with `{name}` placeholders. `render` substitutes every placeholder and panics on one without an argument
/// (a toolchain defect, caught by crates/cpl-rtspec/tests/tables.rs); the C runtime renders the same templates with its
/// own substitution routine, which crates/cpl-rt/tests/generated_tables.rs compares with `render` for every template.
pub struct Template { pub name: &'static str, pub text: &'static str }
pub const CONTRACT_VIOLATION: Template;           // "{clause_kind} clause failed: {clause}"
// ... one constant per fault and value message whose text a specification fixes, each byte for byte the text the
// evaluator wrote in Milestone 1: ASSERTION_FAILED, UNRESOLVED_HOLE, DIVISION_BY_ZERO, REFINEMENT_VIOLATION,
// RESOURCE_STACK ("the call depth limit of {limit} activations was exceeded"), RESOURCE_STACK_GUARD, RESOURCE_TIME,
// RESOURCE_MEMORY, RESOURCE_STEPS, INTERNAL_ERROR ("internal error ({kind}): {detail}") and every internal_error
// detail template, CHECK_FAILED ("{value} does not satisfy {predicate}"), PARSE_INT_FAILED, BYTES_TO_TEXT_FAILED, and
// the binding messages of semantics S77 and S80 and stdlib S45.
pub const ALL: &[Template];
pub fn render(t: &Template, args: &[(&str, &str)]) -> String;

// ---- cpl_rtspec::whitespace -------------------------------------------------
pub const RANGES: [(u32, u32); 10];               // the 25 White_Space scalars of stdlib S22 as inclusive ranges
pub fn is_white_space(c: char) -> bool;

// ---- cpl_rtspec::builtins ---------------------------------------------------
pub struct RequiresSpec { pub clause: &'static str, pub inputs: &'static [&'static str] }   // the fallback of Section 1.7.4
pub struct BuiltinSpec {
    pub id: &'static str, pub callable: bool,     // the 6 type ids and 7 generator ids are not callable (Section 3.6)
    pub c_entry: &'static str,                    // the C entry point of a callable builtin (spec/cabi.md Section 2)
    pub fallback_symbol: &'static str,            // the stub's qualified name, for a program that does not declare the stub
    pub requires: &'static [RequiresSpec],        // the precondition clauses in source order (stdlib 10.4)
}
pub const BUILTINS: [BuiltinSpec; 59];            // the registry of stdlib 10.6, sorted by id
pub const IDS: [&str; 59];                        // the same ids: cpl_sema::codes::BUILTIN_IDS (Section 1.6)
pub fn spec(id: &str) -> Option<&'static BuiltinSpec>;

// ---- cpl_rtspec::limits -----------------------------------------------------
pub const DEFAULT_MAX_CALL_DEPTH: u32 = 10_000;   // semantics S38
pub const EVAL_STACK_SIZE: usize = 1 << 30;       // the evaluation thread of both engines (Section 1.7, spec/cabi.md Section 3)
pub const GUARD_MARGIN: usize = 64 << 20;         // or an eighth of a smaller stack
pub const C_API_VERSION: (u16, u16) = (0, 1);     // (major, minor) of the C API (spec/cabi.md Section 4); BuildOutput::runtime.api
pub const DRIVER_PROTOCOL: u32 = 1;               // cpl-driver protocol 1 (spec/cabi.md Section 3)

// ---- cpl_rtspec::arith ------------------------------------------------------
/// T9's one division as amended by T70: (a / b truncated toward zero, a - (a / b) * b), None for a zero divisor.
pub fn div_rem(a: &BigInt, b: &BigInt) -> Option<(BigInt, BigInt)>;

// ---- cpl_rtspec::foreign ----------------------------------------------------
pub const KINDS: [&str; 7];                       // std.foreign.ForeignErrorKind's variants in declaration order
pub const DETAIL_ORDER: &[&str];                  // the fixed detail keys of stdlib S45, in their order

// ---- cpl_rtspec::interop ----------------------------------------------------
pub struct InteropCode { pub code: &'static str, pub status: i32 }
pub const CODES: &[InteropCode];                  // the interop_* codes of spec/cabi.md Section 4 with their C statuses, in its order
```

Decision: [D-266] T70. every text and table that a specification fixes
and that both engines need is defined once, in `cpl-rtspec`: the fault and
value message templates, the White_Space ranges of stdlib S22, the builtin
specifications (ids, callability, C entry points, fallback symbols,
precondition texts and input names), the limits and the versions of the C
API and the driver protocol, the `ForeignErrorKind` names and the detail
order of stdlib S45, the `interop_*` codes with their statuses
(`spec/cabi.md` Section 4), and the one `Int` division. The evaluator
renders and divides through it, the C runtime receives it as a generated
header, and `cpl-sema` takes the builtin ids and the division from it
instead of keeping copies. This amends T9 (Section 1.7.1): the one function
that divides is `cpl_rtspec::arith::div_rem`. Rationale: two
implementations of one semantics drift first in their texts and tables,
and generated C cannot drift from the Rust it is generated from; one
division cannot disagree between the checker's constant folding and
either engine (stdlib S47); what cannot be generated (the big integers,
UTF-8, value text, the JSON reader and writer) is ported function by
function and fuzzed against the Rust originals (D7, Section 12.6).
Rejected alternatives: copies kept equal by tests (each copy is one more
place to forget); the tables inside `cpl-eval` (`cpl-sema` cannot depend
on it, Section 1.2, and the runtime's generator would need the
interpreter's crate for data); a runtime shared with the evaluator, which
T68 rejects.

### 1.12 `cpl-rt` public API

`cpl-rt` (Milestone 2) ships the C runtime of `spec/cabi.md` Section 2,
the C11 sources under `crates/cpl-rt/c/` and the public headers `cpl.h`
and `cpl_adapter.h` under `c/include/`, as source embedded in the `cpl`
binary. `crates/cpl-rt/build.rs` reads every `*.c` and `*.h` file under
`c/` (the *runtime files*; any other file there, such as `c/README.md`, is
neither embedded nor digested) in sorted path order and writes
`$OUT_DIR/runtime_files.rs` with `RUNTIME_FILES`, one `(relative path,
include_str!(...))` pair per file; it prints `cargo:rerun-if-changed` for
the directory and every file and fails the build on a runtime file that is
not UTF-8: T52's mechanism for `std/` (Section 3.6). Cargo compiles no C:
`cpl build` writes the runtime into the package it builds (Section 7.2),
so no prebuilt runtime exists to be stale, and `cargo test` compiles it
through the same `cpl_emit_c::package` code (Section 12.8), except in the
tests of `cpl-rt` itself, which may depend on no crate but `cpl-rtspec`
and `cpl-diag` (Section 1.2) and compile the runtime's C unit tests
through `cpl_rt::testing` below.

```rust
pub static RUNTIME_FILES: &[(&str, &str)];        // ("cpl_rt.h", "..."), ("include/cpl.h", "..."), ...: the runtime files, sorted by path
pub const RUNTIME_VERSION: &str;                  // = cpl_diag::version::TOOLCHAIN: the runtime is versioned with the toolchain
/// Embedded, or the CPL_RUNTIME_DIR override: every file below that directory, for runtime development only, as
/// StdSource::Dir is for std (Section 1.6); the CLI sets Dir from the environment variable when it is present.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RuntimeSource { #[default] Embedded, Dir(std::path::PathBuf) }
/// cpl_rt_gen.h: the tables of cpl-rtspec formatted as C (the message templates, the White_Space ranges, the builtin
/// table, the limits, the interop codes, the ForeignErrorKind names); written at emit time, never checked in.
pub fn generated_header() -> String;
/// The runtime as a package's src/ receives it (Section 7.2), assembled as spec/cabi.md 2.2 states: cpl_rt.c (the
/// amalgamation behind its prologue for `prefix`), the runtime's headers, cpl_rt_gen.h, and the two mains,
/// cpl_rt_exe.c (written with the executable artifact) and cpl_rt_driver.c (drivers only), as (relative path, text)
/// pairs.
pub fn amalgamation(prefix: &str, source: &RuntimeSource) -> std::io::Result<Vec<(String, String)>>;
/// blake3 over `path LF content LF` for every runtime file in sorted path order and then `cpl_rt_gen.h LF` and the
/// generated header with its LF: the construction of std_digest (Section 3.6). Recorded in build.json
/// (`runtime.digest`), cpl.lock (`targets.c.runtime_digest`), the package's library information, and the banner of every
/// generated header (spec/cabi.md Section 5). With CPL_RUNTIME_DIR it covers the files used, and BuildOutput's
/// `runtime.source` is "dir".
pub fn runtime_digest(source: &RuntimeSource) -> std::io::Result<cpl_diag::json::Digest>;
/// The fixed flag sets of spec/cabi.md 1.5 as data, the one Rust copy of them: `cpl_emit_c::cc` builds every compile and
/// link command from them, and the runtime's own C tests compile with them. The flags that name a path or a unit
/// (`-ffile-prefix-map=<out>=.`, gcc's `-frandom-seed=<unit digest>`, `-MD -MF <file>`) are the caller's.
pub mod flags {
    pub const BASE: &[&str];
    pub const OPTIMIZED: &[&str];
    pub const DEBUG: &[&str];
    pub const SANITIZE_ADDRESS_UNDEFINED: &[&str];
    pub const SANITIZE_THREAD: &[&str];
    pub const GENERATED_WRAPPER: &[&str];          // added to the adapter set for a declarative binding's wrapper
    pub const SHARED_LINK: &[&str];                // the shared link's fixed flags (-soname and the script: the caller's)
    pub const WERROR: &str;                        // "-Werror": the toolchain tests' addition (Section 12.8)
}
/// Doc-hidden, for the tests of cpl-rt, which cannot reach `cpl_emit_c::cc` or `cpl_emit_c::package` (Section 1.2):
/// `find_cc` is the compiler of T72's precedence without `--cc` and a manifest (`CPL_CC`, else `cc` on PATH), and
/// `compile_and_run` compiles the given C files with `flags` in T72's scrubbed environment and runs the result.
/// `crates/cpl-rt/tests/c/program_stub.c`, a hand-written minimal program table (`CPLPKG(program)` and `CPLPKG(code)`
/// of the prefix `rttest`, holding the std declarations the runtime constructs), is linked into every C unit test.
pub mod testing {
    pub fn find_cc() -> Option<std::path::PathBuf>;
    pub fn compile_and_run(cc: &std::path::Path, sources: &[std::path::PathBuf], flags: &[&str])
        -> std::io::Result<std::process::Output>;
}
```

### 1.13 `cpl-emit-c` public API

`cpl-emit-c` (Milestone 2) turns a checked `Program` and its
`InteropPlan` (Section 1.6) into a C package and builds it: the lowering
and printing of `spec/cabi.md` Section 1, the tables of reported facts
computed with the evaluator's own helpers (cabi.md Section 3, Section
1.7.4), the package and adapter headers and the declarative-binding
wrappers (cabi.md Sections 5 and 6), the version script, compiler
discovery, compilation and linking, the caches, `build.json`, the drivers
of the native engine, and the lock's C entries (Section 11.2). It never
calls the checker's passes or the `Interpreter` (Section 1.2), and every
process it runs goes through `std::process::Command` with the scrubbed
environment of T72.

```rust
// ---- cpl_emit_c (emission) ----------------------------------------------------
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmitConfig {
    pub with_tests: bool,                         // a driver's units (-DCPL_WITH_TESTS=1): test bodies, range bounds, entry trampolines, adapters
    pub coverage: bool,                           // -DCPLRT_COVERAGE=1 counters, for the coverage gate (Section 12.6)
    pub release: bool,                            // the library information records release = 1
    pub runtime: cpl_rt::RuntimeSource,
}
pub struct EmittedFile { pub path: String /* relative to the output root: "include/reader.h", "src/reader_prog.c" */, pub text: String }
pub struct EmittedPackage {
    pub files: Vec<EmittedFile>,                  // the include/ and src/ files of Section 7.2's layout, in path order
    pub program_digest: Digest,                   // the program digest of spec/cabi.md 1.6, over the emitted program units (the declarative-binding wrappers included): BuildOutput::program_digest, the driver's key and banner
    pub exports_digest: Digest,                   // of the canonical export table (spec/cabi.md Section 9): BuildOutput, targets.c.exports_digest
    pub tables: TablesModel,                      // the Rust model of the emitted tables, for the table-fidelity test (Section 12)
}
pub struct TablesModel { /* per DefId, site, clause, hole, and stub: the facts of spec/cabi.md Section 3 */ }
/// Err only for a program with a definition marked HasErrors, which no command passes (T58): one internal_error,
/// kind emit_failed. Of `project` it reads the manifest (the project name of the banners, the `[interop.c]` keys, and
/// `[run] max_call_depth`, the default limit of a native executable, spec/cabi.md Section 8) and
/// the root (the adapter files and vendored sources of the plan's bindings); locations come from the program's source
/// map. A test that builds HIR by hand (`cpl_eval::testing::ProgramBuilder`) passes a synthetic `Project` (`root` None,
/// the manifest of a minimal `cpl.toml`, the program's source map, no files) with `InteropPlan::empty`; `driver` and
/// `cpl_cabi::NativeSession::start` take the same pair.
pub fn emit(p: &Program, project: &Project, plan: &InteropPlan, cfg: &EmitConfig) -> Result<EmittedPackage, Vec<Diagnostic>>;

// ---- cpl_emit_c::cc: compiler discovery (T72) ---------------------------------------
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Family { Gcc, Clang }
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Profile { Optimized, Debug }        // the flag sets of spec/cabi.md Section 1
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)] pub enum Sanitizer { Address, Undefined, Thread }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiler { pub path: PathBuf, pub family: Family, pub version: String, pub target: String, pub digest: Digest }
impl Compiler {
    /// T72's precedence: `explicit` (`--cc`), `manifest` (`[interop.c] compiler`), the environment variable CPL_CC, then
    /// `cc` on PATH; $CC is never read. A bare command name is looked up on PATH, a path is taken as written, and `path`
    /// is the resolved driver binary. The probe reads the family from the predefined macros (clang when `__clang__` is
    /// defined, else gcc when `__GNUC__` is), the version of clang from `__clang_major__`, `__clang_minor__`, and
    /// `__clang_patchlevel__` and that of gcc from -dumpfullversion, the target from -dumpmachine, and `digest` is
    /// blake3 of the binary. None found:
    /// build_cc_unavailable (`searched`); a family other than gcc or clang, gcc before 9, or clang before 12:
    /// build_cc_unsupported (`found`, `minimum`).
    pub fn discover(explicit: Option<&Path>, manifest: Option<&str>) -> Result<Compiler, Diagnostic>;
    /// Whether a one-line program compiles and links with these sanitizers' flags (Section 12.8's skip policy, and
    /// build_sanitizer_unavailable).
    pub fn supports(&self, sanitizers: &[Sanitizer]) -> bool;
}

// ---- cpl_emit_c::package: building (spec/cabi.md Section 1) ------------------------
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildConfig {
    pub out_dir: PathBuf,                         // `--out`, default <project>/.cpl/build/c
    pub cache_dir: PathBuf,                       // the per-user cache of `cpl build` (Section 7.2): <user cache>/cpl/<project key>/c
    pub profile: cc::Profile,
    pub sanitize: Vec<cc::Sanitizer>,
    pub artifacts: Vec<ArtifactKind>,             // cpl_project's (Section 11.1): `--artifact`, else `[interop.c] artifacts`
    pub release: bool,                            // the lock comparisons of Section 11.2 (the release check ran first, Section 7.2)
    pub lock: bool,                               // write the lock after a successful build (Section 11.2)
    pub cc: Option<PathBuf>,                      // `--cc`
    pub jobs: usize,                              // at least 1
    pub runtime: cpl_rt::RuntimeSource,
    pub werror: bool,                             // adds -Werror (cpl_rt::flags::WERROR) to the compiles of the runtime and the emitted code; never set by the CLI, always by the library tests (Section 12.8)
}
/// A successful build: its BuildOutput, also written as <out>/build.json, and its warnings and infos.
pub struct Built { pub output: BuildOutput, pub diagnostics: Vec<Diagnostic> }
/// Compiler discovery and the target check, emission, compilation (in parallel, with -MD depfiles, each object cached
/// and reused by the rule of spec/cabi.md 1.6), linking, and build.json; then, with `release`, the lock comparisons
/// and, with `lock`, the lock write of Section 11.2. Builds that share a cache_dir may run at once, the parallel tests of
/// one test binary among them, because every cache entry is written atomically (spec/cabi.md 1.6). Err holds the
/// failure's diagnostics (the build_* codes and internal_error kinds of Appendix B) after the warnings found before it.
pub fn build(project: &Project, p: &Program, plan: &InteropPlan, cfg: &BuildConfig) -> Result<Built, Vec<Diagnostic>>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverConfig {
    pub cache_dir: PathBuf,
    pub profile: cc::Profile,                     // Debug for `--engine native`
    pub sanitize: Vec<cc::Sanitizer>,             // D8 (Section 12.6)
    pub coverage: bool,
    pub cc: Option<PathBuf>,                      // None from the CLI: the manifest key, CPL_CC, or cc (T72)
    pub runtime: cpl_rt::RuntimeSource,
    pub werror: bool,                             // as BuildConfig::werror
}
/// A driver as built: its path, and what the receipt's `native` object records (Section 8.2).
pub struct DriverBuild { pub path: PathBuf, pub compiler: cc::Compiler, pub runtime_digest: Digest }
/// The program's driver (spec/cabi.md 3.8): the program emitted with_tests, the runtime, every backend-c adapter of the
/// plan with its vendored sources, and the driver main, built into <cache_dir>/driver/<key>/<prefix>-driver, with the
/// key and the prefix of spec/cabi.md 1.6 (the key covers the program, the runtime, the compiler, every flag, -Werror
/// included, and the bindings' `c_input_digests` of Section 11.2), or taken from there when cabi.md 1.6's reuse rule
/// allows, so that an edited header or adapter rebuilds it.
pub fn driver(project: &Project, p: &Program, plan: &InteropPlan, cfg: &DriverConfig) -> Result<DriverBuild, Vec<Diagnostic>>;

// ---- cpl_emit_c::codes ------------------------------------------------------------------
// One DiagCode per build_* code of Appendix B (BUILD_CC_UNAVAILABLE, ...), build_target_not_enabled included, and the
// internal_error kinds EMITTED_C_REJECTED ("emitted_c_rejected") and EMIT_FAILED ("emit_failed") as &str constants.
```

### 1.14 `cpl-cabi` public API

`cpl-cabi` (Milestone 2) is the toolchain's side of the C boundary: the
client of cpl-driver protocol 1 (`spec/cabi.md` Section 3), the native
engine built on it, the reconstruction of a native fault as a
`cpl_eval::Fault`, the node-list codec of values, the `CapabilityImpl`
through which the evaluator reaches a backend-`c` adapter (`CAdapter`,
cabi.md Section 7), and the comparison of two engines' outputs under
semantics S75. The toolchain never loads C into its own process: every
native evaluation runs in a driver child process, so a crash in emitted
code, the runtime, or an adapter cannot take `cpl` down, and a sanitized
driver runs where an instrumented library could not be loaded. Its tests
hold the differential harness and the C host suites (Sections 12.6-12.7).

```rust
// ---- cpl_cabi::protocol and wire (spec/cabi.md Section 3) ------------------------
pub mod protocol {
    pub enum Request { /* one variant per request of cabi.md Section 3 */ }
    pub enum Response { /* ... */ }
    pub struct FaultRecord { /* kind, clause_kind, resource, message, symbol, facet, span, backtrace, data, diagnostic */ }
    pub struct Script { /* the responses of a bind_scripted host */ }
}
pub mod wire {
    pub struct Nodes { /* the flat post-order node list of a value */ }
    pub struct WireError { pub message: String }
    pub fn to_nodes(v: &Value) -> Nodes;                                    // iterative; a shared buffer is sent once
    pub fn from_nodes(p: &Program, n: &Nodes) -> Result<Value, WireError>;   // iterative
}
/// The evaluator's Fault for a native fault record: the kind, the symbol (StableId::parse), spans and frames from the
/// Program's own ids, and the payload parsed with serde_json's recursion limit disabled, on the evaluation thread (T71).
/// From there cpl-cli and the runner render it with the evaluator's code (Fault::to_diagnostic, Section 1.7.3).
pub fn fault_from_record(p: &Program, r: &protocol::FaultRecord) -> cpl_eval::Fault;

// ---- cpl_cabi::NativeSession: the native engine -------------------------------------------
/// A handle to one driver child: clones share the child, so that the tests of one `cpl test` run share one driver.
#[derive(Clone)]
pub struct NativeSession<'p> { /* the driver child, the program, the configuration */ }
impl<'p> NativeSession<'p> {
    /// Build or reuse the driver (cpl_emit_c::package::driver), spawn it, refuse a banner whose program digest is not
    /// the one built (spec/cabi.md 3.8), and send `configure` with `cfg`'s max_call_depth, time_budget, and
    /// check_contracts and with `max_heap_bytes` 0 (no heap budget).
    pub fn start(p: &'p Program, project: &Project, plan: &InteropPlan, cfg: &EvalConfig, driver: &DriverConfig)
        -> Result<Self, NativeError>;
    pub fn build(&self) -> &DriverBuild;           // its digests are the receipt's `native` object (Section 8.2)
    /// `configure` again (spec/cabi.md 3.8): a fresh runtime in the driver, handles numbered from 0, `cfg`'s limits.
    pub fn reconfigure(&mut self, cfg: &EvalConfig) -> Result<(), NativeError>;
    /// The compiled-in backend-c adapter of `binding` (the `bind` request); a grant that fails is NativeError::Refused.
    pub fn bind(&mut self, binding: &str) -> Result<HandleId, NativeError>;
    pub fn bind_scripted(&mut self, capability: DefId, label: &str, script: protocol::Script) -> Result<HandleId, NativeError>;
    pub fn call(&mut self, f: DefId, args: &[Value], using: &[HandleId], check_arguments: bool)
        -> Result<Result<Value, Fault>, NativeError>;
    pub fn native_renderings(&self) -> Option<&Renderings>;      // the C side's renderings of the last outcome
    pub fn coverage(&mut self) -> Result<Coverage, NativeError>; // the counters of a coverage driver (Section 12.6)
    /// The NativeError that a TestEngine or RunEngine request of this session or of a clone met, if any.
    pub fn take_error(&mut self) -> Option<NativeError>;
    pub fn shutdown(self) -> Result<ShutdownReport, NativeError>;   // live handles, capabilities, objects; exit status
}
pub struct Renderings { pub json: Option<String>, pub text: Option<String>, pub diagnostic: Option<String> }   // C value JSON, value text, S49 JSON
/// The counters of a coverage driver (the `coverage` request, spec/cabi.md 3.8): executed HIR nodes by variant
/// ("ExprKind::Call", "Stmt::While", "PatternKind::Variant"), builtin calls by registry id, and claims by the kind of
/// their claim site, as the coverage gate of Section 12.6 counts them.
pub struct Coverage { pub nodes: BTreeMap<String, u64>, pub builtins: BTreeMap<String, u64>, pub claims: BTreeMap<String, u64> }
/// run_body, failing_refinement, and range_bound requests. A request that meets a NativeError keeps it for
/// `take_error` and answers with the fault `internal_error` of its kind (`native_driver_failed` or `native_protocol`);
/// `cpl test` calls `take_error` after every test and then ends the command with that error (exit 2). The factory that
/// `cpl test --engine native` passes to `run_tests_with` returns, for each test, a clone of the run's one session after
/// `reconfigure`, whose failure is kept for `take_error` in the same way.
impl<'p> cpl_eval::test::TestEngine<'p> for NativeSession<'p> { /* ... */ }
/// `bind` takes `HostSource::Binding` alone and sends the driver's `bind` request for it (`HostSource::Impl` is refused,
/// internal_error kind call_mismatch); `call_checked` is `call` with `check_arguments` true. `cpl run` passes, for each
/// `using` parameter, the plan's binding of its capability (`InteropPlan::binding_for`).
impl<'p> cpl_eval::run::RunEngine for NativeSession<'p> { /* ... */ }
/// A failure of the native engine or of a grant, never of the program: the driver could not be built (its diagnostics,
/// which the commands report as `cpl build` reports them), it died (internal_error kind native_driver_failed, with
/// `signal` or `exit_status`), it sent a malformed message (kind native_protocol), or it refused a request (its
/// `{"error": E}` answer, spec/cabi.md 3.8, as a Diagnostic: `interop_bind_failed` for a grant that failed, exit 1 under
/// semantics S78; any other refusal, `internal_error` kind `runtime_invariant` included, is a toolchain defect, exit 2).
pub enum NativeError { Build(Vec<Diagnostic>), DriverFailed(Diagnostic), Protocol(Diagnostic), Refused(Diagnostic) }
impl NativeError { pub fn into_diagnostics(self) -> Vec<Diagnostic>; }

// ---- cpl_cabi::adapter: the evaluator and C adapters (spec/cabi.md Section 7) ------------
pub mod adapter {
    /// One granted backend-c binding: its handle in the driver child that the CAdapters of one `ManifestHosts` share.
    pub struct CAdapter { /* the shared driver child and the handle */ }
    /// cap_call requests: the binding layer only, no clauses and no validation; a reported failure becomes
    /// `cpl_eval::boundary::foreign_failure`'s value or fault, and a driver that died the outcome of spec/cabi.md 7.2.
    impl cpl_eval::CapabilityImpl for CAdapter { /* ... */ }
    /// The hosts of `cpl run` (semantics S78): `available` is whether the plan has a backend-c binding of the parameter's
    /// capability (`InteropPlan::binding_for`) and reads nothing else; `create`, asked in `using`-parameter order, builds
    /// (or takes from the cache), spawns, and configures the program's driver at its first call and grants the binding
    /// in it (spec/cabi.md 7.2), giving a CAdapter bound with
    /// `BindOptions { label: "<binding>", validate_results: true }`, `<binding>` the name of its `[bindings.<binding>]`
    /// table, which is the binding name of semantics 6.9 in both engines; a grant that fails is the
    /// `interop_bind_failed` diagnostic of the driver's answer.
    pub struct ManifestHosts<'a> { /* the program, the project, the plan, the driver configuration, the shared driver child */ }
    impl cpl_eval::run::RunHosts for ManifestHosts<'_> { /* ... */ }
    pub fn manifest_hosts<'a>(p: &'a Program, project: &'a Project, plan: &'a InteropPlan, driver: &'a DriverConfig)
        -> ManifestHosts<'a>;
}

// ---- cpl_cabi::scripted: the driver's scripted host on the evaluator's side -----------------
pub mod scripted {
    /// A CapabilityImpl over a protocol::Script with the matching rule of the driver's `bind_scripted` (spec/cabi.md
    /// 3.8): a call takes the first unused response whose operation and arguments equal its own (structural equality)
    /// and answers with its result, an ill-typed one included, or reports its foreign error through
    /// `cpl_eval::boundary::foreign_failure`; a call that no response matches is the `binding_protocol` fault of
    /// semantics S80 (spec/cabi.md 3.8), its detail rendered from the cpl-rtspec template that both engines render. The
    /// suites bind it with `BindOptions { label, validate_results: true }` under the label the driver's script carries,
    /// so that both engines bind one script under one binding name (semantics 6.9).
    pub struct ScriptHost { /* the script and the capability */ }
    /// `label` is the binding name of the driver's `bind_scripted` (spec/cabi.md 3.8), which the reported failures
    /// (`adapter`) and the `binding_protocol` fault (`binding`) carry.
    impl ScriptHost { pub fn new(capability: DefId, label: &str, script: protocol::Script) -> ScriptHost; }
    impl cpl_eval::CapabilityImpl for ScriptHost { /* ... */ }
}

// ---- cpl_cabi::compare: semantics S75 between two engines ------------------------------------
pub mod compare {
    /// The two outputs as compact JSON after the S75 normalization (Section 12.6), and the offset of their first
    /// differing byte.
    pub struct Divergence { pub eval: String, pub native: String, pub offset: usize }
    pub fn run_outputs(eval: &RunOutput, native: &RunOutput) -> Option<Divergence>;   // rules 1 and 2
    /// Rules 1 and 3; `sanctioned` says whether an evaluation of the test produced a time or a guard fault on either
    /// engine, which the caller observes by wrapping the TestEngine of each engine.
    pub fn test_records(eval: &TestCaseRecord, native: &TestCaseRecord, sanctioned: bool) -> Option<Divergence>;
    pub fn normalize(v: &mut serde_json::Value);   // rule 1, for whole envelopes (D3)
}

// ---- cpl_cabi::codes ----------------------------------------------------------------------
// ENGINE_DIVERGENCE ("engine_divergence"), and the internal_error kinds NATIVE_DRIVER_FAILED ("native_driver_failed")
// and NATIVE_PROTOCOL ("native_protocol") as &str constants.
```

## 2. The AST, trivia, and the formatter

### 2.1 Conventions

The AST lives in `cpl_syntax::ast` (`crates/cpl-syntax/src/ast.rs`, whose
module documentation maps every nonterminal to its type). Node names follow
the grammar's nonterminals (grammar Appendix B). Every node is a plain
`struct` or `enum` with public fields deriving `Clone, Debug, PartialEq,
Eq, Serialize` (a few also `Copy`, `Default`, or `Hash`, as Section 2.2
marks); there are no parent pointers, interners, or arenas. Identifiers are
`String`s (`ast::Name`).

Decision: [D-145] T10. the AST stores identifier text as `String` and has no
interner. Rationale: Milestone 1 projects are small, every consumer
(`cpl-sema`, `cpl-index`, the formatter) can use `&str` directly, and an
interner would have to be threaded through every crate boundary. Rejected
alternative: a `Symbol(u32)` interner (faster comparisons, more plumbing).

Spans: every node with a `span: Span` field covers exactly the tokens its
production consumed, excluding comments and blank lines, with three
additions: `Declaration::span` also covers the declaration's doc comments
and annotations, and the span of every other node that carries `docs` (a
field, variant, operation, change, task, or patch form) covers its doc
comments; `Block::span` runs from `{` to `}` inclusive; the span of a file
node (`Module`, `WorkFile`, `PatchFile`) is the whole file. A `Trivia`
value carries no span of its own; its comments do.

Trivia: the type `Trivia` is attached to every node that can begin a line
and to the elements of comma-separated groups (Section 2.3). `Docs` is
separate from `Trivia`: doc comments are the `docs` facet and never travel
as comments.

```rust
/// An ordinary `//` comment (Section 2.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Comment {
    pub span: Span,
    pub text: String,                 // the source text from `//` to the end of the line: "// note", "//// banner"
    pub blank_line_before: bool,      // at least one blank line separated the comment from what precedes it
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Trivia {
    pub leading: Vec<Comment>,        // own-line comments above the node, in order (Section 2.3 rules C and D)
    pub trailing: Vec<Comment>,       // in source order: comments inside the node that no inner node claimed (rule G), then the comment after its last token on its last line (rule B)
    pub blank_line_before: bool,      // a blank line directly precedes the node's first token (after its leading comments, if any)
}
impl Trivia {
    pub fn is_empty(&self) -> bool;       // no comment and no blank line
    pub fn has_comments(&self) -> bool;   // at least one comment (blank lines do not count)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocLine { pub span: Span, pub text: String }        // one `///` line: the text after `///` with one leading space removed
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Docs { pub span: Span, pub lines: Vec<DocLine> }    // span: the first through the last `///` line
impl Docs { pub fn text(&self) -> String; }                    // the lines joined with "\n" (grammar 1.2), trailing whitespace kept; the `docs` facet is Fragment::Docs, which trims it (Section 5.2)
```

### 2.2 Node definitions

The listing is `crates/cpl-syntax/src/ast.rs` without its bodies and
attributes. Every type derives `Clone, Debug, PartialEq, Eq, Serialize`;
the types marked `Copy` or `Hash` also derive those. `Comment`, `Trivia`,
`DocLine`, and `Docs` are in Section 2.1.

```rust
use cpl_diag::Span;
use serde::Serialize;

// ---- names and literals -----------------------------------------------------
pub struct Name { pub text: String, pub span: Span }                    // LOWER_IDENT or UPPER_IDENT; the class is checked by the grammar position
impl Name { pub fn new(text: impl Into<String>, span: Span) -> Name; pub fn is_upper(&self) -> bool; }   // is_upper: the UPPER_IDENT class
pub struct TextLiteral { pub span: Span, pub value: String }            // a position that admits only TEXT (test names, goals, git refs); decoded
pub struct IntLiteral { pub span: Span, pub digits: String }            // a position that admits only INT (`workers 3`); digits without separators

// ---- files ------------------------------------------------------------------
pub struct Module {                                                     // grammar: source_file
    pub span: Span,                                                     // the whole file
    pub trivia: Trivia,                                                 // comments above the header; the header line's trailing comment
    pub docs: Option<Docs>,                                             // module doc comments
    pub header: ModuleHeader,
    pub uses: Vec<UseDecl>,
    pub declarations: Vec<Declaration>,                                 // source order
    pub trailing_comments: Vec<Comment>,                                // comments after the last declaration (Section 2.3 rule F)
}
pub type SourceFile = Module;                                           // grammar Appendix B's name
pub struct ModuleHeader { pub span: Span, pub path: ModulePath }
pub struct ModulePath { pub span: Span, pub segments: Vec<Name> }
impl ModulePath { pub fn dotted(&self) -> String; }                     // "a.b.c"
pub struct UseDecl {
    pub span: Span, pub trivia: Trivia,
    pub path: UsePath,                                                  // use_path
    pub braced: bool,                                                   // the list form `use a.b.{c}` (true) or the single form `use a.b.c` (false; one item)
    pub items: Vec<UseItem>,                                            // >= 1
    pub group: Group,                                                   // layout of the `{ }` list; unused when !braced
    pub trailing_comments: Vec<Comment>,                                // comments after the last item, before `}`
}
pub struct UsePath { pub span: Span, pub segments: Vec<Name> }
impl UsePath { pub fn dotted(&self) -> String; }
pub struct UseItem { pub span: Span, pub trivia: Trivia, pub name: Name }

// ---- declarations -----------------------------------------------------------
pub struct Declaration {                                                // grammar: declaration (decl_prefix = docs + annotations)
    pub span: Span,                                                     // includes the doc comments and annotations
    pub trivia: Trivia,                                                 // comments above the declaration, also those among its prefix lines
    pub docs: Option<Docs>,                                             // every DOC_COMMENT of the prefix, in source order (grammar 1.2)
    pub annotations: Vec<Annotation>,                                   // source order
    pub kind: DeclKind,
}
pub enum DeclKind { Type(TypeDecl), Struct(StructDecl), Enum(EnumDecl), Capability(CapabilityDecl), Fn(FnDecl), Test(TestDecl), Property(PropertyDecl) }
impl Declaration {
    pub fn name(&self) -> Option<&Name>;                                // None for tests and properties, whose names are TEXT
    pub fn display_name(&self) -> &str;                                 // the identifier, or the test or property name text
    pub fn is_pub(&self) -> bool;                                       // always false for tests and properties (D67)
    pub fn annotation(&self, name: &str) -> Option<&Annotation>;
    pub fn has_annotation(&self, name: &str) -> bool;                   // `has_annotation("prove")`: the verification_policy facet
    pub fn id(&self) -> Option<&str>;                                   // the TEXT argument of `@id("...")`: the identity facet
}
pub struct Annotation {
    pub span: Span,
    pub trivia: Trivia,                                                 // trailing only: a comment above an annotation is the declaration's
    pub name: Name,                                                     // the spelling after `@`, a keyword included (`property`, D71)
    pub args: Vec<Literal>,                                             // empty for `@name` and for `@name()` alike
    pub group: Group,
    pub parens: bool,                                                   // written with an argument list, possibly empty (`@name()`, `@id("x")`); false for a bare `@name`; the formatter keeps `@name()` (T14), the dump prints `(annotation name ())`
}

pub struct TypeDecl { pub span: Span, pub is_pub: bool, pub name: Name, pub kind: TypeDeclKind }
pub enum TypeDeclKind {
    Wrapper(WrapperDecl),                                               // pub type PageId(Text)
    Refinement(RefinementDecl),                                         // pub type Positive = Int where self > 0 (D64: `where` is mandatory)
    Opaque(OpaqueDecl),                                                 // pub type List<T> with no definition: a std `@builtin` type (stdlib G2, Section 3.6)
}
pub struct WrapperDecl { pub span: Span, pub payload: Type }            // span: the parentheses
pub struct RefinementDecl { pub span: Span, pub base: Type, pub predicate: Expr }   // span: `=` to the end of the predicate; `self` is the underlying value
pub struct OpaqueDecl { pub span: Span, pub generic_params: Option<GenericParams> }  // span: the generic parameters, or an empty span after the name
pub struct StructDecl { pub span: Span, pub is_pub: bool, pub name: Name, pub generic_params: Option<GenericParams>, pub fields: Vec<FieldDecl>, pub trailing_comments: Vec<Comment> }
pub struct FieldDecl { pub span: Span, pub trivia: Trivia, pub docs: Option<Docs>, pub name: Name, pub ty: Type }
pub struct EnumDecl { pub span: Span, pub is_pub: bool, pub name: Name, pub generic_params: Option<GenericParams>, pub variants: Vec<VariantDecl>, pub trailing_comments: Vec<Comment> }
pub struct VariantDecl {
    pub span: Span, pub trivia: Trivia, pub docs: Option<Docs>, pub name: Name,
    pub payload: Vec<Type>,                                             // positional payload types; empty when the variant has no parentheses
    pub group: Group,
}
pub struct CapabilityDecl { pub span: Span, pub is_pub: bool, pub name: Name, pub operations: Vec<OpDecl>, pub trailing_comments: Vec<Comment> }
pub struct OpDecl {
    pub span: Span, pub trivia: Trivia, pub docs: Option<Docs>, pub name: Name,
    pub params: Params,                                                 // `params.using` is always empty: operations take no `using` parameters (grammar 3.8)
    pub ret: Type,
    pub clauses: Vec<ContractClause>,                                   // source order
}
pub struct FnDecl {
    pub span: Span, pub is_pub: bool, pub name: Name,
    pub generic_params: Option<GenericParams>,
    pub params: Params,                                                 // positional first, then `using` (grammar 3.9)
    pub ret: Type,
    pub clauses: Vec<FnClause>,                                         // `effects`, `requires`, `ensures`, `decreases` in source order (D66)
    pub body: Option<Block>,                                            // None only for the bodiless stub form (grammar 3.9, stdlib G1, Section 3.6)
}
impl FnDecl {
    pub fn is_bodiless(&self) -> bool;
    pub fn effects(&self) -> Option<&EffectsClause>;
    pub fn decreases(&self) -> Option<&DecreasesClause>;
    pub fn contract(&self) -> impl Iterator<Item = &ContractClause>;   // `requires` and `ensures` clauses in source order
    pub fn requires(&self) -> impl Iterator<Item = &ContractClause>;
    pub fn ensures(&self) -> impl Iterator<Item = &ContractClause>;
}
pub struct Params { pub span: Span, pub positional: Vec<Param>, pub using: Vec<UsingParam>, pub group: Group, pub trailing_comments: Vec<Comment> }
pub struct Param { pub span: Span, pub trivia: Trivia, pub name: Name, pub ty: Type }
pub struct UsingParam { pub span: Span, pub trivia: Trivia, pub name: Name, pub ty: Type }   // `using db: PageRead`
pub enum FnClause { Effects(EffectsClause), Contract(ContractClause), Decreases(DecreasesClause) }
impl FnClause { pub fn span(&self) -> Span; pub fn trivia(&self) -> &Trivia; }
pub struct EffectsClause { pub span: Span, pub trivia: Trivia, pub effects: Vec<EffectRef>, pub group: Group, pub trailing_comments: Vec<Comment> }
pub struct EffectRef { pub span: Span, pub trivia: Trivia, pub param: Name, pub operation: Name }   // param.operation
pub struct ContractClause { pub span: Span, pub trivia: Trivia, pub kind: ContractKind, pub expr: Expr }
pub enum ContractKind { Requires, Ensures }                             // Copy
impl ContractKind { pub fn keyword(self) -> &'static str; }
pub struct DecreasesClause { pub span: Span, pub trivia: Trivia, pub expr: Expr }
pub struct TestDecl { pub span: Span, pub name: TextLiteral, pub body: Block }
pub struct PropertyDecl { pub span: Span, pub name: TextLiteral, pub for_clauses: Vec<ForClause>, pub body: Block }   // one or more for_clauses
pub struct ForClause { pub span: Span, pub trivia: Trivia, pub binder: Name, pub range: Expr }

// ---- types ------------------------------------------------------------------
pub struct Type { pub span: Span, pub path: TypePath, pub generic_args: Option<GenericArgs> }
pub struct TypePath { pub span: Span, pub modules: Vec<Name>, pub name: Name }   // (LOWER_IDENT ".")* UPPER_IDENT; modules may be empty
impl TypePath { pub fn dotted(&self) -> String; }
pub struct GenericArgs { pub span: Span, pub args: Vec<Type> }          // `<T, U>` in a type or a generic call
pub struct GenericParams { pub span: Span, pub params: Vec<Name> }      // `<T, U>` after a declared name

// ---- statements -------------------------------------------------------------
pub struct Block {
    pub span: Span,                                                     // `{` to `}` inclusive
    pub statements: Vec<Statement>,
    pub tail: Option<ExprStmt>,                                         // a final expression statement: the block's value (grammar 3.11)
    pub trailing_comments: Vec<Comment>,                                // comments after the last statement, before `}`
}
impl Block { pub fn is_empty(&self) -> bool; }                          // no statements and no tail
pub enum Statement {
    Let(LetStmt), Var(VarStmt), Assign(AssignStmt), Return(ReturnStmt), Assert(AssertStmt), While(WhileStmt),
    Expr(ExprStmt),                                                     // a non-tail expression statement (must have type Unit)
}
impl Statement { pub fn span(&self) -> Span; pub fn trivia(&self) -> &Trivia; pub fn trivia_mut(&mut self) -> &mut Trivia; }
pub enum LetBinding { Name(Name), Wildcard(Span) }                      // `let x` or the discard `let _` (D24)
pub struct LetStmt { pub span: Span, pub trivia: Trivia, pub binding: LetBinding, pub ty: Option<Type>, pub value: Expr }
pub struct VarStmt { pub span: Span, pub trivia: Trivia, pub name: Name, pub ty: Option<Type>, pub value: Expr }
pub struct AssignStmt { pub span: Span, pub trivia: Trivia, pub target: Name, pub value: Expr }
pub struct ReturnStmt { pub span: Span, pub trivia: Trivia, pub value: Option<Expr> }
pub struct AssertStmt { pub span: Span, pub trivia: Trivia, pub condition: Expr }
pub struct WhileStmt {
    pub span: Span, pub trivia: Trivia,
    pub condition: Expr,
    pub clauses: Vec<WhileClause>,                                      // `invariant` and `decreases` clauses in source order (D66)
    pub body: Block,
}
impl WhileStmt { pub fn invariants(&self) -> impl Iterator<Item = &InvariantClause>; pub fn decreases(&self) -> Option<&DecreasesClause>; }
pub enum WhileClause { Invariant(InvariantClause), Decreases(DecreasesClause) }
impl WhileClause { pub fn span(&self) -> Span; pub fn trivia(&self) -> &Trivia; }
pub struct InvariantClause { pub span: Span, pub trivia: Trivia, pub expr: Expr }
pub struct ExprStmt { pub span: Span, pub trivia: Trivia, pub expr: Expr }   // Statement::Expr, or a Block's tail

// ---- expressions ------------------------------------------------------------
pub struct Expr { pub span: Span, pub kind: ExprKind }
pub enum ExprKind {
    Literal(Literal),
    Name(Name),                                                         // a bare LOWER_IDENT or UPPER_IDENT (`name`)
    Hole(Name),                                                         // `?name`; the Name has no `?`
    Forall(Box<ForallExpr>),
    Binary(Box<BinaryExpr>),
    Unary(Box<UnaryExpr>),
    Member(Box<MemberExpr>),                                            // a.b
    Call(Box<CallExpr>),
    Propagate(Box<Expr>),                                               // e?
    Paren(Box<Expr>),                                                   // preserved for the formatter (Section 2.4 rule 8)
    List(Box<ListLiteral>),
    Struct(Box<StructLiteral>),
    If(Box<IfExpr>),
    Match(Box<MatchExpr>),
}
pub struct Literal { pub span: Span, pub kind: LiteralKind }
pub enum LiteralKind { Int(String), Text(String), Bool(bool) }          // Int: decimal digits without separators, no sign; Text: decoded
pub struct ForallExpr { pub span: Span, pub binder: Name, pub collection: Expr, pub predicate: Expr }
pub struct BinaryExpr {
    pub span: Span, pub op: BinaryOp, pub op_span: Span,
    pub break_before_op: bool,                                          // a source line break directly before the operator (Section 2.4 rule 9)
    pub lhs: Expr, pub rhs: Expr,
}
pub enum BinaryOp { Or, And, Eq, Ne, Lt, Le, Gt, Ge, Add, Sub, Concat, Mul, Div, Rem }   // Copy, Hash
impl BinaryOp { pub fn symbol(self) -> &'static str; pub fn is_comparison(self) -> bool; }
pub struct UnaryExpr { pub span: Span, pub op: UnaryOp, pub op_span: Span, pub operand: Expr }
pub enum UnaryOp { Not, Neg }                                           // Copy, Hash
impl UnaryOp { pub fn symbol(self) -> &'static str; }
pub struct MemberExpr { pub span: Span, pub target: Expr, pub member: Name }
pub struct CallExpr { pub span: Span, pub callee: Expr, pub generic_args: Option<GenericArgs>, pub args: Args }
pub struct Args { pub span: Span, pub positional: Vec<Arg>, pub using: Vec<UsingArg>, pub group: Group, pub trailing_comments: Vec<Comment> }
pub struct Arg { pub span: Span, pub trivia: Trivia, pub expr: Expr }
pub struct UsingArg { pub span: Span, pub trivia: Trivia, pub expr: Expr }   // `using expr`
pub struct ListLiteral { pub span: Span, pub elements: Vec<ListElem>, pub group: Group, pub trailing_comments: Vec<Comment> }
pub struct ListElem { pub span: Span, pub trivia: Trivia, pub expr: Expr }
pub struct StructLiteral { pub span: Span, pub path: TypePath, pub entries: StructEntries }
pub struct StructEntries {
    pub span: Span,                                                     // the braces
    pub spread: Option<Expr>,                                           // the `..base` expression, always first when present (D29)
    pub spread_trivia: Trivia,                                          // the spread entry's comments (empty without a spread)
    pub fields: Vec<FieldInit>,
    pub group: Group,
    pub trailing_comments: Vec<Comment>,
}
pub struct FieldInit { pub span: Span, pub trivia: Trivia, pub name: Name, pub value: Expr }
pub struct IfExpr { pub span: Span, pub condition: Expr, pub then_block: Block, pub else_branch: Option<ElseBranch> }
pub enum ElseBranch { Block(Block), If(Box<IfExpr>) }                  // `else { ... }` or `else if ...`
pub struct MatchExpr { pub span: Span, pub scrutinee: Expr, pub arms: Vec<MatchArm>, pub trailing_comments: Vec<Comment> }
pub struct MatchArm { pub span: Span, pub trivia: Trivia, pub pattern: Pattern, pub body: ArmBody }
pub enum ArmBody { Block(Block), Expr(Expr) }

// ---- patterns ---------------------------------------------------------------
pub struct Pattern { pub span: Span, pub kind: PatternKind }
pub enum PatternKind {
    Wildcard,                                                           // `_`
    Int { negative: bool, digits: String },                             // `"-"? INT`
    Text(String),
    Bool(bool),
    Binding(Name),                                                      // a bare LOWER_IDENT
    Ctor(CtorPattern),
}
pub struct CtorPattern { pub path: CtorPath, pub args: Option<Vec<Pattern>>, pub group: Group }   // args None: a constructor written without parentheses
pub struct CtorPath { pub span: Span, pub qualifier: Vec<Name>, pub name: Name }   // (LOWER ".")* UPPER ("." UPPER)*; qualifier: everything before the name
impl CtorPath { pub fn dotted(&self) -> String; }

// ---- .work ------------------------------------------------------------------
pub struct WorkFile { pub span: Span, pub uses: Vec<UseDecl>, pub changes: Vec<ChangeDecl>, pub trailing_comments: Vec<Comment> }
pub struct ChangeDecl { pub span: Span, pub trivia: Trivia, pub docs: Option<Docs>, pub name: Name, pub items: Vec<ChangeItem>, pub trailing_comments: Vec<Comment> }
impl ChangeDecl {
    pub fn tasks(&self) -> impl Iterator<Item = &TaskDecl>;
    pub fn base(&self) -> Option<&GitRef>;
    pub fn target(&self) -> Option<&GitRef>;
    pub fn workers(&self) -> Option<&IntLiteral>;
    pub fn integrate(&self) -> Option<&IntegrateDecl>;
}
pub struct ChangeItem { pub span: Span, pub trivia: Trivia, pub kind: ChangeItemKind }
pub enum ChangeItemKind { Base(GitRef), Target(GitRef), Workers(IntLiteral), Task(TaskDecl), Integrate(IntegrateDecl) }
pub struct GitRef { pub span: Span, pub reference: TextLiteral }        // git("ref")
pub struct TaskDecl { pub span: Span, pub docs: Option<Docs>, pub name: Name, pub after: Option<AfterClause>, pub items: Vec<TaskItem>, pub trailing_comments: Vec<Comment> }
pub struct AfterClause { pub span: Span, pub tasks: Vec<Name> }         // `after a, b`
pub struct TaskItem { pub span: Span, pub trivia: Trivia, pub kind: TaskItemKind }
pub enum TaskItemKind { Write(SelectorSet), Add(SelectorSet), Depend(SelectorSet), Preserve(SelectorSet), Goal(TextLiteral) }
impl TaskItemKind { pub fn keyword(&self) -> &'static str; }
pub struct SelectorSet { pub span: Span, pub selectors: Vec<Selector>, pub group: Group, pub trailing_comments: Vec<Comment> }   // possibly empty: `write {}`
pub struct IntegrateDecl { pub span: Span, pub requirements: Vec<RequireItem>, pub trailing_comments: Vec<Comment> }
pub struct RequireItem { pub span: Span, pub trivia: Trivia, pub requirement: Requirement }
pub struct Requirement { pub span: Span, pub kind: RequirementKind }
pub enum RequirementKind { Typecheck, NoHoles, NoUnresolvedAssumptions, Prove(Selector), Test(Selector), Review(Name) }   // grammar D36's six forms; Review: a task name
pub struct Selector {
    pub span: Span, pub trivia: Trivia,
    pub path: SelectorPath,                                             // the segments before a final `*` or facet; the whole path of a declaration selector
    pub kind: SelectorKind,
}
pub struct SelectorPath { pub span: Span, pub segments: Vec<Name> }
impl SelectorPath { pub fn dotted(&self) -> String; }
pub enum SelectorKind { Namespace, Facet(Facet), Declaration }          // `path.*`; `path.body`, `path.definition`, ...; bare `path` (D38)
pub struct Facet { pub span: Span, pub kind: FacetKind }
pub enum FacetKind { Identity, Name, Signature, Contract, Effects, Body, Docs, VerificationPolicy, Definition }   // Copy, Hash
impl FacetKind { pub fn parse(spelling: &str) -> Option<FacetKind>; pub fn as_str(self) -> &'static str; }

// ---- .patch -----------------------------------------------------------------
pub struct PatchFile { pub span: Span, pub forms: Vec<PatchForm>, pub trailing_comments: Vec<Comment> }
pub struct PatchForm { pub span: Span, pub trivia: Trivia, pub docs: Option<Docs>, pub target: Selector, pub expect: ExpectClause, pub body: Block }
pub struct ExpectClause { pub span: Span, pub trivia: Trivia, pub base: Selector }   // `expect base(selector)`

// ---- layout memory ----------------------------------------------------------
/// Layout of a bracketed, comma-separated group as written (Section 2.4 rule 6). Copy, Default.
pub struct Group {
    pub multiline: bool,              // a line terminator occurred between the brackets at the group's own level
}
```

`Group` records only whether the author broke the group, and only at the
group's own bracket level: a line break inside a nested group (a bracket
pair that is itself a group of 2.4 rule 6) or a block does not count,
while one inside a generic list `<...>`, which is no group and opens no
newline mode of its own, counts for the group around it (`[g<` LF
`Int>(x)]` and `fn h(x: List<` LF `Int>)` are broken groups). Comments inside a group attach to its elements, which
carry `Trivia` (`Param`, `UsingParam`, `Arg`, `UsingArg`, `ListElem`,
`FieldInit` and the spread entry's `StructEntries::spread_trivia`,
`EffectRef`, `UseItem`, `Selector`); the comments after the last element
are the `trailing_comments` of the node that owns the group (`Params`,
`Args`, `ListLiteral`, `StructEntries`, `EffectsClause`, `UseDecl`,
`SelectorSet`). The groups whose elements carry no trivia (annotation
arguments, variant payloads, constructor-pattern payloads) have no
`trailing_comments`: a comment inside one is trailing trivia of the
innermost attachable node that contains it (Section 2.3 rule G).

### 2.3 Comment attachment

This section is the normative model of *trivia*: how ordinary `//`
comments and blank lines, which the grammar discards, survive parsing so
that the formatter reproduces them (2.4 rules 4 and 11) and `strip_layout`
(Section 12) can remove them. The lexer drops comments and blank lines from
the token stream but returns them out of band, in source order
(`Lexed::trivia`: one `TriviaToken` of kind `Comment` per comment and of
kind `BlankLine` per blank line, Section 1.5). The parser attaches every
comment to exactly one node, by position, with these rules applied in
order:

| Rule | Condition | Attachment |
| - | - | - |
| A | the comment is a `///` line | not a comment: it is a `DOC_COMMENT` token and becomes `Docs` of the declaration or member it precedes (grammar 1.2); the `docs` facet is stored separately and never travels as trivia |
| B | the comment follows a node's last token on the same line, with only horizontal whitespace, or one separating comma, between them | `trailing` of that node: the innermost *attachable node* whose last token is the last token on that line (`let x = 1 // c`: the statement; `module m // c`: the module, `Module::trivia`). The separating comma does not prevent the attachment, so in `f(a, // c` the comment is trailing trivia of `a` and the group's layout is unchanged |
| C | otherwise, the comment lies between a header's last token or clause and a `{` that begins a new line | `leading` of the first node inside those braces, or one of the body's `trailing_comments` when it has no node (rules D and E applied inside the braces); the formatter therefore prints it below the `{` |
| D | otherwise, an attachable node starts after the comment within the same enclosing sequence | `leading` of the first such node: a declaration, `use` declaration, statement, match arm, field, variant, operation, parameter, clause, effect reference, argument, list element, field initializer or spread entry, `use` item, selector, or `.work`/`.patch` item (several consecutive comments form one `leading` list, in order; a comment after `{` on a header's line is leading trivia of the first statement; a comment above the module header is the module's, and one among the lines of a declaration's prefix is the declaration's, as one among the doc lines of a field, variant, operation, change, task, or patch form is that node's, printed above its docs, 2.4 rule 5). "The same enclosing sequence" is the comment's own sequence, a generic list `<...>` counting as one: a node inside a bracket pair that opens after the comment does not claim it, and no node after a pair closes claims a comment written inside it; such a comment falls to rule G (`(a // c` LF `) + g(b)`: the statement's; `text: // c` LF `f(x), id: y`: the field initializer `text: f(x)`'s). A block's braces are the exception, as rule C requires: a comment still pending at a block's `{` is the leading trivia of the block's first statement (`g(if c { 1 } // c` LF `else { 2 })` gives it to `2`) |
| E | otherwise, the comment is inside a brace or bracket sequence with no following node (after the last statement of a block, the last member of a body, the last arm, the last element of a group) | `trailing_comments` of the node that owns that sequence (its *dangling* comments) |
| F | otherwise (after the last item of a file) | `Module::trailing_comments` / `WorkFile::trailing_comments` / `PatchFile::trailing_comments` |
| G | a comment inside a node's span that no inner node claims by B-F (inside a parenthesized expression, a generic list, a group whose elements carry no trivia, or a `match` scrutinee that spans lines; one of an enclosing sequence pending when a nested bracket pair opens, rule D) | `trailing` of the innermost attachable node that contains it (`if (c // i` LF `) {`: the `if` statement's; `g<Int // e` LF `>(x)`: the list element's); the formatter prints it after that node's last token |

Attachable nodes are exactly those with a `Trivia` field: `Module` (the
comments above its header and the header line's trailing comment),
`UseDecl`, `UseItem`, `Declaration`, `Annotation` (trailing comments only),
`FieldDecl`, `VariantDecl`, `OpDecl`, `Param`, `UsingParam`,
`EffectsClause`, `EffectRef`, `ContractClause`, `DecreasesClause`,
`InvariantClause`, `ForClause`, the seven statement nodes (`LetStmt`,
`VarStmt`, `AssignStmt`, `ReturnStmt`, `AssertStmt`, `WhileStmt`,
`ExprStmt`), `Arg`, `UsingArg`, `ListElem`, `FieldInit`, the spread entry
(`StructEntries::spread_trivia`), `MatchArm`, `Selector`, `ChangeDecl`,
`ChangeItem` (which carries the trivia of a task or `integrate` item),
`TaskItem`, `RequireItem`, `PatchForm`, and `ExpectClause`. A sequence is
the content of one pair of brackets or braces, or a file's top level.

Blank lines are recorded, not attached: `Trivia::blank_line_before` is true
when a line consisting only of horizontal whitespace directly precedes the
node's first token (after its leading comments, if any), and
`Comment::blank_line_before` is true when such a line directly precedes a
comment; several blank lines count as one. The first node of any brace
body or file never records a preceding blank line, and the parser enforces
it: `Trivia::blank_line_before` is false on the first node of every
`lines(X)` body, on the module of a `.cpl` file (whose header comes first),
and on the first `use` or `change` of a `.work` file and the first patch
form of a `.patch` file; the elements of a comma-separated group never
record one either. `Comment::blank_line_before` is recorded as written,
also on a leading comment of such a first node, but the formatter prints no
blank line after `{` or at the start of a file (2.4 rule 4). Everywhere
else the formatter preserves both flags (2.4 rules 4 and 11). A
consequence of the first-node rule is that a blank line between the
leading comments of a first node and the node itself is not reproduced
(`{` LF `// section` LF LF `let x = 1` loses its blank line): T11's
preservation of that blank line applies to every node but the first of a
body or file. This also keeps the output idempotent when 2.4 rule 12 moves
a `.work` item to the front of its body.

Decision: [D-146] T11. comments attach to the following attachable node as leading
trivia; a comment on the same line as code attaches to the preceding node
as trailing trivia (rules B and G); comments after the last element of a
body or file are dangling trivia of that body or file; every node records
whether a blank line directly precedes it, and every leading comment
whether one precedes it; doc comments are not comments. Rationale: this is
the only assignment that is unambiguous from positions alone, and it
matches how the corpus uses `// glue:` comments (above the thing they
describe); recording blank lines on both the comment and the node
preserves an author's grouping (`// section`, a blank line, a declaration)
that dropping either would collapse. Rejected alternatives: attaching by
indentation or by proximity (ambiguous when a comment sits between two
blank lines); dropping the blank line between a leading comment and its
node (an earlier wording of this decision).

### 2.4 Canonical layout

`format_module` is a pure function of the AST: two ASTs that are equal
produce identical text, and `parse(format(ast))` is equal to `ast` modulo
`strip_layout` (Section 12) for every AST the parser can produce (round
trip): spans, the layout memory, blank-line flags, which node owns each
comment (rule 11, T63), the clause and `.work` item orders of T13, and the
trailing spaces and tabs of doc lines change, nothing else.
Idempotency, `format(parse(format(x))) == format(x)`, holds exactly
(Section 12). The rules:

| # | Rule |
| - | - |
| 1 | Indentation is four spaces per level, and tokens are separated by spaces: the formatter emits no tab of its own (a tab inside the text of a comment or a doc line is part of that text and is kept, rule 11). Lines have no trailing whitespace, which here, in rule 11, and in the `docs` facet (Section 5.2) means spaces and tabs, the horizontal whitespace of grammar 1.1: other Unicode white space at the end of a comment or a doc line (U+00A0, U+3000, U+2028) is text and stays. Line terminator is LF. The file ends with exactly one LF. A byte-order mark is removed. |
| 2 | One statement, member, clause, item, arm, or `.work` item per line. |
| 3 | File order: module doc comments, `module` header, one blank line, `use` declarations in source order, one blank line, items in source order separated by exactly one blank line, except that a `type` declaration written on the line directly below another `type` declaration stays there when neither has doc comments or annotations and neither a blank line nor a comment separates them (T51). Doc comments and annotations are part of the item. If there are no `use` declarations the header is followed by one blank line and the first item. |
| 4 | Within a brace body (block, struct/enum/capability body, match body, change/task/integrate body) and among `use` declarations: consecutive nodes are separated by one blank line if `blank_line_before` is true, otherwise none. No blank line after `{`, before `}`, or at the start of a file. Clauses (`effects`, `requires`, `ensures`, `decreases`, `invariant`, `for`, and a patch form's `expect`) ignore `blank_line_before`: none between them, also before a comment written between two clauses (whose own `blank_line_before`, rule 11, is ignored there). |
| 5 | Declaration layout: doc comments first (all `///` lines of the prefix, even those written after an annotation, grammar 1.2) (`/// ` + text, or `///` for an empty line), then annotations one per line in source order (`@id("...")`, `@prove`), then the declaration (for a standard-library stub, its header and clauses with no `{`, rule 7). Ordinary comments written among the prefix lines print above the doc comments, after the comments written above the declaration's first token and the blank line the source had before that token (T11), with the further trailing comments of the annotations (rule 11) among them in source order (all are leading trivia of the declaration, Section 2.3 rule D). Annotation arguments are a group of rule 6 like any other (`@id(` LF `"x"` LF `)` stays broken, T12); their elements carry no trivia, so a comment inside them is the annotation's trailing comment and prints after its `)` (Section 2.3 rule G). |
| 6 | Groups (parameter lists, argument lists, list literals, struct literals, effects sets, use lists, annotation arguments, selector sets, variant payloads, constructor-pattern payloads): printed *flat* (`f(a, b)`, `[1, 2]`, `Page { ..p, text: t }`, `effects { db.get }`, `write { paginate.body }`) unless (i) `Group::multiline` is true, or (ii) any element or the group's `trailing_comments` carries a comment, or (iii) the flat rendering of the line would exceed 100 columns (counted in scalar values, including indentation and the line's trailing comment). A *broken* group prints the opening bracket at the end of the current line, each element on its own line one level deeper followed by `,` (trailing comma included), and the closing bracket on its own line at the original indentation. Empty groups print `()`, `[]`, `{}`, `effects {}`, `write {}`. When a line is too long (iii), the outermost flat group on the line is broken first, the leftmost one when the line holds several (two calls joined by an operator, a constructor pattern and the arm's call), and the rule is re-applied to each resulting line. Generic lists `<T, U>` are never groups and never break. |
| 7 | Function header: `pub fn name<T>(params) -> Ret` on one line (the params group may break under rule 6). Clauses follow, each on its own line indented one level, in canonical order: `effects`, every `requires` in source order, every `ensures` in source order, `decreases`. The body's `{` is placed on the header line when there are no clauses and the params group is flat; otherwise on its own line at the header's indentation. The same `{` rule applies to `while` (`invariant`s then `decreases`), `property` (`for` clauses), `patch` (`expect`), and to an `if`, `else if`, `match`, or `while` header in which a comma group printed broken: its `{` goes on its own line at the header's indentation, except inside a comma group, where it stays on the line that closes the header (T64). Capability operations: signature line then contract clauses, indented one level deeper than the operation. A bodiless `fn` (the stub form of grammar 3.9, `FnDecl::body` is `None`) prints its header and its clauses by the same rules and nothing after them: no `{` and no body, so the declaration ends with its header line or its last clause line. A definition-less `type` (`TypeDeclKind::Opaque`) is one line: `pub type List<T>`. |
| 8 | Expressions print with one space around binary operators, `=`, `->`, `=>`, and after `:` and `,`; no space before `,` or `:`; no spaces inside `()` `[]` `<>`; one space inside the braces of a struct literal, an `effects` set, and a `.work` selector set (`Page { ..p }`, `effects { db.get }`, `write { paginate.body }`), and none inside a `use` list's braces (`use std.text.{trim}`, `use a.b.{c, D}`, as every corpus file writes them); `?`, `.member`, `..spread`, unary `-` and `?hole` attach to their operand; `not` is followed by one space. Parentheses are preserved exactly as written (never added, never removed). The spread prints first. `forall x in c: p`. `if c {` ... `} else if c2 {` ... `} else {` ... `}`; `match s {` with arms `pattern => expr` or `pattern => {` block `}`. Blocks are always multi-line except the empty block `{}`. |
| 9 | Line breaks inside an expression are removed, with one exception: inside a `Paren` group, a source line break immediately before a binary operator is preserved (`BinaryExpr::break_before_op`); the operator then begins a continuation line indented one level deeper than the line containing the opening parenthesis. |
| 10 | Text literals are re-escaped canonically, over exactly the set grammar D62 forbids raw and semantics S40 escapes: `\\`, `\"`, `\n`, `\r`, `\t`; every other scalar value in U+0000-U+001F, U+007F (DEL), and the bidirectional formatting characters grammar D75 forbids raw (U+202A-U+202E, U+2066-U+2069), as `\u{H}` with the minimal number of lowercase hex digits (`\u{1}`, `\u{7f}`, `\u{202e}`); every other scalar value literally. Int literals print their digit string without separators. |
| 11 | Comments: `leading` comments print on their own lines at the node's indentation, text byte-for-byte from `//` (so `//x` stays `//x`, `// x` stays `// x`, and a tab in the text stays a tab; trailing spaces and tabs are dropped, rule 1), each preceded by one blank line when its `blank_line_before` is true (never directly after an opening `{`, `(`, or `[`, at the start of a file, or between clauses, rule 4); a `trailing` comment prints after one space following the node's last token; a second and every further trailing comment of the same node prints on a line of its own where the next parse attaches it (T63): above the next node of the same sequence (where nothing comes between, simply the next lines, at that node's indentation), below the `{` of a body that follows a header's last clause or a patch's `expect` line (Section 2.3 rule C), after the blank line that separates two declarations (rule 3; also after the last clause of a bodiless `fn`), at the operations' indentation after a capability operation's last clause, above the doc comments of the declaration an annotation belongs to (rule 5), and directly below the last declaration of a file, where it becomes a file trailing comment; `trailing_comments` print at the sequence's inner indentation; a comment written between a header and its `{` prints below the `{` (Section 2.3 rule C). |
| 12 | `.work` files: `use` declarations, one blank line, changes separated by one blank line; inside `change`: `base`, `target`, `workers`, tasks in source order, `integrate`; inside `task`: `write`, `add`, `depend`, `preserve`, `goal`; `after` on the task's header line. `.patch` files: `patch sel` newline `    expect base(sel)` newline `{` body `}`, patch forms separated by exactly one blank line. |

Decision: [D-147] T12. line breaking is width-aware but never joins what the author
broke: a group stays broken once broken (rule 6 (i)), and the formatter
breaks groups only when a line exceeds 100 columns (rule 6 (iii)).
Rationale: the design's own examples break parameter lists that would fit
on one line (`load_page`, `paginate_ints`) and keep others flat; a pure
width rule would rewrite the design's text, while a pure
preserve-multi-line rule would leave 300-column lines untouched. The
hybrid is idempotent (a broken group re-parses as `multiline`) and
reproduces every corpus file byte-for-byte. Rejected alternatives: a pure
width rule (rustfmt style; rewrites the corpus) and no width rule at all
(unbounded lines).

Decision: [D-148] T13. the formatter never reorders declarations, `use`
declarations, struct fields, enum variants, capability operations, match
arms, effect references, or set elements; it reorders only header clauses
into canonical order (rule 7) and `.work` item kinds (rule 12), both of
which the grammar already declares order-insensitive. Rationale: the design
says the formatter preserves declaration order, and reordering `use` lines
would move the corpus's `// glue:` comments away from their lines.
Rejected alternative: sorting `use` declarations and effect sets
(canonical, but moves comments).

Decision: [D-149] T14. the formatter never inserts, removes, or rewrites `@id`
annotations, and changes other tokens in four fixed ways only: it inserts
the trailing comma of a broken group and drops that of a flat one (rule 6;
grammar 2.5 leaves the comma to the formatter), drops the digit separators
of an `Int` literal and re-spells text-literal escapes (rule 10), and
moves doc comments before the annotations (rule 5); everything else it
changes is whitespace, line structure, where a comment prints (rule 11,
T63), and clause order (T13). `cpl fmt` is therefore a pure function of its input; `cpl id
assign` is the only command that adds `@id` lines (Section 4.2).
Rationale: a formatter that assigns identities makes `fmt --check` fail on
freshly written code and makes two formatting runs of the same input differ.
Rejected alternative: `fmt` assigning missing ids (one command fewer, but
`fmt` would need a random source and a project).

Decision: [D-241] T51. two adjacent `type` declarations stay adjacent: a
`type` declaration whose first token is on the line directly below the
last line of another `type` declaration is printed directly below it when
neither declaration has doc comments or annotations and the second has no
leading comment and no blank line before it; every other pair of
declarations, `type` pairs with docs, annotations, or a comment between
them included, is separated by exactly one blank line (rule 3). Rationale:
the design writes `pub type PageId(Text)` directly above `pub type Positive
= Int where self > 0` (design lines 53-54), the corpus copies that text
byte for byte (`examples/reader/src/reader/pages.cpl`), verbatim design
text is never edited (`spec/README.md`), and Section 12 requires `format(X)
== X` for every corpus file; this is the narrowest exception that keeps
the design's text canonical, and the condition is decided from the tree's
trivia alone (`Trivia::leading`, `Trivia::blank_line_before`), so the rule
stays idempotent. Rejected alternatives: one blank line between every two
declarations without exception (rewrites the design's text, so the corpus
could not be canonical); keeping any author-adjacent declarations together
(lets declarations of every kind run together, which rule 3 exists to
prevent).

Decision: [D-259] T63. a node's second and every further trailing comment
is printed on a line of its own exactly where the next parse attaches it
(Section 2.3 rules C to F), and so gets a new owner: above the next node
of its sequence, below the `{` of a body that follows a header's last
clause, after the blank line between two declarations, above the doc
comments of the declaration an annotation belongs to, and below the last
declaration of a file, where it becomes a file trailing comment (rule 11).
The round trip of this section therefore holds modulo `strip_layout`
(Section 12), which removes every comment. Rationale: rule 11's earlier
"on its own line below it" put the comment where the parser attaches it to
whatever follows, so a second formatting pass moved it again
(`fmt(fmt(x)) != fmt(x)` in seven positions; between two adjacent `type`
declarations the comment, now the second one's leading comment, also ends
T51's adjacency); printing it where its next owner prints it is the one
placement that keeps formatting idempotent without new syntax, and no
comment is lost or reordered. Rejected alternatives: keeping it directly
below the node (not idempotent); joining the comments into one line
(rewrites their text).

Decision: [D-260] T64. the `{` of an `if`, `else if`, `match`, or `while`
header in which a comma group printed broken (by any of rule 6's three
reasons) goes on its own line at the header's indentation, as after a
clause (rule 7), except when the header stands inside a comma group (an
`if` used as an argument or a list element), where the `{` stays on the
line that closes the header. Rationale: a `{` directly after a broken
group's closing bracket reads as part of the group and hides where the
body starts; inside a comma group, a line break before the `{` would be a
line break at the enclosing group's own level, which the next parse
records as `Group::multiline` (Section 2.2), so the enclosing group would
break on the second pass (`g(if f(` LF `a) {...})`) and formatting would
not be idempotent. Rejected alternatives: the `{` on its own line
everywhere (not idempotent inside groups); never moving it (an earlier
behaviour of the formatter, which left rule 7's "headers whose group
broke" without effect).

#### 2.4.1 Worked example

Input (not canonical):

```text
module demo.layout
use std.text.{trim}
pub fn load_page(id: PageId,   using db: PageRead) -> Result<Page, StoreError>
    ensures true
    effects { db.get }
{
    // fetch
    let page = db.get(id)?   // may fail


    Ok(page)
}
```

Output:

```text
module demo.layout

use std.text.{trim}

pub fn load_page(id: PageId, using db: PageRead) -> Result<Page, StoreError>
    effects { db.get }
    ensures true
{
    // fetch
    let page = db.get(id)? // may fail

    Ok(page)
}
```

The params group was flat in the source, so it stays flat; `{` is on its
own line because clauses are present; `effects` moves before `ensures`;
the two blank lines collapse to one; the trailing comment keeps one space;
the `use` list keeps its braces tight (rule 8), so the line is unchanged.

## 3. Pipeline and intermediate representations

### 3.1 Stages

```text
 file bytes ──lex──► tokens + trivia ──parse──► ast::Module (one per file)            cpl-syntax
      │
      ├──► cpl.toml / cpl.lock ──► Manifest, Lock                                      cpl-project (cpl_sema::project)
      │
 ast::Module* + std ──collect──► ModuleDef graph, Def table (DefId, StableId)          cpl-sema::collect
                     ──resolve──► names bound (DefId / LocalId), imports checked        cpl-sema::resolve
                     ──type──► hir bodies with Ty on every Expr, refinement proofs      cpl-sema::types
                     ──effects──► inferred effect sets vs declared bounds               cpl-sema::effects
                     ──contracts──► purity and binding rules of clauses                 cpl-sema::contracts
                     ──policy──► annotation vocabulary, release rules, hole reports     cpl-sema::policy
                                      │
                                      ▼
                                   Program (typed HIR)                                 cpl-hir (types), cpl-sema (construction)
                                   ├──► cpl-eval: Interpreter, run_function, run_tests (through cpl-hir only, T50)
                                   ├──► cpl-index: facets (via Program::ast), Index, receipts
                                   ├──► cpl-work: selector resolution (cpl_sema::resolve_selector), patch validation
                                   └──► cpl-emit-c: C packages and drivers, with cpl_sema::interop's plan (Milestone 2, Section 1.13)
```

When any file has a parse diagnostic, `check` reports the parse
diagnostic of every broken file and performs no resolution or typing (D59,
T6, semantics S52). Otherwise every file proceeds through every stage even
when an earlier stage reported errors, and stages skip definitions marked
`HasErrors` where continuing would only cascade, so one run reports every
independent semantic error.

### 3.2 `FileId` and `SourceMap` usage

One `SourceMap` per `Project`, shared with the `Program` as
`Arc<SourceMap>`. `FileId`s are assigned in this order: std files in their
embedded (sorted) order, registered as `std/<file name>` with origin `Std`
(the five std files of 0.1 are `FileId`s 0-4: `bytes`, `foreign`, `gen`,
`prelude`, `text`), then project source-set files in lexicographic order
of project-relative path, then virtual files as commands create them.
"Lexicographic order of path" here, in T15, and wherever a rule orders
files by path (Section 3.7, semantics S63 (4)) is the byte order of the
`/`-joined path string, not a component-wise order: `src/a.cpl` <
`src/a/b.cpl` < `src/a_b.cpl`. `cpl.toml` and `cpl.lock` are not
registered (their diagnostics are span-less, semantics S63 (6)). Spans
never cross files. Nothing downstream of `cpl-syntax` reads file text
except through `SourceMap::text(span)` (diagnostics, facet text, patch
application).

Decision: [D-150] T15. std files receive the lowest `FileId`s and project files
follow in path order, so that a `Program`'s `FileId`s are a deterministic
function of the source set and std version. Rationale: golden tests and
`--json` outputs embed locations; deterministic ids keep them stable
across runs. Rejected alternative: discovery order (filesystem-dependent).

### 3.3 The typed HIR (`cpl-hir`, re-exported as `cpl_sema::hir`)

The types of this section, and `Program` with the accessors of Section
1.6, are defined in the crate `cpl-hir` (T50). `cpl-sema` builds the HIR
and re-exports the crate as `cpl_sema::hir`, so `cpl_sema::hir::Def` and
`cpl_hir::Def` name one type; `cpl-eval` uses the crate directly, and
`cpl-index` and `cpl-work` reach it through `cpl_sema::hir`.

The HIR is a tree, like the AST, with every expression annotated with its
type and every name resolved. It contains no unresolved paths, no `else if` chains (desugared to nested
`If` in an `else` block), and no member-access nodes (members of intrinsic
types become calls, Section 3.6). There are no type aliases (grammar D64).

```rust
use cpl_diag::{Span, FileId, FileOrigin, StableId, SymbolPath, DeclName};
use num_bigint::BigInt;

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize)] pub struct DefId(pub u32);
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize)] pub struct ModuleId(pub u32);
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)] pub struct LocalId(pub u32);     // per definition
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)] pub struct HoleId(pub u32);      // per program

pub struct Program {
    pub defs: Vec<Def>,                            // index = DefId.0
    pub modules: Vec<ModuleDef>,                   // index = ModuleId.0
    pub std_defs: StdDefs,
    pub references: Vec<Reference>,                // sorted by (from, span)
    pub imports: Vec<Import>,                      // every `use` item of every file (Section 6.5); imports are not references
    pub refinement_classes: Vec<RefinementClassDef>,   // index = RefinementClass.0; one per distinct (base, resolved predicate), S55
    pub source_map: std::sync::Arc<SourceMap>,
    pub asts: Vec<(FileId, std::sync::Arc<ast::Module>)>,
}
/// DefIds of the std declarations the checker and evaluator name directly (prelude types and the std.gen refinement).
pub struct StdDefs { pub unit: DefId, pub option: DefId, pub result: DefId, pub constraint_error: DefId, pub foreign_error: DefId, pub positive: DefId }
pub struct Import { pub file: FileId, pub target: ImportTarget, pub span: Span }
pub enum ImportTarget { Def(DefId), Module(ModuleId), Namespace(Vec<String>) }   // Namespace: a submodule alias that is only a prefix of module paths, no module has exactly that path (semantics 1.6)
pub struct RefinementClassDef { pub base: Ty, pub predicate: Expr, pub members: Vec<DefId> }   // `predicate`: the resolved HIR predicate (no parentheses, names bound to DefIds); classes compare `base` and `predicate` structurally (S55)

pub struct ModuleDef {
    pub id: ModuleId,
    pub path: Vec<String>,                         // ["reader", "pages"]
    pub origin: FileOrigin,                        // Project or Std
    pub files: Vec<FileId>,                        // path order
    pub defs: Vec<DefId>,                          // (file order, declaration order)
    pub docs: Option<String>,                      // Section 3.7
}

pub struct Def {
    pub id: DefId,
    pub stable_id: StableId,
    pub provisional: bool,                         // no `@id` in source (Section 4.3)
    pub module: ModuleId,
    pub name: DeclName,                            // Ident("paginate") | Text("pagination ...") for tests and properties | Member { .. } for std stubs (Section 4.4)
    pub is_pub: bool,
    pub file: FileId,
    pub decl_index: usize,                         // index into that file's ast::Module::declarations
    pub span: Span,                                // the ast::Declaration span
    pub docs: Option<String>,                      // joined doc lines
    pub policy: VerificationPolicy,
    pub status: DefStatus,
    pub kind: DefKind,
}
#[derive(Copy, Clone, PartialEq, Eq)] pub enum DefStatus { Ok, HasErrors }
#[derive(Copy, Clone, PartialEq, Eq, Default)] pub struct VerificationPolicy { pub prove: bool }

pub enum DefKind {
    Fn(FnDef), Wrapper(WrapperDef), Refinement(RefinementDef),
    Struct(StructDef), Enum(EnumDef), Capability(CapabilityDef), Test(TestDef), Property(PropertyDef),
    BuiltinType(BuiltinTypeDef),                   // a definition-less std type (`@builtin("int") pub type Int`, `pub type List<T>`, `pub type Gen<T>`; stdlib S35, grammar G2)
}
// DefKind helpers: id_kind() ("fn", "ty", "cap", "test", "prop"), keyword(), is_type(), is_test(), generics(), locals(),
// and as_fn(), as_wrapper(), as_refinement(), ... accessors.
pub struct FnDef {
    pub generics: Vec<String>,                     // T, U; referenced as Ty::Param(index)
    pub params: Vec<ParamDef>,                     // positional
    pub using: Vec<ParamDef>,                      // capability parameters; ty is Ty::Capability
    pub ret: Ty,
    pub effects: EffectBound,
    pub requires: Vec<Clause>,                     // source order
    pub ensures: Vec<Clause>,                      // source order; `result` is `result_local`
    pub result_local: Option<LocalId>,
    pub decreases: Option<Clause>,
    pub locals: Vec<LocalDef>,                     // index = LocalId.0; params first
    pub body: FnBody,
}
pub enum FnBody { Block(Block), Builtin(String) }  // Builtin: the `@builtin("<id>")` registry id, e.g. "text_trim" (Section 3.6)
pub struct ParamDef { pub local: LocalId, pub name: String, pub ty: Ty, pub span: Span }
pub struct LocalDef { pub id: LocalId, pub name: String, pub ty: Ty, pub mutable: bool, pub kind: BindingKind, pub span: Span }
pub struct EffectBound {
    pub declared: Option<BTreeSet<EffectRef>>,     // None: no `effects` clause
    pub inferred: BTreeSet<EffectRef>,             // what the body actually performs, Section 3.4
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)] pub struct EffectRef { pub param: LocalId, pub op: String }
pub struct Clause { pub expr: Expr, pub text: String /* canonical, Section 5.2 */, pub span: Span }

pub struct WrapperDef { pub inner: Ty }
pub struct RefinementDef { pub base: Ty, pub self_local: LocalId, pub predicate: Clause, pub locals: Vec<LocalDef> }   // locals: index = LocalId.0, `self` and the predicate's `forall` binders
pub struct StructDef { pub generics: Vec<String>, pub fields: Vec<FieldDef> }
pub struct FieldDef { pub name: String, pub ty: Ty, pub docs: Option<String>, pub span: Span }
pub struct EnumDef { pub generics: Vec<String>, pub variants: Vec<VariantDef> }
pub struct VariantDef { pub name: String, pub payload: Vec<Ty>, pub docs: Option<String>, pub span: Span }
pub struct CapabilityDef { pub ops: Vec<OpDef> }
pub struct OpDef { pub name: String, pub params: Vec<ParamDef>, pub ret: Ty, pub requires: Vec<Clause>, pub ensures: Vec<Clause>,
                   pub result_local: Option<LocalId>,   // the local `result` names in `ensures`
                   pub locals: Vec<LocalDef>, pub docs: Option<String>, pub span: Span }
pub struct TestDef { pub locals: Vec<LocalDef>, pub body: Block }
pub struct PropertyDef { pub binders: Vec<Binder>, pub locals: Vec<LocalDef>, pub body: Block }
pub struct Binder { pub local: LocalId, pub gen: GenExpr, pub elem_ty: Ty, pub span: Span }
/// A definition-less std type. Its uses are the intrinsic Ty forms (Ty::Int, Ty::List, Ty::Gen, ...), never Ty::Adt.
pub struct BuiltinTypeDef { pub generics: Vec<String>, pub builtin: String /* "int", "bool", "text", "bytes", "list", "gen" */ }

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)] pub struct RefinementClass(pub u32);
#[derive(Clone, Eq, Debug)]                        // PartialEq and Hash are written by hand: `Refined` compares its class only (S55)
pub enum Ty {
    Int, Bool, Text, Bytes,
    List(Box<Ty>),
    Adt(DefId, Vec<Ty>),                           // struct or enum, including prelude Unit, Option<T>, Result<T, E>, ConstraintError
    Wrapper(DefId),                                // nominal wrapper
    Refined(RefinementClass, DefId),               // refinement type (semantics S55, T39): identity is the class; the DefId names the declaration written at the use site, for diagnostics; its base is RefinementDef::base
    Param(u32),                                    // generic parameter of the enclosing definition
    Capability(DefId),
    Gen(Box<Ty>),                                  // hermetic generator type (semantics S14): only `for` ranges and `gen` arguments
    Error,
}

pub struct Expr { pub kind: ExprKind, pub ty: Ty, pub span: Span }
pub enum ExprKind {
    Lit(Lit),
    Local(LocalId),
    Ctor { def: DefId, variant: u32, args: Vec<Expr> },              // enum variant incl. Ok/Err/Some/None/Unit
    Wrap { def: DefId, arg: Box<Expr> },                             // PageId("x")
    Field { base: Box<Expr>, def: DefId, index: u32 },               // struct field
    Call { callee: DefId, generic_args: Vec<Ty>, args: Vec<Expr>, using: Vec<Expr> },   // fn, @builtin fn, or @method/@property stub with the receiver first
    CapCall { handle: Box<Expr>, capability: DefId, op: u32, args: Vec<Expr> },
    Check { refined: DefId, arg: Box<Expr> },                        // check<R>(x): Result<R, ConstraintError>
    Propagate(Box<Expr>),                                            // e?  (Result only in 0.1)
    Unary { op: ast::UnaryOp, operand: Box<Expr> },
    Binary { op: ast::BinaryOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Forall { binder: LocalId, collection: Box<Expr>, predicate: Box<Expr> },
    Hole { id: HoleId, name: String },
    List(Vec<Expr>),
    StructLit { def: DefId, spread: Option<Box<Expr>>, fields: Vec<(u32, Expr)> },     // explicit fields by index
    If { cond: Box<Expr>, then_block: Block, else_block: Option<Block> },
    Match { scrutinee: Box<Expr>, arms: Vec<Arm> },
    Widen { expr: Box<Expr>, from: DefId },                          // Refined(from) -> its base type
    Refine { expr: Box<Expr>, refined: DefId, proof: RefinementProof },   // base -> Refined, statically established
    Gen(GenExpr),                                                    // a `gen.*` expression; ty is Ty::Gen(elem)
    Error,
}
pub enum Lit { Int(BigInt), Text(String), Bool(bool) }
pub enum RefinementProof { Constant }              // semantics S16 only (T39)
pub struct Arm { pub pattern: Pattern, pub body: Block, pub span: Span }
pub struct Block { pub stmts: Vec<Stmt>, pub tail: Option<Box<Expr>>, pub ty: Ty, pub span: Span }
pub enum Stmt {
    Let { local: LocalId, value: Expr },
    Discard(Expr),                                                   // let _ = e
    Assign { local: LocalId, value: Expr },
    Return(Option<Expr>),
    Assert { expr: Expr, text: String },
    While { cond: Expr, invariants: Vec<Clause>, decreases: Option<Clause>, body: Block },
    Expr(Expr),                                                      // Unit-typed, non-tail
}
pub struct Pattern { pub kind: PatternKind, pub ty: Ty, pub span: Span }
pub enum PatternKind {
    Wildcard, Int(BigInt), Text(String), Bool(bool), Bind(LocalId),
    Ctor { def: DefId, variant: u32, args: Vec<Pattern> },
    Unwrap { def: DefId, arg: Box<Pattern> },                        // wrapper pattern PageId(t)
}

/// Generator expressions of `for` clauses (Section 3.5).
pub enum GenExpr { Int, PositiveInt, Bool, Text, List(Box<GenExpr>), Option(Box<GenExpr>), Range(Box<Expr>, Box<Expr>) }

pub struct Reference { pub from: DefId, pub to: DefId, pub kind: RefKind, pub span: Span, pub interface: bool }
#[derive(Copy, Clone, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum RefKind { Call, TypeUse, Ctor, FieldAccess, CapabilityOp, Check }   // a `use` import is an `Import`, not a `Reference` (Section 6.5)
```

Every HIR type derives `Clone, Debug, PartialEq, Eq` (the id newtypes
also `Copy, Hash, PartialOrd, Ord`; `Program` only `Clone, Debug`; `Ty`'s
`PartialEq` and `Hash` are written by hand). Shapes the checker guarantees
and the evaluator and the index rely on: a member call is a `Call` of the
member's stub with the receiver first (T19); a capability operation call is
`CapCall` whose `handle` is the `Local` of a `using` parameter;
`Call::using[j]` lines up with the callee's `FnDef::using[j]` (S28), and the
callee's bound is its `FnDef::effects.declared`; any other use of a handle
is a `Local` of type `Ty::Capability`, which the effect stage diagnoses
(S27; `db(x)` is that `Local` alone); `var x = e` is `Stmt::Let` on a
mutable local; a literal proved against a refinement is a `Refine` node
(T39), while the implicit `Unit` of a missing tail or a bare `return`
proved against a refinement of `Unit` (semantics 3.10, 3.11) is no node:
the block keeps `tail: None` with that refinement as its `Block::ty`, and
the statement stays `Return(None)`, so the evaluator's re-check of
`Refine` nodes does not repeat those proofs (each decided a predicate on
the single `Unit` value); locals are numbered parameters first, then `using` parameters;
`result_local` is set when an `ensures` clause uses `result`;
`Clause::text` is the canonical clause text (T56) and `Clause::span` runs
from the clause's keyword (`requires n >= 0`); struct literal fields are
`(index, expr)` pairs in source order; a hole inside a contract expression
has a `HoleId` but no hole report (S64); statements carry no span, so a
stage that needs one reads `Program::ast`; and no inference variable
survives into the stored HIR.

Decision: [D-151] T16. the HIR is a boxed tree (like the AST), not an arena, and
`Block` stores its tail expression explicitly. Rationale: the interpreter
and the index walk it recursively and never need random access; an arena
would force every consumer to carry the arena reference. Rejected
alternative: `ExprId` arenas (better for incremental analysis, which
Milestone 1 does not do).

Decision: [D-152] T17. `Reference::interface` is true when the reference occurs in
the definition's signature, contract, or effects (not in its body, test
body, or property body); "interface dependencies" in the design are
exactly the targets of interface references. Rationale: this is the
smallest set a caller needs to use a declaration correctly, which is what
the default `inspect` response is for. Rejected alternative: all
dependencies by default (larger responses, no benefit for callers).

Decision: [D-153] T39. refinement proofs in Milestone 1 have exactly one form,
the constant-expression proof of `spec/semantics.md` S16, recorded as
`ExprKind::Refine { proof: RefinementProof::Constant }`. The design's
property (`for size in gen.positive_int` then `paginate(items, size)`)
typechecks not by a proof but by *type identity*: `gen.positive_int` has
type `Gen<std.gen.Positive>` (stdlib 8.1) and refinement identity is
structural (semantics S55, stdlib S29), so `std.gen.Positive` and
`reader.pages.Positive` are one `RefinementClass` and `Ty::Refined`
compares that class. Any other flow of a base value into a refinement is
`cannot_prove_refinement` with the note "use `check<R>(x)`". The
interpreter re-evaluates `Refine` nodes when `check_contracts` is on and
raises `internal_error` (`kind: "refinement_proof_unsound"`) if a
predicate fails, so an unsound proof rule can never produce a wrong value
silently; the re-check guards the checker and is no check of the program
(semantics 3.5: no runtime check is emitted for a proof, so a proved
refinement never faults as `refinement_violation`), observable only
through the budgets it spends (refinement frames, the time budget).
Rationale: typing is owned by `spec/semantics.md`, and S55 is
the mechanism both it and the library specification chose; a second,
toolchain-private proof rule (an earlier revision's "established
predicate" rule, which typed the binder `Int`) would have given hole
reports, `inspect`, and receipts a binder type the semantics does not
assign, and `for sizes in gen.list(gen.positive_int)` a `List<Int>` with
no predicate at all. Rejected alternatives: the established-predicate
rule; typing `gen.positive_int` as `Gen<Int>` and rejecting the design's
property.

### 3.4 Where diagnostics are produced

Codes of the ten language categories are defined in `spec/semantics.md`
9.4 (the complete Milestone 1 list) and are not duplicated here; the
toolchain categories are defined in Appendix B. The `category` field of
every diagnostic (semantics S49) names the row below that produced it.

| Stage (crate) | `category` | Codes (examples; the owning list is authoritative) |
| - | - | - |
| lexer and parser (`cpl-syntax`) | `parse` | exactly the `parse` table of semantics 9.4 (`invalid_character`, `invalid_escape`, `unterminated_text_literal`, `syntax_error`, `unknown_requirement`, ...), messages beginning with the grammar's quoted text (T6, grammar D72) |
| project loading (`cpl-project`, re-exported as `cpl_sema::project`) | `project` (Appendix B), `parse`, and `resolve` | `project_manifest_invalid` and `project_lock_invalid` (the fatal `LoadError::Manifest` and `LoadError::Lock`), `project_manifest_unknown_key`, `project_source_dir_missing`, `project_duplicate_file_module`, `project_no_sources` (W), `project_lock_toolchain_mismatch` (W); the parse code of every file that does not read as UTF-8 or does not parse; `unsupported_edition`, `reserved_module_path`, `module_file_misplaced` (W) (semantics 1.1-1.2). Loading never emits `project_lock_missing`, `project_lock_stale`, or a `project_binding_*` code, nor, from Milestone 2, any C code of Section 11.1 other than `project_manifest_invalid` and `project_manifest_unknown_key` for the C keys |
| the check driver (`cpl-sema::check`) | `project` | under `--release`, `project_lock_missing` (W) and `project_lock_stale` (T37 and T75, Section 11.2); after name resolution, the four `project_binding_*` codes through `cpl_project::check_bindings` (Section 11.1), and from Milestone 2, after them, the C binding checks of `cpl_project::check_c_bindings` (`project_c_target_not_enabled`, the seven backend-`c` binding codes, the syntax of `project_cbind_invalid`) and the export plan of `cpl_sema::interop::plan` (the `project_export_*` codes but `project_export_skipped_generic`, `project_type_name_unknown`, the semantics of `project_cbind_invalid`) |
| collect and resolve (`cpl-sema::collect`, `::imports`, `::sig`, `::names`) | `resolve`; `effect` for the codes of a declared bound and of handle types; `internal` for `std_builtin_missing` | `duplicate_declaration`, `duplicate_identity`, `unresolved_module`, `unresolved_name`, `private_item`, `private_type_in_public_signature`, `private_in_public_contract` (S4: the resolution walk of clauses and predicates, a public capability's operation clauses included), `unknown_annotation`, `annotation_misplaced`, `duplicate_annotation`, `invalid_annotation_argument`, `ambiguous_constructor`, `cannot_run_effectful`, `cannot_run_generic`; with the signatures, the codes of a declared bound and of handle types (`effect_param_not_capability`, `unknown_effect_operation`, `duplicate_effect` (W), `using_param_not_capability`, `capability_in_value_position`); plus the toolchain's `id_kind_mismatch` (W), `id_missing` (lint), and `std_builtin_missing` |
| typing (`cpl-sema::types`) | `type`; `effect`, `hole`, `release_policy`, and `resolve` for the codes noted | `type_mismatch`, `cannot_infer_type`, `cannot_prove_refinement`, `refinement_false`, `constant_evaluation_fault`, `arity_mismatch`, `not_callable`, `unknown_member`, `property_called`, `method_not_called`, `unused_value`, `non_exhaustive_match`, `unreachable_arm` (W), `propagate_*`, `for_range_not_generator`, `generator_outside_for`; `using_argument_not_capability` (category `effect`: only the syntax tells a bare name from another expression, S28); the hole reports and `hole` (I), `hole_type_unknown`, `hole_in_release`; then `unused_import` (W, category `resolve`), once the typer has completed the constructor resolutions of S10 that wait for an expected type |
| effects (`cpl-sema::effects`) | `effect` | `undeclared_effect`, `unused_capability` (W), `capability_as_value`, `capability_equality`; fills `EffectBound::inferred` |
| contracts (`cpl-sema::contracts`) | `contract`; `effect` for `effect_in_contract` | `hole_in_contract`, `propagate_in_contract`, `statement_in_contract`, `recursive_contract`, `unused_decreases` (W), `missing_decreases` (W); `verification_result` (I) and `CheckResult::verification`; `effect_in_contract` keeps its semantics 9.4 category `effect` (S60) |
| release policy (`cpl-sema::policy`) | `release_policy` | `verification_unsatisfied` (`hole_in_release` is the typer's) |
| evaluation (`cpl-eval`) | `contract`, `test`, `hole`, `runtime`, `internal` | `contract_violation`, `assertion_failed`, `unresolved_hole`, `division_by_zero`, `refinement_violation`, `resource_exhaustion`, `internal_error` (Section 1.7.3) |
| test runner (`cpl-eval::test`) | `test`, `contract` | `test_result` (I/E), `empty_test_selection`, `contract_counterexample` |
| index and git (`cpl-index`) | `index`, `git` | `index_unknown_symbol`, `index_ambiguous_symbol`, `index_partial` (W), `git_not_a_repository`, `git_unknown_revision`, `git_no_project_at_revision`, `git_command_failed` |
| work and patches (`cpl-work`) | `work`, `patch`, `git` | Section 7.2 (`cpl work check`, including `git_unavailable` (W), its only producer) and Section 10.5 |
| command line (`cpl-cli`) | `usage`, `internal`; from Milestone 2 also `project`, `build`, and `interop` | `usage_no_project`, `usage_bad_argument`, `usage_not_a_function`, `usage_not_provable`, `usage_not_implemented`, `internal_error` (exit 2); `project_export_skipped_generic` (I, `cpl build`), `build_target_not_enabled`, `build_lock_missing` (`cpl build --release`), `interop_bind_failed` (`cpl run`), `engine_divergence` (with `cpl-cabi`) |
| native build (`cpl-emit-c`, Milestone 2) | `build`, `internal` | every `build_*` code but `build_target_not_enabled` and `build_lock_missing` (Appendix B); `internal_error` kinds `emitted_c_rejected` and `emit_failed` (exit 2) |
| native engine (`cpl-cabi`, Milestone 2) | `internal` | `engine_divergence`; `internal_error` kinds `native_driver_failed` and `native_protocol` (exit 2); native faults are rebuilt as the evaluator's (`fault_from_record`), so their codes are the evaluation row's |
| C runtime (`cpl-rt`, Milestone 2), through the C API and a driver's `error` responses | `interop` | the `interop_*` codes of `spec/cabi.md` Section 4 |

### 3.5 Generators

A `for` clause's range expression is an ordinary expression of the
hermetic type `Gen<T>` (semantics S14): `gen` is the prelude's alias of the
module `std.gen` (semantics S54, stdlib S1), so `gen.int` resolves through
the module branch of semantics 2.4 to the zero-parameter `@property` stub
`std.gen.int`, and `gen.list(g)` is an ordinary call of the `@builtin`
function `std.gen.list` (stdlib 8.5). The typing stage lowers every
`Gen<T>`-typed expression to `ExprKind::Gen(GenExpr)`, the form the
property runner interprets; the element type of the binder follows from
the table. A `Gen<T>`-typed expression anywhere other than a `for` range or
a `std.gen` argument is `generator_outside_for` (semantics S14); `gen`
itself in value position is `not_a_value` (semantics 3.8).

| Source | `GenExpr` | binder type (stdlib 8.1) |
| - | - | - |
| `gen.int` | `Int` | `Int` |
| `gen.positive_int` | `PositiveInt` | `std.gen.Positive` (`Int where self > 0`), one type with every user refinement of that base and predicate (semantics S55, T39) |
| `gen.bool` | `Bool` | `Bool` |
| `gen.text` | `Text` | `Text` |
| `gen.list(g)` | `List(g)` | `List<elem(g)>` |
| `gen.option(g)` | `Option(g)` | `Option<elem(g)>` |
| `gen.range(lo, hi)` | `Range(lo, hi)` | `Int`; `lo` and `hi` are contract expressions typed `⇐ Int`, evaluated once per property before case 0 (stdlib 8.2; a fault while they evaluate is reported per semantics S71); `lo <= hi` is the stub's `requires` and its violation faults the property with zero cases |

Library: `spec/stdlib.md` owns the generator vocabulary (8.1), the
distributions (8.3), and the shrink candidates (8.4); an addition there
extends `GenExpr` here.

Decision: [D-154] T18. generator expressions are resolved and typed like any other
expression, by semantics S54's module alias and the `std.gen` stubs, and
are *lowered* to the special form `ExprKind::Gen(GenExpr)` of type
`Ty::Gen(elem)` in the HIR: the lowering maps each `std.gen` stub to its
`GenExpr` row by the stub's `@builtin` id (`gen_int`, `gen_list`, ...), so
the table above is the HIR's vocabulary, not a resolution rule. `Ty::Gen`
is hermetic exactly as semantics S14 says (`for` ranges and `std.gen`
arguments only; `generator_outside_for`, `for_range_not_generator`,
`unresolved_type` for a written `Gen<T>`), and no `Gen` value exists at
run time outside the property runner. Rationale: the semantics document
owns resolution, and its reading (a zero-parameter `@property` read without
parentheses, D43) needs no special form at the source level; a closed HIR
node is still the right shape for the runner, which interprets a fixed
vocabulary and never calls a `Gen`-valued function. Rejected alternatives:
a resolver-level special form that bypasses the stubs (an earlier revision
of this decision; it would have given `gen.int` a type the semantics does
not assign and no `DefId` for the index to record); a prelude type with
module constants (needs two language features the grammar excludes).

### 3.6 Standard-library stubs

The standard library is declared in CPL syntax in `std/*.cpl` and
implemented in Rust in `cpl-eval` and, from Milestone 2, in C in `cpl-rt`
for the native engine and every C package (`spec/cabi.md` Section 2); the
texts and tables both implementations share are `cpl-rtspec`'s (T70).
Loading rules:

| Rule | Value |
| - | - |
| Location | `std/` at the workspace root; one file per module (`std/prelude.cpl` is `module std.prelude`, `std/text.cpl` is `module std.text`, ...). `spec/stdlib.md` lists the files. |
| Embedding | `crates/cpl-project/build.rs` (T52) reads the `*.cpl` files directly in `../../std` (the workspace's `std/`) in sorted file-name order and generates `$OUT_DIR/std_files.rs` with `pub static STD_FILES: &[(&str, &str)]`, one `(file name, include_str!(...))` pair per file (`("prelude.cpl", ...)`); it prints `cargo:rerun-if-changed` for the directory and for each file, and fails the build on a stub that is not UTF-8. No Rust edit is needed to add a std file. Loading registers the files first, as `std/<file name>` with `FileOrigin::Std` (FileIds 0-4 in 0.1, Section 3.2). |
| Override | `LoadOptions::std = StdSource::Dir(path)`: every `*.cpl` file directly in that directory, in sorted name order (`LoadError::Std` when it is unreadable or holds none); the CLI sets it from the `CPL_STD_DIR` environment variable when present (library development only). |
| Parsing | the ordinary `.cpl` parser with `FileOrigin::Std`. Std files parse under the grammar plus the two deltas of stdlib 10.2: G1, a bodiless `fn` (`FnDecl::body = None`), and G2, a definition-less `type` (`TypeDeclKind::Opaque`). The parser accepts both forms in every file; `cpl check` applies stdlib 10.2's check rules (a bodiless `fn` or opaque `type` without `@builtin`, and any of the three std annotations outside the std unit, are errors). |
| Std-only annotations | exactly stdlib 10.2-10.3: `@builtin("<id>")` on a bodiless `fn` or a definition-less `type` names its Rust implementation by registry id (`list_take`, `text_trim`, `gen`; stdlib 10.6 is the complete set); `@method` / `@property` on a `@builtin` fn make it a member of the head type of its first parameter (`Int`, `Bool`, `Text`, `Bytes`, `List`, `Option`, `Result`), entered in the member table keyed by (receiver head, name) with the qualified name `std.prelude.<Receiver>.<name>` (stdlib S37: `std.prelude.List.take`, `std.prelude.Int.min`) and never callable as a free function; `property` members are written without parentheses (`page.len`), `method` members with (D43); a zero-parameter `@property` of `std.gen` is a generator property (`gen.int`, semantics S54). All three annotations are `unknown_annotation` in project files. There is no `std.list` module in 0.1 (stdlib S33). |
| Prelude | every project module implicitly imports every `pub` declaration of `std.prelude` (types `Unit`, `Option`, `Result`, `ConstraintError`, their constructors, and the free functions the library specification places there, such as `flatten`; `check` is a typechecker intrinsic) plus the module alias `gen` for `std.gen` (stdlib S1, semantics S54); explicit `use std.prelude.{...}` is permitted and redundant. `std.*` and `gen.*` module paths are reserved: a project file with such a header is `reserved_module_path` (semantics 1.1). |
| Identity | std declarations carry no `@id` annotations (stdlib Section 11 is the normative text of the stub files), so their identities are the provisional identities of semantics S2, their qualified names (`std.prelude.flatten`, `std.prelude.List.take`, `std.gen.Positive`, Section 4.3; stdlib S43), flagged `provisional: true` in JSON like every other unannotated declaration but stable because std is versioned with the toolchain and never renamed within an edition; `cpl id assign` never edits std files. |
| Digest | `std_digest` = blake3 over the concatenation of `name LF content LF` for every std file in sorted order, where `name` is the file name (`prelude.cpl`); `cpl_project::std_digest(STD_FILES)` for the embedded library, and `Project::std_digest()` for the std files a project was actually loaded with (the `CPL_STD_DIR` ones included), which is what every receipt (Section 8), the lock (11.2), and the release lock check record. |
| Dispatch | `cpl_eval::builtins::lookup(id: &str) -> Option<BuiltinFn>` where `type BuiltinFn = fn(&mut Interpreter, &[Value]) -> Result<Value, Fault>`, keyed by the `@builtin` id; `ids()` lists exactly the 59 ids of stdlib 10.6, sorted. A stub whose id the registry does not know is the check error `std_builtin_missing` (category `internal`) spanning its `@builtin` annotation, with `name` = the id, and its definition is marked `HasErrors`: `cpl-sema` checks the id against `cpl_sema::codes::BUILTIN_IDS`, a copy of the registry's list, because it cannot depend on `cpl-eval` (from Milestone 2 the constant is `cpl_rtspec::builtins::IDS`, which the registry is tested against, T70), and loading does not raise it (`cpl-project` cannot either). The six type ids (`int`, `bool`, `text`, `bytes`, `list`, `gen`) and the seven `gen_*` ids are registered but not callable: calling one is `internal_error` (`kind` `builtin_type_called` or `generator_not_a_value`), and `gen_range` checks its `requires lo <= hi` first (the runner calls that check at setup, Section 1.7.2). |

`check<R>(x)` is not a library function: the resolver turns it into
`ExprKind::Check` because its type depends on the type argument in a way
the type grammar cannot express. The library specification documents it
alongside the prelude for readers.

Decision: [D-155] T19. intrinsic-type members (`len`, `is_empty`, `take`, `drop`,
`drop_last`, `push`, and whatever `spec/stdlib.md` adds) are the
`@method`/`@property` stubs of `std/prelude.cpl` and `x.len` lowers to
`Call { callee: std.prelude.List.len, args: [x] }`; the HIR has no
member-access node. Rationale: one call node
gives the interpreter one dispatch path (`FnBody::Builtin`), gives the
verifier the member's CPL-syntax contract, and gives the index a real
`DefId` to record as a dependency. Rejected alternative: a hard-coded
member table in `cpl-sema` plus `Property`/`Method` HIR nodes (duplicates
every signature in Rust).

### 3.7 Modules spanning several files

1. Every `*.cpl` file under every `[build] sources` directory is parsed,
   once even when two overlapping source directories reach it (that is
   `project_duplicate_file_module`, with `path` and `sources`, and the file
   is still loaded once); its `module` header names its module. Files
   with the same header form one `ModuleDef`, `files` sorted by path. A
   project file whose module path begins with `std` or `gen` is
   `reserved_module_path` (raised by loading, spanning the module path,
   semantics 1.1) and is not collected: its declarations take no part in
   the program, and it is not also `module_file_misplaced` (one diagnostic
   per mistake).
2. `ModuleDef::defs` lists declarations in (file path, declaration order). Two
   declarations with the same name in one module, in any files, are one
   `duplicate_declaration` (tests: `duplicate_test_name`) at the later
   declaration in that order, with a note at the earlier (semantics S63).
3. `use` declarations are file-scoped: they bind names only in the file
   that contains them. A module's own declarations are visible in all of
   its files without import.
4. Module docs are the concatenation of each file's module doc comments in
   file order, separated by one blank line.
5. The conventional location of `module a.b.c` is `<sources>/a/b/c.cpl` or,
   for additional files of the same module, `<sources>/a/b/c/<name>.cpl`.
   Any other location is the warning `module_file_misplaced` (raised by
   loading) spanning the header's module path, with `module` and
   `expected` (the conventional path) (semantics 1.2). Nothing else depends
   on paths.

Decision: [D-156] T20. imports are file-scoped, not module-scoped (as semantics S5
also decides). Rationale: a
reader of one file sees every name it uses declared or imported in that
file, which is the design's "imports are explicit"; module-scoped imports
would make a file's meaning depend on a sibling. Rejected alternative:
module-scoped `use` (fewer repeated lines, hidden context).

## 4. Stable identity

### 4.1 The `@id` value

A generated identity is a kind prefix, an underscore, and 16 characters
encoding 80 random bits:

```text
generated_id = kind "_" base32{16}
kind         = "fn" | "ty" | "cap" | "test" | "prop"
base32       = "a".."z" | "2".."7"          // RFC 4648 alphabet, lowercase, no padding
```

| Declaration | kind |
| - | - |
| `fn` | `fn` |
| `type` (wrapper, refinement), `struct`, `enum` | `ty` |
| `capability` | `cap` |
| `test` | `test` |
| `property` | `prop` |

Generation: draw 10 bytes from the supplied `rand::Rng` (OS randomness in
the CLI, Section 1.3; a seeded generator in tests), encode them as 16
base32 characters, most significant bit first (80 bits exactly, so no
padding and no unused bits), prepend the kind; an id equal to an `@id`
value already in the project or planned earlier in the same run is drawn
again. Example: `fn_k7w3qz2mb6ptx4ve` (an earlier example,
`fn_k7w3qz2mb9ptx4ve`, contained `9`, which is not a base32 digit: it is an
accepted value but not of the generated format).

Accepted syntax of any `@id` argument (generated or hand-written):
`^[a-z][a-z0-9]*(_[a-z0-9]+)+$`, 4 to 64 characters. The design's
illustrative `pgn_7c2d` is accepted; `@id("Foo")`, `@id("")`, `@id(7)`,
and two literals are `invalid_annotation_argument` (semantics 1.8). The
*generated format* is the stricter regex `^(fn|ty|cap|test|prop)_[a-z2-7]{16}$`
(`is_generated_format`, Section 1.8); it is what T21's `id_kind_mismatch`
tests, and `pgn_7c2d` does not match it.

Decision: [D-157] T21. the kind prefix is informative, not authoritative: a
hand-written `@id("pgn_7c2d")` on a fn is valid, and a generated
`fn_...` moved onto a type by a human remains that type's identity.
`cpl check` reports `id_kind_mismatch` as a warning only for generated-format
ids whose prefix disagrees with the declaration kind. Rationale: identity
must survive edits, and the prefix exists to make ids readable in reviews
and diffs, not to encode semantics. Rejected alternative: rejecting a
mismatched prefix (would turn a re-kinding refactor into an identity
change).

### 4.2 `cpl id assign`

`cpl id assign [paths]` inserts an `@id` annotation for every top-level
declaration that lacks one. With no paths it covers every source-set
`.cpl` file; with paths, the named files or directories (recursively, not
through directory links, T54). Inside a project a named file must be a
`.cpl` file of its source set (else `usage_bad_argument`) and a named
directory selects the source-set files under it; outside any project the
named files and those under the named directories are loaded together as
one project of their own, so that no id is drawn twice among them. Output
paths follow Section 7.1.

Algorithm (`cpl_index::plan_id_assignments` + `apply_id_assignments`):

1. Load the project (parse only; typing is not required). A file with
   syntax errors is skipped with its diagnostics reported (exit 1).
2. For every file in path order and every `Declaration` of
   `Module::declarations` in source order whose `annotations` contain no
   `id` (a malformed `@id` counts as one and is left for the checker to
   report): generate an id of the declaration's kind (`IdKind::for_decl`).
3. Insert the line `@id("<id>")` followed by LF at the start of the line
   that holds the declaration's first annotation if it has any, otherwise
   the first token of its declaration (`pub` or the keyword), after its
   doc comments. The inserted line is at column 0, before any indentation
   that line has (indentation is never significant, grammar 1.1, and
   `IdAssignment::indent` records it; it is `""` in a formatted file), so
   the declaration's own line keeps every byte and `cpl fmt` later lays it
   out. No other byte changes; a CRLF file also gets an LF-terminated line.
4. Write the file. Report every assignment (human, one line each:
   `src/reader/pages.cpl:21  @id("fn_k7w3qz2mb6ptx4ve")  reader.pages.clean`;
   JSON: Section 7.2, where `line` is the line of the insertion point
   in the original text: the first annotation's line when there is one,
   else the keyword's).

Rules: `cpl fmt` never inserts, removes, or changes ids (T14); `cpl id
assign` never changes an existing id, never touches std files, never
reuses an id, and is idempotent (a second run assigns nothing).
`--check` writes nothing and exits 1 when any declaration lacks an id; it
reports the assignments a run would make, with ids drawn as that run would
draw them, so the ids it shows differ from run to run (Section 7.1) and
are not the ids a later run writes.

Decision: [D-262] T66. `cpl id assign` changes nothing but the inserted
`@id` lines (step 3), so a file that was canonical need not stay canonical
(the inserted line is an annotation, which ends T51's exception for two
adjacent `type` declarations: after `cpl id assign`, `cpl fmt --check`
flags `examples/reader`'s `src/reader/pages.cpl`); formatting follows id
assignment: a user runs `cpl fmt` after `cpl id assign`, and the
integrator formats every file it assigned ids in before it commits
(Section 10.6). Rationale: an assignment that reformatted would rewrite
lines nobody asked it to touch, while formatting is layout only and
changes no facet hash or module content hash (which hashes the formatted
text, Section 1.8), so formatting after the assignment changes nothing the
checks judge; the integrator formats before it runs the checks, so its
receipts record the formatted files (Section 10.6). Rejected alternatives: inserting the blank line rule 3 then
requires (makes `cpl id assign` a partial formatter that must know every
layout rule); committing non-canonical trees from the integrator (`cpl fmt
--check` would fail on every integration commit that assigned an id).

### 4.3 Provisional identities

A declaration without `@id` has a *provisional identity*: its qualified
name (semantics S2), spelled exactly as `SymbolPath::display` renders it
(Section 4.4):

```text
provisional_id = module_path "." decl_name
decl_name      = identifier                       // fn, type, struct, enum, capability
               | UPPER_IDENT "." identifier        // a std @method/@property stub: std.prelude.List.take (stdlib S37)
               | text_literal                     // test and property names, canonically escaped, in double quotes
```

Examples: `reader.pages.clean`, `reader.pages.Page`,
`tests.pagination."pagination handles empty input"`, `std.prelude.flatten`.
The member form is the name of a `@method` or `@property` `fn` with at
least one parameter in a module whose path begins with `std`, the receiver
being the head of its first parameter's type; every member of 0.1 is
declared in `std.prelude` (semantics 3.9), so it is always
`std.prelude.<Receiver>.<name>` (Section 3.6, stdlib S37), and a
zero-parameter `@property` (`std.gen.int`) has no receiver. A declaration
whose `@id` is malformed (not exactly one text argument of the accepted
syntax) has its provisional identity, and `cpl id assign` leaves it alone
(the checker reports `invalid_annotation_argument`).
A qualified name always contains a `.` and an accepted `@id` value never
does (Section 4.1), so the two forms can never collide and a provisional id
can never be written into source; `StableId::is_provisional` tests exactly
that, and the single producer is `StableId::provisional`. Where an
`@id`-annotated declaration and an unannotated one are both in play,
`qualified_name` always accompanies `symbol` (semantics S49), so consumers
never need to parse either. Provisional identities are unstable under rename and move: a
renamed declaration appears in a semantic diff as `identity_removed` plus
`identity_added`, and a patch's `expect base(...)` on a provisional id
fails after a rename. JSON always flags them (`"provisional": true`).
`cpl check` can report `id_missing` for public declarations as an opt-in
lint (`--lint id_missing`); it is off by default so that fresh code checks
clean. The work pipeline does not leave identities provisional: `cpl work
check` warns `work_provisional_identity` for every declaration a task or
an integrate requirement names without an `@id` (Section 7.2), and the
integrator assigns an id to every declaration still lacking one before it
runs the integration checks (Section 10.6, T47), so integrated code
carries the stable identities the design's rename and move guarantees
depend on.

### 4.4 Symbol paths

```rust
// cpl_diag::id
pub enum DeclName {
    Ident(String),                         // fn, type, struct, enum, capability
    Text(String),                          // a test or property: the two share one namespace (S3), so a path cannot tell them apart; `SymbolBlock::kind` does
    Member { receiver: String, name: String },   // a std `@method`/`@property` stub: `std.prelude.List.take` (stdlib S37)
}
pub struct SymbolPath { pub module: Vec<String>, pub name: DeclName }
impl SymbolPath {
    /// `reader.pages.paginate`, `tests.pagination."pagination handles empty input"`, `std.prelude.List.take`.
    /// A quoted final segment is `Text`; an `UPPER_IDENT` segment before a final `LOWER_IDENT` is a `Member` receiver.
    pub fn parse(s: &str) -> Result<SymbolPath, String>;
    pub fn display(&self) -> String;       // the same syntax; text segments canonically escaped
}
```

The display form is the `path` field of every `SymbolVersion`,
`DiagnosticJson::backtrace` frame, inspect response, and receipt. `.work`
selectors cannot name a single test (their segments are identifiers,
grammar 4.4); they address tests through namespace wildcards
(`tests.pagination.*`), as the design does.

Decision: [D-158] T22. tests and properties are addressed by a quoted final path
segment holding their name text. Rationale: the design gives tests stable
identities "in namespaces like ordinary declarations" but names them with
text literals; quoting is the only form that is unambiguous and round-trips
through the formatter's escaping. Rejected alternative: slugifying names
(`pagination_handles_empty_input`), which is lossy and collides.

### 4.5 Duplicate detection

During collection `cpl-sema` builds `StableId -> DefId` over every
definition of the project and of std (and, from Milestone 2 on, of every
locked dependency). A second occurrence of an id is one `duplicate_identity`
(error) at the later declaration in (file path, declaration order),
spanning its name token, with `symbol` = the duplicated `@id` value,
`qualified_name` = the later declaration's qualified name, `facet:
"identity"`, and a note at the earlier declaration's name token (semantics
S63 (3) and (4); the note names the earlier file through its span). The
second definition keeps its `Def` but is given its provisional identity in
the `Program` so that every map stays well-defined; its `status` is
`HasErrors`, and the semantic index takes that identity from the
`Program`, not from the syntax, which still shows the `@id` value:
`Index::version` gives the second definition that id with `provisional:
true` and the `identity` and `definition` hashes of that identity.
`cpl_index::facet_map` (which runs without `cpl-sema`) records the same
situation among the declarations of the source set (std is not part of a
revision, Section 5.4) in `FacetMap::duplicates` and keeps the first
occurrence in path order.

### 4.6 Copying, renaming, moving

| Edit | Identity | Toolchain behavior |
| - | - | - |
| rename a declaration, keeping `@id` | preserved | semantic diff: `identity_renamed`; callers' bodies change (`body_changed` on each) |
| move a declaration to another module, keeping `@id` | preserved | semantic diff: `moved`; `cpl check` warns `module_file_misplaced` only if the *file* is misplaced |
| copy a declaration | the copy must get a new identity | the copied `@id` line is a duplicate: `duplicate_identity` until the copy's `@id` line is deleted and `cpl id assign` runs; a task's patch that introduces a duplicate id is rejected at validation |
| change an `@id` value | a new identity | semantic diff: `identity_removed` + `identity_added`; a `.work` task can never do this: `identity` is not a writable facet (Section 10.3) |

## 5. Facets and content versions

### 5.1 Coverage

A declaration's facets, by kind (there are no type aliases, grammar D64).
`-` means the facet does not apply and is absent from the declaration's
facet map.

| Facet | `fn` | `type` wrapper | refinement | `struct` | `enum` | `capability` | `test` | `property` |
| - | - | - | - | - | - | - | - | - |
| `identity` | `@id` value | same | same | same | same | same | same | same |
| `name` | name | name | name | name | name | name | name text | name text |
| `signature` | `pub`, generics, parameters (positional and `using`), return type | `pub`, payload type | `pub`, base type | `pub`, generics, fields (names and types) | `pub`, generics, variants (names and payloads) | `pub`, operations (names, parameters, return types) | - | `for` clauses |
| `contract` | `requires`, `ensures`, `decreases` clauses | - | the `where` predicate | - | - | every operation's `requires`/`ensures` clauses | - | - |
| `effects` | the `effects` set | - | - | - | - | - | - | - |
| `body` | the block | - | - | - | - | - | the block | the block |
| `docs` | declaration doc comment | same | same | declaration and field doc comments | declaration and variant doc comments | declaration and operation doc comments | same | same |
| `verification_policy` | `@prove` | - | - | - | - | - | - | - |

A definition-less std `type` (`TypeDeclKind::Opaque`, `pub type List<T>`)
has the facets of a wrapper (`identity`, `name`, `signature`, `docs`), and
a bodiless `fn` (a std stub, grammar 3.9) has all eight, its `body` text
being empty (a `fn` with a body never has an empty `body` text: an empty
block is `{}`). Both forms also parse in project files, where the checker
rejects them (`missing_body`); the facet map gives them these facets
anyway, being syntactic.

Decision: [D-159] T23. a function's `decreases` clause belongs to the `contract`
facet, and loop `invariant`/`decreases` clauses belong to the `body`.
Rationale: a function-level measure is a specification of the declaration
read by callers' proofs, like `requires`; loop annotations are
implementation detail inside the body. The design lists `requires` and
`ensures` for the contract and places `@prove` in `verification_policy`,
which is about *whether* obligations must be discharged, not *what* they
are. Rejected alternatives: `decreases` in `verification_policy`; `decreases`
in `body`.

Decision: [D-160] T24. the `name` facet is separate from `signature`: the
signature's canonical text replaces the declaration name with `_`.
Rationale: a rename must register as exactly `identity_renamed`, not also
`signature_changed`, for the design's "renames retain identity" to be
visible in the semantic diff. Rejected alternative: name inside the
signature (simpler text, noisier diffs).

Decision: [D-161] T25. the `docs` facet of a struct, enum, or capability covers its
members' doc comments as well as its own, and member doc comments are
excluded from `signature`. Rationale: editing a field's documentation must
not appear as a signature change to the integrator. Rejected alternative:
member docs inside `signature`.

### 5.2 Canonical text

Canonical text is produced by `cpl_syntax::formatter::format_fragment` (Section 1.5) on the
relevant AST parts, at indentation level 0, with LF line ends and no
trailing newline. Comments (ordinary `//`) are *excluded* from every facet
text: facets are produced with `format_fragment(_, FragmentMode::Canonical)`
(Section 1.5), which skips every `Trivia`. Doc comments appear only in
`docs`. Source line breaks inside an expression are not reproduced either:
canonical text ignores `Group::multiline` and `BinaryExpr::break_before_op`
(Section 2.4 rules 6 and 9) and the width budget, so every signature line
and `effects` set is one line, and so is every clause whose expression
contains no `if` or `match`; the blocks of an `if` or `match` keep the
multi-line layout of rule 8, so such a clause begins an unindented line and
continues over the lines of its blocks (T56), and a contract facet cannot
be split back into clauses by line. Parentheses are kept exactly as
written (rule 8). The one-line `clause` of semantics 9.2 and the HIR's
`Clause::text` are derived from the same rendering by T56.

| Facet | Canonical text |
| - | - |
| `identity` | the `@id` value, or the provisional id |
| `name` | the identifier; for tests and properties the name as a canonically escaped text literal, with quotes (the canonical rendering of a `Text` literal expression holding the name) |
| `signature` of `fn` | `pub fn _<T, U>(a: A, b: B, using c: C) -> R` on one line, params always flat, `pub ` only when public |
| `signature` of wrapper / refinement / std opaque type | `pub type _(Text)` / `pub type _ = Int` / `pub type _<T>` |
| `signature` of struct / enum | `pub struct _<A> {` LF, one `    name: Type` per field, LF `}`; enum likewise with `    Variant(T1, T2)` lines; with no members `pub struct _ {}` / `pub enum _ {}`, as the formatter prints an empty body |
| `signature` of capability | `pub capability _ {` LF, one `    fn name(params) -> R` per operation (no contracts), LF `}`; `pub capability _ {}` with no operations |
| `signature` of property | one `for x in <gen>` per line |
| `contract` of `fn` | every `requires <expr>` in source order, then every `ensures <expr>` in source order, then `decreases <expr>`; each clause begins an unindented line; empty string when there are none |
| `contract` of refinement | `where <expr>` |
| `contract` of capability | for each operation that has clauses, in declaration order: `fn name` LF then its `    requires` lines followed by its `    ensures` lines; empty string when no operation has clauses |
| `effects` | `effects { a.x, b.y }` with the references sorted by their `param.op` string and deduplicated; empty string when there is no clause or the set is empty |
| `body` | the block formatted at level 0 (`{` LF ... LF `}`), including loop invariants and `decreases`, with no blank lines; empty for a bodiless `fn` |
| `docs` | the declaration's doc lines joined with LF, each without trailing spaces and tabs (2.4 rule 1: other Unicode white space at a line's end is text and stays; so that formatting never changes the facet, T14), a final empty `///` line kept as an empty last line (the text then ends in LF); for struct/enum/capability followed, for each member with docs in member order, by LF `member <name>` LF and its lines (a declaration whose only docs are on members therefore starts with LF); empty string when there are none |
| `verification_policy` | `@prove` when present, else empty string |

Example, for the design's `paginate`:

```text
signature:            pub fn _<T>(items: List<T>, size: Positive) -> List<List<T>>
contract:             ensures flatten(result) == items
                      ensures forall page in result: page.len > 0 and page.len <= size
                      ensures forall page in result.drop_last(): page.len == size
effects:              (empty)
verification_policy:  @prove
identity:             pgn_7c2d
name:                 paginate
```

Decision: [D-162] T26. the `effects` canonical text sorts and deduplicates the
set, while the formatter preserves the author's order in source (T13).
Rationale: an effect bound is a set; reordering it must not read as a
change to the integrator. Rejected alternative: source order (then `effects
{ a.x, b.y }` and `effects { b.y, a.x }` would be different versions).

Decision: [D-246] T56. a clause has two canonical texts: its *facet text*,
`format_fragment(Fragment::Expr, Canonical)`, which keeps the multi-line
layout of the blocks of an `if` or `match` (2.4 rule 8) and is what the
`contract` facet (and the `body` facet, for loop clauses) hashes; and its
*clause text*, the facet text with every line break and the indentation
after it replaced by one space (`if n > 0 { let m = n m < 10 } else { true
}`), which is the HIR's `Clause::text` and `Stmt::Assert::text`, the
`clause` field of every diagnostic (semantics 9.2), the obligation
`clause` (Section 8.1), and the `requires`/`ensures`/`invariants` of the
hole report (semantics S48). The two coincide for every clause without an
`if` or `match`. Canonical text has no comments and escapes every line
break inside a text literal, so every line break it has is layout and the
clause text loses nothing a key needs. Rationale: a facet text is hashed
and shown to readers, where the block layout is what the formatter prints
and keeps hashes independent of source layout, while `clause` must be one
line (semantics 9.2: a stable key, one line per clause in the hole report
and in human output); a block holds `let` statements and a tail (semantics
S32) that two-dimensional text separates by line, which no one-line block
syntax of the grammar can express. Rejected alternatives: one-line facet
texts (the facet would need a block syntax the formatter never prints, and
a facet text would no longer be a fragment the formatter produces); a
multi-line `clause` (breaks the one-line key of semantics 9.2 and the
one-line rendering of contract violations).

### 5.3 Content hash

```text
facet_hash(text) = lowercase_hex(blake3(utf8(text)))[0..32]
```

Thirty-two hex characters are 128 bits. Test vector: the empty string
hashes to `af1349b9f5f9a1a6a0404dea36dcc949` (the first 32 hex characters
of BLAKE3 of the empty input). A facet with empty canonical text (no
`effects`, no `@prove`, no docs) therefore has that hash rather than being
absent, which keeps every applicable facet present in every map.

Decision: [D-163] T27. facet hashes are truncated to 128 bits; file and tree
digests (`Digest`) keep the full 256 bits. Rationale: facet hashes appear
in every inspect response, receipt, and candidate record and are compared
for equality only, where 128 bits is ample; file digests bind evidence to
inputs and cost nothing to keep whole. Rejected alternative: 256 bits
everywhere (longer JSON, no benefit) or 64 bits (collision risk across a
large monorepo's history).

### 5.4 The facet map of a revision

`cpl_index::facet_map(project, revision)`:

1. For every parsed `.cpl` file of the source set in path order (std is
   never part of a revision, Section 0, and a file that failed to parse
   contributes nothing), for every `Declaration` of `Module::declarations`
   in order: determine kind, name, stable id (from a well-formed `@id` or
   provisional, Section 4.3, using the file's `module` header), applicable
   facets, canonical texts (`facet_text(module_path, decl, facet)`), and
   hashes.
2. `definition_hash` = `facet_hash` of the text formed by one line
   `<facet>=<hash>` per applicable facet in `Facet::ALL` order, joined with
   LF.
3. Insert into `entries` keyed by stable id; on a duplicate id keep the
   first and record every occurrence's file in `duplicates` (in id order;
   a file that declares an id twice is listed twice).

The map is purely syntactic (no resolution), so `cpl diff` and baseline
checks work on revisions that do not typecheck, and it is the same whether
built from a directory or from a git tree (Section 9.3; symbolic links per
T54). `Program`-level `SymbolVersion`s (used by `Index`, receipts, inspect)
are produced by the same function, `symbol_version`, through
`Program::ast(def)`; the two never disagree because there is one
implementation, except that the `Program` gives the later occurrence of a
duplicated `@id` its provisional identity (Section 4.5), which the `Index`
uses in place of the syntax's `@id` value.

### 5.5 Baselines and `expect base(x.body)`

A task's *baseline* is the facet map of the snapshot the task was given:

```json
{
  "revision": { "commit": "3f2e...", "state": "clean", "digest": "..." },
  "snapshot_commit": "3f2e...",
  "predecessors": ["cand_7f3a..."],
  "facets": { "pgn_7c2d": { "path": "reader.pages.paginate", "provisional": false,
                            "facets": { "identity": "...", "name": "...", "signature": "...", "contract": "...",
                                        "effects": "...", "body": "...", "docs": "...", "verification_policy": "..." },
                            "definition": "...",
                            "kind": "fn", "module": "reader.pages", "name": "paginate", "file": "src/reader/pages.cpl" } }
}
```

`predecessors` lists candidate digests whose outputs were layered onto the
`base` commit (Section 10.4); for initial tasks it is empty.
`snapshot_commit` is the 10.1 field. Each `facets` entry is a `FacetEntry`
(Section 1.8): the `SymbolVersion` fields without `id` (the key), then
`kind`, `module`, `name`, `file`, and `effects` (the `param.op` strings,
present only when non-empty), so that a restarted coordinator can rebuild
the facet map that enforcement and the semantic diff read (T31); a
`duplicates` list `[{ "id", "files" }]` follows when the map has any. The
facet map's revision is `revision`. Decoding refuses an unknown `kind` and
a `provisional` flag that contradicts the form of its id. The check for
`patch S expect base(S')`:

1. Resolve `S` and `S'` in the patch's scope to `(id, facet)` pairs; they
   must be equal and the facet must be `body` (`patch_facet_unsupported`
   otherwise, D39).
2. `expected = baseline.facets[id].facets[body]`; absent id is
   `patch_unknown_symbol`, and an entry without a `body` facet (a kind
   that has none) is `patch_facet_unsupported`.
3. `current = facet_map(tree being patched)[id].facets[body]`; absent is
   `patch_target_missing`.
4. `current != expected` is `patch_stale` with both hashes (the payload
   fields `expected` and `actual`, semantics 9.2's vocabulary); equal
   proceeds to apply (Section 10.5).

At task-level validation the tree is the baseline itself, so step 4 is
trivially satisfied and the check guards only against a patch authored
against a different snapshot; at integration the tree is the current
target head, and step 4 is what refuses a stale body replacement after the
target moved.

## 6. Semantic index and `cpl inspect`

### 6.1 Query model

A query names one *target* and an *include set*.

| Target form | Example | Resolution |
| - | - | - |
| stable id | `pgn_7c2d`, `fn_k7w3qz2mb6ptx4ve` | `Program::by_stable_id`; tried first when the argument matches the accepted id syntax of Section 4.1 and an entry exists |
| provisional id | `reader.pages.clean` | same table; a provisional id is the qualified name (Section 4.3), so this row and the next resolve identically |
| qualified symbol path | `reader.pages.paginate`, `tests.pagination."pagination handles empty input"` | `SymbolPath::parse` then `Program::lookup` |
| bare name | `paginate`, `take`, `List.take`, `"pagination handles empty input"` | `Target::Name`: every definition so named in project and std; an identifier names the declarations of that name, the std member stubs of that name (`take` is `std.prelude.List.take`), and the tests and properties whose name text it is; a `Receiver.member` pair names that member; a quoted text names tests and properties. Exactly one match resolves; several are `index_ambiguous_symbol` (`name`, and `candidates`, their qualified names in display-path order, with a note at each declaration); none is `index_unknown_symbol` (`name`; a note when the path's module does not exist, or when the path is a capability operation, whose capability is the declaration to query) |

Std declarations are first-class targets (`std.text.trim`).
`Target::parse` reads an argument by these rows in order and never fails:
an accepted `@id` spelling is an id (one with no entry is then read as a
bare name), a qualified symbol path is a path (one that names nothing is
then read as the provisional id it spells), an identifier, a
`Receiver.member` pair, or a quoted name is a bare name, and anything
else, which no identifier or path spells, is the name text of a test or
property written without its quotes (`pagination handles empty input`, as
a shell passes it, or `Foo.Bar`, which is neither a path nor a
`Receiver.member` pair), so
an argument that names nothing is `index_unknown_symbol` with the target
as read in `name`, never a usage error. Definitions that share one
qualified name (a `duplicate_declaration`) count once, as the first, which
`Program::lookup` finds.

### 6.2 Include set

```text
include     = include_item ("," include_item)*
include_item = "signature" | "contract" | "effects" | "implementation" | "docs"
             | "callers" | "dependencies" | "obligations" | "tests" | "changes"
```

The list is read strictly (`IncludeSet::parse`): no spaces, no empty item,
and `interface_dependencies` is not an item; a repeated item counts once.
Anything else is `usage_bad_argument`.

| Item | Adds to the response |
| - | - |
| (always) | the target's `symbol` block with identity, name, kind, location, facet versions, and `verification_policy` |
| `signature` | `signature` text (with the name, for reading; the facet hash is in `facets`) |
| `contract` | `contract` clauses as text |
| `effects` | `effects` set |
| `docs` | `docs` text |
| `implementation` | `implementation`: the body's canonical text, plus `holes` inside it |
| `dependencies` | `dependencies`: every definition the target references (interface and body), each with its own `symbol` block including signature and contract |
| `callers` | `callers`: every definition referencing the target, with call-site locations |
| `obligations` | `obligations`: proof obligations of the target and their current status (Section 8.1) |
| `tests` | `tests`: tests and properties whose static call closure reaches the target |
| `changes` | `changes`: semantic-diff statements about the target between `--since <rev>` (default `HEAD~1`) and the current revision |

The default include set, when `--include` is absent, is `signature,
contract, effects, docs, interface_dependencies`: the target with
`signature, contract, effects, docs` plus its *interface dependencies*
(T17), each with the same four. `interface_dependencies` is an `Include`
item that the default set implies and the command line never names (an
explicit `--include signature,contract,effects,docs` yields a response
without the dependency list), so the response shape is a function of the
effective include set alone. This is the design's "target and its
interface dependencies"; `callers` and `implementation` are opt-in, as the
design requires.

Decision: [D-164] T28. `dependencies` in the include set means *all* direct
dependencies (interface and body); the default set's dependency list is
the interface subset and is labelled `interface_dependencies`. Rationale:
the design distinguishes the default ("interface dependencies") from
explicit expansion; naming the two lists differently keeps a consumer
from mistaking the default for the full set. Rejected alternative: one
list with a filter flag.

### 6.3 Revision

```rust
// cpl_index::revision
pub fn source_digest(project: &Project) -> Digest;
pub fn current_revision(project: &Project, repo: Option<&GitRepo>) -> Result<Revision, GitError>;
```

`source_digest`: for every file of the source set (Section 0: `cpl.toml`,
the `.cpl`, `.work`, and `.patch` files; never `cpl.lock`, never std) in
path order, the line `<path> <hex blake3 of bytes>` terminated by LF; the
digest is blake3 of the concatenation. `current_revision`:

| Situation | `commit` | `state` |
| - | - | - |
| no repository, or `git` not installed | `null` | `worktree` (silently: `check`, `run`, `test`, `fmt`, and `inspect` never emit `git_unavailable`; only `cpl work check` does, Section 7.2) |
| repository without commits | `null` | `worktree` |
| `git --no-optional-locks status --porcelain --untracked-files=all -- <source-set roots>` is empty | `git rev-parse --verify HEAD` | `clean` |
| otherwise | HEAD | `worktree` |

`digest` is always `source_digest` of the loaded tree. The paths passed to
`git status` are the project-relative source-set *roots* (`cpl.toml`, each
`[build] sources` directory, with `"."` the project root, and `work`)
prefixed with `GitRepo::project_prefix`, so a change to a file under a root
that is no member (`src/notes.txt`, `work/deep/x.work`) makes the state
`worktree` while the digest is unchanged; every git invocation also sets
`GIT_LITERAL_PATHSPECS=1`, so that a directory named `lib*` or `proj[1]`
matches only itself. From Milestone 2 (T82) the paths also include, for
every backend-`c` binding of the manifest, its adapter file and the
`sources` and `include_dirs` of its `[bindings.<name>.c]` (Section 11.1),
so that an uncommitted edit of a build input makes the state `worktree`
while the digest, over the source set alone, is unchanged; a project
without such a binding, every Milestone 1 project among them, passes the
paths above and nothing else. `clean` is git's answer, not a comparison
of digests: a member git ignores (a gitignored `src/gen.cpl` is still a member),
checkout filters (`core.autocrlf`, `.gitattributes` `eol` and smudge), and
a symbolic link whose target lies outside the roots or the repository can
leave the loaded tree different from the commit's tree while the state is
`clean`. A consumer that needs the commit's own digest computes
`source_digest` of the project loaded from `tree_at(commit)` and compares.
The digest's line format `<path> <hex>` LF is unambiguous only for paths
without a line feed; edition 0.1 does not reject such paths (a known
limitation).

Decision: [D-326] T82. Build inputs (Section 0) stay outside the source
set, so that no revision digest, facet map, or receipt input covers them,
but from Milestone 2 every output that depends on them says so: the state
of a revision covers the build inputs of the manifest's backend-`c`
bindings (above), so that an uncommitted edit of one makes it `worktree`;
every binding that `cpl run` binds reports the digest of its build inputs
(`RunBinding::inputs`, Section 7.2); the determinism rule of Section 7.1
names them; and a build input is read-only for every task until a file
selector for it, proposed for Milestone 6 (`adapter.<binding>`), makes it
grantable (Section 10.3), while a task whose boundary is
`ProcessAccess::Toolchain` may still build and run them, since that
boundary admits the processes that the `cpl` binary starts itself (Section
10.8). This amends the roots of this section and the determinism rule of
Section 7.1 for projects with backend-`c` bindings, and states which
processes the `Toolchain` boundary of Section 10.8 admits; no Milestone 1
project has such a binding, so no Milestone 1 output changes. Rationale:
design line 245 requires every response to identify its source revision "so
context from different snapshots is not silently combined", and an adapter
change invalidates the evidence that rests on it (line 604), while a run of
an uncommitted adapter edit would otherwise report the state `clean` at a
commit that cannot reproduce its result; keeping build inputs out of the
source set keeps the identities, facets, and receipts of CPL code
independent of native files, which the lock pins instead (T75), and the
authorized task that design line 602 asks for needs the coordinator of
Milestone 6, while a task that may not edit an adapter can still run it
under the narrower of the two boundaries that admit child processes.
Rejected alternatives: build inputs as members of the source set (every
revision digest and receipt would change with a vendored C file); a
revision state blind to build inputs (an uncommitted adapter would run
under a `clean` revision); a file selector for build inputs in Milestone 2
(nothing enforces grants before Milestone 6); requiring
`ProcessAccess::Any` for a task that builds or runs C (a task that cannot
change the C it runs would need the widest boundary to run it).

### 6.4 Response schema

`cpl inspect reader.pages.paginate --include signature,contract,effects,callers --json`:

```json
{
  "command": "inspect",
  "toolchain": { "version": "0.1.0", "edition": "0.1", "json_schema": 1 },
  "revision": { "commit": "3f2e9c1d...", "state": "clean", "digest": "9a0b..." },
  "status": "ok",
  "diagnostics": [],
  "include": ["signature", "contract", "effects", "callers"],
  "symbol": {
    "id": "pgn_7c2d", "path": "reader.pages.paginate", "provisional": false,
    "facets": { "identity": "…", "name": "…", "signature": "…", "contract": "…", "effects": "…",
                "body": "…", "docs": "…", "verification_policy": "…" },
    "definition": "…",
    "kind": "fn", "module": "reader.pages", "public": true,
    "location": { "file": "src/reader/pages.cpl", "line": 43, "col": 1, "end_line": 67, "end_col": 2 },
    "verification_policy": { "prove": true },
    "signature": "pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>",
    "contract": ["ensures flatten(result) == items",
                 "ensures forall page in result: page.len > 0 and page.len <= size",
                 "ensures forall page in result.drop_last(): page.len == size"],
    "effects": []
  },
  "callers": [
    { "id": "reader.pages.paginate_ints", "path": "reader.pages.paginate_ints", "provisional": true,
      "facets": { "…": "…" }, "definition": "…", "kind": "fn", "…": "…",
      "call_sites": [ { "file": "src/reader/pages.cpl", "line": 75, "col": 8, "end_line": 75, "end_col": 37 } ] },
    { "id": "tests.pagination.\"pagination handles empty input\"", "path": "tests.pagination.\"pagination handles empty input\"",
      "provisional": true, "facets": { "…": "…" }, "definition": "…", "kind": "test", "…": "…",
      "call_sites": [ { "file": "src/tests/pagination.cpl", "line": 13, "col": 12, "end_line": 13, "end_col": 30 } ] },
    "…" ]
}
```

The explicit list names no dependency item, so the response has no
dependency list (6.2); the default set's response has
`interface_dependencies` instead of `callers`, each block with the same
four facet keys (`reader.pages.Positive` with `"signature": "pub type
Positive = Int"`, `"contract": ["where self > 0"]`, `"effects": []`,
`"docs": null`, `"relation": "type_use"`).

A symbol block is written in the order of `SymbolBlock`'s fields (Section
7.1: keys in declaration order): the `SymbolVersion` keys first (`id`,
`path`, `provisional`, `facets`, `definition`; the version is flattened
into the block), then `kind`, `module`, `public`, `location`,
`verification_policy`, `signature`, `contract`, `effects`, `docs`,
`implementation`, `holes`, `relation`, `interface`, `call_sites`; the `"…"`
keys above elide `module`, `public`, and `location`, which every block
carries.

| Key | Present when | Type |
| - | - | - |
| `include` | always | the effective include set, sorted |
| `symbol` | always | symbol block (table below) |
| `interface_dependencies` | `interface_dependencies` in the effective set (the default set, Section 6.2) | list of symbol blocks with `relation` |
| `dependencies` | `dependencies` included | list of symbol blocks with `relation` and `interface: bool` |
| `callers` | `callers` included | symbol blocks with `call_sites: [Location]` |
| `obligations` | `obligations` included | Section 8.1 `Obligation` objects |
| `tests` | `tests` included | symbol blocks with `relation: "direct" \| "transitive"` |
| `changes` | `changes` included | `{ "since": Revision, "statements": [DiffStatement] }` (Section 9) |

Symbol block (`cpl_diag::json::SymbolBlock`):

| Key | Type | Notes |
| - | - | - |
| `id`, `path`, `provisional`, `facets`, `definition` | as `SymbolVersion` | always |
| `kind` | `"fn" \| "ty" \| "cap" \| "test" \| "prop"` | always |
| `module` | string | always |
| `public` | bool | always |
| `location` | `Location` | always; std symbols use `std/<file>.cpl` |
| `verification_policy` | `{ "prove": bool }` | always for fn; omitted otherwise |
| `signature` | string | when `signature` included, for every kind: the header with its name (`test "<name>"` for a test, which has no `signature` facet; `property "<name>"` followed by its indented `for` lines for a property) |
| `contract` | `[string]` | when `contract` included, for every kind (`[]` for a kind without the facet, 5.1): canonical clause texts in canonical order; a capability's are `fn <op> <keyword> <clause>`, per operation clause |
| `effects` | `[string]` | when `effects` included, for every kind (`[]` for every kind but `fn`): sorted `param.op` strings |
| `docs` | string or `null` | when `docs` included |
| `implementation` | string | when `implementation` included; canonical body text (fn, test, property only) |
| `holes` | `[HoleReportJson]` | when `implementation` is included (a function, test, or property): the holes inside the body, the check's hole reports with their `dependencies` (an index built without them refuses, Section 1.8); absent otherwise |
| `relation` | `Relation` (`cpl_diag::json`): a `RefKind` spelling (`call`, `type_use`, `ctor`, `field_access`, `capability_op`, `check`) in dependency lists (the first reference's, 6.5), `direct` (the test itself has a `Call` or `CapabilityOp` reference to the target) or `transitive` in `tests` | in dependency lists and `tests` |
| `interface` | bool | in `dependencies` |
| `call_sites` | `[Location]` | in `callers`: for a `Call` or `CapabilityOp` reference the whole call expression (`paginate(items, checked_size)`, `xs.take(1)`, `xs.len`; a generator call of a `for` clause, which the HIR keeps only as a span-less `GenExpr`, T18, from the syntax), for any other reference its own span (the type, constructor, or field name); in span order, identical sites once |

The facet-text keys depend on the include set alone (6.2), so they are
written for every kind once included; `facets` shows which facets the
kind has. Dependency blocks (both lists) always carry `signature` and
`contract` and, when the set includes them, `effects` and `docs` (so the
default set gives each the same four); caller and test blocks carry no
facet texts, only the version, `kind`, `module`, `public`, `location`,
`verification_policy` (functions), and `call_sites` or `relation`.

Every nested symbol block carries its own facet versions, so no context
from two snapshots can be combined without the mismatch being visible
(all versions in one response come from one `revision`).

### 6.5 Computing callers and dependencies

`cpl-sema` records a `Reference` for every resolved use of a definition
while typing (Section 3.3): a call (`Call`), a type mention in a
signature, local annotation, generic argument, or struct literal
(`TypeUse`), a constructor or wrapper use in an expression or pattern
(`Ctor`), a field access (`FieldAccess`, to the struct), a capability
operation call (`CapabilityOp`, to the capability), and a `check<R>`
(`Check`, to the refinement). A `use` import is file-scoped (T20) and
belongs to no declaration, so it is not a `Reference`: it is recorded in
`Program::imports` as `Import { file, target, span }`. `interface` is set
per T17. References into contract clauses are interface references with their
natural kind (`flatten` in an `ensures` is an interface `Call`).

```text
dependencies(d)           = distinct `to` over references with from == d, keeping the first (kind, interface) per target
interface_dependencies(d) = those with interface == true
callers(d)                = distinct `from` over references with to == d and kind in {Call, CapabilityOp, Ctor, TypeUse, FieldAccess, Check}
tests_for(d)              = every test/property t such that d is in the transitive closure of Call and CapabilityOp edges from t
```

References are in (from, span) order and, at one span, in the order the
checker recorded them (`Program::sort_references` is a stable sort):
`check<R>(x)` records the type use of `R` before its `Check`, so such a
dependency's `relation` is `type_use`. A recursive definition is its own
caller and its own dependency (its hole reports' `symbols` list it too,
Section 7.2). `tests_for` follows the call closure: a test that only names
a type is not among that type's tests.
Imports are not references, so a `use` line never makes a file a caller.
Lists are sorted by display path. The index is rebuilt from scratch
for each command in Milestone 1; there is no on-disk cache (the design
treats caches as disposable, and a project of the corpus's size indexes
in milliseconds).

## 7. Command-line contract

### 7.1 Common rules

| Rule | Value |
| - | - |
| Project discovery | `--project DIR` names the project root (must contain `cpl.toml`); otherwise the nearest ancestor of the current directory containing `cpl.toml`. Commands that need a project and find none exit 2 with `usage_no_project`. `cpl fmt` with explicit paths and `cpl id assign` with explicit paths outside any project work without one, as do `cpl fmt --stdin` and the reserved `cpl work run` and `cpl work integrate`; `cpl work check` locates the project before it reads its file. `--project` and `--json` may precede or follow the command. `CPL_STD_DIR` replaces the embedded standard library (Section 3.6); from Milestone 2, `CPL_RUNTIME_DIR` replaces the embedded C runtime (Section 1.12) and `CPL_CC` names the C compiler when neither `--cc` nor `[interop.c] compiler` does (T72). |
| `--json` | emit exactly one JSON object (an `Envelope`, Section 1.4.2) on stdout and nothing else on stdout; pretty-printed with two-space indentation and keys in declaration order (the field order of the `cpl_diag::json` types; a `serde_json` map inside them, such as a diagnostic's flattened payload, value JSON, or a receipt's `check_configuration`, is a sorted map, T4, so its keys come out sorted, except `DiffStatement::detail`, Section 9.2; which optional keys are `null` and which are omitted is fixed in Appendix C); stderr is used only for crash traces. The envelope's `diagnostics` are the objects of semantics S49, in `Diagnostics::sorted` order (file path, span start, severity, code; spanless diagnostics last, in emission order), so two implementations produce the same array (T46). Without `--json`, diagnostics render to stderr (Section 1.4.1) in the same order and results to stdout, except that `cpl test`'s `test_result` diagnostics are its stdout lines (Section 7.2) and are not repeated on stderr. |
| `--jsonl` | (`check` and `test` only) stream one S49 diagnostic object per line in emission order, then one final line holding the envelope without `diagnostics`; for consumers that want progress. Emission order is deterministic: for `check` it is the sorted order above (checking completes before anything is emitted), for `test` the completion order of the selected tests, which is their qualified-name order (T8). On any other command `--jsonl` is `usage_bad_argument`, reported in that command's own form (human, or one envelope under `--json`); `--json` together with `--jsonl` is `usage_bad_argument` too. |
| Exit codes | `0` success (`status: ok`); `1` the command ran and reported errors or failures (`status: unsuccessful`, T44: deliberately not a proof or test word, so `cpl prove` with an `unknown` result reports `status: unsuccessful, result: unknown`); `2` usage error, missing project, unreadable input, git failure, or internal error (`status: error`): an internal error is the command's own (a panic, kind `panic`; a file that cannot be read or written, kind `io`; a standard library that cannot be loaded, kind `std_unavailable`), while an `internal_error` *fault* raised by evaluated code is a fault like any other and exits 1 (T61). A git failure other than "not installed" or "not a repository" stops every command with `git_command_failed`, `check`, `run`, and `test` included; those two only make the working tree's revision `{ commit: null, state: worktree }`, silently (Section 6.3). A command line clap refuses is `usage_bad_argument` (exit 2), printed in the form the line asked for (one envelope under `--json`; JSONL under `check --jsonl` or `test --jsonl`); `--help` and `--version` print to stdout and exit `0`. Warnings and info alone exit `0`. |
| Determinism | given the same source set, flags, seed, and toolchain, every output is byte-identical except `duration_ms` fields, `produced_at`, OS-generated seeds, and the ids `cpl id assign` draws from OS randomness (also under `--check`, Section 4.2); from Milestone 2 also `cpl build`'s `cached` (whether it reused the cache), while its artifacts are byte-identical for one compiler and flag set (`spec/cabi.md` Section 1); and from Milestone 2 an output of a command that builds or runs C (`cpl build`, `cpl run` with a backend-`c` binding, `--engine native` and `both`) depends on the build inputs and the C compiler as well, which `build.json`, the `inputs` of `cpl run`'s `bindings`, and a test receipt's `check_configuration.native` record (T82). |
| Encoding | stdin, stdout, file contents are UTF-8; paths in output are project-relative with `/` separators; a file named on the command line outside the located project, or with no project (`cpl fmt`, `cpl id assign`, `cpl work check`), is shown relative to the current directory when it lies under it, else absolute. |
| Reserved | `cpl work run` and `cpl work integrate` (Milestone 6) exit 2 with `usage_not_implemented` and a note naming the milestone. |

Decision: [D-165] T46. every list of diagnostics this toolchain emits, in text and
in JSON, is in `Diagnostics::sorted` order: by file path, then span start,
then severity, then code, with spanless diagnostics last in emission
order; emission order within a stage is file path then item order (Section
1.6), so the spanless tail is deterministic too. Rationale: Section 12.4
compares `--json` output against golden files, and a stage-ordered or
traversal-ordered array would differ between two correct implementations.
Rejected alternative: emission order everywhere (observable, and
implementation-defined within a stage).

Decision: [D-254] T58. the commands that act on the checked program,
`cpl run`, `cpl test`, `cpl prove`, and `cpl work check`, first check the
project in development mode and act only when it checks without errors;
otherwise they report the check's errors and warnings and exit 1, running,
proving, or validating nothing. Their envelopes carry the check's errors
and warnings (`cpl work check` only when the check fails: otherwise its
diagnostics are the validation's) but never its infos (`hole`,
`verification_result`), which are `cpl check`'s; `cpl inspect` answers
anyway and reports of the check only `index_partial` (Section 7.2). `cpl work check` disregards the
check's diagnostics located in `.work` and `.patch` files, which are no
part of the program (semantics S53), so a different work file that does
not parse does not stop the validation of this one. Rationale: a run, a
proof, or a selector of a program with errors is not well defined (a file
that failed to parse leaves the program without definitions, semantics
S52, so every selector would be a cascade, S62), and an info that every
command repeated would be noise the agent already has from `cpl check`.
Rejected alternatives: proving or validating a program with errors
(obligations and targets of definitions that failed to check); repeating
the check's infos in every envelope.

Decision: [D-255] T59. every command that names one declaration (`cpl
inspect`, `cpl run`, `cpl prove`) resolves it by Section 6.1
(`Index::resolve` of `Target::parse`): a stable or provisional id, a
qualified path, or a bare name that names exactly one definition (`cpl run
paginate_ints`). A target that names nothing, or a bare name that names
several, is `index_unknown_symbol` or `index_ambiguous_symbol`: exit 1 for
`cpl inspect`, whose answer it is, and exit 2 for `cpl run` and `cpl
prove`, for which it is a usage error like a target that is no function.
Rationale: one resolution rule makes the three commands accept the same
spellings, and the exit codes follow what each command is: a query, for
which "no such declaration" is a result, or an action on one function, for
which a name that names no function is a bad argument (7.4 gives `prove`
exit 2 for a target that is no function). Rejected alternatives: qualified
names and ids only for `cpl run` and `cpl prove` (an earlier wording of
7.2); exit 1 for every unresolved target (an unresolved `cpl run` target
would look like a fault).

Decision: [D-256] T60. an envelope's `revision` is `null` exactly when no
project is involved: none is located, the located one does not load, the
command form reads none (`cpl fmt --stdin`, `cpl fmt` and `cpl id assign`
on files outside any project, the reserved `cpl work run` and `cpl work
integrate`), clap refused the command line (no command ran), or the report
is of a panic (`internal_error`, kind `panic`, whose command's state is
gone); every other report, a refusal found before the project was loaded
included (an `--arg` value, a test selector or `--id`, `--lint`,
`--include`, `--json` with `--jsonl`, a path or a read error of `cpl fmt`,
`cpl id assign`, or `cpl work check`), names the revision of the located
project. Rationale: design line 245 requires every response to identify
the source revision, and a refusal's revision must not depend on the order
in which a command happens to check its arguments. Rejected alternative:
`null` for every refusal found before loading (an earlier behaviour, which
made the field depend on that order).

```text
cpl [--project DIR] [--json] <command> [args]

commands:
  check      typecheck the project
  fmt        format .cpl, .work, and .patch files
  run        run one function on constant arguments
  test       run tests and properties
  inspect    query the semantic index
  prove      report verification status for one symbol
  diff       semantic diff between two revisions
  id assign  insert missing @id annotations
  work check parse and validate a .work file against the project
  build      build the project's C package (Milestone 2)
  work run / work integrate   reserved for Milestone 6
```

### 7.2 Commands

#### `cpl check [--json] [--jsonl] [--release] [--project DIR] [--lint NAME]... [--deny-warnings] [--lock]`

Loads the project (the source set and std; `work/` files are parsed for
syntax only and take no part in checking, semantics S53), runs
`check_with`, and reports.

| Flag | Effect |
| - | - |
| `--release` | `CheckConfig::release = true` (semantics S47): every hole in a `fn` body is the error `hole_in_release`, reported in place of `hole` (semantics S64; a hole in a `test`/`property` body stays `hole`, raised to severity `warning`); every `@prove` declaration whose verification status is `unknown` or `failed` is the error `verification_unsatisfied` (no manifest switch weakens it, T36; `proved` and `conditional` pass, the latter with its assumptions named in the receipt, Section 8.1); `cpl.lock`, when present, must be fresh (`project_lock_stale`, error; from Milestone 2, when the lock has C entries, the digests of the C build inputs too, T75), and a missing lock is the warning `project_lock_missing` (T37) |
| `--lint NAME` | enable an off-by-default lint (`id_missing`, Section 4.3) |
| `--deny-warnings` | exit 1 when any warning is reported |
| `--lock` | after a successful check, write `cpl.lock` for the current tree (Section 11.2) |

In development mode every hole is the diagnostic `hole` (severity `info`,
payload per semantics S48 plus `dependencies.symbols`), and every `@prove`
declaration yields `verification_result` (`info`, the verifier's `status`
and `reason`, `spec/verification.md`). `cpl-sema` emits both
without the parts that need Git and the semantic index: `cpl-cli` adds the
hole's `dependencies` object and the `receipt` of `verification_result`,
built from the same values, through `cpl-index`
(`Index::complete_check_diagnostics`: the `SymbolVersion`s of
`Index::dependencies` of the hole's declaration, Section 6.5, and the
content hashes of its modules); the output is `Index::check_output`. A hole
whose expected type is erroneous is reported with `expected_type: "?"`
(semantics S69). `--deny-warnings` is `CheckConfig::warnings_as_errors`,
read by `CheckResult::has_errors`.

Human output: diagnostics on stderr; one summary line on stdout:
`check: 0 errors, 2 warnings, 1 hole, 14 definitions in 3 modules`. The
`summary` counts the check's errors and warnings, its hole reports, and
the project's definitions, modules, and `.cpl` files (`files`; std, `.work`,
and `.patch` files are not counted).

JSON (`CheckOutput`, flattened into the envelope):

```json
{
  "command": "check", "toolchain": { "…": "…" }, "revision": { "…": "…" }, "status": "ok",
  "diagnostics": [
    { "code": "hole", "category": "hole", "severity": "info", "message": "unfinished expression ?build_pages",
      "span": { "file": "src/holes/pages.cpl", "line": 20, "col": 5, "end_line": 20, "end_col": 17 },
      "symbol": "pgn_7c2d", "qualified_name": "holes.pages.paginate", "facet": "body",
      "hole": "build_pages", "expected_type": "List<List<T>>",
      "bindings": [ { "name": "size", "type": "Positive", "kind": "parameter" }, { "name": "items", "type": "List<T>", "kind": "parameter" } ],
      "capabilities": [],
      "requires": [], "ensures": [ "flatten(result) == items", "…" ], "invariants": [],
      "dependencies": { "source_revision": "3f2e…", "toolchain": { "cpl": "0.1.0", "edition": "0.1", "std": "…" },
                        "modules": [ { "module": "holes.pages", "content_hash": "…" } ],
                        "symbols": [ { "id": "holes.pages.Positive", "path": "holes.pages.Positive", "provisional": true, "facets": { "…": "…" }, "definition": "…" } ] } },
    { "code": "verification_result", "category": "contract", "severity": "info", "message": "paginate: unknown (unresolved hole)",
      "span": { "file": "src/holes/pages.cpl", "line": 15, "col": 8, "end_line": 15, "end_col": 16 },
      "symbol": "pgn_7c2d", "qualified_name": "holes.pages.paginate", "facet": "verification_policy", "status": "unknown", "reason": "unresolved_hole",
      "receipt": { "source_revision": "3f2e…", "toolchain": { "cpl": "0.1.0", "edition": "0.1", "std": "…" }, "modules": [ "…" ],
                   "check_configuration": { "release": false }, "assumptions": [] } } ],
  "release": false,
  "summary": { "errors": 0, "warnings": 0, "holes": 2, "definitions": 5, "modules": 1, "files": 1 },
  "holes": [ "…the same hole payloads, without the envelope fields…" ],
  "verification": [ { "symbol": "pgn_7c2d", "qualified_name": "holes.pages.paginate", "status": "unknown", "reason": "unresolved_hole" } ]
}
```

`dependencies` follows semantics S48 (`source_revision`; `dirty`, present
only when `true`; `toolchain` as `{ cpl, edition, std }`; `modules` with
per-module content hashes: the project modules of the declaration and of
what it references, its own included, never a std module) and adds
`symbols`: the `SymbolVersion`s of every definition the enclosing
declaration references, std definitions and, for a recursive declaration,
itself included, in display-path order, which is the finer-grained form
the semantic index provides. `modules[].content_hash` is
`cpl_index::module_content_hash` (Section 1.8: blake3 of each file's
`format_module` text, in path order, joined with LF); `toolchain.std` is
`std_digest` (Section 3.6); every `verification_result` and `test_result`
diagnostic carries the `receipt` object of semantics S59, built from the
same values (`check_configuration` is `{ release }` for verification and
`{ cases, max_shrinks, max_call_depth, seed }` for tests); `bindings` lists the
innermost scope first and, within a scope, the most recently declared
binding first (parameters therefore appear in reverse declaration order,
`size` before `items` above); `mutable` is emitted only when `true`. `capabilities` entries are `{ "parameter": "db", "capability":
"reader.pages.PageRead", "operations": [ { "name": "get", "signature": "fn get(id: PageId) -> Result<Page, StoreError>", "in_bound": true } ] }`.

Exit: 1 if any error (or warning with `--deny-warnings`); 2 if the project
cannot be loaded (`LoadError`); else 0.

#### `cpl fmt [--check] [--stdin KIND] [paths]`

| Form | Behavior |
| - | - |
| `cpl fmt` | formats every `.cpl`, `.work`, and `.patch` file of the source set (Section 0), in place; a file is rewritten only when its content changes |
| `cpl fmt paths...` | formats the named files, or every `.cpl`/`.work`/`.patch` file under named directories (recursively); no project required |
| `--check` | writes nothing; exit 1 if any file would change, listing them one per line on stdout |
| `--stdin KIND` | `KIND` is `cpl`, `work`, or `patch`; reads one file from stdin, writes the formatted text to stdout; paths are not allowed |

A file with a lexical or syntax error is reported (its `parse` diagnostic
on stderr, T6), left untouched, and makes the exit code 1. `cpl fmt
paths...` descends named directories without following directory links
(T54); a named path that does not exist, or a named file that is no
`.cpl`, `.work`, or `.patch` file, is `usage_bad_argument` (exit 2), and a
file that cannot be written is `internal_error` (kind `io`, exit 2).
`--stdin` writes the formatted text to stdout, which `--json` reserves for
the one envelope, so `--stdin --json` without `--check` is
`usage_bad_argument`; with `--check` only the envelope is printed. The
text read from stdin is the virtual file `<virtual:stdin>`.
Idempotency (`fmt(fmt(x)) == fmt(x)`) is a tested guarantee (Section 12).
`fmt` never assigns ids (T14).

JSON (`FmtOutput`):

```json
{ "command": "fmt", "…": "…", "status": "ok",
  "check": false,
  "files": [ { "path": "src/reader/pages.cpl", "result": "unchanged" },
             { "path": "work/pagination.work", "result": "formatted" } ] }
```

`result` is `unchanged`, `formatted`, `would_format` (with `--check`), or
`error`. Exit: 1 if any `would_format` or `error`; else 0.

#### `cpl run <function> [arg]... [--arg JSON]... [--json] [--max-call-depth N] [--engine eval|native|both]`

Checks the project (any error: exit 1 with the check's errors and
warnings, nothing runs, T58), resolves `<function>` by Section 6.1 (T59: a
qualified name, an `@id`, a provisional id, or a bare name that names one
definition), binds the arguments, calls the function with contracts
checked, and prints the result.

Arguments bind to positional parameters in order and come in two forms
that may be mixed: a bare argument is a CPL *constant expression*
(semantics S39: literals, constructors, `[...]`, struct literals, unary
minus, `not`, parentheses; `cpl run reader.pages.paginate_ints
"[1, 2, 3]" 2`), parsed with `cpl_syntax::parse_expr` and typed against
the parameter; `--arg JSON` is value JSON (Section 7.3) decoded against the
parameter. This section owns the argument syntax: a binary operator, which
the proof rule's constant expressions of semantics 3.5 admit, is not part
of it (`1 + 2` is `usage_bad_argument`); a constructor may be written bare
or qualified, the written path being a suffix of the constructor's full
path (`Missing`, `StoreError.Missing`, `reader.pages.StoreError.Missing`;
for a wrapper `Tag` of module `vals.main`: `Tag`, `main.Tag`, or
`vals.main.Tag`); the
spans of a constant-expression diagnostic are offsets into the argument's
own text (`cpl_eval::run::ARGUMENT_FILE`), which the CLI registers as the
virtual file `<virtual:argument N>` (N 0-based). A bare argument that
begins with `-` and is not a plain negative number (`-(7)`, `- 5`) is read
as a flag unless it follows `--` (`cpl run m.f -- -(7)`). Argument indices are 0-based.
The function may be any function of the checked program, a standard-library
one included (`cpl run std.text.split '"a,b"' '","'`, a member stub such as
`std.prelude.Int.abs` with its receiver as the first argument). An `--arg`
object with a duplicate key is `usage_bad_argument`: the CLI's JSON reader
must detect it, since `value_from_json` receives an already-parsed value in
which duplicates have collapsed; and a value nested deeper than the JSON
reader's limit is `usage_bad_argument` too (`serde_json` counts to 128 and
refuses the 128th level, so the deepest value accepted has 127 levels of
arrays and objects). The `--arg` values are read with the command line,
before the project is loaded, so a malformed one is reported before a
target that does not resolve (its envelope still names the revision,
T60); `run_function`'s refusal order applies to everything after.
The task's `--arg` form and the semantics' positional form are the same
values spelled two ways. A refinement-typed parameter is checked at run
time (`refinement_violation` on failure, semantics S39). A `Bytes`
parameter has no constant-expression form (D8) and is supplied only as
`--arg {"$type": "Bytes", "hex": "..."}`.

From Milestone 2 a function with `using` parameters runs when the
capability of every `using` parameter is the `capability` of exactly one
`[bindings.*]` table whose backend this toolchain runs, in Milestone 2
`backend = "c"` with `"c"` in `[interop] targets`; semantics S78 owns the
refusal order and the order in which the handles are bound (in
`using`-parameter order, numbered from 0, before the arguments are
checked). Each handle is the binding's adapter (`spec/cabi.md` Section 6),
which the evaluator reaches through `cpl_cabi::adapter::CAdapter` in a
driver process, bound with result validation (semantics S77), and which
the driver grants in its own process under `--engine native`. Binding an
adapter builds the program's driver (Section 1.13), so it needs a C
compiler on every engine. `--engine` selects the engine (Section 7.2,
`--engine`).

The rows are in check order (semantics S78): the refusals and the decoding
of the arguments, then the build of every driver that a binding or the
engine needs, then the grants, then the call; under `--engine both` the
comparison follows the call, and a driver can die whenever it runs.

| Condition | Code | Exit |
| - | - | - |
| not found, ambiguous, not a function, or a generator | `index_unknown_symbol`, `index_ambiguous_symbol` (T59) / `usage_not_a_function` (T61) | 2 |
| function is generic | `cannot_run_generic` (semantics S39; the note names a monomorphic wrapper such as `paginate_ints`) | 2 |
| a `using` parameter whose capability has no available binding (semantics S78; Milestone 1 has none, so there every function with `using` parameters) | `cannot_run_effectful` (semantics S39, with S78's note: no `[bindings.*]` table binds the capability, or its binding's backend arrives in a later milestone) | 2 |
| wrong argument count, unparsable or ill-typed argument | `usage_bad_argument` (payload: `argument`, the 0-based index; `pointer`, the RFC 6901 position in a value; `key`, a duplicate key; `parameters` and `arguments`, the counts; `parse_code`, the parse code of a constant expression that does not parse) | 2 |
| no C compiler for a binding's driver or for `--engine native` or `both`; a driver that cannot be built | `build_cc_unavailable`; the failure as `cpl build` reports it (Section 7.4) | 2; `cpl build`'s |
| a binding's adapter cannot be granted (its `init` failed, a declarative `[state] create` returned NULL) | `interop_bind_failed` (`spec/cabi.md` Section 4), the adapter's diagnostic as a note | 1 |
| the call faults | the fault's diagnostic (Section 1.7.3), whatever its code, `internal_error` included (T61) | 1 |
| the call returns | the value | 0 (an `Err(...)` value is still a value) |
| the engines' outputs differ under `--engine both` | `engine_divergence` (Section 7.2 `--engine`) | 2 |
| the driver dies or breaks the protocol | `internal_error`, kind `native_driver_failed` (with `signal` or `exit_status`) or `native_protocol` | 2 |

Human output: the result in value text (semantics S40) on one line on
stdout: `Ok([[1, 2], [3]])`.

JSON (`RunOutput`):

```json
{ "command": "run", "…": "…", "status": "ok",
  "function": { "id": "reader.pages.paginate_ints", "path": "reader.pages.paginate_ints", "provisional": true, "facets": { "…": "…" }, "definition": "…" },
  "arguments": [[1, 2, 3], 2],
  "result": { "$type": "Result", "$variant": "Ok", "$payload": [[[1, 2], [3]]] },
  "result_text": "Ok([[1, 2], [3]])",
  "fault": null }
```

On a fault `result` and `result_text` are `null` and `fault` is the
diagnostic (also present in `diagnostics`). `arguments` is the value JSON
of every argument as `run_function` decoded it (`RunOutcome::arguments`),
each at its parameter's type, so the two spellings of one value report the
same JSON in its canonical form (`--arg -0` reports `0`, semantics S40),
on a fault too. From Milestone 2, a run that bound handles also carries
`"bindings": [{ "parameter": "codec", "binding": "image_codec", "handle":
0, "inputs": "<64 hex>" }]` (one entry per `using` parameter, in its
order; `inputs` is the blake3 of `path LF digest LF` for every file of
the binding that `c_input_digests` lists, Section 11.2, and every header
that its compilation read, `spec/cabi.md` 1.6, in path order, so that the
output names the native code it ran, T82), and a run under
`--engine native` or `both` carries `"engine"`; neither key is present
otherwise (`EngineRunOutput`, Appendix C), so the Milestone 1 output is
unchanged.

Decision: [D-257] T61. `cpl run` refuses a generator, a function whose
declared return type is a `Gen<T>` (the `std.gen` stubs: `cpl run
std.gen.int`), with `usage_not_a_function` (exit 2), as it refuses a
declaration that is no function: `run_function` reports it as
`RunError::NotAFunction`, before the generic and effect checks. A fault
raised while the call runs is a fault whatever its code, `internal_error`
included (exit 1, the `fault` of `RunOutput`); the exit 2 of
`internal_error` in 7.1 and Appendix B is that of a failure of the command
itself (a panic, a file that cannot be read or written, a standard library
that cannot be loaded). Rationale: a `Gen<T>` has no value outside a
property's `for` clause (semantics S14), so running a generator is a
request the command can refuse before evaluation, and an `internal_error`
raised during evaluation reports a runtime state the program reached,
which `cpl test` likewise reports as the failing test's fault. Rejected
alternatives: running it and reporting `internal_error` (kind
`generator_not_a_value`) as the run's fault (the earlier behaviour, which
blames a runtime state for a usage mistake); exit 2 for every
`internal_error` fault (`cpl run` and `cpl test` would then disagree about
one fault).

#### `cpl test [selector] [--json] [--jsonl] [--seed N] [--cases N] [--name TEXT] [--id ID] [--fail-fast] [--max-shrinks N] [--timeout-ms N] [--receipt PATH] [--engine eval|native|both]`

Checks the project (errors: exit 1 with the check's errors and warnings,
nothing runs, T58), selects tests, runs them (Section 1.7.2), and
reports.

| Argument | Meaning |
| - | - |
| `selector` | a module path (`tests.pagination`) or namespace wildcard (`tests.pagination.*`), both meaning the namespace: every test and property of that module and of every module below it (T43); anything else is `usage_bad_argument` (exit 2, with `selector`); absent: all |
| `--name TEXT` | only the test or property with exactly that name text (within the selector) |
| `--id ID` | only that stable id, generated or provisional (one that is no stable id is `usage_bad_argument`, with `id`) |
| `--seed N` | run seed (u64, decimal); default: 64 random bits from the OS, always reported |
| `--cases N` | cases per property, at least 1 (T57), default `[test] cases` (100) |
| `--max-shrinks N` | shrink budget per failure, default `[test] max_shrinks` (1000) |
| `--timeout-ms N` | wall-clock budget per body evaluation: each test, each property case, and each shrink rerun separately, and nothing else (a property's range setup and the runner's refinement checks of drawn values and shrink candidates run without a budget, Section 1.7.2); default `[test] timeout_ms` (0 = none); exceeding it is `resource_exhaustion` (`time`) |
| `--fail-fast` | stop after the first failing test |
| `--receipt PATH` | also write the test receipt (Section 8.2) to `PATH` (one that cannot be written is `internal_error`, kind `io`, exit 2) |
| `--engine E` | `eval` (default), `native`, or `both` (Milestone 2; Section 7.2 `--engine`); tests and properties bind no capability on any engine (semantics S30), so a receipt's `fixtures` stays `[]` |

An empty selection is `empty_test_selection` (exit 1, its `selector` the
`TestSelectorSpec` object of Appendix C), as the design requires for
integration selectors. Every test produces one `test_result`
diagnostic (`info` when tested, `error` when failed; semantics S46; no
span, so under T46 the `test_result` diagnostics stay in completion order,
`cpl_eval::test_result_diagnostic`), and the runner emits
`contract_counterexample` for violations observed in called declarations
(Section 1.7.3). `cpl-cli` fills each record's `receipt` (S59) before it
emits the diagnostics.

Human output, one line per test on stdout, then a summary:

```text
ok       tests.pagination."pagination handles empty input"
ok       tests.pagination."pagination preserves a short tail"
FAILED   tests.pagination."pagination preserves input"  (case 37; shrunk in 12 steps)
         error[contract_violation]: ensures clause failed: flatten(result) == items
           --> src/reader/pages.cpl:46:13
         shrunk: items = [0, 0, 0], size = 2
test: 2 tested, 1 failed; seed 1446357982114401307
```

These stdout lines are the text form of the `test_result` diagnostics
(`ok` or `FAILED` padded to nine columns, then the qualified name; a
failure adds the fault's two stable lines of 1.4.1 and, for a property, the
shrunk input in value text), printed as each test completes, so stderr does
not repeat them; every other diagnostic goes to stderr.

JSON (`TestReport` flattened, plus `receipt`):

```json
{ "command": "test", "…": "…", "status": "unsuccessful",
  "seed": 1446357982114401307, "cases": 100, "selected": 3, "tested": 2, "failed": 1,
  "results": [
    { "symbol": "tests.pagination.\"pagination handles empty input\"", "qualified_name": "tests.pagination.\"pagination handles empty input\"",
      "kind": "test", "status": "tested", "duration_ms": 0, "receipt": { "…": "semantics S59" } },
    { "symbol": "tests.pagination.\"pagination preserves input\"", "qualified_name": "tests.pagination.\"pagination preserves input\"",
      "kind": "property", "status": "failed", "duration_ms": 3,
      "seed": 1446357982114401307, "property_seed": "c0f6e2a9d4b17358a91c4e07bd2f6a53e8d1b2c9f04a7e6d3b5c8f1a2e4d7c90", "cases": 38,
      "failing_case": { "index": 37, "input": { "items": [7, -3, 0, 12, 5], "size": 2 } },
      "shrunk": { "input": { "items": [0, 0, 0], "size": 2 }, "shrink_steps": 12, "shrink_exhausted": false },
      "fault": { "code": "contract_violation", "category": "contract", "severity": "error", "clause_kind": "ensures", "…": "…" },
      "counterexamples": [ { "code": "contract_counterexample", "symbol": "pgn_7c2d", "facet": "body", "clause": "flatten(result) == items",
                             "input": { "items": [0, 0, 0], "size": 2 }, "actual": [[0, 0]], "status": "failed",
                             "found_by": "tests.pagination.\"pagination preserves input\"", "…": "…" } ],
      "receipt": { "…": "semantics S59" } } ],
  "receipt": { "…": "Section 8.2" } }
```

Record fields follow semantics S46 (`seed`, `property_seed`, `cases`,
`failing_case`, `shrunk`, `fault`, `receipt` present only where S46 lists
them; each record is one `test_result` diagnostic); `kind` and
`counterexamples` are additions. `status` is S50's test vocabulary,
`tested` or `failed`, never `passed` (T44); `property_seed` is the
64-hex-digit digest of Section 1.7 (T45). Exit: 1 if any `failed` or the
selection is empty; else 0.

Decision: [D-253] T57. `--cases N` and `[test] cases` are at least 1:
`--cases 0` is `usage_bad_argument` (exit 2) and `cases = 0` in `cpl.toml`
is `project_manifest_invalid` (T53, an integer out of range: "must be an
integer from 1 to 4294967295"); the library's `TestConfig::cases` still
accepts 0, under which a property runs no case. Rationale: a property run
with zero cases never evaluates its body, yet would be reported `tested`
and receipted as evidence, the vacuous evidence the design rejects (line
421: an empty selection is a configuration failure, never evidence;
semantics S46). Rejected alternatives: accepting 0 and reporting the
property `tested` (vacuous); reporting it `failed` (a configuration
mistake is no test failure).

#### `cpl run --engine` and `cpl test --engine` (Milestone 2)

`--engine eval|native|both` selects what evaluates the checked program.
The refusals of each command (T58, T59, T61, and `run_function`'s order,
shared through `prepare_run`, Section 1.7.4) come first and are the
evaluator's on every engine, so a refused command needs no C compiler.

| Engine | Behavior |
| - | - |
| `eval` (default) | the reference evaluator (Sections 1.7-1.7.3), the oracle; the envelope is Milestone 1's, with `bindings` after a run that bound handles |
| `native` | the program's driver (`cpl_emit_c::package::driver`, debug profile, cached by its key, Section 1.13) performs the evaluations through `cpl_cabi::NativeSession`; argument decoding, the runner's seeds, generation, and shrinking, rendering, and the envelope stay the evaluator's code, because native outcomes come back as `cpl_eval::Value`s and as `cpl_eval::Fault`s (`fault_from_record`) |
| `both` | the command runs on both engines and compares under semantics S75 (`cpl_cabi::compare`): `cpl run` its `RunOutput`, `cpl test` every `TestCaseRecord`, each after the S75 normalization of Section 12.6. Equal outputs give the evaluator's output with `"engine": "both"`; each differing output is one `engine_divergence` |

`engine_divergence` is an error of category `internal`, span-less, with
the payload `eval` and `native` (each that engine's output as compact
JSON) and `function` (`cpl run`) or `test` (the record's qualified name);
the envelope is then the evaluator's output with `"engine": "both"` and
status `error`, exit 2. A divergence is a defect of the toolchain, the
command's own internal error (T61), never a failure of the program.

Wire keys. `RunOutput`, `TestOutput`, `TestReport`, and `TestCaseRecord`
are unchanged; `cpl run` and `cpl test` serialize `EngineRunOutput` and
`EngineTestOutput` (Appendix C), which flatten them and add `engine`,
omitted under `eval`, and, for `cpl run`, `bindings`, omitted when no
handle was bound. A test receipt's `core.check_configuration`, an open
object (semantics S59), gains `"engine"` and `"native": {
"runtime_digest", "compiler_digest" }` under `native` and `both` only.
Every Milestone 1 golden therefore stays byte-identical, and
`cpl_diag::version::JSON_SCHEMA` stays 1.

Failures of the engine itself: no C compiler is `build_cc_unavailable`
(exit 2); a driver that cannot be built is reported as `cpl build` reports
the failure, with its exit code (Section 7.4); a driver that dies is
`internal_error`, kind `native_driver_failed` (with `signal` or
`exit_status`), and a malformed driver message kind `native_protocol`,
both exit 2. Under `cpl test`, `--timeout-ms` budgets the same
evaluations on both engines (Section 1.7.2), and a time-budget or
stack-guard fault compares as S75 sanctions, by rules 2 and 3 of the S75
normalization (Section 12.6).

Decision: [D-269] T73. `cpl run` and `cpl test` take `--engine
eval|native|both`, default `eval`: the native engine is a driver
subprocess whose outcomes the evaluator's code renders, `both` compares
under semantics S75 and reports a difference as `engine_divergence`
(category `internal`, exit 2), and the keys `engine` and `bindings` of an
output, and `engine` and `native` of a test receipt's
`core.check_configuration`, appear only for a non-default engine or a run
that bound handles, the output's through wrapper wire types. Rationale:
one selector keeps each command's contract, refusals, and goldens, and the comparison of `both` runs in the very
commands an agent uses; a divergence is the toolchain's defect, so it
takes the internal-error exit of T61, never the program-failure exit 1;
wrapper types with omitted keys keep every Milestone 1 struct and golden
byte-identical (T77). Rejected alternatives: a separate `cpl native run`
command (a second spelling of one action); `both` as the default (it
doubles every run's cost, and the differential suites are the gate,
Section 12.6); `engine_divergence` exiting 1 (it would read as a failure
of the program); an `engine` key in every output (every Milestone 1 golden
and struct literal would change).

#### `cpl inspect <symbol> [--include LIST] [--json] [--since REV]`

Section 6. `--include` takes the comma list of Section 6.2 (an unknown
or empty item, a space, or `interface_dependencies` is
`usage_bad_argument`, with `include`, exit 2). `--since REV` sets the base
for `changes` (default `HEAD~1`, `cpl_index::DEFAULT_SINCE`); without
`changes` in the include set it has nothing to set and is ignored (the
response's `include` shows the effective set). The command checks the
project in development mode, builds the index (`Index::from_check`), and
answers with `Index::inspect`; the target is resolved (T59) before the
`changes` base is read, so a target that names nothing exits 1 without
consulting git. With `changes`: no repository is `git_not_a_repository`,
no runnable `git` `git_command_failed`, a `--since` that does not resolve
`git_unknown_revision`, one without `cpl.toml` `git_no_project_at_revision`,
all exit 2. Human output (stdout) prints the same information as the JSON
in labelled sections: `revision`, `symbol <path>` with the target's fields
and facet hash table, then `signature`, `contract`, `effects`, `docs`,
`implementation`, `holes` (each hole's whole report), `interface
dependencies`, `dependencies`, `callers` (with call sites),
`obligations`, `tests`, and `changes since <commit>`, for what the set
includes, each nested symbol with its own fields and facet hash table.
Exit: 0 when the target resolves; 1 for `index_unknown_symbol` or
`index_ambiguous_symbol`; the project must check without errors for
callers/tests to be complete, but inspect still answers with a warning
`index_partial` when it does not, the one diagnostic of the check it
reports (T58).

#### `cpl prove <symbol> [--json]`

Checks the project (errors: exit 1 with the check's errors and warnings,
nothing is proved, T58), resolves the symbol by Section 6.1 (T59: one that
names nothing, or a bare name that names several, exit 2; a resolved one
that is no `fn` is `usage_not_provable`, exit 2), enumerates its proof
obligations (Section 8.1), asks the verifier, and emits a verification
receipt and one `verification_result` diagnostic (info; semantics S36)
for the symbol, whose `receipt` is the receipt's `core`
(`Index::prove`). The verifier (`cpl-prove`, `spec/verification.md`)
decides each obligation: `proved`, `failed` with the inputs that break it,
or `unknown` with a reason; the refinements the type checker established
stay `proved` (`{"by": "literal"}`) and a `termination` obligation
`unknown` (`no_measure`, 8.1). The function's `result` is `proved` when
every obligation is proved, termination is known, and no typed hole is
left (a function without obligations included), so the command exits 0;
otherwise it exits 1 (T29), for every function, `@prove` or not. A failed
`verification_result` carries `obligation` (the first failed id) and
`model`. It carries the note that termination is unknown as well whenever
semantics 5.4's conditions hold (5.5), for any function.

Human output (stdout; the `verification_result` on stderr), for
`examples/reader`'s `paginate`, whose ten obligations are listed in 8.1:

```text
prove reader.pages.paginate (pgn_7c2d): unknown
  10 obligations: 4 proved, 0 failed, 6 unknown
  receipt: 7e1c…  (blake3 of the receipt)
```

The receipt line holds the whole 64-hex-digit `receipt_digest` (Section
8), shortened here.

JSON (`ProveOutput`):

```json
{ "command": "prove", "…": "…", "status": "unsuccessful",
  "symbol": { "id": "pgn_7c2d", "path": "reader.pages.paginate", "…": "…" },
  "result": "unknown",
  "obligations": [ { "id": "pgn_7c2d#ensures/1", "kind": "ensures", "clause": "flatten(result) == items", "status": "unknown", "reason": "unsupported_expression" } ],
  "receipt": { "…": "Section 8.1" } }
```

`symbol` is the target's `SymbolVersion` (Section 1.4.2), not a symbol
block.

Decision: [D-166] T29. `cpl prove` exits 0 only when every obligation is `proved`;
`unknown` and `failed` both exit 1 (with `result` distinguishing them).
Rationale: a script that treats exit 0 as "verified" must never be
satisfied by an unknown result; the design insists that unknown does not
weaken a requirement. Rejected alternative: exit 0 for `unknown` (would
make Milestone 1 scripts pass vacuously).

#### `cpl diff <range> [--semantic] [--exit-code] [--json]`

Semantic diff (Section 9) between two revisions of the project. `--semantic`
is accepted for compatibility with the design's spelling and is the only
mode in Milestone 1 (there is no textual mode; use `git diff`).

| `<range>` | Old revision | New revision |
| - | - | - |
| `A...B` | `git merge-base A B` | `B` |
| `A..B` | `A` | `B` |
| `A` | `A` | the working tree |

The range splits at its first `...`, else at its first `..` (no ref name
contains `..`); a side left empty is `git_unknown_revision` (`A..` is no
shorthand for `A..HEAD`). The names are resolved in the order written,
before any tree is read.
Each named revision is resolved with `git rev-parse --verify <rev>^{commit}`;
failures are `git_unknown_revision` (exit 2): every failing exit status of
that command (git exits 1 for a name that is no object and 128 for one such
as `HEAD@{5}` past the end of the reflog or `@{upstream}` without one), and
a name that is empty or starts with `-`, which is refused without running
git (`cpl_index::resolve_rev`). For `A...B` the two names are resolved in
that order and the first unknown one is reported; two commits without a
common ancestor are `git_command_failed` (its `stderr` names them; the
message gives the command and its status). A revision's project tree is
materialized with Section 9.3 and loaded with `load_project_from_tree`; a
revision without `cpl.toml` at the project prefix is `git_no_project_at_revision`
(exit 2, with `revision` and `commit`; for the merge-base side of `A...B`,
which no name spells, `revision` is its commit id): `tree_at` returns an
empty tree for it, which loads as `LoadError::NoManifest`; a commit whose
`cpl.toml` or `cpl.lock` does not load is `project_manifest_invalid` or
`project_lock_invalid` with the same two fields (exit 2). Without a
repository the command is `git_not_a_repository` (`path: "."`), without a
runnable `git` `git_command_failed`. The envelope's `revision` is the
working tree's, as for every command run in a project (Section 6.3); the
two sides are `old` and `new`.

Human output: one statement per line, `<statement>  <path>  (<id>)`, with
the detail of a rename (`identity_renamed  reader.pages.paginate
(pgn_7c2d)  was reader.pages.pages_of`) or a move (`from <old module>`),
the references an effect statement adds or removes (`added db.put`), and
`kind changed` on the two statements of a kind change (`, was <old path>`
on its removal when the declaration was renamed too, both statements
being at the new path, 9.1); then one note per id declared more than once
in either revision. Statements are ordered by path then by the statement
order of Section 9.1; an empty diff prints nothing, as `git diff` does.

JSON (`DiffOutput`): Section 9.2. Exit: 0 whenever the diff was computed
(even if non-empty); `--exit-code` makes a non-empty diff exit 1, as in
`git diff`.

#### `cpl id assign [paths] [--check] [--json]`

Section 4.2. Human output: one line per assignment,
`src/reader/pages.cpl:21  @id("fn_k7w3qz2mb6ptx4ve")  reader.pages.clean`.

JSON (`IdAssignOutput`):

```json
{ "command": "id assign", "…": "…", "status": "ok",
  "check": false,
  "assignments": [ { "path": "src/reader/pages.cpl", "line": 21, "symbol": "reader.pages.clean", "kind": "fn", "id": "fn_k7w3qz2mb6ptx4ve" } ],
  "skipped_files": [ { "path": "src/broken.cpl", "reason": "syntax errors" } ] }
```

Exit: 0 when every declaration has an id afterwards; 1 with `--check` when
assignments are needed or when files were skipped for syntax errors.

#### `cpl work check <file> [--json]`

Parses `<file>` as a `.work` file, loads and checks the project, and
validates the declaration against it with the project's `[work.policy]`
(`cpl_work::validate_work_file_with_policy`, whose `WorkCheckOutput` is
the envelope's result); a project with check errors is not validated
(T58). A target's stable identity is the `Program`'s (`Def::stable_id`),
the key of the facet maps of Section 5.4, except that the later occurrence
of a duplicated `@id` carries its provisional identity (4.5). The file may be any path: a member of the source set is the
project's own parse of it, and any other file is read, registered beside
the project's files, and parsed on its own; it is shown as 7.1 shows a
named file, and a missing file or a directory is `usage_bad_argument`
(with `path`). Validation rules and their codes:

| Rule | Code | Severity |
| - | - | - |
| every `use` resolves (as in a `.cpl` file, semantics 1.6; the other import codes, `generator_import`, `duplicate_import`, `prelude_name_redefined`, and `import_conflicts_with_declaration`, are not reported, and a selector that fails through a `use` item that already failed is not reported again, semantics S62) | `unresolved_module`, `unresolved_name`, `private_item` | error |
| `base` and `target` present; `workers >= 1` | `work_missing_base`, `work_missing_target`, `work_bad_workers` | error |
| `base`/`target` refs resolve in the repository (skipped with the warning `git_unavailable` when there is no repository or no `git`: once per file, span-less, with `reason` `no_repository`, `not_a_repository`, or `not_installed`; this command is the only producer of that code; any other git failure is `git_command_failed`, with `change`, `item`, and `ref`); `target` names a branch, since the integrator advances it (grammar D35): `refs/heads/<target>`, or the target itself when spelled `refs/heads/...`, is a well-formed ref name that `git rev-parse --verify` resolves (10.7), so `HEAD`, a tag, a commit id, an expression such as `main~1`, and the spelling `heads/main` are refused | `work_unknown_ref`, `work_target_not_branch` | error |
| every `write`/`depend`/`preserve` selector resolves to at least one declaration or is a file selector (Section 10.3, T42; a capability operation `m.Cap.op` is no declaration, so such a selector names nothing); an `add` namespace is a well-formed module path not under `std` or `gen` that names no declaration (T67) and need *not* contain a module yet (it may be the namespace in which a task creates the first module, design line 353); a file selector is meaningful in a `write` set only: in `depend` and `preserve` it is valid and listed in the output but has no effect (Section 10.3) | `work_selector_empty` (error, with `selector` and `set`, and for an `add` namespace that names a declaration its `symbol` and `qualified_name`); `work_add_namespace_new` (warning: no module exists under the namespace); `work_file_selector_shadowed` (warning: a file selector's spelling also resolves to a declaration, which wins) | error / warning |
| every file selector of a `write` set is listed in `[work.policy] files` | `work_file_not_grantable` | error |
| every declaration named individually (by a declaration or facet selector; a namespace wildcard names a namespace, not its declarations) in a `write`, `depend`, or `preserve` set or in an integrate `prove`/`test` requirement carries an `@id` (Section 4.3, T47): one warning per declaration, at its first naming selector with a note at each further one, and none for a standard-library declaration, which carries no `@id` and is never in a tree the integrator assigns ids in | `work_provisional_identity` | warning |
| facet selectors name a facet applicable to the declaration kind (`Positive.body` is not) | `work_facet_not_applicable` | error |
| `identity` is never in a `write` or `add` set | `work_identity_not_writable` | error |
| `after` names tasks of the same change; the graph is acyclic | `work_unknown_task`, `work_cycle` | error |
| a change declares at most 4,096 tasks (the order and overlap rules compare every pair of tasks; past the limit they are not run) | `work_too_many_tasks` | error |
| `write` sets of unordered tasks (neither reaches the other through `after`) do not overlap at facet level (`.definition` counts as every applicable facet but `identity`; file selectors are not compared); `add` namespaces of unordered tasks do not overlap (T43, segment-wise); `ChangeRun::open` checks the same again when a change opens (Section 10.2) | `work_overlapping_writes` | error |
| `require test(sel)` matches at least one test: a namespace selects the tests in it (T43), a declaration selector selects `Index::tests_for` of that declaration, and a facet selector is `work_facet_not_applicable` (it still names its declaration individually); `require prove(sel)` names functions: a namespace selects the functions in it, and a selector that names nothing, or a declaration or namespace with no function, is `work_prove_not_function`; `require review(t)` names a task | `work_test_selector_empty`, `work_prove_not_function`, `work_unknown_task` | error |
| no facet is in both the `write` set and the `preserve` set of one task | `work_preserve_written` | error |
| a task has neither `write` nor `add` and is not named by a `review` requirement | `work_task_without_authority` | warning |

Each rule reports one diagnostic per offending item: one `work_cycle` per
cycle, one `work_overlapping_writes` per pair of tasks and kind of overlap
(facets, or namespaces), one `git_unavailable` per file; each spans the
offending selector, item, or change (`git_unavailable` has no span). The
payload fields are `change`, `item` (`base` or `target`), `ref`,
`commit`, `found` (the `workers` value), `selector`, `set` (`write`,
`depend`, `preserve`, or `add`), `file`, `namespace`, `namespaces`,
`kind`, `task`, `tasks`, `overlap`, and `reason`. Two rules of other
sections are not checked here: the fixed exclusions of Section 10.3 (a
selector that names a std declaration or the change's own work file
validates, and the grant subtracts it when the change opens) and the
project-wide uniqueness of change names (grammar D34).

Human output: diagnostics on stderr, then `work check: <file>: N changes,
M tasks, ok` on stdout (`..., K errors` in place of `ok` when the
declaration has errors).

JSON (`WorkCheckOutput`):

```json
{ "command": "work check", "…": "…", "status": "ok",
  "file": "work/pagination.work",
  "changes": [ {
    "name": "pagination_update", "base": "main", "base_commit": "3f2e…", "target": "main", "workers": 3,
    "tasks": [
      { "name": "implementation", "after": [],
        "write": [ { "selector": "paginate.body", "targets": [ { "id": "pgn_7c2d", "facet": "body" } ] } ],
        "add": [], "depend": [ "…" ], "preserve": [ "…" ],
        "goal": "Implement pagination and discharge its proof obligations." },
      { "name": "examples", "after": [], "write": [], "add": [ { "selector": "tests.pagination.*", "namespace": "tests.pagination" } ], "…": "…" },
      { "name": "audit", "after": ["implementation", "examples"], "write": [], "…": "…" } ],
    "integrate": [ { "require": "typecheck" }, { "require": "prove", "targets": [ { "id": "pgn_7c2d", "facet": "definition" } ] },
                   { "require": "test", "namespace": "tests.pagination", "matched": 3 }, { "require": "review", "task": "audit" }, { "require": "no_holes" } ] } ] }
```

A `require no_unresolved_assumptions` line is `{ "require":
"no_unresolved_assumptions" }`; a file selector in a `write` set is `{
"selector": "project.manifest", "file": "cpl.toml" }` (Section 10.3). Exit:
1 on any error, the check's included (T58); 2 if the file or the project
cannot be read, no project is located (`usage_no_project`), or the file is
missing or a directory (`usage_bad_argument`). Without a runnable `git`
the validator is handed the project root as its repository, so that the
warning's `reason` is `not_installed` rather than `no_repository`.

Decision: [D-263] T67. an `add` namespace `p.*` whose path names a
declaration in the file's scope (`double.*` through `use
app.core.{double}`, or `app.core.double.*`) and under which no module
exists is `work_selector_empty` (with the declaration's `symbol` and
`qualified_name` and `set: "add"`), not the warning
`work_add_namespace_new`; when a module exists under the path, the
namespace is that module's and is valid. Only the whole path is tested
(`app.core.double.sub.*` is `work_add_namespace_new` alone), resolved with
`cpl_sema::resolve_selector` like every other selector. An `add`
namespace under `std` or `gen`, or one that is not a module path, is
`work_selector_empty` too, the error code of the table's row. Rationale:
grammar D38 makes `*` a namespace wildcard and never a facet wildcard
("`paginate.*` is an error if `paginate` is a declaration"), so a
namespace written for every facet of a declaration is a mistake to report,
while semantics 2.4 lets a module `x.y` coexist with an item `y` of `x` and
resolves the chain to the module, as a `write` set's namespace does.
Rejected alternatives: the warning `work_add_namespace_new` for every new
namespace (an earlier behaviour, silent about the facet-wildcard
mistake); rejecting the namespace even when the module exists
(contradicts 2.4).

#### `cpl work run <file>` and `cpl work integrate <change>`

Reserved for Milestone 6 (Section 10). In Milestone 1 both exit 2 with
`usage_not_implemented` and a note naming the milestone; they need no
project, so their envelope's `revision` is `null` (T60). From Milestone 6, `cpl work integrate` requires
the integrator authority the host supplies with the invocation (Section
10.6: `work_not_authorized` otherwise, exit 2), because integration
authority belongs to the coordinator or an authorized actor, not to every
worker (design line 365).

#### `cpl build [--target c|python|rust|go|java] [--release] [--profile optimized|debug] [--sanitize LIST] [--artifact KIND]... [--out DIR] [--lock] [--cc PATH] [--python PATH] [--jobs N] [--json]`

Milestone 2. Builds the project's C package: checks the project, plans
its C surface (`CheckResult::interop`, Section 1.6), and runs
`cpl_emit_c::package::build` (Section 1.13). T58 applies to `cpl build` as
to the commands it names: any check error exits 1 with the check's errors
and warnings, never its infos, and nothing is built; under `--release` the
check runs in release mode. The code generated, its flags and profiles,
the compile and link steps, the artifacts, and the version script are
`spec/cabi.md` Section 1's; this section owns the command.

| Flag | Effect |
| - | - |
| `--target c\|python\|rust\|go\|java` | `c` (the default) builds the C package; from Milestone 3 `python` builds the Python package of `spec/python.md` Section 1 (default output root `<project>/.cpl/build/python`); `rust`, `go`, and `java` (`kotlin` and `jvm` name `java`) write the host package of `spec/hosts.md` Section 1 (default output root `<project>/.cpl/build/<target>`), compile nothing, and refuse `--artifact`, `--sanitize`, and `--lock` as `usage_bad_argument`; `node` is `usage_not_implemented` (exit 2, a note naming Milestone 4) until Milestone 4, and any other value `usage_bad_argument`; a project without the target in `[interop] targets` is `build_target_not_enabled` (exit 1, with `targets`) |
| `--release` | the release check of `cpl check --release` (semantics S47: `hole_in_release`, `verification_unsatisfied`, `project_lock_stale`, T37 and T75), with `build_lock_missing` (error) in place of its warning `project_lock_missing`, decided with the check and so reported beside its errors; after a successful build, the comparisons of Section 11.2 (`build_lock_mismatch`, `build_abi_changed`); the library records `release = 1`. It is release policy, never an optimization switch |
| `--profile optimized\|debug` | the flag sets of cabi.md Section 1; default `optimized` |
| `--sanitize LIST` | comma-separated `address` and `undefined` (together or alone), or `thread` alone; any other name or combination is `usage_bad_argument`: the sanitizer flag sets of cabi.md Section 1, at compile and link time; one that the compiler cannot link (`Compiler::supports`) is `build_sanitizer_unavailable` (exit 2, with `sanitizer`) |
| `--artifact shared\|static\|executable` | repeatable; replaces `[interop.c] artifacts` |
| `--out DIR` | the output root; default `<project>/.cpl/build/c`; a directory that cannot be created or written is `internal_error`, kind `io` (exit 2) |
| `--lock` | after a successful build, write `cpl.lock`: the tables `cpl check --lock` writes and the C entries of Section 11.2, unless that would record an ABI break (T81) |
| `--cc PATH` | the C compiler, first in T72's precedence |
| `--python PATH` | Milestone 3: the Python interpreter of `--target python` (its headers and version) and of the Python workers that `cpl run` and `cpl test` start; else `CPL_PYTHON`, else `python3` on `PATH` (`spec/python.md` 1, 6.5); one that is not CPython 3.10 or later with `Python.h` is `build_python_unavailable` (exit 2, with `interpreter` and `reason`) for `--target python` |
| `--jobs N` | parallel compilations, at least 1 (`--jobs 0` is `usage_bad_argument`); default the number of CPUs |
| `--json` | one envelope with the `BuildOutput` below |

Compiler and environment (T72). The compiler is the first of `--cc`,
`[interop.c] compiler`, the environment variable `CPL_CC`, and `cc` on
`PATH` (`cpl_emit_c::cc::Compiler::discover`); `$CC`, `CFLAGS`, and
`LDFLAGS` are never read. None found is `build_cc_unavailable` (exit 2,
with `searched`); a family other than gcc or clang, gcc before 9, or clang
before 12 is `build_cc_unsupported` (exit 2, with `found` and `minimum`).
Every compiler and linker process runs with an environment from which
`CPATH`, `C_INCLUDE_PATH`, `CPLUS_INCLUDE_PATH`, `LIBRARY_PATH`,
`COMPILER_PATH`, `GCC_EXEC_PREFIX`, `DEPENDENCIES_OUTPUT`,
`SUNPRO_DEPENDENCIES`, and `CCC_OVERRIDE_OPTIONS` are removed, so search
paths come from the manifest alone; the other tools a build runs, `ar` and
`objcopy` for the static library (`spec/cabi.md` 1.6), are the ones the
compiler names (`-print-prog-name=ar`, `-print-prog-name=objcopy`), run in
the same environment. The compiler's target (`-dumpmachine`)
is checked against the platform matrix of cabi.md Section 1: a target
outside it is `build_target_unsupported` (exit 2, with `target`), and a
target of its second tier gets the warning `build_target_untested`.

Steps, in this order: the check (the release check under `--release`,
with `build_lock_missing`), so that a project with check errors reports
them whatever its targets; the target (`build_target_not_enabled`); the
interop plan, whose `project_export_skipped_generic` infos (Section 11.1)
`cpl build` alone reports; compiler discovery and the target check;
emission; compilation, each object cached in `<cache>/obj/`, `<cache>` the per-user `$XDG_CACHE_HOME/cpl/<project key>/c` (else `$HOME/.cache/cpl/<project key>/c`; `<project key>` the first 32 hex digits of the blake3 of the project root's canonical path)
and reused by the rule of `spec/cabi.md` 1.6 (never while a header that
its depfile lists has changed); linking; `build.json`; under `--release`
the lock comparisons; with `--lock` the lock write. A build whose every input matches the cache reuses its outputs
and reports `cached: true`.

Layout:

```text
<out>/                                   default <project>/.cpl/build/c; caches in the per-user cache (above)
├── include/
│   ├── cpl.h                            C API 0.1, identical in every package (spec/cabi.md Appendix A)
│   ├── cpl_adapter.h
│   ├── <prefix>.h                       the package header (cabi.md Section 5)
│   └── <prefix>_adapter_<binding>.h     one per reviewed-wrapper binding (cabi.md Section 6)
├── src/
│   ├── cpl_rt.c, cpl_rt*.h              the runtime amalgamation behind its prologue, its headers, its generated tables (cabi.md Section 2.2)
│   ├── cpl_rt_exe.c                     with the executable artifact
│   ├── <prefix>_prog.c                  every emitted function, the literal constants, the entry table CPLPKG(code)
│   ├── <prefix>_tables.c                CPLPKG(program): definitions, sites, clauses, binding sites, holes, refinements, types, stub facts, payload-less variants, the export and grant tables, the library information
│   ├── <prefix>_api.c                   exports, the runtime and library functions, type accessors, capability constructors, grants
│   ├── <prefix>_bind_<binding>.c        one per declarative binding (cabi.md Section 6)
│   └── <prefix>.map                     the ELF version script
├── obj/                                 objects and -MD depfiles
├── lib/
│   ├── lib<prefix>.so.<abi>, lib<prefix>.so -> lib<prefix>.so.<abi>
│   └── lib<prefix>.a
├── bin/<prefix>                         with the executable artifact (cabi.md Section 8)
└── build.json                           the BuildOutput below
```

The units of `src/` and their contents are `spec/cabi.md` 1.6's; the tree
above names them for orientation.

A host may link `lib/`, or vendor the package by compiling its units of
`src/` with the adapters and vendored sources that `build.json` lists
under `inputs`, as `spec/cabi.md` 1.6 states (which units, the C11
compiler, and the flags the units depend on, `-fno-strict-aliasing` among
them). `.cpl/build` and the per-user cache are disposable execution state
(design line 40), outside the source set. The cache is the user's, never
the project's: a repository could commit cache entries (a compiled object,
a driver) that a build would link or run without any source review seeing
them, so objects and drivers are cached under `$XDG_CACHE_HOME/cpl/<project
key>/c` (an absolute `XDG_CACHE_HOME`; else `$HOME/.cache/cpl/<project
key>/c`; with neither, a fresh private directory per process, so nothing is
reused), `<project key>` the first 32 hex digits of the blake3 of the
project root's canonical path. The default output directory
`.cpl/build/<target>` is written only when no component below the project
root on the way to it is a symbolic link (`build_target_unsupported`
otherwise), and `build.json`, a package's files, and `cpl.lock` are written
by renaming a fresh temporary file over the old one, which replaces a link
there instead of writing through it.

JSON (`BuildOutput`, flattened into the envelope; also `<out>/build.json`):

```json
{ "command": "build", "toolchain": { "…": "…" }, "revision": { "…": "…" }, "status": "ok",
  "diagnostics": [ { "code": "project_export_skipped_generic", "category": "project", "severity": "info",
                     "message": "…", "function": "reader.pages.paginate" } ],
  "target": "c", "release": false, "profile": "optimized", "sanitize": [], "out_dir": ".cpl/build/c",
  "compiler": { "path": "/usr/bin/x86_64-linux-gnu-gcc-11", "family": "gcc", "version": "11.4.0",
                "target": "x86_64-linux-gnu", "digest": "<64 hex>" },
  "runtime": { "version": "0.1.0", "api": "0.1", "source": "embedded", "digest": "<64 hex>" },
  "program_digest": "<64 hex>", "exports_digest": "<64 hex>",
  "exports": [ { "symbol": "reader_image_width", "function": "reader.images.image_width", "id": "reader.images.image_width",
                 "signature": "<64 hex>", "int": "handle", "effects": ["codec.dimensions"] },
               { "symbol": "reader_load_page", "function": "reader.pages.load_page", "id": "reader.pages.load_page",
                 "signature": "<64 hex>", "int": "handle", "effects": ["db.get"] },
               { "symbol": "reader_load_text", "function": "reader.pages.load_text", "id": "reader.pages.load_text",
                 "signature": "<64 hex>", "int": "handle", "effects": ["db.get"] },
               { "symbol": "reader_paginate_ints", "function": "reader.pages.paginate_ints", "id": "reader.pages.paginate_ints",
                 "signature": "<64 hex>", "int": "handle", "effects": [] } ],
  "bindings": [],
  "artifacts": [ { "kind": "header", "path": ".cpl/build/c/include/cpl.h", "digest": "<64 hex>" },
                 { "kind": "header", "path": ".cpl/build/c/include/cpl_adapter.h", "digest": "<64 hex>" },
                 { "kind": "header", "path": ".cpl/build/c/include/reader.h", "digest": "<64 hex>" },
                 { "kind": "static_library", "path": ".cpl/build/c/lib/libreader.a", "digest": "<64 hex>" },
                 { "kind": "shared_library", "path": ".cpl/build/c/lib/libreader.so.0", "digest": "<64 hex>" } ],
  "inputs": [],
  "holes": 0,
  "lock": { "checked": false, "written": false },
  "cached": false,
  "duration_ms": 812 }
```

| Key | Meaning |
| - | - |
| `profile`, `sanitize` | the flag sets in effect (`sanitize` in the order `address`, `undefined`, `thread`) |
| `out_dir`, `path` | project-relative when under the project, else absolute (Section 7.1); `compiler.path` is the resolved driver binary |
| `runtime` | `version` is `cpl_rt::RUNTIME_VERSION`, `api` is `cpl_rtspec::limits::C_API_VERSION` as `"<major>.<minor>"`, `source` is `"embedded"` or `"dir"` (`CPL_RUNTIME_DIR`), `digest` is `cpl_rt::runtime_digest` (Section 1.12) |
| `exports` | one entry per export symbol in export order (T74): its target, identity, ABI digest (the `signature` of cabi.md Section 9), integer mapping (`handle`, `int64`, `int32`, `uint64`, `uint32`), and declared effect bound as sorted `param.op` strings |
| `bindings` | one entry per backend-`c` binding in name order: `{ "binding", "capability", "form" ("wrapper" or "declarative"), "isolation", "grant" }`, `grant` the C function a host calls to obtain the adapter (`cboundary_bind_image_codec`); empty for the verbatim `reader`, whose one binding is backend `python` |
| `artifacts` | every file the build produced for a host, in path order: `kind` `header` (each file of `include/`), `shared_library`, `static_library`, or `executable` |
| `inputs` | every build input (Section 0) in path order: `{ "kind" ("adapter" or "vendored_source"), "binding", "path", "digest" }` |
| `holes` | the unresolved holes linked into the package (0 under `--release`, semantics S47) |
| `lock` | `checked`: the comparisons of Section 11.2 ran (`--release`); `written`: `cpl.lock` was written (`--lock`) |
| `cached`, `duration_ms` | whether the outputs were reused from the cache; the time spent. `duration_ms` and the compiler's `path`, `version`, `digest`, and `target` are volatile in goldens (Section 12.4) |

On a failure the envelope carries no `BuildOutput` keys (`NoOutput`,
Section 1.4.2), only the diagnostics. Human output: the diagnostics on
stderr; on stdout one line per artifact, `<kind>  <path>`, then the summary
`build: <prefix> (c, <profile>[, release][, sanitize <list>]): <n>
exports, <m> bindings, <k> artifacts in <out_dir>`. Exit: Section 7.4.

Decision: [D-268] T72. `cpl build` is the one command that compiles: it
builds the C package of the checked program under the fixed flags of
cabi.md Section 1, takes the compiler from `--cc`, `[interop.c]
compiler`, `CPL_CC`, or `cc` on `PATH` and never from `$CC`, removes the
compiler's search-path variables from the environment of every compiler
and linker process, writes the layout above outside the source set, and
records everything it used in `build.json` and, on request, the lock; an
environment that cannot do what was asked (no or an unsupported compiler,
target, or sanitizer) exits 2, while a failure of the project's own C
inputs (an adapter, a vendored source, a native library) exits 1.
Rationale: implicit environment inputs are the hidden context the design
removes (T35's reasoning), and a build that names and records its compiler
can be reproduced and pinned (design line 598: the lock record includes
the compiler and the target ABI; line 600: build scripts execute as
controlled build steps); disposable outputs outside the source set keep
revisions and their digests independent of builds. Rejected alternatives:
reading `CC`, `CFLAGS`, and `LDFLAGS` implicitly; the `cc` crate (T71);
building as a side effect of `cpl check` (a semantic query would run build
steps, against line 600).

### 7.3 Encoding of CPL values

Two canonical renderings exist, both fixed by `spec/semantics.md` S40 and
implemented once in `cpl_eval::json` (`value_to_json`, `value_from_json`,
`value_text`).

*Value text* (human output of `cpl run`, `message` strings, `--arg`-free
positional arguments) is CPL literal syntax: `Int` in decimal, `Text` as a
`TEXT` literal with the grammar's escapes, `Bool` and `Unit` as their
constructors, lists as `[a, b]`, structs as `Page { id: PageId("p"), text:
"t" }`, variants as `Missing(PageId("p"))` / `Unavailable`, wrappers as
`PageId("p")`, `Bytes` as `Bytes("<lowercase hex>")`, and a capability
handle as `<handle N: qualified.Cap>`. Value text is a display form and is
not guaranteed to re-parse as a constant expression (`Bytes("…")` has no
literal form, D8); value JSON is the round-trip form.

*Value JSON* (`--arg`, `result`, `input`, `actual`, `expected`,
`bindings`, `failing_case`, `shrunk`, receipts) is type-directed in both
directions:

| CPL type | JSON | Notes |
| - | - | - |
| `Int` | number | arbitrary digits (`serde_json` `arbitrary_precision`); never a string; decoding rejects fractions and exponents |
| `Bool` | `true` / `false` | |
| `Text` | string | decoding rejects unpaired surrogates (`\ud800`) |
| `Unit` | `{ "$type": "Unit" }` | |
| `Bytes` | `{ "$type": "Bytes", "hex": "<lowercase hex>" }` | |
| `List<T>` | array | |
| struct | `{ "$type": "<qualified name>", "<field>": ..., ... }` | keys sorted, like every JSON map (T4: JSON objects are unordered, so this is the same value, and value text keeps declaration order); decoding requires every field, and a duplicate key is the CLI reader's to reject (Section 7.2), since `value_from_json` receives a parsed value |
| enum (including `Option`, `Result`) | `{ "$type": "<qualified name>", "$variant": "<Variant>", "$payload": [...] }` | `$payload` omitted for payload-less variants, and decoding rejects it there (an empty array included); `Some(Some(1))` nests two tagged objects |
| nominal wrapper | `{ "$type": "<qualified name>", "$payload": [v] }` | `PageId("p1")` is `{ "$type": "reader.pages.PageId", "$payload": ["p1"] }` |
| refined `Int` | as `Int` | `value_from_json` decodes at the root base type and does not evaluate predicates (it has no evaluator); `cpl_eval::json::refined_positions` lists every refinement-typed position of a decoded value (RFC 6901 pointer, refinement, value) in the order of semantics S74 (siblings in value order, the positions inside a refinement's base before the refinement), and `run_function` checks each one in that order (`refinement_violation`, S39); any other decoder of arguments does the same (`usage_bad_argument`) |
| capability handle | `{ "$type": "<capability qualified name>", "$handle": 3 }` (`{ "$handle": 3 }` from the untyped encoder) | output only; never decodable |
| generic parameter | encoded by its value (a hole's `bindings` and the `input` of a violation in a generic function are encoded that way) | not decodable: `cpl run` refuses generic functions |

Decoding is strict: an object key the row does not name is rejected,
`Unit` must be exactly `{ "$type": "Unit" }`, and `Bytes` hex must be
lowercase with an even number of digits; a decoding error is an
`ArgError` whose `pointer` is the RFC 6901 position of the offending value
(`/1/id/$payload/0`). The value encoders and the value-text renderer, like
dropping a value, run without recursion (Section 1.7, `stack`).

Decision: [D-167] T30. `$type` carries the qualified name for project and `std`
declarations (`reader.pages.PageId`, `std.foreign.ForeignError`) and the
bare prelude name for prelude types (`Unit`, `Option`, `Result`,
`ConstraintError`). Rationale: S40 fixes the `$`-tagged shape but not the
spelling of prelude type names; the bare name matches how the design's
text and every diagnostic refer to them, and the `$` prefix cannot collide
with a `LOWER_IDENT` field. Rejected alternative: `std.prelude.Option`
(correct but noisy in every counterexample).

### 7.4 Exit code summary

| Command | 0 | 1 | 2 |
| - | - | - | - |
| `check` | no errors | errors (or warnings with `--deny-warnings`) | project unreadable, `cpl.lock` not writable (`--lock`) |
| `fmt` | all files formatted / unchanged | `--check` differences, or syntax errors | bad paths, `--stdin` with paths, `--stdin --json` without `--check`, a file not writable |
| `run` | returned a value | check errors, or a fault (whatever its code, T61); `interop_bind_failed` | unknown or ambiguous symbol (T59), not a function or a generator (T61), generic, a `using` capability without an available binding (semantics S78), bad arguments; `build_cc_unavailable`, `engine_divergence`, `internal_error` kinds `native_driver_failed` and `native_protocol` |
| `test` | all selected tested | failures, empty selection, check errors | bad flags (`--cases 0`, T57), a `--receipt` path not writable; `build_cc_unavailable`, `engine_divergence`, `internal_error` kinds `native_driver_failed` and `native_protocol` |
| `build` | built, or reused from the cache | check errors; release-policy failures (`hole_in_release`, `verification_unsatisfied`, `project_lock_stale`, `build_lock_missing`, `build_lock_mismatch`, `build_abi_changed`); the `project_*` errors of the interop plan; `build_target_not_enabled`; `build_adapter_compile_failed`, `build_native_dependency_missing`, `build_link_failed`; `build_abi_changed` under `--lock` (T81) | usage errors, `usage_not_implemented` (`--target python` or `node`); `build_cc_unavailable`, `build_cc_unsupported`, `build_target_unsupported`, `build_sanitizer_unavailable`; an output or `cpl.lock` that cannot be written (`internal_error` kind `io`); emitted or runtime C that the compiler rejects (`internal_error` kind `emitted_c_rejected`, with the compiler's `stderr`: a toolchain defect) |
| `inspect` | resolved | unknown or ambiguous symbol | bad include, a `changes` base that cannot be read (`git_*`) |
| `prove` | all obligations proved | any `unknown` or `failed`, check errors | unknown or ambiguous symbol (T59), not a function |
| `diff` | computed (`--exit-code`: empty) | `--exit-code` and non-empty | bad range, no repository or no `git`, a revision without a project or with a manifest that does not load |
| `id assign` | nothing left unassigned | `--check` found work, or skipped files | bad paths, a file not writable |
| `work check` | valid | validation errors, check errors, or a file that does not parse | unreadable, no project, a missing file or a directory |
| `work run`, `work integrate` | - | - | not implemented |

Every command also exits 2 for a command line clap refuses
(`usage_bad_argument`), a missing project (`usage_no_project`), a git
failure (7.1), and an internal error of its own (7.1). From Milestone 2,
`cpl run` and `cpl test` report a driver that cannot be built with the
exit code `cpl build` gives that failure.

## 8. Receipts

A receipt is the durable evidence record of one check. Receipts are JSON
documents produced by `cpl-index::receipt` from inputs every command
already has; they are embedded in `--json` output (`cpl prove`, `cpl
test`), written with `--receipt PATH`, and stored by the integrator
(Section 10.6). Two receipts with equal `receipt_digest` are the same
evidence.

Common fields of every receipt:

| Key | Type | Meaning |
| - | - | - |
| `receipt` | `"verification" \| "test"` | kind |
| `schema` | integer | `cpl_diag::version::JSON_SCHEMA` |
| `toolchain` | `ToolchainInfo` | compiler version and edition |
| `revision` | `Revision` | source revision checked |
| `inputs` | object | `manifest` (Digest), `lock` (Digest or null; the lock is not a source-set member, Section 0), `std` (Digest), `sources` (object: path to Digest for every source-set file, `cpl.toml` included) |
| `core` | `ReceiptCore` | the receipt object of semantics S59, equal to the `receipt` field of the corresponding `verification_result` or `test_result` diagnostic: `source_revision`, `dirty` (only when true), `toolchain: { cpl, edition, std }`, `modules` (for a test receipt the union over the selected tests), `check_configuration`, `assumptions`, and for a test receipt `fixtures` and `environment`; the other fields of a receipt restate parts of it in the toolchain's own wire types and add the rest |
| `produced_at` | RFC 3339 string | informational; excluded from the digest; `cpl-index` sets it to the current UTC time, in whole seconds |
| `duration_ms` | integer | informational; excluded from the digest; `cpl-index` writes 0 and the caller sets it once the check is timed |

`receipt_digest` = blake3 of the receipt serialized with every object's
keys sorted by their UTF-8 bytes, no whitespace, and the two informational
fields removed from the top level only (keys of those names deeper down
are evidence and stay); strings and numbers are written as `serde_json`
writes them (integers digit for digit). This is not RFC 8785 (JCS): JCS
sorts by UTF-16 code units, which differs from UTF-8 byte order only for
keys that mix U+E000-U+FFFF with characters above U+FFFF, and a number read
back from stored JSON keeps its spelling, so `1e3` and `1000` digest
differently.

### 8.1 Verification receipt

Obligations of a function `f` with a body (`Index::obligations`; a
`@builtin` stub's clauses are axioms about it, semantics S61, and no other
kind has obligations), each with an id that is stable across runs for an
unchanged definition, listed in the order of this table and numbered by
T62:

| Obligation id | kind | Meaning |
| - | - | - |
| `<id>#ensures/<i>` | `ensures` | the i-th `ensures` clause (1-based, canonical order) holds on return |
| `<id>#decreases` | `decreases` | the function's measure is nonnegative and strictly decreases on recursive calls |
| `<id>#termination` | `termination` | present instead of `decreases` for a recursive function (`Index::is_recursive`) without a measure: clause `""`, located at the function's name, status `unknown` with reason `no_measure` |
| `<id>#loop/<n>/invariant/<i>/init` | `invariant_init` | invariant i of the n-th loop (body order) holds on entry |
| `<id>#loop/<n>/invariant/<i>/preserve` | `invariant_preserve` | invariant i holds after each iteration |
| `<id>#loop/<n>/decreases` | `loop_decreases` | loop measure nonnegative and decreasing |
| `<id>#call/<n>/requires/<i>` | `call_requires` | the i-th precondition of the callee at the n-th call site (T62: the call sites of the function's clauses, then of its body, every call counted; the callee's clauses in canonical order), located at the call |
| `<id>#div/<n>` | `division` | the divisor of the n-th `/` or `%` (T62) is nonzero (stdlib S8, semantics 5.1); clause `<divisor> != 0` |
| `<id>#refine/<n>` | `refinement` | the n-th `Refine` coercion's predicate (statically established ones carry status `proved` with evidence `{"by": "literal"}`, S16) |
| `<id>#assert/<n>` | `assert` | the n-th `assert` statement, located from its `assert` keyword |

```json
{
  "receipt": "verification",
  "schema": 1,
  "toolchain": { "version": "0.1.0", "edition": "0.1", "json_schema": 1 },
  "verifier": { "name": "cpl-prove", "version": "0.1.0", "configuration": { "theory": "linear_integer_arithmetic" } },
  "revision": { "commit": "3f2e…", "state": "clean", "digest": "9a0b…" },
  "core": { "source_revision": "3f2e…", "toolchain": { "cpl": "0.1.0", "edition": "0.1", "std": "…" },
            "modules": [ { "module": "reader.pages", "content_hash": "…" } ],
            "check_configuration": { "release": false }, "assumptions": [] },
  "target": { "id": "pgn_7c2d", "path": "reader.pages.paginate", "provisional": false, "facets": { "…": "…" }, "definition": "…" },
  "dependencies": [ { "id": "reader.pages.Positive", "path": "reader.pages.Positive", "provisional": true, "facets": { "…": "…" }, "definition": "…" },
                    { "id": "std.prelude.flatten", "path": "std.prelude.flatten", "provisional": true, "…": "…" } ],
  "inputs": { "manifest": "…", "lock": null, "std": "…",
              "sources": { "cpl.toml": "…", "src/reader/images.cpl": "…", "src/reader/pages.cpl": "…", "src/tests/pagination.cpl": "…", "work/pagination.patch": "…", "work/pagination.work": "…" } },
  "configuration": { "release": false, "policy": { "prove": true }, "proof_timeout_ms": 10000, "admitted": [] },
  "status": "unknown",
  "obligations": [
    { "id": "pgn_7c2d#ensures/1", "kind": "ensures", "clause": "flatten(result) == items",
      "location": { "file": "src/reader/pages.cpl", "line": 46, "col": 5, "end_line": 46, "end_col": 37 },
      "status": "unknown", "reason": "unsupported_expression", "evidence": { "candidate": { "items": [], "size": 1 } }, "assumptions": [] } ],
  "assumptions": [],
  "produced_at": "2026-10-27T21:00:00Z",
  "duration_ms": 4
}
```

| Field | Values |
| - | - |
| `status` | the verifier's result (`spec/verification.md`): `proved` (every obligation proved, termination known, no typed hole, and no assumptions), `conditional` (every obligation proved, at least one named assumption), `failed` (some obligation failed), `unknown` (otherwise, with the reason of the first unknown obligation, `no_measure`, or `unresolved_hole`); a function with no obligation is `proved` |
| `obligations[].status` | `proved \| failed \| unknown` |
| `obligations[].reason` | for `unknown`: `unsupported_expression`, `invariant_too_weak`, `nonlinear_arithmetic`, `integer_reasoning`, `limit`, `indirect_recursion`, `has_errors`, `no_measure` (`spec/verification.md`); null otherwise. A proof that relies on a holed callee's contract is not `unknown`: it is `proved` with an assumption of kind `hole`, making the receipt `conditional` (design line 225) |
| `obligations[].evidence` | for `failed`: `{ "model": {...}, "concrete": true }`, the inputs (value JSON by parameter name) on which the obligation fails, reported on the `verification_result` as `obligation` and `model`; for `proved`: `{"by": "linear_arithmetic", "paths": n}`, `{"by": "unreachable", "paths": 0}`, `{"by": "literal"}`, or `{"by": "bounded_constant_evaluation"}`; for `unknown`: `null` or `{"candidate": {...}}`, inputs the verifier could not confirm as a counterexample |
| `assumptions[]` | `{ "id": "asm_<n>", "kind": "unchecked_callee" \| "unproved_dependency" \| "hole" \| "foreign_adapter", "symbol": id, "facet": facet or null, "description": string, "admitted": bool }`; obligations reference assumptions by id. The first three kinds are assumptions about CPL implementations; `foreign_adapter` is a capability operation's contract, always named and never grounds for rejection (design line 136). `admitted` is true when `symbol` is listed in `[verification] admit`, the design's "explicitly admitted by project policy" |
| `dependencies` | the target's dependencies (interface and body) plus, from Milestone 5, every implementation the proof inlined and every external specification used, each with its facet versions |
| `configuration.admitted` | `[verification] admit` ids that were in effect |

How the status is consumed: `cpl check --release` rejects `unknown` and
`failed` (`verification_unsatisfied`, T36); an integrate `prove(sel)`
requirement accepts `proved` and `conditional` (a conditional proof is
closed relative to assumptions that stay named in the receipt and the
commit trailers); `no_unresolved_assumptions` additionally rejects every
assumption about a CPL implementation that is not admitted (Section 10.6,
grammar D36). `conditional` is never a weakening of `@prove`: its
obligations are all proved.

The verifier decides every obligation except `refinement` (the type
checker's) and `termination` (no measure to state). `examples/reader`'s
`paginate` has ten: `ensures/1` to `ensures/3`, `loop/1/invariant/1/init`
and `/preserve`, `loop/1/invariant/2/init` and `/preserve`,
`loop/1/decreases`, and `call/11/requires/1` and `call/12/requires/1` (the
`n >= 0` of `take` and `drop`, the eleventh and twelfth call sites of its
clauses and body). The two call preconditions, the loop measure, and the
second invariant's initialization are proved; the others compare lists or
quantify over them and are `unknown` (`unsupported_expression`), so the
status is `unknown`.

Decision: [D-258] T62. obligations are numbered over one walk of the
function: its `requires`, `ensures`, and `decreases` clauses in canonical
order, then its body, each in pre-order (an outer call before the calls in
its arguments). The `<n>` of `call/<n>` counts every call site of the walk,
a member call or property access (`rest.len`, `xs.take(1)`) and a
capability operation call included, whether or not its callee has a
precondition; the `<n>` of `div/<n>`, `refine/<n>`, `assert/<n>`, and
`loop/<n>` counts its own kind in the same walk; `<i>` counts a callee's,
a loop's, or the function's own clauses in canonical order. Rationale: an
id must be stable for an unchanged definition and depend on the
function's own text alone, so it cannot depend on which callees have
preconditions, and a call in a clause has preconditions that the clause's
evaluation relies on exactly as a division there has a divisor
(semantics 5.1); canonical clause order keeps the ids stable when clauses
are reordered (grammar D66). Rejected alternatives: counting only calls
whose callee has a precondition (an edit to a callee's contract would
renumber its callers' obligations); numbering the calls of the body only
(a clause's calls would have no obligation, though its divisions do).

### 8.2 Test receipt

```json
{
  "receipt": "test",
  "schema": 1,
  "toolchain": { "…": "…" },
  "revision": { "…": "…" },
  "inputs": { "…": "as above" },
  "core": { "source_revision": "3f2e…", "toolchain": { "cpl": "0.1.0", "edition": "0.1", "std": "…" },
            "modules": [ { "module": "reader.pages", "content_hash": "…" }, { "module": "tests.pagination", "content_hash": "…" } ],
            "check_configuration": { "cases": 100, "max_shrinks": 1000, "max_call_depth": 10000, "seed": 1446357982114401307 },
            "assumptions": [], "fixtures": [], "environment": { "os": "linux", "arch": "x86_64" } },
  "configuration": { "seed": 1446357982114401307, "cases": 100, "max_shrinks": 1000, "timeout_ms": 0,
                     "selector": { "namespace": ["tests", "pagination"], "name": null, "id": null },
                     "check_contracts": true, "fail_fast": false },
  "tested": [ { "id": "pgn_7c2d", "path": "reader.pages.paginate", "…": "…" }, { "id": "std.prelude.flatten", "path": "std.prelude.flatten", "provisional": true, "…": "…" } ],
  "tests": [ { "symbol": "tests.pagination.\"pagination preserves input\"", "qualified_name": "tests.pagination.\"pagination preserves input\"",
               "kind": "property", "status": "tested", "property_seed": "c0f6e2a9d4b17358a91c4e07bd2f6a53e8d1b2c9f04a7e6d3b5c8f1a2e4d7c90", "cases": 100 } ],
  "summary": { "selected": 3, "tested": 3, "failed": 0 },
  "fixtures": [],
  "assumptions": [],
  "environment": { "os": "linux", "arch": "x86_64" },
  "status": "tested",
  "produced_at": "…",
  "duration_ms": 12
}
```

| Field | Meaning |
| - | - |
| `configuration.seed` | the run seed; with each property's identity (and its recorded `property_seed`, Section 1.7) every property's stream, and so every case sequence, is reproducible (T45) |
| `tests` | one `TestReceiptEntry` per record of the report: the deterministic part of a `TestCaseRecord` (`symbol`, `qualified_name`, `kind`, `status`, and for a property `property_seed` and `cases`; no `duration_ms`, run `seed`, or nested `receipt`) |
| `tested` | every definition in the static call closure of the selected tests, the tests themselves included, so that a test's own version is part of the evidence a reused receipt must match (tests depend on implementations, as the design states), with facet versions, sorted by display path and deduplicated by id (as is `dependencies` in Section 8.1) |
| `fixtures` | capability bindings supplied to the run: `{ "capability": path, "binding": name, "digest": Digest }`, one per fixture that the run granted, sorted by capability path (semantics S83; `spec/python.md` Section 8); empty in Milestones 1 and 2 |
| `assumptions` | the named assumptions the run depended on, `Assumption` objects as in Section 8.1: one of kind `foreign_adapter` per fixture binding (an adapter's contract is an assumption, design line 213) and one of kind `hole` per holed declaration a test reached; `[]` in Milestone 1. The design's test receipt includes everything a verification receipt does (line 261), and semantics S59 lists the same field |
| `environment` | environment inputs the run depended on: `os`, `arch`, and from Milestone 3 the adapter runtimes (`python`, `node`) and declared environment variables; nothing else is recorded |
| `status` | `tested` when at least one test was selected and every selected test is `tested`, else `failed` (S50's test vocabulary, T44): an empty selection is a configuration failure (design line 421), never evidence that something was tested |

From Milestone 2, a run under `--engine native` or `both` adds `engine`
and `native` to `core.check_configuration` (Section 7.2 `--engine`), so a
receipt of the native engine is never mistaken for one of the evaluator.

The integrator reuses a test receipt only when `inputs`, `configuration`,
`tested` versions, `fixtures`, `assumptions`, and `environment` all equal
the candidate's (Section 10.6).

## 9. Semantic diff

### 9.1 Statement vocabulary

For stable id `id` present in the old facet map as `o` and/or the new map
as `n`:

| Statement | Condition | `detail` |
| - | - | - |
| `identity_added` | `id` only in new | `{ "version": SymbolVersion }` |
| `identity_removed` | `id` only in old | `{ "version": SymbolVersion }` of the old side |
| `identity_renamed` | both; `name` hashes differ | `{ "from": o.path, "to": n.path }` |
| `moved` | both; `module` differs | `{ "from_module": o.module, "to_module": n.module }` |
| `signature_changed` | both; `signature` hashes differ | `{ "old": hash, "new": hash }` |
| `contract_changed` | both; `contract` hashes differ | `{ "old": hash, "new": hash }` |
| `effect_bound_widened` | both; `n.effects \ o.effects` non-empty | `{ "added": [refs] }` |
| `effect_bound_narrowed` | both; `o.effects \ n.effects` non-empty | `{ "removed": [refs] }` (both statements appear when the sets are incomparable) |
| `body_changed` | both; `body` hashes differ | `{ "old": hash, "new": hash }` |
| `docs_changed` | both; `docs` hashes differ | `{ "old": hash, "new": hash }` |
| `verification_policy_changed` | both; `verification_policy` hashes differ | `{ "old": text, "new": text }`: the facet texts recovered from the hashes, `"@prove"` or `""` (the raw hash for a text that is neither) |

A statement's `facet` is the facet whose hashes differ: `name` for
`identity_renamed`, `effects` for the two effect statements,
`verification_policy` for `verification_policy_changed`, and the named
facet for the four `*_changed` facet statements; `identity_added`,
`identity_removed`, and `moved` have `facet: null`. This is what the grant
checks of Section 10.5 read. A facet that applies on one side only (the
declaration changed kind so that its facet set changed, e.g. a `fn` became
a `struct`, or a wrapper `type` became a refinement) is reported as
`identity_removed` plus `identity_added`, each with `"note":
"kind_changed"` in its `detail` (`{ "version": ..., "note": "kind_changed"
}`), because the two facet sets are not comparable; a kind change that
keeps the facet set (a wrapper `type` that became a `struct`, a `struct`
that became an `enum`) is compared facet by facet like any other edit
(`signature_changed`). The `identity` facet itself never produces a
statement (equal by construction). A statement's `path` is the new
side's whenever the id is in the new map, else the old side's, so both
statements of a kind change that also renamed carry the new path (the
removal keeps the old side's version, with its path, in
`detail.version`); statements are ordered by `path` and then by table
order.

Decision: [D-168] T31. the semantic diff is computed only from the two facet maps
(syntactic), never from typed programs, and effect widening/narrowing uses
the effect sets stored in `FacetEntry::effects`. Rationale: the diff must
work between any two revisions, including ones that do not typecheck, and
every statement in the vocabulary is decidable from facets alone.
Rejected alternative: diffing HIR (richer statements such as "callers
affected", but undefined for broken revisions).

### 9.2 JSON (`DiffOutput`)

```json
{ "command": "diff", "…": "…", "status": "ok",
  "old": { "commit": "a1b2…", "state": "clean", "digest": "…" },
  "new": { "commit": "3f2e…", "state": "clean", "digest": "…" },
  "statements": [
    { "statement": "contract_changed", "id": "pgn_7c2d", "path": "reader.pages.paginate", "kind": "fn", "facet": "contract",
      "detail": { "old": "…", "new": "…" } },
    { "statement": "identity_added", "id": "tests.pagination.\"pagination rejects zero\"", "path": "tests.pagination.\"pagination rejects zero\"", "kind": "test", "facet": null,
      "detail": { "version": { "…": "…" } } } ],
  "duplicates": { "old": [], "new": [] } }
```

`duplicates` lists ids declared more than once in either revision (Section
4.5) as `{ "id", "files" }` objects (`DuplicateId`), in id order, `files`
holding the file of every occurrence in (file path, declaration order), so
a file that declares an id twice appears twice; the diff uses the first
occurrence and reports the duplicates so the reader knows the statements
for those ids are unreliable.

`detail` is a `serde_json::Value`, but it is written with the keys of
every object in it in declared order, not sorted (Section 7.1): first the
detail keys of the 9.1 table in table order (`version`, `note`, `from`,
`to`, `from_module`, `to_module`, `old`, `new`, `added`, `removed`), then
the fields of an embedded `SymbolVersion` in their declaration order (`id`,
`path`, `provisional`, `facets`, `definition`), then facet names in
`Facet::ALL` order, and any other key last, sorted. Deserializing ignores
the order.

### 9.3 Obtaining both trees

```text
toplevel = git rev-parse --show-toplevel        (run from the project root; project_prefix = project root relative to toplevel, "" or "dir/")
old, new = resolve per the range table (Section 7.2 `cpl diff`)
for each commit c:
    1. the entry of <prefix>cpl.toml from git ls-tree -z c -- <prefix>cpl.toml, read like every other file (step 4)
       # missing: an empty tree, which loads as LoadError::NoManifest, the CLI's git_no_project_at_revision
    2. manifest = parse(its bytes) -> [build] sources   # one that does not parse: the tree holds cpl.toml and cpl.lock alone, so loading reports the error
    3. entries = git ls-tree -r -z c -- <prefix>cpl.toml <prefix>cpl.lock <prefix><src>/ ... <prefix>work/
       keep entries whose path matches cpl.toml, cpl.lock (read, never digested), <src>/**/*.cpl, work/*.work, work/*.patch   # the source set of Section 0
       a regular file (blob) is requested by its oid, a symbolic link (mode 120000) as "<c>:<path>"; submodules are dropped
    4. contents = git cat-file --batch --follow-symlinks   (stdin: one request per line; stdout: "<oid> blob <size>\n" + bytes + "\n")
       # a link is followed inside c's tree (T54) and holds the bytes of the file it leads to; one that leads to no file of c
       # (a directory, nothing, a loop, a path through a file, a target outside the repository) is left out of the tree
    5. tree = VirtualTree { path without prefix -> bytes }
    6. project = load_project_from_tree(tree, FileOrigin::Revision(c), &LoadOptions::default()); map = facet_map(project, Revision { commit: c, state: clean, digest: source_digest(project) })
"the working tree" = load_project(root) and current_revision
semantic_diff(old_map, new_map)
```

Every `git` invocation is `git -C <toplevel> ...` with `LC_ALL=C`,
`GIT_TERMINAL_PROMPT=0`, and `GIT_LITERAL_PATHSPECS=1` (so that a path such
as `lib*` or `proj[1]` matches only itself), stdout and stderr captured; a
non-zero status is `GitError::Command`, except for `rev-parse --verify`,
whose every failure is `UnknownRevision` (Section 1.8). `cat-file --batch`
is one process for the whole tree, so the number of git processes per
revision stays fixed.

Decision: [D-169] T32. revision trees are materialized in memory with `ls-tree`
plus `cat-file --batch`, never by checking out or by `git archive`.
Rationale: two processes per revision, no temporary directory, no
dependence on `tar`, and the loader already accepts a `VirtualTree`.
Rejected alternatives: `git show c:path` per file (one process per file);
`git worktree add` (touches the filesystem for a read-only query).

## 10. Work coordinator data model (Milestone 6)

This section fixes the data model and algorithms so that `cpl-work` can be
written against them from Milestone 1 (model, validation, patches) and
completed in Milestone 6 (scheduler, leases, integration). The scheduler
enforces the filesystem, process, network, and integration boundaries of
every task, as the design requires ("Merely parsing a declaration does not
sandbox an agent", design line 361): it starts workers only through a host
*sandbox adapter* configured from the task's grant (Section 10.8), refuses
to run a task whose boundary the host cannot enforce, and records in every
lease, candidate, and integration commit exactly the boundary that was
enforced, never more.

### 10.1 Records

```rust
pub struct ChangeRunId(String);     // "chg_" + 16 base32 chars encoding 80 random bits the caller draws (cpl-work has no rand), issued when a change is opened
pub struct TaskId { pub change: ChangeRunId, pub name: String }
pub struct CandidateId(String);     // "cand_" + facet_hash (blake3, first 32 hex) of "<change>/<task>\n<attempt>\n<token>\n<tree oid>" (CandidateId::derive)
pub struct FencingToken(pub u64);   // strictly increasing per change run
pub struct Timestamp(pub String);   // RFC 3339, UTC; now, from_unix_seconds, add_seconds, cmp_instant, to_utc

/// The host-supplied identity and authority of the actor who opens a change or integrates it; never read from a .work file.
pub struct Actor { pub id: String, pub authority: Authority }
pub struct Authority {
    pub write: Option<BTreeSet<(StableId, Facet)>>,   // None: unlimited (within policy)
    pub add: Option<Vec<Vec<String>>>,                // None: unlimited (within policy)
    pub files: BTreeSet<FileSelector>,                // Section 10.3
    pub boundary: ExecutionBoundary,                  // Section 10.8
    pub may_integrate: bool,                          // design line 365: integration authority is not every worker's
}
pub struct FrozenInputs {
    pub manifest: Digest, pub lock: Option<Digest>, pub work_file: Digest,
    pub toolchain: ToolchainInfo,
    pub policy: RunPolicy,                         // [verification], [test], [work] as they were when the change opened (design line 351)
}
pub struct ChangeRecord {
    pub id: ChangeRunId,
    pub name: String,                              // `change pagination_update`
    pub work_file: String, pub work_digest: Digest,
    pub base_commit: String,                       // `base git("main")` resolved when opened; frozen
    pub target_ref: String,                        // "refs/heads/main"
    pub workers: u32,
    pub opened_by: Actor,                          // the submitter; its authority bounds every grant (Section 10.3)
    pub frozen_inputs: FrozenInputs,               // digests of cpl.toml, cpl.lock, the .work file, toolchain version; the run's policy
    pub state: ChangeState,
    pub tasks: Vec<TaskId>,
    pub next_token: u64,
    pub opened_at: Timestamp,
}
pub enum ChangeState { Open, Integrating, Integrated { commit: String }, Failed { reason: String }, Cancelled }

pub struct TaskRecord {
    pub id: TaskId,
    pub spec: TaskSpec,                            // Section 1.9
    pub grant: Grant,                              // Section 10.3
    pub state: TaskState,
    pub attempt: u32,                              // incremented on every (re)assignment
    pub lease: Option<Lease>,
    pub baseline: Option<Baseline>,                // Section 5.5; set when the task becomes Ready
    pub candidate: Option<CandidateId>,            // latest accepted or submitted candidate
    pub blocked: Option<String>,                   // reason while Blocked
    pub history: Vec<Transition>,
}
pub struct Transition { pub from: TaskState, pub to: TaskState, pub at: Timestamp, pub token: Option<FencingToken>, pub reason: String }   // a failed validation's diagnostics are summarized in `reason`
pub struct Lease {
    pub task: TaskId, pub attempt: u32, pub token: FencingToken,
    pub holder: String,                            // opaque worker identity
    pub worktree: std::path::PathBuf,              // isolated checkout of the baseline
    pub boundary: ExecutionBoundary,               // the boundary the Sandbox reported as enforced for this worker (Section 10.8)
    pub issued_at: Timestamp, pub expires_at: Timestamp,
}
pub struct Baseline { pub revision: Revision, pub snapshot_commit: String, pub predecessors: Vec<CandidateId>, pub facets: FacetMap }

pub struct Candidate {
    pub id: CandidateId,
    pub task: TaskId, pub attempt: u32, pub token: FencingToken,
    pub baseline: Baseline,
    pub tree: String,                              // git tree oid of the candidate source tree
    pub snapshot_commit: String,                   // refs/cpl/candidates/<id>, parent = baseline.snapshot_commit
    pub patches: Vec<PatchSummary>,                // the submitted .patch forms, resolved and consumed (Section 10.5)
    pub boundary: ExecutionBoundary,               // copied from the lease: what the worker that produced this tree ran under
    pub facet_diff: Vec<DiffStatement>,            // semantic_diff(baseline.facets, facet_map(tree))
    pub dependency_versions: Vec<SymbolVersion>,   // every symbol the changed facets reference, at the candidate
    pub evidence: Vec<Digest>,                     // receipts produced at task-level validation
    pub review: Option<ReviewRecord>,              // for review tasks
    pub submitted_at: Timestamp,
}
pub struct PatchSummary { pub target: (StableId, Facet), pub expected: FacetHash, pub applied: FacetHash }
pub struct ReviewRecord { pub reviewer_task: TaskId, pub reviewed: Vec<CandidateId>, pub verdict: Verdict, pub notes: String, pub token: FencingToken }
pub enum Verdict { Approve, Reject, NeedsChanges }
```

Persistence: `ChangeRecord`s and `TaskRecord`s live in
`<project root>/.cpl/work/<change run id>.json` (disposable execution
state), written atomically; candidates, snapshots, and receipts are git
objects under `refs/cpl/candidates/<id>` and `refs/cpl/evidence/<change run
id>` (durable results with provenance). A coordinator restart reloads the
JSON, voids every lease whose `expires_at` has passed, and leaves worktrees
in place for inspection or reassignment.

Times fail closed. Every recorded time (`Transition::at`,
`Lease::issued_at`, `Lease::expires_at`) is the operation's `now` respelled
in UTC: a `now` with an offset is normalized, not refused, and a `now` that
is not RFC 3339 makes every operation that would record a time fail with
`ScheduleError::InvalidTimestamp` and change nothing (`propagate_failures`
and `invalidate_dependents`, which return no error, record nothing then).
A lease whose `expires_at` is not RFC 3339 counts as expired. Opening a
change writes no history entry (a `Transition` has no "(new)" state), and a
renewal is recorded as `Running` to `Running`.

Decision: [D-170] T33. coordinator state is a JSON file under `.cpl/work/` and
every durable artifact is a git object; no database. Rationale: the design
says Git is the source of record and caches and leases are disposable; a
JSON file is inspectable by humans and agents and a worktree can always be
reconstructed from a snapshot commit. Rejected alternative: SQLite
(transactions for free, but a second store to back up and a native
dependency).

### 10.2 Task states and transitions

| From | To | Trigger | Conditions |
| - | - | - | - |
| (new) | `Ready` | change opened | the task has no `after` predecessors; `ChangeRun::open` refuses a `workers` of 0 (`work_bad_workers`), duplicate task names, an `after` naming no task (`work_unknown_task`), a cycle (`work_cycle`), and unordered tasks with overlapping write sets or `add` namespaces (`work_overlapping_writes`, by the rule of `cpl work check`, Section 7.2), so a run never starts from a model the design's scheduler would reject |
| (new) | `Blocked` | change opened | it has predecessors (reason `waiting_for: [...]`) |
| `Blocked` | `Ready` | every predecessor `Accepted` or `Integrated` | candidate snapshot assembled (Section 10.4) and its basic checks pass; the snapshot's `Baseline::predecessors` must be exactly the current candidates of the task's predecessors, each once, in any order (otherwise the stale-baseline refusal, naming the missing and unexpected candidates) |
| `Blocked` | `Failed` | a predecessor `Failed`/`Cancelled`, or assembly conflict unresolvable | |
| `Ready` | `Running` | assignment | `workers` not exceeded; the `Sandbox` can enforce `grant.boundary` (else `work_boundary_unenforceable` and the task stays `Ready`, checked before any token, lease, or launch); lease issued with a fresh token, which is spent even if the launch fails; worktree created at the baseline snapshot; the worker started through `Sandbox::launch`, whose reported boundary is recorded in the lease (Section 10.8); a reported boundary wider than the grant is refused (`work_boundary_unenforceable`) rather than recorded |
| `Running` | `Running` | lease renewal | same token, before expiry (recorded as a `Running` to `Running` transition) |
| `Running` | `Ready` | lease expired without submission, or worker reported failure | attempt incremented on the next assignment; the expired lease stays recorded and the old worktree is kept until the task is reassigned |
| `Running` | `Submitted` | submission with the current, unexpired token | candidate recorded (idempotent on (task, attempt, token, tree)) |
| `Submitted` | `Accepted` | task-level validation passed | scope enforcement (Section 10.5), typecheck of the candidate tree, patch `expect` checks, the task's own tests if any `add` covers tests |
| `Submitted` | `Ready` | task-level validation failed | diagnostics attached; attempt incremented on reassignment |
| `Accepted` | `Blocked` | a predecessor's accepted candidate changed (re-run) or a review of this candidate became stale | reason `invalidated_by: <candidate id>` |
| `Accepted` | `Integrated` | the integration commit including this candidate advanced the target | |
| any non-terminal | `Cancelled` | operator request | |
| `Accepted` | `Failed` | integration retries exhausted with this candidate stale | |

A submission whose token is lower than the task's current token, or whose
lease has expired, is rejected with `work_stale_submission` and changes
nothing; this is the fencing rule. Resubmitting the same candidate
(same task, attempt, token, tree oid) returns the existing `CandidateId`
and is a no-op. The idempotence test comes first: a repeat of the task's
recorded candidate returns its id whatever the task's state now (after
expiry or acceptance included), while a repeat of an earlier, superseded
candidate is subject to the fencing rule. Every other refusal of a
submission, a renewal, or a failure report that the fencing rule implies is
`work_stale_submission` as well: a token that was never issued (higher than
the current one, or no lease at all), an attempt that is not the lease's,
a task that is no longer `Running` (`Ready` after a failed validation,
`Blocked`, `Cancelled`), and a change run that has ended. `Integrated`,
`Failed`, and `Cancelled` are final, for tasks and for change runs (`is_terminal`);
an operation the state machine refuses changes nothing.

### 10.3 Grants

`Grant` is computed when the change is opened and never changes for the
run:

```text
requested   = task.write ∪ task.add ∪ task.preserve ∪ the file selectors of task.write
              (resolved to (id, facet) pairs, namespaces, and FileSelectors; D68: a task requests nothing else)
policy      = [work.policy] of cpl.toml (Section 11.1; absent = everything writable except the fixed exclusions, no files, the default boundary)
authority   = the submitter's Authority, supplied by the host when the change is opened (absent = policy)
grant.write    = task.write ∩ policy.write ∩ authority.write, minus fixed exclusions
grant.add      = task.add namespaces ∩ policy.add ∩ authority.add
grant.files    = file selectors of task.write ∩ policy.files ∩ authority.files, minus the running change's own .work file
grant.preserve = task.preserve ∪ (every facet of every declaration named by an integrate `prove`/`test` requirement that the task does not write)
grant.boundary = the per-field minimum of policy.boundary and authority.boundary        (Section 10.8; a task cannot request more, D68)
fixed exclusions: (any id, identity); std; files outside the source set; the running change's own .work file (frozen for the run, design line 351)
```

`write` of a `.definition` selector grants every applicable facet except
`identity`. `add` of a namespace grants creation of new declarations in
modules under that namespace and nothing else: existing declarations there
remain read-only, so a task with `add { tests.pagination.* }` cannot edit
an existing acceptance test. Goals, comments, and docs never widen a
grant. A file selector counts in a `write` set only: in `depend` or
`preserve` it requests nothing (`TaskSpec` has `write_files` alone and
`Grant::preserve` holds facets), which loses nothing, since a project file
is read-only for every task not granted it. `cpl work check` (Section 7.2)
does not apply the fixed exclusions: a `write` selector that names a std
declaration or the running change's own work file validates, and is
subtracted here. Build inputs (Section 0) lie outside the source set and
are therefore fixed exclusions: in edition 0.1 no task can be granted
one, and a file selector for them, `adapter.<binding>`, is proposed for
Milestone 6 (T82).

Decision: [D-171] T42. three *file selectors* let a task be granted the project
files the design makes "read-only ... unless a task explicitly has
authority over them" (manifests, acceptance policy, build inputs; design
lines 367 and 602): `project.manifest` is `cpl.toml`, `project.lock` is
`cpl.lock`, and `work.<stem>` is `work/<stem>.work`. They are written in a
`write` set like any other selector (grammar 4.4 parses them as
declaration selectors), are resolved only after declaration resolution has
failed for the spelling (a declaration of that name wins, with the warning
`work_file_selector_shadowed`), cover the whole file, and are granted only
when `[work.policy] files` and the submitter's authority both list them.
A granted edit to `[verification]`, `[test]`, or `[work]` in `cpl.toml`
takes effect for later runs; this run's checks read `frozen_inputs.policy`
(design line 351). `.patch` files under `work/` are submission artifacts,
not grantable files: the coordinator applies and removes them (Section
10.5). Rationale: the design requires an authorized task to be able to
update bindings and regenerate locks, and spelling the grant as a selector
keeps the grammar's single request vocabulary (D68) and the design's rule
that selectors are resolved by tooling and never as filesystem globs (no
file selector names a pattern). Rejected alternatives: making every
non-`.cpl` file permanently read-only (an earlier revision of this
section; it contradicts design line 367); a glob-style `files { "cpl.toml"
}` item (a grammar change and a filesystem vocabulary).

Decision: [D-172] T43. a namespace wildcard `m.*`, wherever a `.work` file, `cpl
test`, or `cpl inspect` accepts one, denotes module `m` and every module
whose path has `m` as a proper prefix (`tests.*` covers `tests.pagination`
and `tests.pagination.edge`), and a bare module path in `cpl test` means
the same. Rationale: a module path is a namespace, not a container
(semantics 1.2), `add { tests.* }` must authorize creating
`tests.pagination` (design line 353), and one rule for selection, grants,
and overlap detection keeps `work_overlapping_writes` and
`work_test_selector_empty` consistent with what the grant enforces.
Semantics S42 and grammar D38 cite this rule. Rejected alternative: the
module itself only (then no selector could name a namespace a task is to
populate).

### 10.4 Candidate snapshot assembly for dependent tasks

```text
assemble(task):
    tree = tree of change.base_commit
    for each predecessor p in topological order of `after` (ties by task name):
        c = p.candidate (must be Accepted)
        for each statement s in c.facet_diff:
            if s touches an (id, facet) already modified by an earlier predecessor in this assembly: conflict -> task Blocked (reason work_conflict, naming both candidates)
        tree = apply c's text changes to tree (c.tree's versions of every file c modified; files modified by two predecessors are merged facet by facet by re-applying each candidate's patches with expect checks)
    checks: load_project_from_tree(tree) parses; check() has no errors; every `depend` selector of the task still resolves
    snapshot = commit-tree(write-tree(tree), parent = base_commit) stored at refs/cpl/snapshots/<task id>/<attempt>
    task.baseline = Baseline { revision: snapshot as clean commit, snapshot_commit, predecessors: [candidate ids], facets: facet_map(tree) }
```

Speculative alternatives (two candidates for one task) are never merged:
the task's `candidate` is the one selected by validation or an operator;
the others remain under `refs/cpl/candidates/` for inspection. The
topological order of `predecessors` is how assembly layers them; it is not
part of the `Blocked` to `Ready` check, which compares the set (Section
10.2).

### 10.5 Write/add/preserve enforcement over facet diffs

Run at task-level validation (`Submitted -> Accepted`) and again at
integration against the assembled tree:

```text
enforce(candidate, grant):
    violations = []
    # 1. files other than <sources>/**/*.cpl
    for each path in candidate.tree ∪ baseline tree outside <sources>/**/*.cpl whose bytes differ (added, changed, or removed):
        work/*.patch added or changed -> consumed: applied (below) and removed from the tree; never a violation
        work/*.patch removed       -> neither consumed nor a violation
        cpl.toml / cpl.lock / work/*.work -> ok iff its file selector (project.manifest / project.lock / work.<stem>) ∈ grant.files; else violation(patch_out_of_scope, path)
        any other path             -> violation(patch_out_of_scope, path)
    # 2. facet-level statements
    for s in semantic_diff(baseline.facets, facet_map(candidate.tree)).statements:
        match s.statement:
            identity_added        -> ok iff some grant.add namespace covers s.module (prefix match on module path, segment by segment, T43); else violation(patch_out_of_scope)
            identity_removed      -> ok iff (s.id, every applicable facet but identity) ⊆ grant.write; else violation   # identity is in no grant (10.3)
            identity_renamed      -> ok iff (s.id, name) ∈ grant.write
            moved                 -> ok iff (s.id, every applicable facet but identity) ⊆ grant.write      # a move is not a facet; it needs the whole declaration
            <facet>_changed, effect_bound_widened, effect_bound_narrowed, verification_policy_changed
                                  -> ok iff (s.id, s.facet) ∈ grant.write; else violation(patch_out_of_scope)
    # 3. preserve
    for (id, facet) in grant.preserve: if hash differs between baseline and candidate -> violation(patch_preserved_facet)
    # 4. identity integrity (the two identities of one declaration are paired by its qualified name)
    for each declaration: if provisional in baseline and non-provisional now (or vice versa) outside grant.write(name) -> violation(patch_identity_changed)
    for each declaration whose @id value changed -> violation(patch_identity_changed), whatever the grant (Section 4.6: a task can never do this)
    duplicates in candidate map (any, those the baseline already had included) -> violation(duplicate_identity)
    # 5. non-facet text in .cpl files (use lines, module headers, comments, blank lines)
    for each .cpl file that differs: ok iff the file contains at least one granted statement from step 2 and its module header is unchanged (or the file is new and its module lies under a grant.add namespace); else violation(patch_out_of_scope, path)
        # a statement counts for its declaration's file on both sides (a move touches two files); a removed source file
        # needs a granted statement only (it has no header left); an unreadable module header counts as changed; a new
        # file passes only through the add clause
    for each rebound name r (TreeDiff::rebound, T55): ok iff (r.id, r.facet) ∈ grant.write; else violation(patch_out_of_scope, r.path)
    accept iff violations is empty
```

Decision: [D-245] T55. rule 5 also refuses a candidate that changes what
read-only code refers to: when a declaration present in both trees keeps
the text of its `signature`, `contract`, `effects`, or `body`, but a name
that text uses imports something else in its candidate file than in its
baseline file (a `use` line added, removed, or repointed, or the
declaration moved to another file of its module, whose imports differ),
that facet counts as changed for the grant and needs `(id, facet) ∈
grant.write`, else `patch_out_of_scope`. "Uses a name" is approximated by
the identifiers of the facet's canonical text that are not preceded by `.`,
so a local, parameter, or field label spelled like an imported name also
needs authority (the check errs toward refusing); a changed facet is left
to rule 2, a move to another module to the `moved` statement, and a file
side that does not parse is skipped (step 2 already sees its declarations
removed). Rationale: imports are scoped to the file (semantics S5), so
repointing one changes the meaning of every declaration of the file that
uses the name while no facet hash changes, and the design makes
"everything outside the granted write and add sets ... read-only" and says
an `add` task cannot change "unrelated acceptance tests" (design lines 353
and 285); without the rule an add-only task could redirect existing tests
to a look-alike it creates in its own namespace. Rejected alternatives:
letting `use` lines ride on any granted statement of their file (an
earlier reading of rule 5, which has exactly that hole); making `use`
lines read-only except for tasks with authority over the whole file (no
file-level grant exists for `.cpl` files).

Renames that rewrite callers produce `body_changed` statements on each
caller; by rule 2 those need write authority over the callers' bodies,
which is the design's "the change must also authorize those resulting
edits". Identity tracking never grants anything (`identity` is excluded
from every grant).

Patch application (`apply_patch`): locate the target declaration's body
block span in the file text of the tree (`FnDecl::body`, or the body of a
`test` or `property`, which also have a `body` facet, Section 5.1; the
first occurrence in path order when an id is declared twice), replace the
bytes of the block with `format_fragment(Block, FragmentMode::Source)`
(comments kept) re-indented to the whitespace of the declaration's line,
with the file's own line ending (CRLF or LF) at the block, and leave every
other byte unchanged. The project text must equal the tree's bytes (the
project loaded from that tree; `internal_error` otherwise). The patch body
is type-checked in the target's scope by loading the patched tree. A submitted `.patch` file that does not parse is
rejected with `patch_syntax`, whose `notes` carry the parse diagnostic
(the parser stops at the first error, T6, so the file yields no
`PatchFile`); nothing from it is applied. Patch files are consumed: after
application every `work/*.patch` file the candidate added or changed is
removed from the tree and the applied forms are recorded as
`Candidate::patches` (provenance), so a `.patch` file is never integrated
and never a scope violation.

Known limitations of edition 0.1: facet hashes exclude comments (5.2) and
sort effects (T26), so a patch or edit that changes only the comments of a
body, or only the order of an `effects` set, produces no statement; it
passes `validate_patch` and rule 3 but changes a file without a granted
statement, which rule 5 refuses even for a task that may write that body
or effects facet. Likewise moving a declaration between two files of one
module, or renaming a file, produces no statement (`moved` needs another
module), so rule 5 refuses both files unless each also holds a granted
statement; and since a removed `work/*.patch` is no violation, any task
may delete a patch file the base commits.

### 10.6 Integration

```text
integrate(change, actor):
    actor.authority.may_integrate, else -> work_not_authorized (nothing changes; design line 365)
    change.state = Integrating
    for attempt in 1..=max_integration_retries (default 5):
        head = git rev-parse --verify <target_ref>
        tree = tree of head
        order = tasks with Accepted candidates, topological by `after`
        for t in order:
            for patch in t.candidate.patches: re-check `expect base` against facet_map(tree)   # Section 5.5 step 4
                stale -> t.state = Blocked (reason work_stale_against_target); change.state = Open; return Blocked (operator or rescheduling re-validates)
            before = tree; tree = apply t.candidate changes
            run enforce(t.candidate, t.grant) with before as the baseline side and tree as the candidate side   # only t's own changes are judged against t's grant
        tree = apply plan_id_assignments(tree) (Section 4.2): every declaration still lacking an @id receives one (T47) -
               after enforcement, so the inserted lines are never judged a scope change, and before the checks, so receipts and the commit carry the final identities
        format every file that received an id (format_module, T66): layout only, so no facet hash or module content hash changes; before the checks, so receipts record the formatted files and the commit is canonical
        policy = change.frozen_inputs.policy                       # never the assembled tree's [verification]/[test]/[work] (design line 351)
        checks, each producing or reusing a receipt:
            typecheck      -> check_with(tree, release = false) has no errors
            no_holes       -> check_with(tree, release = true) reports no `hole_in_release` (its `verification_unsatisfied` results are judged by prove(...) below, not here)
            prove(sel)     -> verification receipt for each target with status proved or conditional; a conditional proof is closed relative to assumptions that stay named in the receipt and in the CPL-Receipt trailers (design line 136)
            no_unresolved_assumptions
                           -> no receipt used by prove(...) carries an assumption of kind unchecked_callee, unproved_dependency, or hole with admitted = false
                              (grammar D36: the design's strict integration requirement); foreign_adapter assumptions are never rejected and remain listed
            test(sel)      -> selector matches >= 1 test (else configuration failure work_test_selector_empty); test receipt status tested; seed = first 8 bytes of blake3(change run id) as u64 for reproducibility
            review(task)   -> a ReviewRecord exists with verdict Approve whose `reviewed` equals exactly the current candidate ids of the reviewer's predecessors; otherwise the review is stale (work_review_stale) and integration stops
        reuse rule: a receipt is reused iff its inputs (sources, std, lock, manifest), toolchain, configuration, dependency versions (and for tests: fixtures, assumptions, environment) equal the assembled tree's; otherwise the check reruns
        if any check fails: change.state = Failed { reason }; attach receipts; return
        rewrite cpl.lock for `tree` (Section 11.2)
        new_tree = write-tree(tree); commit = commit-tree new_tree -p head -m message(change, candidates, receipts, actor)
        store receipts: [work] evidence = "refs" -> git objects under refs/cpl/evidence/<change run id>/<commit>;
                        [work] evidence = "tree" -> files <evidence_dir>/<change run id>/<receipt digest>.json inside new_tree (Section 11.1, T34)
        if git update-ref <target_ref> <commit> <head> succeeds: mark candidates Integrated; change.state = Integrated { commit }; return
        else: continue   # the target moved after validation: retry against the new head
    change.state = Failed { reason: "target moved <n> times" }
```

`git update-ref <ref> <new> <old>` is the compare-and-swap: Git refuses
the update when the ref no longer equals `<old>`, so a target that advanced
after validation is never overwritten and the loop re-validates against
the new head. The integration commit message carries provenance trailers:

```text
Integrate pagination_update

CPL-Change: chg_…
CPL-Candidate: implementation@2 cand_…
CPL-Candidate: examples@1 cand_…
CPL-Review: audit approve cand_… cand_…
CPL-Receipt: verification 7e1c…
CPL-Receipt: test 51a0…
CPL-Assumption: foreign_adapter reader.images.ImageCodec.dimensions
CPL-Integrator: <actor id>
CPL-Boundary: filesystem=worktree process=toolchain network=none
CPL-Toolchain: 0.1.0
```

`CPL-Assumption:` lines name every assumption of every receipt the commit
relies on (kind and symbol), so external adapter assumptions remain
visible in the history as the design requires; `CPL-Integrator:` records
the authorized actor; `CPL-Boundary:` records, field by field, the widest
boundary any contributing worker ran under (`cpl_work::boundary_text`
spelling), the one claim that holds for every contribution, since the
coordinator claims no more than was enforced (Section 10.8).

Known limitation of edition 0.1: because each integration gives an `@id`
to every declaration still lacking one (T47), a pending candidate whose
patch targets a provisional identity finds no such declaration at the next
attempt after any other change integrated (`patch_target_missing`, and the
task is `Blocked` with `work_stale_against_target`), although its body
never changed; tasks avoid this by naming declarations that carry an `@id`
(`work_provisional_identity` warns, Section 7.2).

Decision: [D-173] T34. receipts are stored as git objects under
`refs/cpl/evidence/...` and referenced from commit trailers by default
(`[work] evidence = "refs"`); because the design makes the evidence
location project policy (line 427), `[work] evidence = "tree"` instead
commits them as files under `[work] evidence_dir` (default `evidence`),
a directory outside the source set (Section 0) and therefore outside
every source digest. Rationale: the design requires that enough be
committed to rerun checks; sources and configuration are in the commit,
receipts are reproducible from them, and keeping receipts out of the
*source set* avoids a self-referential digest problem (a receipt cannot
include the digest of a set that contains the receipt), whichever
location policy picks. Rejected alternatives: a committed directory
inside the source set (breaks source digests); no policy knob (an earlier
revision of this decision; the design is explicit that the location is
policy).

Decision: [D-174] T47. provisional identities do not survive integration: `cpl
work check` warns `work_provisional_identity` for every declaration a task
or requirement names without an `@id`, and the integrator runs
`plan_id_assignments` over the assembled tree after scope enforcement and
before the checks, so every declaration a candidate adds (and every
pre-existing one still lacking an id) is committed with a generated `@id`.
Rationale: the design says ids are generated by tooling for top-level
definitions and that renames and moves retain identity (line 138); a
project whose `expect base`, `preserve`, and semantic diff ran on
provisional ids would silently lose that robustness at the first rename.
The check is a warning, not an error, because the corpus itself names
`Positive.definition` without an `@id` and must validate clean of errors.
Rejected alternatives: an error (rejects the corpus); enabling
`id_missing` by default (makes every fresh file warn under `cpl check`).

### 10.7 Git command inventory

| Operation | Command | Used by |
| - | - | - |
| discover repository | `git rev-parse --show-toplevel` | index, work |
| resolve a ref | `git rev-parse --verify <rev>^{commit}` | index, work |
| merge base | `git merge-base <a> <b>` | diff |
| dirty check | `git --no-optional-locks status --porcelain --untracked-files=all -- <paths>` | index |
| list tree | `git ls-tree -r -z <commit> -- <paths>` | index |
| read blobs | `git cat-file --batch --follow-symlinks` | index |
| isolated checkout | `git worktree add --detach <path> <commit>` / `git worktree remove --force <path>` | work |
| snapshot a worktree | `git -C <worktree> add -A` then `git -C <worktree> write-tree` | work |
| make a commit object | `git commit-tree <tree> -p <parent> -m <message>` | work |
| store a snapshot or evidence ref | `git update-ref refs/cpl/... <commit>` | work |
| atomic advance | `git update-ref <ref> <new> <old>` | work |
| store evidence blobs | `git hash-object -w --stdin`, `git mktree` | work |

Milestone 1 implements the first six rows (inside `cpl-index`) and the
wrappers of the rest in `cpl_work::git` with tests against a temporary
repository. `update_ref_cas(ref, new, old)` passes `old` to git, which
resolves it as any revision, so a symbolic `old` such as
`refs/heads/main` would make the compare vacuous: callers pass the full
object id `rev-parse --verify` returned (Section 10.6), and an empty `old`
(or the 40-zero id) requires that the ref not exist yet. A refused compare
is `GitError::Command` (git's status 128).

### 10.8 Execution boundaries

The design makes the scheduler responsible for enforcing filesystem,
process, network, and integration boundaries (line 361). Integration
boundaries are the grants (10.3, 10.5) and the integrator authority
(10.6); the other three form an `ExecutionBoundary`:

```rust
// cpl_diag::json, re-exported as cpl_work::scheduler items (Section 1.4.2; cpl_sema::project's [work.policy] needs them)
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExecutionBoundary { pub filesystem: FsAccess, pub process: ProcessAccess, pub network: NetworkAccess }   // Default: the strictest
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum FsAccess { #[default] Worktree, WorktreeAndRepository, Host }      // ordered strictest first: write only inside the task's worktree (default); plus read access to the repository; unrestricted
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum ProcessAccess { #[default] None, Toolchain, Any }                  // no child processes (default); only the `cpl` binary; unrestricted
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum NetworkAccess { #[default] None, Adapters, Any }                   // none (default); only the hosts the bound adapters declare; unrestricted
impl ExecutionBoundary {
    pub fn min(&self, other: &Self) -> Self;      // per field, the stricter of each pair
    pub fn within(&self, limit: &Self) -> bool;   // every field at most as permissive as limit's
}
// `Sandbox` is declared in Section 1.9; `Sandbox::enforceable()` is the strictest boundary the host can enforce.
```

| Rule | Behavior |
| - | - |
| source | `grant.boundary` is the per-field minimum of `[work.policy]` (Section 11.1) and the submitter's authority; a task declaration cannot request a wider one (grammar D68 gives it no item to do so) |
| refusal | before issuing a lease (and before any token is drawn) the coordinator checks `sandbox.enforceable().within(&grant.boundary)`, per field: the host must be able to confine the worker at least as strictly as the grant demands; a boundary the host cannot enforce is `work_boundary_unenforceable`, the task stays `Ready`, and no lease, worktree, or agent is created |
| launch | a worker is started only through `Sandbox::launch(lease, grant.boundary)`, never by the coordinator directly; the boundary it returns (equal or stricter) is recorded as `Lease::boundary` and copied to `Candidate::boundary` at submission; a returned boundary wider than the grant is refused with `work_boundary_unenforceable` and not recorded |
| record | `cpl work` output, receipts attached to candidates, and the integration commit's `CPL-Boundary:` trailer state the recorded boundary, never the requested one: the coordinator records what was enforced and claims no more |
| host | `cpl-cli` implements `Sandbox` over the host's mechanism (an OS sandbox, a container, or a worker process with brokered resources, design line 487); a host with no mechanism reports `enforceable()` as `Host`/`Any`/`Any` and can therefore run only tasks whose policy grants exactly that, which makes the absence of a sandbox visible rather than silent |
| toolchain processes | `ProcessAccess::Toolchain` admits the `cpl` binary and the processes that `cpl` starts itself for a command: `git` (Sections 6.3, 10.7) and, from Milestone 2, the C compiler with the tools it names (T72) and the drivers of Section 1.13, which run a binding's `in_process` adapter with the driver's privileges (semantics S78); a task confined to it can therefore run `cpl build`, the native engine, and `cpl run` with a backend-`c` binding, whose build inputs it cannot edit (T82) |
| budgets | concurrency (`workers`), lease time (`[work] lease_seconds`), and external-call budgets remain execution policy enforced by the host and the adapters (design line 383); they are not part of the boundary |

Decision: [D-175] T40. the coordinator owns boundary enforcement through the
`Sandbox` adapter: the boundary is computed from policy and authority, a
lease is refused when the host cannot enforce it, workers are launched
only through the adapter, and only the enforced boundary is ever recorded.
Rationale: the design assigns enforcement of filesystem, process, network,
and integration boundaries to the scheduler (line 361) and says that
parsing a declaration does not sandbox an agent; a coordinator that
delegated enforcement wholesale to "the host" would have nothing to refuse
with and nothing truthful to record. The mechanism itself is the host's
(line 487), which is why it is an adapter trait implemented in `cpl-cli`,
like `CheckRunner` (T2). Rejected alternatives: leaving sandboxing to the
host entirely (an earlier revision of this section's introduction;
contradicts line 361); a `request { filesystem, network }` task item
(D68 rejected it for the grammar).

## 11. Project files

### 11.1 `cpl.toml` (edition 0.1)

```toml
[project]
name = "reader"                 # required; ^[a-z][a-z0-9_]*$
edition = "0.1"                 # required; the only accepted value in this toolchain
description = "..."             # optional

[build]
sources = ["src"]               # optional; default ["src"]; project-relative directories, '/' separators ("." is the project root; one trailing '/' is accepted)

[interop]
targets = ["python", "node", "c"]   # optional; default []; each in {python, node, c, rust, go, java, kotlin}; inert in Milestone 1; "c" active from Milestone 2 (T74), "python" and "node" from Milestones 3 and 4; "rust", "go", "java" (and "kotlin", which enables "java") are the source hosts of spec/hosts.md

[interop.c]                     # optional; Milestone 2; requires "c", "python", or "node" in targets (the Python and Node
prefix = "reader"               # packages contain the C package); its keys are in the table below

[interop.python]                # optional; Milestone 3; requires "python" in targets (spec/python.md Section 1)
package = "reader_cpl"          # the import name; default "<name>_cpl"
version = "0.1.0"               # the distribution version; default "0.1.0"
paths = ["."]                   # project-relative Python path of the workers cpl run and cpl test start; default ["."]

[bindings.image_codec]          # zero or more; parsed and validated, inert in Milestone 1; backend "c" active from Milestone 2, "python" from Milestone 3
capability = "reader.images.ImageCodec"   # required; must resolve to a `pub capability` (project_binding_unknown_capability)
backend = "python"                        # required; one of python, node, c (else project_binding_bad_backend); must be in [interop] targets (else project_binding_target_not_enabled); one of the two at most (T48)
adapter = "reader_adapters.images"        # required; backend-specific; backend c: a project-relative .c or .cbind.toml file (T74); backend python: a dotted module path (spec/python.md 5.3)
isolation = "worker"                      # optional; "worker" (default) or "in_process"; backend c: "in_process" only in Milestone 2 (T74), either from Milestone 3
timeout_ms = 5000                         # optional; Milestone 3; a positive integer bounding each answer of the binding's worker (spec/python.md 6.5)

[verification]
proof_timeout_ms = 10000        # optional; default 10000; used from Milestone 5
admit = []                      # optional; stable ids of *dependencies* whose unverified specifications the project admits:
                                # an assumption of kind unchecked_callee, unproved_dependency, or hole about a listed id is
                                # `admitted` (design line 136). Nothing admits a declaration's own @prove obligations:
                                # `cpl check --release` rejects unknown and failed without a switch (semantics S36, T36)

[run]
max_call_depth = 10000          # optional; default 10000 (semantics S38); CLI --max-call-depth overrides

[test]
cases = 100                     # optional; default 100 (semantics S44); at least 1 (T57); CLI --cases overrides
max_shrinks = 1000              # optional; default 1000 (semantics S45); CLI --max-shrinks overrides
timeout_ms = 0                  # optional; default 0 = no wall-clock budget per case; CLI --timeout-ms overrides

[test.fixtures]                 # optional; Milestone 3: capability path = the binding that supplies it to tests (semantics S83)
"reader.pages.PageRead" = "page_store"

[work]                          # optional; reserved for Milestone 6; parsed and validated, inert before
lease_seconds = 900
max_integration_retries = 5
evidence = "refs"               # optional; "refs" (default): receipts under refs/cpl/evidence/...; "tree": committed files (T34)
evidence_dir = "evidence"       # optional; project-relative, used with evidence = "tree"; never part of the source set

[work.policy]                   # optional; the project's limits on every grant (Section 10.3, 10.8); absent = the defaults shown
write = ["*"]                   # selectors every task may be granted to write ("*" = everything but the fixed exclusions)
add = ["*"]                     # namespaces every task may be granted to add in
files = []                      # file selectors tasks may be granted: "project.manifest", "project.lock", "work.<stem>" (T42)
filesystem = "worktree"         # "worktree" (default) | "worktree_and_repository" | "host"      (Section 10.8)
process = "none"                # "none" (default) | "toolchain" | "any"
network = "none"                # "none" (default) | "adapters" | "any"
```

```rust
// cpl_sema::project = cpl_project (T52). Manifest and every section type derive Clone, Debug, PartialEq, Eq, Serialize,
// Deserialize.
// Parsed in two steps: a TOML table first, then field by field, so that unknown keys are diagnostics (exit 1) and bad
// values a ManifestError (exit 2, T53) rather than deserialization failures; no `deny_unknown_fields` derive is used.
// Every section except ProjectSection (whose keys are required) implements Default with the defaults below; Manifest
// itself has none.
pub struct Manifest { pub project: ProjectSection, pub build: BuildSection, pub interop: InteropSection,
                      pub bindings: BTreeMap<String, BindingSection>, pub verification: VerificationSection,
                      pub run: RunSection, pub test: TestSection, pub work: WorkSection,
                      pub interop_c: Option<CInteropSection>,               // Milestone 2 (T80): [interop.c], None when absent
                      pub binding_c: BTreeMap<String, CBindingSection> }    // Milestone 2 (T80): [bindings.<name>.c], by binding name
pub struct ProjectSection { pub name: String, pub edition: String, pub description: Option<String> }
pub struct BuildSection { pub sources: Vec<String> }                 // Default: ["src"]; entries stored normalized, without a trailing '/'
pub struct InteropSection { pub targets: Vec<String> }               // Default: []
pub struct BindingSection { pub capability: String, pub backend: String, pub adapter: String, pub isolation: Option<String> }
pub struct VerificationSection { pub proof_timeout_ms: u64, pub admit: Vec<String> }   // Default: 10000, []
pub struct RunSection { pub max_call_depth: u32 }                    // Default: 10_000
pub struct TestSection { pub cases: u32, pub max_shrinks: u32, pub timeout_ms: u64 }   // Default: 100, 1000, 0; cases >= 1 (T57)
pub struct WorkSection { pub lease_seconds: u64, pub max_integration_retries: u32, pub evidence: EvidenceLocation, pub evidence_dir: String, pub policy: WorkPolicySection }   // Default: 900, 5, Refs, "evidence", WorkPolicySection::default()
pub enum EvidenceLocation { Refs, Tree }                             // Default: Refs
pub struct WorkPolicySection { pub write: Vec<String>, pub add: Vec<String>, pub files: Vec<String>, pub boundary: ExecutionBoundary }   // selector spellings, resolved when a change opens; `boundary` is the flat keys filesystem/process/network; Default: ["*"], ["*"], [], the strictest boundary
/// Ok(manifest, diagnostics): the diagnostics are `project_manifest_unknown_key` (one per unknown table or key, with the
/// dotted `key`) and `unsupported_edition` (with `found`), span-less (semantics S63 (6)) and in document order. Err
/// (`project_manifest_invalid`, the fatal LoadError::Manifest) when the text is not TOML, a required key is missing, or
/// any key has the wrong type or a value outside its domain (T53).
pub fn parse_manifest(text: &str) -> Result<(Manifest, Vec<Diagnostic>), ManifestError>;
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub struct ManifestError { pub message: String, pub line: Option<u32> }   // line: 1-based in cpl.toml, when known
/// The `[bindings.*]` checks, which need name resolution: `cpl_sema::check` calls them after the resolve stage, with
/// `is_pub_capability` answering whether a dotted path names a `pub capability` of the program. Per binding, in name
/// order and key order (T48): `project_binding_unknown_capability`; `project_binding_bad_backend`, or else
/// `project_binding_target_not_enabled` (with `targets`); `project_binding_bad_isolation`. Every diagnostic is a
/// span-less error of category `project` with the fields `binding` (the table's name), `key` (dotted), and `found`.
pub fn check_bindings(manifest: &Manifest, is_pub_capability: impl Fn(&str) -> bool) -> Vec<Diagnostic>;

// ---- Milestone 2: the C target (T74, T80) ------------------------------------------------------------
pub struct CInteropSection {                     // [interop.c]; Default: the defaults of the table below
    pub prefix: Option<String>, pub abi: u16, pub artifacts: Vec<ArtifactKind>, pub compiler: Option<String>,
    pub cflags: Vec<String>, pub ldflags: Vec<String>,
    pub exports: Option<Vec<ExportEntry>>,       // None: the default export set
    pub type_names: BTreeMap<String, String>,    // qualified type name -> C name
}
pub struct ExportEntry { pub function: String, pub symbol: Option<String>, pub int: IntMapping }   // a string entry is { function, None, Handle }
#[serde(rename_all = "snake_case")] pub enum IntMapping { #[default] Handle, Int64, Int32, Uint64, Uint32 }
#[serde(rename_all = "snake_case")] pub enum ArtifactKind { Shared, Static, Executable }
pub struct CBindingSection { pub sources: Vec<String>, pub include_dirs: Vec<String>, pub defines: Vec<String>,
                             pub libraries: Vec<String>, pub library_dirs: Vec<String>, pub cflags: Vec<String> }   // Default: all empty
#[serde(rename_all = "snake_case")] pub enum AdapterForm { Wrapper, Declarative }   // a .c adapter; a .cbind.toml adapter
impl Manifest { pub fn c_prefix(&self) -> &str; }   // `[interop.c] prefix`, else `[project] name`
/// The rules of the C-target table below that need no name resolution, per binding in name order: the diagnostics, and
/// the declarative bindings whose files were read and parsed (for `cpl_sema::interop::plan`). With `root` None (a
/// VirtualTree) the file checks are skipped and `cbind` is empty.
pub fn check_c_bindings(manifest: &Manifest, root: Option<&std::path::Path>) -> CBindingCheck;
pub struct CBindingCheck { pub diagnostics: Vec<Diagnostic>, pub cbind: BTreeMap<String, cbind::CBindSpec> }
pub mod cbind {
    /// A declarative binding (`.cbind.toml`) read by the closed schema of spec/cabi.md Section 6; its fields mirror that
    /// schema. Derives Clone, Debug, PartialEq, Eq.
    pub struct CBindSpec { /* [library], [state], [ops.<name>], [structs.<name>] */ }
    pub struct CBindError { pub key: String, pub line: Option<u32>, pub reason: String }   // project_cbind_invalid's fields
    pub fn parse_cbind(text: &str) -> Result<CBindSpec, CBindError>;
}
```

Validation (`project_*` codes): `cpl.toml` that is not UTF-8 or does not
parse as TOML, a required key that is missing, and any key with the wrong
type or a value outside its domain is the fatal `project_manifest_invalid`
(`LoadError::Manifest` with the line when known, exit 2; T53); an unknown
table or key is the error `project_manifest_unknown_key` (a diagnostic: the
project still loads, exit 1); `edition` other than `"0.1"` is the error
`unsupported_edition` (resolve, semantics 1.1; a diagnostic, exit 1, with
the value kept in the manifest); a source directory that does not exist is
`project_source_dir_missing` (error, with `path`); a `.cpl` file reached
from two overlapping source directories is `project_duplicate_file_module`
(error, with `path` and `sources`; the file is loaded once); an empty source
set is the warning `project_no_sources`; the binding checks run after name
resolution (`check_bindings`, T48), so loading never reports a
`project_binding_*` code. Every one of these diagnostics is span-less. There
are no deprecated or aliased keys in edition 0.1.

From Milestone 2 the C target is active (T74). `"c"` in `[interop]
targets` enables `cpl build --target c` (Section 7.2), backend-`c`
bindings, and the optional table `[interop.c]`; a backend-`c` binding
names its adapter file and may say how to compile and link it in the
subtable `[bindings.<name>.c]`. The adapter forms, the `.cbind.toml`
schema, and what each key means for the generated code are `spec/cabi.md`
Sections 5 and 6's; this section owns the keys and their validation. A
backend-`c` binding of the `cboundary` fixture (Section 12.7):

```toml
[interop]
targets = ["c"]

[interop.c]
prefix = "cboundary"
abi = 1

[bindings.image_codec]
capability = "reader.images.ImageCodec"   # a pub capability (T48, unchanged)
backend = "c"
adapter = "adapters/image_codec.c"        # backend c: a project-relative .c reviewed wrapper or .cbind.toml declarative binding
isolation = "in_process"                  # backend c: required, and the only value in edition 0.1

[bindings.image_codec.c]                  # optional, backend c only: how to compile and link the adapter
sources = ["vendor/imgtest/imgtest.c"]    # vendored C sources compiled into the package, pinned by digest (build inputs, Section 0)
include_dirs = ["vendor/imgtest"]         # project-relative directories
defines = ["IMGTEST_STATIC=1"]            # "NAME" or "NAME=VALUE", NAME a C identifier
libraries = ["extra"]                     # libraries by name (-l<name>), pinned by the file the linker opened (Section 11.2)
library_dirs = ["vendor/libextra"]        # project-relative directories
cflags = []                               # appended when compiling this binding's adapter and sources
```

The keys of `[interop.c]`, all optional:

| Key | Domain | Default |
| - | - | - |
| `prefix` | `^[a-z][a-z0-9]*(_[a-z0-9]+)*$`, at most 31 characters, not starting with `cpl`: words joined by single `_`, so that a prefix neither contains `__` nor ends in `_`, which keeps the namespaced internal symbols of two packages apart (`spec/cabi.md` 1.6); and not the name of a C11 standard header or of a POSIX header the runtime includes (`stdlib`, `string`, `time`, `pthread`, ...: `cpl_project::SYSTEM_HEADERS`), since `<prefix>.h` beside the package's and its hosts' `-I<out>/include` would shadow it; the C prefix of every exported name (cabi.md Section 5) | `[project] name` |
| `abi` | an integer from 0 to 65535: the package ABI major (soname `lib<prefix>.so.<abi>`, cabi.md Section 9) | `0` |
| `artifacts` | an array of `"shared"`, `"static"`, and `"executable"` (a repeated entry counts once; an empty array builds the headers and sources alone) | `["shared", "static"]` |
| `compiler` | a command name or a path (T72) | none: `CPL_CC`, then `cc` |
| `cflags` | an array of strings, appended when compiling adapter and vendored sources, never emitted code (cabi.md Section 1) | `[]` |
| `ldflags` | an array of strings, appended to every link | `[]` |
| `exports` | an array whose entries are a target string or an inline table `{ function = "<target>", symbol = "<C name>", int = "<mapping>" }` (`symbol` and `int` optional, `int` one of `handle`, `int64`, `int32`, `uint64`, `uint32`, default `handle`); each entry is one entry point, so one function may be listed twice (its handle form and an `int64` form) | absent: the default export set (T74) |
| `type_names` | an inline table from the qualified name of a type to a C name `^[a-z][a-z0-9_]*$`, to resolve a collision of generated names (cabi.md Section 5) | `{}` |

An export target is resolved like a command's target (Section 6.1, T59: a
qualified name, an `@id`, a provisional id, or a bare name that names
exactly one definition) and must name a `pub fn` declared in the project
that is neither generic nor a generator. When `exports` is absent, the
export set is every such function of the project, each in its handle form
with its default symbol, in qualified-name order, and every generic `pub
fn` it leaves out is reported by `cpl build` alone as the info
`project_export_skipped_generic`, so that `cpl check`'s results on the
corpus do not change. The verbatim `examples/reader` therefore exports
`reader.images.image_width`, `reader.pages.load_page`,
`reader.pages.load_text`, and `reader.pages.paginate_ints` and skips the
generic `reader.pages.paginate`.

Validation of the C keys. T53 applies to every value of `[interop.c]` and
of `[bindings.<name>.c]`: unlike the keys of a binding table itself (T48),
these values need no name resolution and are checked when the manifest is
parsed, so a value outside its domain is the fatal
`project_manifest_invalid`; an unknown key in either table is
`project_manifest_unknown_key` (T35). When `"c"` is in `[interop] targets`
the effective prefix (`prefix`, else `[project] name`) must match the
prefix pattern, else `project_manifest_invalid`, so a project whose name
does not (longer than 31 characters, starting with `cpl`, or with `__` or
a trailing `_`) sets `prefix`. Every code of the table but the fatal
`project_manifest_invalid` is a span-less diagnostic of category `project` that the check driver
reports, never loading:

| Condition | Checked by | Code |
| - | - | - |
| `prefix`, `abi`, `artifacts`, `compiler`, `cflags`, `ldflags` outside their domains; a `type_names` value outside its pattern; an `exports` entry that is neither a string nor a table of `function`, `symbol`, and `int`; an `int` outside its five values; a path of `[bindings.<name>.c]` that is absolute or has a `..` segment; a `defines` entry that is not `NAME` or `NAME=VALUE` with `NAME` a C identifier | parsing (T53) | `project_manifest_invalid` (fatal, exit 2) |
| `[interop.c]` present while none of `"c"`, `"python"`, and `"node"` is in `targets` | `check_c_bindings` | `project_c_target_not_enabled` (E; `key`, `targets`) |
| Milestone 3: `[interop.python]` present while `"python"` is not in `targets` | `check_python` | `project_python_target_not_enabled` (E; `key`, `targets`) |
| Milestone 3: `[interop.python] package` or `version` outside its domain (`spec/python.md` Section 1) | `check_python` | `project_python_option_invalid` (E; `key`, `found`, `reason`) |
| Milestone 3: an `[interop.python] paths` directory that does not exist | `check_python` | `project_binding_path_missing` (E; `key` `"interop.python.paths"`, `path`) |
| Milestone 3: a backend-`python` binding whose `adapter` is not a dotted path of Python identifiers | `check_python` | `project_binding_adapter_invalid` (E; `binding`, `found`) |
| Milestone 3: a `timeout_ms` integer that is not positive, or `timeout_ms` on a backend-`c` binding with `isolation = "in_process"` (a value that is no integer is `project_manifest_invalid`, T53) | `check_python` | `project_binding_timeout_invalid` (E; `binding`, `found`) |
| Milestone 3: a `[test.fixtures]` value that names no binding / a binding whose capability is not the key | `check_python` | `project_fixture_binding_unknown` (E; `capability`, `binding`) / `project_fixture_capability_mismatch` (E; `capability`, `binding`, `found`) |
| a backend-`c` or backend-`python` binding whose name (that of its `[bindings.<name>]` table) does not match `^[a-z][a-z0-9_]*$`, so that its grant `<prefix>_bind_<name>` (C) or `bind_<name>` (Python) would be no identifier (cabi.md Section 5) | `check_c_bindings` | `project_binding_name_invalid` (E; `binding`) |
| Milestone 2 only: a backend-`c` binding whose `isolation` is absent (default `worker`) or not `"in_process"`; from Milestone 3 the code is not raised, since a C adapter may run in a worker (`spec/python.md` 6.6) | `check_c_bindings` | `project_binding_isolation_unsupported` (E; `binding`, `found`) |
| a backend-`c` binding whose `adapter` ends neither in `.c` nor in `.cbind.toml` | `check_c_bindings` | `project_binding_adapter_kind` (E; `binding`, `found`) |
| a backend-`c` `adapter` that is not a project-relative path (it is absolute or has a `..` segment) or names no regular file under the project root (a link to a file counts as that file, T54) | `check_c_bindings` | `project_binding_adapter_missing` (E; `binding`, `path`) |
| a `sources` file or a directory of `include_dirs` or `library_dirs` that does not exist | `check_c_bindings` | `project_binding_path_missing` (E; `binding`, `key`, `path`) |
| a `[bindings.<name>.c]` subtable on a binding whose backend is not `c` | `check_c_bindings` | `project_binding_c_options_misplaced` (E; `binding`) |
| two backend-`c` bindings of one capability | `check_c_bindings` | `project_binding_duplicate_capability` (E; `capability`, `bindings`) |
| a `.cbind.toml` that does not satisfy the closed schema of cabi.md Section 6 | `check_c_bindings` (the syntax) and `interop::plan` (the operations and types against the capability) | `project_cbind_invalid` (E; `binding`, `key`, `line`, `reason`) |
| an `exports` target that names nothing / several definitions | `interop::plan` | `project_export_unknown` (E; `entry`, `index`) / `project_export_ambiguous` (E; `entry`, `candidates`) |
| it names a declaration that is no function, or a generator | `interop::plan` | `project_export_not_function` (E; `entry`, `kind`) |
| it names a private function / a standard-library declaration | `interop::plan` | `project_export_not_public` (E; `entry`) / `project_export_not_project` (E; `entry`) |
| it names a generic function | `interop::plan` | `project_export_generic` (E; `entry`; a note names the project's monomorphic callers, as `cannot_run_generic`'s does) |
| an export symbol, explicit or the default one of the default set, outside `^<prefix>_[a-z0-9_]{1,56}$` or in a reserved family (`<prefix>_runtime_*`, `<prefix>_library_*`, `<prefix>_type_*`, `<prefix>_bind_*`, `<prefix>_adapter_*`) | `interop::plan` | `project_export_symbol_invalid` (E; `symbol`, `reason`) |
| two generated identifiers coincide (cabi.md Section 5) | `interop::plan` | `project_export_symbol_collision` (E; `symbol`, `items`) |
| `int` on an entry whose function has no `Int` at the top level of its signature | `interop::plan` | `project_export_int_unused` (W; `entry`) |
| a `type_names` key that names no type reachable from the exports | `interop::plan` | `project_type_name_unknown` (E; `key`) |
| the default export set left out a generic `pub fn` | `cpl build` only | `project_export_skipped_generic` (I; `function`) |

Where the checks live. `cpl_project::check_bindings` (T48) is unchanged:
it still accepts `backend = "c"` with any `adapter`, as its Milestone 1
test requires. The backend-`c` rules are the new
`cpl_project::check_c_bindings(&manifest, project.root.as_deref())`, whose
file checks are skipped when there is no root (a `VirtualTree`; `cpl
build` always has one), and the export, type-name, and `.cbind.toml`
semantics are `cpl_sema::interop::plan` (Section 1.6); the check driver
calls both after `check_bindings`. None of them fires on a Milestone 1
project: the only projects with `"c"` in `targets` are `examples/reader`
(no `[interop.c]`, one backend-`python` binding, four exportable functions
without collisions) and the case `errors/project/project_binding_bad_backend`
of the `cpl-sema` corpus (a capability and no function), and the
Milestone 1 tests that build a backend-`c` binding call `parse_manifest`
and `check_bindings` directly, never the check driver.

Decision: [D-270] T74. the C target is configured by `[interop.c]` and by
`[bindings.<name>]` tables with `backend = "c"`: such a binding names its
adapter file (a `.c` reviewed wrapper or a `.cbind.toml` declarative
binding), states `isolation = "in_process"`, the only C isolation of
edition 0.1 (cabi.md Section 6), and may say how to compile and link it in
`[bindings.<name>.c]`; at most one backend-`c` binding binds a capability.
When `[interop.c] exports` is absent, the export set is every `pub fn`
declared in the project that is neither generic nor a generator, in its
handle form with its default symbol; when it is present, the export set
is exactly its entries. Every rule is checked by `cpl check`, with the
codes of the table above. Rationale: the design's own manifest for
`reader` (lines 545-556) enables the C target and lists no exports, and its
Export example then imports `paginate_ints` from the generated package
(lines 576-594); `pub` already states a library's surface ("Declarations
are private unless prefixed with pub", line 71), so with the default the
verbatim corpus demonstrates the Export example unchanged, and generic
functions are skipped because they "require concrete export wrappers"
(line 504); the churn of a default surface is bounded by the lock, under
which a removed or changed export without an `abi` bump is
`build_abi_changed` while additions are compatible (Section 11.2); and
since a binding without `isolation` means `worker`, which C cannot provide
before the worker transport of Milestone 3, running native code in process
is a trust decision the manifest must state (design line 487) rather than
a default that silently changes meaning (T35). Rejected alternatives:
exports only from an explicit list (the verbatim corpus could not
demonstrate the Export example without editing its manifest, which is
design text); an `@export` annotation (a language change to the closed
annotation vocabulary for project configuration, which would make the C
name part of a declaration's facets); defaulting a backend-`c` binding to
`in_process`.

Decision: [D-243] T53. every value of `cpl.toml` that 11.1 does not admit
is the fatal `project_manifest_invalid`, not only a missing or ill-typed
required key: an ill-typed optional key, a `[project] name` that does not
match `^[a-z][a-z0-9_]*$`, a `[build] sources` entry that is not a
project-relative directory (absolute, `\`, an empty, `.`, or `..` segment;
`"."` itself is the project root), an `[interop] targets` entry other than
`python`, `node`, `c`, an `admit` entry that is not a stable id, an integer
out of range (`[test] cases = 0` included, T57), an unknown `evidence`, `filesystem`, `process`, or `network`
value, and a `[work.policy] files` entry that is not a file selector. The
values of a `[bindings.*]` table are the exception: they are checked after
name resolution and reported as diagnostics (T48). Rationale: T35 rejects
silently falling back to a default for a misspelled key, and a known key
with an unusable value is the same hidden context; a project whose
manifest cannot be read as written cannot be checked meaningfully, so the
load stops with the line of the offending value. Rejected alternatives:
ignoring a bad optional value in favour of its default (silent, which T35
forbids for keys); a diagnostic per bad value with the default in effect
(every later result would rest on a configuration the author did not
write).

Decision: [D-236] T48. a `[bindings.*]` table reports at most one diagnostic per
key: `backend` is checked for membership in `{python, node, c}` first, and
an invalid value is `project_binding_bad_backend` alone, never also
`project_binding_target_not_enabled` (an invalid backend can never be a
target, so the second check is skipped for it); a valid backend absent
from `[interop] targets` is `project_binding_target_not_enabled`;
`capability` (`project_binding_unknown_capability`) and `isolation`
(`project_binding_bad_isolation`) are validated independently of
`backend`, so one binding may carry up to three diagnostics, one per
offending key. Rationale: the two backend codes describe one wrong value,
and one actionable diagnostic per mistake is what the design asks of
structured failures. Rejected alternative: emitting both backend codes
(the second is fixed by the first's fix).

Decision: [D-176] T35. unknown manifest keys are errors (diagnostics with exit
code 1, not load failures), never warnings. Rationale: a misspelled
`proof_timeout_ms` silently reverting a verification knob to its default
is exactly the kind of hidden context the design exists to remove;
editions, not leniency, provide forward compatibility; and reporting the
key as a diagnostic rather than a `serde` failure keeps the message,
span, and exit code the same as every other project error. Rejected
alternatives: ignoring unknown keys; `#[serde(deny_unknown_fields)]`
(exit 2 with no `project_manifest_unknown_key` diagnostic).

Decision: [D-177] T36. there is no project-level switch that weakens `@prove`:
`cpl check --release` rejects every `@prove` declaration whose
verification status is `unknown` or `failed` with
`verification_unsatisfied`, and accepts `proved` and `conditional` (a
closed proof whose named assumptions stay visible in the receipt, Section
8.1); `[verification] admit` admits named *dependency* assumptions per
stable id and never a declaration's own obligations (semantics S36). With
no verifier, `cpl check --release` therefore fails on `examples/reader`
(one `verification_unsatisfied` for `paginate`) and on `examples/holes`
(two `hole_in_release`, one `verification_unsatisfied`), in both cases
with the warning `project_lock_missing` (T37). Rationale: the design says
`unknown` does not weaken the requirement (line 134), that admission is
per dependency and by project policy (line 136), and that release builds
require the configured obligations to be closed (line 225). Rejected
alternatives: a `release_requires_proofs = false` knob that turned the
error into a warning, with a `[verify] unknown = "admit"` alias (an
earlier revision of this decision; it deleted the meaning of `@prove` for
any project that flipped it, and semantics S36 rejects the same knob);
default `false`.

### 11.2 `cpl.lock`

```toml
# Generated by cpl. Do not edit.
version = 1

[toolchain]
cpl = "0.1.0"
edition = "0.1"
std = "<64 hex>"                # digest of the embedded standard library

[sources]                       # digest of every source-set file at lock time, by project-relative path (Section 0; never the lock itself)
"cpl.toml" = "<64 hex>"
"src/reader/images.cpl" = "<64 hex>"
"src/reader/pages.cpl" = "<64 hex>"
"src/tests/pagination.cpl" = "<64 hex>"
"work/pagination.patch" = "<64 hex>"
"work/pagination.work" = "<64 hex>"

[verification]                  # the manifest's [verification] values at lock time (design line 598: "verification configuration")
proof_timeout_ms = 10000
admit = []

[foreign]                       # reserved: python, node, c package locks (Milestones 2-4); c from Milestone 2 (below)
[adapters]                      # reserved: adapter versions and artifact hashes; the backend-c entries from Milestone 2
[targets]                       # reserved: runtime versions and ABI targets; c from Milestone 2
```

```rust
pub struct Lock { pub version: u32, pub toolchain: LockToolchain, pub sources: BTreeMap<String, Digest>,
                  pub verification: VerificationSection,          // Section 11.1's struct: the locked verification configuration
                  pub foreign: toml::Table, pub adapters: toml::Table, pub targets: toml::Table }
pub struct LockToolchain { pub cpl: String, pub edition: String, pub std: Digest }
/// The format is closed: a key or table the example above does not show is a LockError (`project_lock_invalid`, fatal),
/// as is text that is not UTF-8 or not TOML; the reserved tables are optional when parsing. From Milestone 2 their C
/// entries are validated against the closed schemas below, and a malformed one is a LockError too.
pub fn parse_lock(text: &str) -> Result<Lock, LockError>;
/// Canonical, LF line ends: the header comment, then the fixed keys in the order of the example above, `[sources]` and
/// the reserved tables sorted by key, the values of a reserved table written inline; the reserved tables are always
/// written, and an empty one is followed directly by the next header, as in the example.
pub fn render_lock(lock: &Lock) -> String;
```

| Rule | Behavior |
| - | - |
| writing | `cpl check --lock` writes `cpl.lock` for the current tree (`[sources]`, `[verification]`, `[toolchain]`; reserved sections carried through); the integrator rewrites it the same way in every integration commit; from Milestone 2 `cpl build --lock` writes the same and the C entries below (T75, T81) |
| reading | every project command parses an existing lock (loading does); `toolchain.cpl`/`edition` mismatch is the warning `project_lock_toolchain_mismatch`, with `expected` and `found` as `{ cpl, edition }` objects |
| staleness | `cpl check --release` compares a present lock's `[sources]`, `[verification]`, and `toolchain.std` with the tree and the manifest and fails with `project_lock_stale` (error) when any differs; a missing lock is the warning `project_lock_missing` (the corpus projects carry no lock); without `--release` neither is reported, and loading never reports either: the check driver of `cpl-sema` does (Section 3.4). The tree's values are: `[sources]` = `Project::manifest_digest` under `"cpl.toml"` plus the digest of every `SourceEntry` that is not std, by path; `[verification]` = `Manifest::verification`; `toolchain.std` = `Project::std_digest()`. `project_lock_stale` carries `differences` (those of `"sources"`, `"verification"`, `"toolchain.std"` that differ) and `paths` (the paths whose digests differ, or that one side lacks); both lock diagnostics are span-less. From Milestone 2, when the lock has a C entry, the comparison also covers the digests of the C build inputs, `"adapters"` and `"foreign.c"` (T75, below) |
| reserved sections | parsed as opaque tables and carried through unchanged by `render_lock`; from Milestone 2 their C entries have the closed schemas below (T75) |

From Milestone 2 three keys of the reserved tables have closed schemas
(T75): `[targets] c`, every entry of `[adapters]` whose value is a table
with `backend = "c"`, and `[foreign] c`. Every other key of the three
tables stays an opaque reserved value, carried through unchanged (Python
and Node use them from Milestone 3). The logical schema, shown with dotted
headers for reading, for the `cboundary` fixture (Section 12.7):

```toml
[targets.c]
api = "0.1"                                     # the C API version (cpl_rtspec::limits::C_API_VERSION)
runtime = "0.1.0"                               # cpl_rt::RUNTIME_VERSION
runtime_digest = "<64 hex>"                     # cpl_rt::runtime_digest of the runtime used (Section 1.12)
target = "x86_64-linux-gnu"                     # the compiler's -dumpmachine
compiler = "gcc 11.4.0"                         # family and version
compiler_path = "/usr/bin/x86_64-linux-gnu-gcc-11"   # the resolved driver binary
compiler_digest = "<64 hex>"                    # blake3 of that binary
flags_digest = "<64 hex>"                       # the effective flag set of the locked profile, the manifest's flags included (below)
system_digest = "<64 hex>"                      # the platform files the link opened: crt objects, libc, libgcc, libpthread (below)
prefix = "cboundary"
abi = 1
exports_digest = "<64 hex>"                     # the canonical export table (spec/cabi.md Section 9)
symbols = ["cboundary_bind_image_codec", "cboundary_clean", "…"]   # every name the version script exports, sorted (spec/cabi.md 1.6, 9.3)
[targets.c.exports]                             # one entry per export symbol
cboundary_paginate_ints = { function = "reader.pages.paginate_ints", id = "reader.pages.paginate_ints", signature = "<64 hex>", int = "handle" }

[adapters.image_codec]                          # one entry per backend-c binding
backend = "c"
form = "wrapper"                                # or "declarative"
isolation = "in_process"
capability = "reader.images.ImageCodec"
files = { "adapters/image_codec.c" = "<64 hex>" }

[foreign.c.image_codec]                         # the native dependencies of each backend-c binding
sources = { "vendor/imgtest/imgtest.c" = "<64 hex>" }
headers = "<64 hex>"                            # every header its compilations read, from their depfiles (below)
libraries = { extra = { path = "vendor/libextra/libextra.a", digest = "<64 hex>" } }   # by -l name: the file the linker opened for it (below)
```

`render_lock` writes them as it writes every value of a reserved table,
inline under the table's header, one line per entry with sorted keys, so
the canonical file is:

```toml
[foreign]
c = { image_codec = { headers = "<64 hex>", libraries = { extra = { digest = "<64 hex>", path = "vendor/libextra/libextra.a" } }, sources = { "vendor/imgtest/imgtest.c" = "<64 hex>" } } }

[adapters]
image_codec = { backend = "c", capability = "reader.images.ImageCodec", files = { "adapters/image_codec.c" = "<64 hex>" }, form = "wrapper", isolation = "in_process" }

[targets]
c = { abi = 1, api = "0.1", compiler = "gcc 11.4.0", compiler_digest = "<64 hex>", compiler_path = "/usr/bin/x86_64-linux-gnu-gcc-11", exports = { cboundary_paginate_ints = { function = "reader.pages.paginate_ints", id = "reader.pages.paginate_ints", int = "handle", signature = "<64 hex>" } }, exports_digest = "<64 hex>", flags_digest = "<64 hex>", prefix = "cboundary", runtime = "0.1.0", runtime_digest = "<64 hex>", symbols = ["cboundary_bind_image_codec", "cboundary_clean", "…"], system_digest = "<64 hex>", target = "x86_64-linux-gnu" }
```

Every digest of the C entries is a blake3 digest, as 64 lowercase hex
digits, of a file's bytes or of a text of LF-terminated lines that no
output root or checkout location enters, so that two builds of one
locked input set on one machine write one lock: the texts of
`targets.c.flags_digest`, `foreign.c.<b>.headers`, and
`targets.c.system_digest` are `spec/cabi.md` 1.5's and 6.6's. An entry of
`foreign.c.<b>.libraries` is, for each `-l<name>` of the binding, the
first file of the link's `-Wl,--trace` named `lib<name>.so` or
`lib<name>.a` (a linker script included), by its path (project-relative
under the project root, absolute elsewhere) and the digest of its bytes;
the files that such a script names are platform files, pinned in
`targets.c.system_digest`.

An export's `signature` is its ABI digest, whose inputs (the signature,
the integer mapping, the symbol, and the shapes of the types it reaches)
`spec/cabi.md` Section 9 fixes; a changed `signature` under one `abi` is
an ABI break.

```rust
// Milestone 2 (T75, T80): Lock keeps its fields; its C entries are read and written through typed views.
impl Lock {
    pub fn target_c(&self) -> Option<TargetC>;                         // [targets] c
    pub fn c_adapters(&self) -> BTreeMap<String, AdapterC>;            // the [adapters] entries with backend = "c", by binding
    pub fn foreign_c(&self) -> BTreeMap<String, ForeignC>;             // [foreign] c, by binding
    pub fn set_target_c(&mut self, target: Option<TargetC>);
    pub fn set_c_adapters(&mut self, adapters: BTreeMap<String, AdapterC>);   // replaces the backend-c entries, keeps the others
    pub fn set_foreign_c(&mut self, foreign: BTreeMap<String, ForeignC>);
}
pub struct TargetC {
    pub api: String, pub runtime: String, pub runtime_digest: Digest, pub target: String, pub compiler: String,
    pub compiler_path: String, pub compiler_digest: Digest, pub flags_digest: Digest, pub system_digest: Digest,
    pub prefix: String, pub abi: u16, pub exports_digest: Digest, pub exports: BTreeMap<String, LockedExport>,
    pub symbols: Vec<String>,                                          // sorted
}
pub struct LockedExport { pub function: String, pub id: StableId, pub signature: Digest, pub int: IntMapping }
pub struct AdapterC { pub form: AdapterForm, pub isolation: String, pub capability: String, pub files: BTreeMap<String, Digest> }
pub struct ForeignC { pub sources: BTreeMap<String, Digest>, pub headers: Digest, pub libraries: BTreeMap<String, LockedLibrary> }
pub struct LockedLibrary { pub path: String, pub digest: Digest }
// TargetC, LockedExport, AdapterC, ForeignC, and LockedLibrary derive Clone, Debug, PartialEq, Eq.
/// The digests of the build inputs of every backend-c binding, read from the project root: `adapters` its adapter file,
/// `sources` its [bindings.<name>.c] sources, each by binding and project-relative path; what `adapters.<b>.files` and
/// `foreign.c.<b>.sources` record. A file that cannot be read has no entry (check_c_bindings reports it).
pub fn c_input_digests(manifest: &Manifest, root: &std::path::Path) -> CInputDigests;
pub struct CInputDigests { pub adapters: BTreeMap<String, BTreeMap<String, Digest>>, pub sources: BTreeMap<String, BTreeMap<String, Digest>> }
```

When each part of the lock is written and checked (T75):

| Part | Written by | Checked by | Mismatch |
| - | - | - | - |
| `[sources]`, `[verification]`, `toolchain.std` | `cpl check --lock`, `cpl build --lock`, the integrator | `cpl check --release`, `cpl build --release` (T37) | `project_lock_stale` |
| `adapters.<b>.files`, `foreign.c.<b>.sources` | `cpl build --lock`; `cpl check --lock` and the integrator carry them through | `cpl check --release` and `cpl build --release`, when the lock has a C entry and the project a root: computed from the files (`c_input_digests`), no build step | `project_lock_stale`, with `differences` gaining `"adapters"` and `"foreign.c"` and `paths` the files concerned, a binding present on one side only contributing all of its files |
| `targets.c` (every key), `foreign.c.<b>.headers`, `foreign.c.<b>.libraries` | `cpl build --lock` | `cpl build --release`, after compiler discovery and the build | `build_lock_mismatch` (`differences`: the dotted keys that differ, in key order, without `targets.c.api`, `exports`, `exports_digest`, and `symbols` when the same comparison reports `build_abi_changed`) |
| an ABI break by `spec/cabi.md` 9.3 (a removed or changed export, a missing exported symbol, another C API major), while `prefix` and `abi` are unchanged | | `cpl build --release` (against the lock); `cpl build --lock` (against the lock it replaces, T81) | `build_abi_changed` (`symbol`, `id`, `expected`, `found`), one per break |
| no `cpl.lock`, or a lock without `targets.c` | | `cpl build --release`, with the release check | `build_lock_missing`, in place of the check's warning `project_lock_missing` |

Development builds compare nothing (T37), except that `cpl build --lock`
refuses to record an ABI break (T81), and `cpl check --release` keeps
T37's warning for a missing lock, so the counts of semantics Section 11
are unchanged.

From Milestone 3, `cpl build --target python --lock` also writes
`targets.python` (`package`, `version`, `api`, `compiler_path`,
`compiler_digest`, `flags_digest`, `runtime_digest`,
`python_headers_digest`) and one `adapters.<b> = { backend = "python",
module, digest }` per backend-`python` binding, the digest being the
binding's input digest of `spec/python.md` Section 9; `cpl build --target
python --release` compares `targets.python` after the build
(`build_lock_mismatch`, `differences` its dotted keys) and the Python
adapters' digests with the check, as `adapters.<b>.files` are compared
(`project_lock_stale`, `differences` gaining `"adapters"`); and a lock
without `targets.python` is `build_lock_missing` for it. `Lock` gains
`target_python()`, `set_target_python()`, `python_adapters()`, and
`set_python_adapters()`, as the C views are.

Decision: [D-178] T37. the lock records source digests and the verification
configuration but is only *checked* under `--release` and only *written*
on request (`--lock`) or by the integrator; under `--release` a present
but stale lock is an error and an absent lock is a warning. Rationale: the
design requires source digests and verification configuration in the lock
record (line 598), yet a lock that must change with every edit would be
noise during development; tying staleness to release checks and
integration makes it meaningful exactly where reproducibility is claimed,
and a missing lock cannot be stale, so treating it as an error would make
`cpl check --release` unusable on a project that has never been locked
(the corpus) and would make the acceptance counts of Section 12.3
unreachable. Rejected alternatives: rewriting the lock on every `cpl
check` (constant churn in diffs); `project_lock_stale` for a missing lock
(an earlier revision of this decision).

Decision: [D-271] T75. the C entries of `cpl.lock` have closed schemas
inside the reserved tables of Milestone 1 (`[targets] c`, the `[adapters]`
entries with `backend = "c"`, `[foreign] c`), are rendered inline as every
value of those tables is, and are written by `cpl build --lock` alone,
`cpl check --lock` and the integrator carrying them through; a malformed
C entry is `project_lock_invalid`. Under `--release`, `cpl check` and `cpl
build` compare the digests of the C build inputs when the lock has a C
entry (`project_lock_stale`), and `cpl build` also compares every other C
entry after compiler discovery (`build_lock_mismatch`, `build_abi_changed`)
and requires a lock with `targets.c` (`build_lock_missing`). This amends
T37's table, whose staleness gains the C build inputs, and not its rule
for `cpl check`, under which a missing lock stays a warning and
development mode compares nothing. Rationale: the design's lock record
includes foreign package versions and artifact hashes, adapter versions,
runtime requirements, and the target ABI (line 598), and native headers
alone do not identify the linked library version (line 600), so the
headers the compiler read and the libraries the linker opened are pinned;
a release artifact claims reproducibility, which needs the compiler, the
runtime, and the native inputs pinned, hence `build_lock_missing`; and the
checker compares only what it can compute from files, because the runtime
digest and the export table are known to `cpl-rt` and `cpl-emit-c`, on
which `cpl-sema` must not depend (T69). Rejected alternatives: new fields
on `Lock` (Milestone 1 tests build it literally, T80); sub-table headers in
the canonical file (more readable, but they change `render_lock`'s
Milestone 1 output, which a test pins); recording headers but not
libraries; C package locks in `[adapters]` alone (the table the lock
reserves for package locks is `[foreign]`); comparing the runtime digest
and the export table in `cpl check --release` (that would put the code
emitter behind the checker).

Decision: [D-327] T81. `cpl build --lock` compares the build it is about
to record with the lock it replaces when both have the same `prefix` and
`abi`: an ABI break by `spec/cabi.md` 9.3 (an export removed, or one whose
`signature` or `int` changed, a name of `targets.c.symbols` missing,
another C API major) is `build_abi_changed`, the command exits 1, and
`cpl.lock` is left unchanged; exports and symbols added are recorded. This
amends T37, under which only `--release` compares a lock with the build;
which changes are breaks stays cabi.md 9.3's. Rationale:
`project_lock_stale` and `build_lock_mismatch` are resolved by locking
again, so without this check `cpl build --lock` would record an
incompatible export table or symbol list under the old `abi`, and `cpl
build --release`, which compares the build with the lock (T75), could
never see the break that the unchanged soname and version node promise not
to make (`spec/cabi.md` Section 9); an author who means to break the ABI
raises `[interop.c] abi`. Rejected alternatives: checking only under `--release` (the first
re-lock launders the break); comparing with the lock of a tagged release
(the toolchain knows no release tags).

## 12. Testing strategy

| Layer | Location | What it asserts |
| - | - | - |
| golden parse/format | `crates/cpl-syntax/tests/golden.rs` over three input sets: `examples/**/*.{cpl,work,patch}` (goldens under `crates/cpl-syntax/tests/golden/examples/<path>.<ext>.ast`, since `examples/` is read-only and `pagination.work` and `pagination.patch` share a stem), the syntax corpus `crates/cpl-syntax/tests/corpus/**` (canonical inputs covering every declaration, expression, pattern, and statement form, the std stub forms, comment attachment, and the layout rules, plus non-canonical inputs under `layout/` with `.fmt` goldens), and the design examples E1..E12 in `tests/golden/` (Section 12.1) | for input `X`: `X.ast` (stable AST dump, Section 12.1) matches; `X.fmt` matches when present, otherwise `format(X) == X`; every `.fmt` golden is itself canonical; every set has inputs of all three file kinds; no two inputs share a golden and no golden lacks its input; `UPDATE_GOLDEN=1` writes a `.fmt` only for a non-canonical corpus input, and a non-canonical example always fails |
| formatter idempotency | `crates/cpl-syntax/tests/idempotent.rs` over every input of every corpus in the workspace: the syntax corpus and goldens, the sema, eval, and index corpora, `examples/`, and `std/` (an input that fails to parse, a negative case, is skipped and counted; the examples, the stubs, the syntax corpus, and the goldens must parse) | `fmt(fmt(x)) == fmt(x)`; `strip_layout(parse(fmt(x))) == strip_layout(parse(x))` where `strip_layout` undoes every change the formatter may legitimately make: it clears spans, `Group::multiline`, `Trivia::blank_line_before`, `Comment::blank_line_before`, `BinaryExpr::break_before_op`, and every comment (`Trivia::leading`/`trailing` and every `trailing_comments` list), removes the trailing spaces and tabs of every `DocLine::text` (2.4 rule 1), and puts header clauses and `.work` items into the canonical orders of T13 (2.4 rules 7 and 12; a stable sort, so `requires` clauses keep their source order) |
| lexer and parser units | `crates/cpl-syntax/tests/lexer.rs`, `parser.rs` | every grammar decision `D1`..`D73` has at least one positive and one negative case in these two files, named by number (`d<n>_positive_...`, `d<n>_negative_...`), which a self-checking test enforces; for a decision that constrains the corpus rather than the grammar (D57, D58) the negative case checks that the corpus does not take the decision's rejected alternative |
| negative typecheck | `crates/cpl-sema/tests/corpus/errors/**/*.cpl` (with `ok/` and `release/`) + `crates/cpl-sema/tests/corpus.rs` | Section 12.2 markers match the diagnostics exactly; a diagnostic whose code is in no code table (semantics 9.4, Appendix B) or whose category differs from its table's fails the case |
| positive typecheck | `crates/cpl-sema/tests/corpus/ok/**/*.cpl` and `examples/` (`crates/cpl-sema/tests/examples.rs`) | zero errors, an `ok/` case failing on any error even under an `expect-error` marker; warnings must be listed with `// expect-warning`; `examples/` checks with exactly the development-mode diagnostics of semantics Section 11 (infos only) |
| hole reports | `crates/cpl-sema/tests/holes.rs` over `examples/holes` | expected type, bindings, contracts of `?build_pages` and `?summary` |
| facets and ids | `crates/cpl-index/tests/facets.rs`, `ids.rs` (the `generate_id` format) | canonical texts and hashes for every declaration of `examples/reader` (golden `facets.json`); `generate_id` format; provisional ids; duplicate detection |
| semantic diff | `crates/cpl-index/tests/diff.rs` | pairs of corpus trees (`before/`, `after/`) with expected statements (`diff.json`) covering every statement in Section 9.1 |
| interpreter examples | `crates/cpl-eval/tests/examples.rs` | `run_tests(examples/reader, seed = 1)` all pass; `run_function` table (Section 12.3); the capability-using functions of `examples/reader` (`load_page` and `load_text` against a scripted `PageRead`, `image_width` against a scripted `ImageCodec`) run with hosts bound by `Interpreter::bind_capability`, `Ok`, `Err`, and `ForeignError` paths included, so every function of the design's examples executes |
| interpreter corpus | `crates/cpl-eval/tests/corpus/*.cpl` + `*.run.json`, read by `crates/cpl-eval/tests/corpus.rs` over the checker's output | each case `{ "function", "args", "expect": { "value": json } \| { "fault": code } }`, read as Section 12.5 (T49) fixes |
| property runner | `crates/cpl-eval/tests/properties.rs` | a deliberately wrong `paginate` yields a `contract_counterexample` for a fixed seed with the expected shrunk inputs; determinism: two runs with one seed give identical reports |
| work and patch | `crates/cpl-work/tests/validate.rs`, `patch.rs` | `validate_work_file` on `examples/reader/work/pagination.work` with `repo: None` reports no errors (only the warnings `git_unavailable` and `work_provisional_identity` for `Positive`); mutated copies trigger each `work_*` code; `pagination.patch` applies to a copy of `examples/reader` whose `paginate` body is replaced by `{ ?build_pages }` (same module path, so the patch's selectors resolve through `pagination.work`'s `use` line) and is `patch_stale` against a copy whose body was changed in any other way |
| git wrappers | `crates/cpl-index/tests/git.rs`, `crates/cpl-work/tests/git.rs` | against a temporary repository created in the test (`git init -b main`, two commits): `tree_at`, `current_revision`, `update_ref_cas` success and conflict |
| CLI | `crates/cpl-cli/tests/cli.rs` and the adversarial suites beside it | Section 12.4 |
| shared tables (Milestone 2) | `crates/cpl-rtspec/tests/tables.rs` | `BUILTINS` holds exactly the 59 ids of stdlib 10.6 with the precondition texts and inputs of the stubs; `RANGES` is the 25 scalars of stdlib S22; every template renders with exactly its placeholders; the evaluator's registry, `cpl_sema::codes::BUILTIN_IDS`, and `IDS` list the same ids |
| C runtime | `crates/cpl-rt/tests/runtime_c.rs` over the C unit tests `crates/cpl-rt/tests/c/*.c`; `generated_tables.rs`; `api_table.rs` | each C unit test compiled with the runtime in the debug, optimized, and sanitized configurations and run (through `cpl_rt::testing`, linked with `tests/c/program_stub.c`, Section 1.12); `cpl_rt_gen.h` is what `cpl-rtspec` generates, and the C renderer equals `cpl_rtspec::messages::render` for every template; `struct cpl_api` in `cpl.h` matches the frozen entry order of `crates/cpl-rt/api_v0.txt`, an entry being added only with a raised minor version (`spec/cabi.md` Section 4) |
| emitter | `crates/cpl-emit-c/tests/golden.rs` (with `golden/`), `shape.rs`, `compile.rs`, `tables.rs`, `cbind.rs` | the goldens of Section 12.7; the slot discipline of cabi.md Section 1 on the emitted text of every corpus program; every corpus program's emitted C and the runtime compile with `-Werror` (gcc, and clang when present) and every generated header passes `g++ -std=c++17 -fsyntax-only`; the table-fidelity test: for every `DefId`, site, clause, hole, and stub of every corpus program, the emitted model gives the `Program`'s own answers, and the compiled tables give them through the driver's `tables` request, and `cpl_sema::interop::type_text` names every declaration as `cpl_eval::types::type_name` does; declarative wrapper generation against the test library |
| native engine and C boundary | `crates/cpl-cabi/tests/differential.rs` (with `differential/`), `protocol.rs`, `c_hosts.rs`, `run_bindings.rs`, `threads.rs`, `oom.rs` | Sections 12.6-12.8 |
| C manifest, lock, and plan | `crates/cpl-project/tests/interop.rs`, `lock_c.rs`, `cbind.rs`; `crates/cpl-sema/tests/interop.rs`; directory cases `crates/cpl-sema/tests/corpus/errors/project/<code>/` | every rule of Sections 11.1 and 11.2 with its negative cases; export plans (the default set, resolution, names, collisions); a directory case with `(global)` markers for every `project_*` code that `cpl check` reports, run by the unchanged harness of Section 12.2 |
| evaluator hooks | `crates/cpl-eval/tests/m2_hooks.rs` | `bind_capability_with` and its validation (semantics S77), `foreign_failure` (stdlib S45); `TestEngine` over `Interpreter` giving `run_tests`'s reports; `prepare_run`, `run_function_with_hosts` with its two-step hosts, `requires_spec`, `check_gen_range_at`, `operation_identity`; `scope_at` equal to the interpreter's environment at every binding site of every corpus program (Section 1.7.4) |
| CLI, Milestone 2 | `crates/cpl-cli/tests/build.rs`, `engines.rs`, `workspace_dag.rs`, `acceptance_m2.rs` | `cpl build`'s flags, layout, envelope goldens, exit codes, `--lock`, and `--release`; D3 (Section 12.6); the forbidden edges of Section 1.2; the checklist of Section 12.9 on fresh committed copies |

Golden files are updated by running the suite with `UPDATE_GOLDEN=1`; CI
runs without it. All tests are plain `#[test]` functions driven by directory
walks; no external test harness crate. Milestone 2 edits no Milestone 1
test file: every new test is a new file or a new directory case, so the
accepted suite of Section 13 stays the evaluator's evidence.

### 12.1 AST dump

`cpl_syntax::dump_module(&Module) -> String` (with `dump_work` and
`dump_patch` for the other file forms, Section 1.5) renders a tree as a
deterministic S-expression without spans: every node is `(head child*)`,
doc comments are `(docs "line" ...)`, and a node's trivia, when it has any,
is its last child, `(trivia blank (lead "// c") (lead-blank "// d") (trail
"// e"))`, where `blank` records a blank line before the node, `lead-blank`
a leading comment preceded by a blank line, and `trail` a trailing comment.
An annotation renders as `(annotation name arg*)` and one written with an
empty argument list, `@name()`, as `(annotation name ())`
(`Annotation::parens`, Section 2.2).
A list that fits in 100 columns, or that contains no nested list, prints
on one line; any other list keeps its leading atoms on the first line and
puts each further child on its own line, indented by two spaces. The dump
of grammar E1 (design lines 48-69, `crates/cpl-syntax/tests/golden/E1.ast`):

```text
(module reader.pages
  (use std.text (items trim) (trivia blank))
  (type PageId pub (wrapper (ty Text)) (trivia blank))
  (type Positive pub (refinement (ty Int) (where (> (name self) (int 0)))))
  (struct Page pub (field id (ty PageId)) (field text (ty Text)) (trivia blank))
  (enum StoreError pub (variant Missing (ty PageId)) (variant Unavailable) (trivia blank))
  (fn clean
    (params (param page (ty Page)))
    (ret (ty Page))
    (block
      (tail
        (struct-lit Page
          (spread (name page))
          (field text (call (name trim) (member (name page) text))))))
    (trivia blank)))
```

The dump is the parser's golden format (`crates/cpl-syntax/tests/golden/`);
it is not a stable public contract beyond the test suite.

### 12.2 Negative typecheck markers

```text
let n: Text = 7 // expect-error: type_mismatch
assert paginate(items, 0) == [] // expect-error: refinement_false
// expect-error@+1: unresolved_name
let y = undefined_name
// expect-info: hole   (as a trailing comment on the hole's line, or @+n)
// expect-warning(global): project_lock_missing
```

Decision: [D-179] T38. the marker is a trailing comment `// expect-error: <code>[,
<code>]*` on the line where the diagnostic's primary span *starts*; the
form `// expect-error@+N: <code>` on its own line refers to the line N
lines below; `// expect-error(global): <code>` anywhere refers to a
diagnostic without a span. `expect-warning` and `expect-info` follow the
same forms. The harness compares the exact multiset of (line, severity,
code) pairs; an unexpected diagnostic or an unmatched marker fails the
test. Rationale: trailing markers keep the expectation next to the
offending code and survive formatting (trailing comments stay on their
line, T11); the `@+N` form covers lines that cannot take a trailing
comment (a line ending inside a text literal cannot; one inside an open
bracket can). Rejected alternative: a sidecar `.expected` file per test
(drifts from the source).

Each negative file is a complete module. The harness wraps it in a
`VirtualTree` with a synthetic `cpl.toml` (`name = "errors"`, or `"ok"` for
the positive tree, `edition = "0.1"`), placing the file at the conventional
path of its `module` header (`module negative.x` is `src/negative/x.cpl`)
so that the wrapper itself produces no `module_file_misplaced`; the header
may be anything except a reserved path. Multi-file cases use a directory
with its own `cpl.toml`, loaded as the project, and may carry markers in
any of their `.cpl` files. A `(global)` marker matches a diagnostic that
has no span or whose span lies outside the case's `.cpl` files (in the
manifest). Cases under `release/` are checked with `CheckConfig { release:
true, .. }` and, carrying no `cpl.lock`, each expect the span-less warning
`project_lock_missing`. The line a marker names is the line where the
primary span of semantics S63 starts. For a multi-file case the harness
compares (file, line, severity, code), which T38's (line, severity, code)
is for a single file; it prints a summary line `corpus: <n> files, <p>
pass, <f> fail`.

### 12.3 Interpreter example table

| Call | Expected |
| - | - |
| `reader.pages.paginate_ints([1, 2, 3], 2)` | `Ok([[1, 2], [3]])` |
| `reader.pages.paginate_ints([], 2)` | `Ok([])` |
| `reader.pages.paginate_ints([1], 0)` | `Err(ConstraintError...)` (Library: payload shape) |
| `reader.pages.divide_exactly(6, 3)` | `2` |
| `reader.pages.divide_exactly(7, 0)` | fault `contract_violation`, `clause_kind: "requires"`, `clause: "b != 0"`, `input: {"a": 7, "b": 0}` |
| `reader.pages.divide_exactly(7, 2)` | fault `contract_violation`, `clause_kind: "requires"`, `clause: "a % b == 0"` |
| `reader.pages.positive_size("  12 ")` | `Ok(12)` |
| `reader.pages.positive_size("-3")` | `Err(...)` |
| `holes.pages.summarize(Page { id: PageId("p"), text: "t" })` | fault `unresolved_hole`, `hole: "summary"` |
| `holes.pages.paginate` via `cpl run` | `cannot_run_generic` (exit 2) |
| `cpl test` on `examples/reader` with `--seed 1` | 3 selected, 3 tested; three `test_result` records with `status: "tested"` |
| `cpl check` on `examples/reader` / `examples/holes` | exit 0; `verification_result` (info, unknown) for `paginate`; `holes` adds two `hole` (info) reports |
| `cpl check --release` on `examples/holes` | exit 1 with exactly two `hole_in_release` and one `verification_unsatisfied` errors, plus the warning `project_lock_missing` |
| `cpl check --release` on `examples/reader` | exit 1 with exactly one `verification_unsatisfied` error (semantics Section 11), plus the warning `project_lock_missing`; exit 0 needs a verifier that proves `paginate` (Milestone 5), never a manifest switch (T36) |

From Milestone 2 every `cpl run` and `cpl test` row of the table also runs
with `--engine native` and must give the evaluator's envelope byte for
byte after the S75 normalization (suite D3, Section 12.6).

### 12.4 CLI integration tests

`crates/cpl-cli/tests/cli.rs` runs the built binary
(`env!("CARGO_BIN_EXE_cpl")`) with `std::process::Command`:

1. Copy `examples/reader` and `examples/holes` into a temporary directory
   per test; run `git init -b main` (so that the corpus's `git("main")`
   resolves whatever the user's `init.defaultBranch` is), `git add -A`,
   `git commit` there so revision and diff commands have a repository
   (skip those tests with a message when `git` is absent; the suite passes
   with `git` removed from `PATH`).
2. For every command of Section 7, assert the exit code and parse stdout
   as JSON when `--json` was given; compare against golden files under
   `crates/cpl-cli/tests/golden/`, at least one per command form (`check`,
   `fmt`, `run`, `test`, `inspect`, `prove`, `diff`, `id assign`, `work
   check`, and the reserved `work run` and `work integrate`), after
   normalizing volatile fields (`duration_ms`, `produced_at`, the `commit`
   and `digest` of every revision object, `old`, `new`, and
   `changes.since` included, and `base_commit`; OS-chosen seeds; the ids
   `cpl id assign` draws, normalized to `<kind>_<generated>` after a
   separate assertion that each has the generated format and none repeats;
   and absolute paths). The golden files hold std digests and content and
   facet hashes, so a change to `std/` or to the formatter's canonical
   output requires regenerating them (`UPDATE_GOLDEN=1`).
3. Mutation tests: `fmt --check` on a de-formatted copy exits 1 and names
   the file; `id assign` then `check` shows no provisional ids; `diff` of
   the commit before and after a contract edit yields `contract_changed`;
   `work check` on a `.work` file with overlapping writes exits 1 with
   `work_overlapping_writes`.
4. Human output tests assert only the stable lines (Section 1.4.1) and the
   summary line formats.
5. From Milestone 2, `crates/cpl-cli/tests/build.rs` and `engines.rs` do
   the same for `cpl build` and for `--engine`, with the goldens of
   Section 12.7, normalized as above plus the digests and the compiler's
   `path`, `version`, `digest`, and `target` (the triple: `x86_64-linux-gnu`
   on one distribution is `x86_64-redhat-linux` on another).

### 12.5 Interpreter corpus format

Each `<name>.cpl` under `crates/cpl-eval/tests/corpus/` is a complete
module `corpus.<name>`; its paired `<name>.run.json` is a JSON array of
case objects, read by `crates/cpl-eval/tests/corpus.rs`. The harness loads
each module at `src/corpus/<name>.cpl` of a project of its own, checks it
with `cpl-sema` (so the cases run on the checker's output, with zero
errors and exactly the diagnostics the corpus README names for the
module), and calls `run_function`; a malformed case object, a wrong module
header, a `function` outside its module, or an unpaired file is a harness
error, and the fields a `// why:` comment promises for a fault case are
compared exactly. It prints `eval corpus: <n> cases, <p> pass, <f>
fail`.

Decision: [D-237] T49. a case object has exactly the keys `function`, `args`,
and `expect`, and `expect` has exactly one key, `value` or `fault`; any
other key is a harness error. `function` is the qualified name (semantics
1.4) of a function declared in the paired module, public or private
(`corpus.lists.take_n`), the form `cpl run` and `run_function` accept (7.2,
1.7). `args` is an array with one value-JSON element (7.3, semantics S40)
per positional parameter, decoded against the declared parameter type
exactly as `cpl run --arg JSON` decodes it; an argument that fails a
refinement-typed parameter's predicate is the fault `refinement_violation`
(semantics S39 (a)), never `usage_bad_argument`, because the harness calls
`run_function` with `RunArg::Json`. `expect.value` is the value JSON of the
result encoded against the declared return type (a refined `Int` is a plain
number; `$type` is bare for prelude types and qualified for corpus types;
`$payload` is omitted for payload-less variants; an `Err(...)` is a value,
not a fault). `expect.fault` is a fault code of semantics 6.7, and only the
code is compared; a module that must pin a fault's other fields states them
in a `// why:` comment above the function and is written so that the code
plus the function identify them. Every case runs in checked mode with the
default `EvalConfig`, the embedded standard library, a fresh evaluation per
case, and no calling declaration (a top-level `requires` violation carries
no `caller`, S39 (c)); every corpus module must pass `cpl check` with no
errors. Rationale: a closed shape keeps the format a lookup table that two
harness implementations read identically, and routing arguments through
`run_function` gives the corpus exactly the semantics it exists to test.
Rejected alternatives: extra expectation keys for fault fields (a second
schema beside the diagnostic envelope); bare function names (ambiguous
once a module and a test share a spelling).

### 12.6 Differential suites (Milestone 2)

The native engine is validated against the evaluator, the oracle (T68,
semantics S75), by eight suites. This section owns the suites, their
comparison rules and normalization, the coverage gate, and the gate rule
(T78); `spec/cabi.md` Section 3 owns the mechanisms they rest on (the
driver's requests and counters, the C renderings). D1, D2, and D4-D8 are
`crates/cpl-cabi/tests/differential.rs`, the test binary `differential` of
`cargo test -p cpl-cabi`, which drives a `cpl_eval::Interpreter` and a
`cpl_cabi::NativeSession` over the same checked `Program`; D3 needs the
built `cpl` binary, which only `cpl-cli`'s tests have
(`CARGO_BIN_EXE_cpl`), and is `crates/cpl-cli/tests/engines.rs`.

| Suite | Corpus | Compared |
| - | - | - |
| D1 interpreter corpus | the 74 cases of `crates/cpl-eval/tests/corpus/*.run.json` (12 modules, Section 12.5), each with `check_contracts` true and false (the driver accepts false, `spec/cabi.md` Section 3) | value and fault bytes |
| D2 standard library | every row of stdlib Section 12 as functions of `crates/cpl-cabi/tests/differential/corpus/stdlib.cpl`, and the laws of stdlib 10.7 (I, T, B, L, O, X) transcribed as properties | value and fault bytes; test records |
| D3 acceptance table | every `cpl run` and `cpl test` row of Section 12.3, on fresh committed copies of `examples/reader` and `examples/holes`, with `--engine eval`, `native`, and `both`, `cpl test` with `--seed s` for s in 1..=32 | the `eval` and `native` envelopes byte-identical after the S75 normalization below; no `engine_divergence` under `both` |
| D4 targeted backend corpus | the modules `crates/cpl-cabi/tests/differential/corpus/*.cpl`, each with a `*.diff.json` of cases (below), and the Rust cases of `differential/scripted.rs` and `differential/builder.rs`: every `ExprKind`, `Stmt`, and `PatternKind`; every fault kind and payload key; every claim path of semantics S34, S70, and S71; S35 through helpers and inside `requires`; operation clauses and every outcome of semantics S77 with scripted capabilities (`scripted.rs`: valid, ill-typed, and refinement-violating results; foreign errors with and without a `ForeignError` arm); a refinement over a refinement (`type Small = Positive where self < 10`) as an argument and as an operation's result, one position (semantics S74); `max_call_depth` 0, 1, and 10,000, and the stack guard and a time budget on both engines; `?` exits with `ensures`; struct spread order; values nested 10^6 deep; integers of 10^4 digits; every White_Space scalar, U+200B, U+FEFF, combining marks, non-ASCII case; and states no checked program reaches (an unsound `Refine`, no matching arm, a `Gen` as a value, a builtin type called), built with `cpl_eval::testing::ProgramBuilder` (`builder.rs`) | value and fault bytes |
| D5 input sweeps | every monomorphic function of D1, D2, D4, and D6 that has at least one parameter, only data parameters, and no `using` parameter: N = 200 argument tuples, tuple i drawn at size min(i, 100) (the size of case i, Section 1.7.2) by the sweep rules below from a ChaCha20 stream keyed by `CPL_DIFF_SEED` and the function's identity, each case under a time budget of 250 ms on both engines | value and fault bytes; a divergence prints its seed and tuple |
| D6 harvested programs | every CPL module source that the tests of `cpl-eval` embed (about a hundred module literals, with the variants their helpers assemble) and every module of `crates/cpl-sema/tests/corpus/ok/**`, extracted once into `crates/cpl-cabi/tests/differential/harvested/` (regenerated with `UPDATE_HARVEST=1`) and run with all their tests and properties and with D5's sweeps; a module that does not check is counted and skipped | values, faults, test records |
| D7 runtime fuzz | `Int` `+ - * / %`, the comparisons, `to_text`, and `parse_int` on random and edge integers (every limb boundary, 1 to 2,000 digits, every sign mix); UTF-8 offsets on random bytes; `quote_text`, value text, value JSON, and the JSON reader on random Unicode (the reader and the writers through the driver's `from_json` and `render` requests, `spec/cabi.md` 3.8); the text builtins; 10,000 inputs per operation | the C results against the evaluator's builtins and `cpl_diag::quote_text` |
| D8 sanitizers | D1, D2, D4, D5, and D6 with the driver built `--sanitize address,undefined` and run with `ASAN_OPTIONS=detect_leaks=1:halt_on_error=1` and `UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1` | no ASan, UBSan, or LSan report and no other output on the driver's stderr; zero live objects at shutdown |

D4 cases. A `*.diff.json` file is a JSON array of case objects with the
keys `function` and `args` of T49 (Section 12.5) and two optional keys:
`config`, an object of any of `max_call_depth`, `time_budget_ms`, and
`check_contracts` (an absent key takes `EvalConfig`'s default), and
`expect`, T49's expectation, which is checked against the evaluator before
the engines are compared; any other key is a harness error. A case runs
as `run_function` runs a T49 case, under its configuration, and as the
driver's `call` with `check_arguments` true. A function with `using`
parameters has no JSON case: `scripted.rs` builds each `protocol::Script`
in Rust, ill-typed results included, and binds it on both engines under
one label, the evaluator through `run_function_with_hosts` with
`cpl_cabi::scripted::ScriptHost` and the driver through `bind_scripted`
(Section 1.14). `builder.rs` emits the programs it builds by hand with a
synthetic `Project` and `InteropPlan::empty` (Section 1.13).

Sweep rules (D5, and the sweeps of D6). A parameter type is drawn by its
distribution of stdlib 8.3 when it has one (`Int`, `Bool`, `Text`,
`List<T>`, `Option<T>`, `Positive`); otherwise a struct field by field in
declaration order, an enum by a variant drawn with `below(n)` (stdlib 8.3)
over its `n` variants and then its payloads in order, a wrapper by its
inner type, `Unit` as `Unit`, `Bytes` as the bytes of a
`gen.list(gen.range(0, 255))` draw, and any other refinement by its base,
a draw that its chain rejects being drawn again. A recursive type is drawn
finitely: a value of a data type drawn inside a value of the same type is
drawn at a quarter of the size, and where that size is 0 an `Option` is
`None` and an enum's variant is drawn among those whose payloads cannot
contain a type being drawn (among all when none is such). A function with a
parameter for which 100 draws in a row are rejected is reported as
undrawable and not swept. Each case runs on the evaluator first. A case
whose outcome on either engine is a time fault or a guard fault (rule 2
below) counts as skipped; a case that the evaluator skips, or whose result
has a value JSON longer than 16 MiB (a length the harness computes over
the value's shared structure without rendering it), is not sent to the
driver; every other case is compared. A driver that does not answer a
request within 60 seconds is killed, and the case is a divergence. The
harness prints the compared, skipped, and undrawable counts of every
function, and the suite fails when more than 2% of its cases are skipped.

Comparison rules. For each case: (1) both outcomes are rendered by the
code that `cpl run` and `cpl test` use (`value_to_json`, `value_text`,
`Fault::to_diagnostic`, `Diagnostic::to_json`, the runner's records) and
the strings are compared byte for byte after the S75 normalization below
and nothing else; (2) the native side's own renderings (a value's C value
JSON and value text, a fault's C diagnostic) are compared byte for byte
with the Rust renderings of the same outcome, because they are what a C
host receives; (3) at shutdown the driver's runtime closes with zero live
handles and objects. A divergence prints both compact JSON strings, the
offset of their first differing byte, the program, the function, and the
arguments, so that `cpl run --engine both` replays it.

S75 normalization. Every comparison of two engines' outputs
(`cpl_cabi::compare`, `--engine both`, these suites, `run_bindings.rs`,
and the C host suites of Section 12.7) rewrites both sides by exactly
these rules, which implement the divergences that semantics S75
sanctions, and by no others:

1. Every `duration_ms` key is removed, at any depth, and so are an
   envelope's `engine` key and the keys `engine` and `native` of every
   `check_configuration` object (T73).
2. A *guard fault* is a fault that carries `depth`, a *time fault* one
   with `resource: "time"`. When both outcomes of one evaluation are guard
   faults, each of their fault objects, wherever it appears, is reduced to
   its `code`, `category`, `severity`, `resource`, and `limit` and a
   `depth` of 0; when both are time faults, to its `code`, `category`,
   `severity`, `message`, `resource`, and `limit`; when only one of them is a guard or a time
   fault, or one is a guard fault and the other a time fault, the two
   outcomes are not compared (a `RunOutput` loses `result`,
   `result_text`, and `fault` on both sides).
3. When some evaluation of a test (its body, a case, a shrink rerun, a
   refinement check, a range bound) produced a guard or a time fault on
   either engine, the two `TestCaseRecord`s are compared on `symbol`,
   `qualified_name`, `kind`, `seed`, `property_seed`, and `receipt` alone,
   and also on `status` and on their faults reduced by rule 2 when both
   faults are guard faults or both time faults; the runner's caller learns
   of such faults by wrapping the `TestEngine` of each engine.

Coverage gate. `crates/cpl-cabi/tests/differential/coverage.rs` collects,
from the coverage counters of the drivers (`spec/cabi.md` Section 3) and
from the compared outcomes, what the compared cases exercised, and fails
unless each of these occurred at least once: every `ExprKind`, `Stmt`, and
`PatternKind` variant; every callable builtin id; every fault code; every
reachable (`clause_kind`, `attributed_to`) pair (`requires`/`caller`,
`ensures`/`body`, `ensures`/`binding`, `invariant`/`body`,
`decreases`/`body`); the resources `stack`, through the counter and
through the guard, and `time`; every kind of claim site (a function's
`requires`, `ensures`, and header `decreases`, a loop's `invariant` and
`decreases`, a refinement predicate, an operation's `requires` and
`ensures`, a range bound) and the "already claimed" path of semantics S70;
and every outcome of semantics S77.

Gate rule. The differential suites run in `cargo test --workspace`. A
change under `crates/cpl-rt/c/`, `crates/cpl-emit-c/`, or
`crates/cpl-eval/src/` is not done until they pass with
`CPL_REQUIRE_CC=1` (Section 12.8). The evaluator stays the oracle: a
divergence is fixed on the native side unless the evaluator contradicts
the specifications, and then the specifications decide. `CPL_DIFF_SEED`
reproduces a run, and `CPL_DIFF_SCALE=10` multiplies the budgets for the
nightly run.

Beside the suites, `crates/cpl-cabi/tests/protocol.rs` checks, for
generated values of every kind (nests 10^6 deep and DAGs with sharing
included), that the node-list encoding and decoding and the C value-JSON
reader and writer (through the driver's `from_json` and `render`
requests, `spec/cabi.md` 3.8) give structurally equal values with
identical renderings, and `run_bindings.rs` runs every capability-using
function of the `cboundary` fixture (Section 12.7) with `cpl run`'s
semantics on both engines (the evaluator through `CAdapter`, the native
engine through the driver's in-process grant) and compares results and
faults after the S75 normalization, the `InvalidResult` and
`refinement_violation` outcomes of S77 included.

### 12.7 C host suites (Milestone 2)

`crates/cpl-cabi/tests/c_hosts.rs` builds the fixtures below through
`cpl_emit_c::package::build` and compiles and runs every host program of
`crates/cpl-cabi/tests/c/` against them. The first line of a host program
names its fixtures (`/* fixture: cboundary */`, `/* fixture: cboundary
second */`), which `c_hosts.rs` reads, so that adding a host program needs
no edit of `c_hosts.rs`. Every test copies each fixture it uses into a
directory of its own under `CARGO_TARGET_TMPDIR` and provides the fixture's
vendored files there (below), builds with the `out_dir` inside that copy
and the `cache_dir` that the tests of its binary share (Section 12.8), and
writes nothing under `crates/`. Each host is compiled with `-std=c11 -Wall
-Wextra -Werror` in two configurations: `-O2` against an `optimized`
library, and `-O1 -g -fno-omit-frame-pointer -fsanitize=address,undefined
-fno-sanitize-recover=all` against a library built `--sanitize
address,undefined`, run with `ASAN_OPTIONS=detect_leaks=1:halt_on_error=1`
and `UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1`; each links the
shared and, separately, the static library. Each ends by asserting through
`cpl_runtime_stats` (on a `cpl_stats` initialized with `CPL_STATS_INIT`,
`spec/cabi.md` 4.2) that no handle, capability, or object is live and that
`cpl_runtime_close(rt, 0, NULL)` returns `CPL_OK`; LeakSanitizer must stay
silent. The functions and statuses named below are `spec/cabi.md`'s.

Each host prints one JSON line per check, and `c_hosts.rs` compares each
value line and each `CPL_FAULT` line with the evaluator's outcome of the
same call, computed in process and rendered as `cpl run --json` renders it
(`value_to_json`, `Fault::to_diagnostic(p).to_json(sm)`), after the S75
normalization of Section 12.6:

- A call without `using` parameters runs through `run_function`, a call
  with them through `run_function_with_hosts`: a capability that the C
  host implements is mirrored by a Rust `CapabilityImpl` that gives the C
  callbacks' answers (a `ScriptHost`, Section 1.14, where they are fixed),
  bound with the label `host` and `validate_results: true` (semantics
  6.9), and a grant by the `CAdapter` that `manifest_hosts` creates for
  its binding. Each inner call that
  a re-entrant callback makes is compared on its own with `run_function`
  of that call (semantics S79).
- The stack-guard case runs with `max_call_depth` 4,000,000,000 on both
  sides, the evaluator on its evaluation thread (`on_eval_stack`), and the
  time-budget case under one budget on both; both are compared by rule 2.
- The lines of outcomes that the evaluator has no rendering of are
  checked field by field instead: a `CPL_E_*` line for its status and for
  the `code`, `category`, `severity`, and payload of `spec/cabi.md` 4.9
  (an `interop_*` message is the runtime's wording and is not compared), a
  C callback's `binding_protocol` for the fields of semantics S80, a
  memory fault for its status and `resource: "memory"` (S75 compares
  nothing of it), and a cancellation for `CPL_E_CANCELLED` and
  `interop_cancelled`.

| Program | Fixture | Exercises |
| - | - | - |
| `host_reader.c` | the verbatim `examples/reader` (the default export set, no manifest change) | the design's Export example: `reader_paginate_ints([1, 2, 3], 2)` gives `CPL_OK` and `[[1, 2], [3]]`; `([], 2)` gives `[]`; `([1], 0)` gives `CPL_ERR` with a `ConstraintError` whose `type_name` is `reader.pages.Positive` and whose message is `0 does not satisfy self > 0`; `reader_image_width` with a host `ImageCodec` returning `Ok(Dimensions { 640, 480 })` gives `Ok(640)` |
| `host_basic.c` | `cboundary` | the same through `cboundary_*`; `cboundary_paginate_ints_i64`; `clean` (a struct in and out), `label` (an enum argument), `positive_size` (a `Text` span in, a `Result` out); `Bool` and `Bytes` parameters; every result compared as above |
| `host_ownership.c` | `cboundary` | dup and release pairs; a double release, a use after release, a handle of a second runtime of the same package, both while that runtime is open and after it was closed, a null output (each its documented `interop_*` code, nothing freed or leaked); views valid until release; a release from another thread; `cpl_runtime_close` refusing live handles (`interop_runtime_in_use`) and then `CPL_CLOSE_FORCE`; a nested `Option` 10^5 deep built and released on a 128 KiB thread (iterative release); error objects released after the close |
| `host_failures.c` | `cfaults` | every fault of semantics 6.7 that a checked program can raise, each compared as above: `divide_exactly(7, 0)` and `(7, 2)` (no `caller`), an `ensures`, an `invariant`, a loop and a header `decreases`, `division_by_zero`, `unresolved_hole` with `bindings`, `assertion_failed` in a called body claimed by a clause, the `refinement_violation` of a `Positive` parameter (`CPL_FAULT`, with `parameter` and `pointer`), `resource_exhaustion` at `max_call_depth` 0, 3, and 10,000 (on a 256 MiB thread), the stack guard on a 256 KiB thread (`depth` present), a time budget on a loop that never ends, cancellation from another thread (`CPL_E_CANCELLED`); and the boundary refusals `interop_argument_type_mismatch` (with a `pointer`), `interop_utf8_invalid` (with `offset`), `interop_bool_out_of_range`, `interop_capability_mismatch`, and `CPL_E_RANGE` from an `_i64` export |
| `host_version.c` | `cboundary` | `CPL_E_VERSION` (`interop_version_mismatch`) for `options.api_version` with major 1 and with minor 2; `interop_struct_size_unsupported` for a `struct_size` too small; a smaller but sufficient `struct_size` accepted with defaults; the fields of `cboundary_library_info`; `CPL_CALL_UNCHECKED_CONTRACTS` refused (`interop_option_unsupported`); a deliberately mismatched header guard failing to compile (a compile-must-fail check of `c_hosts.rs`) |
| `host_caps.c` | `cboundary` | a host `PageRead` giving `Ok(page)`, `Err(Missing(id))`, and `Err(Unavailable)`; a result of the wrong type (`internal_error` `binding_result_ill_typed`); `CPL_FOREIGN_ERROR` from an operation without a `ForeignError` arm (`binding_error_unrepresentable`); a bad callback status (`binding_protocol`); a host `ImageCodec` returning width 0 (`Err(ForeignError { kind: InvalidResult })` with the details of stdlib S45); re-entry: the callback calls `cboundary_load_text` and a second export, each a fresh evaluation with no `caller` (semantics S79); `drop` called exactly once, after the last release, for every capability created, and never for a `user` whose `cpl_cap_new` or typed constructor failed (`spec/cabi.md` 5.7) |
| `host_imports.c` | `cboundary` | CPL calls a C library: `cboundary_bind_image_codec`, then `cboundary_image_width` over `imgtest` (`Ok(640)`; the magic input `NEGW` gives `InvalidResult`; each `imgtest` code becomes its kind with `foreign_code`); the opaque decoder created once per grant and destroyed once; `imgtest_live_allocations() == 0` at the end |
| `host_declarative.c` | `cthumbs` | the declarative binding `thumbs`: an owned output buffer released exactly once on every path, the `int32` argument range check (`InvalidArgument`), the error map, the infallible `decoder_version`, `[state]` created once and destroyed once |
| `host_threads.c` | `cboundary` | eight threads calling exports concurrently on shared input handles and one shared capability; results released on other threads; `cpl_cancel_request` from another thread; clean under TSan (Section 12.8) |
| `host_two_packages.c` | `cboundary`, `second` | both packages linked dynamically and, separately, statically into one host: independent runtimes, and a handle of one refused by the other (`interop_handle_foreign`); `c_hosts.rs` also links two static copies of `cboundary`, built into two output directories: with `-Wl,--whole-archive` the link fails with a multiple-definition error, and linked plainly it succeeds and the host holds one copy, the first archive's (`spec/cabi.md` 1.6) |
| `host_oom.c` | `cboundary` | a counting allocator swept over every allocation index of three calls: each index yields `CPL_FAULT` with `resource: "memory"` or `CPL_E_NOMEM`, never a crash or a leak; `max_heap_bytes` gives the same deterministically |

`threads.rs` builds `cboundary` with `--sanitize thread`, compiles
`host_threads.c` with `-std=c11 -O1 -g -fsanitize=thread`, and runs
`setarch $(uname -m) -R ./host_threads` with `TSAN_OPTIONS=halt_on_error=1`
(Section 12.8); `oom.rs` runs the failing-allocator sweep of `host_oom`.

The C test library, `crates/cpl-cabi/tests/clib/imgtest/` (`imgtest.h`,
`imgtest.c`):

| Function | Shape tested |
| - | - |
| `imgtest_decoder *imgtest_decoder_new(void)`, `void imgtest_decoder_free(imgtest_decoder *)` | an opaque handle with ownership; a double free is detected by a magic field and counted |
| `int imgtest_dimensions(imgtest_decoder *, const uint8_t *, size_t, int32_t *w, int32_t *h)` | a borrowed input span; scalar out-parameters; codes 0, -1 (malformed, `IMGTEST_EMALFORMED`), -2 (unsupported, `IMGTEST_EUNSUPPORTED`, the macro the reviewed wrapper of `spec/cabi.md` 6.2 tests), -3 (too large, `IMGTEST_ETOOLARGE`); the magic input `NEGW` yields `w = -3` |
| `int imgtest_thumbnail(imgtest_decoder *, const uint8_t *, size_t, int32_t max_side, uint8_t **out, size_t *out_len)`, `void imgtest_free(void *)` | a library-owned output buffer, released through the library's allocator |
| `const char *imgtest_strerror(int)`, `int32_t imgtest_version(void)` | messages; an infallible scalar |
| `size_t imgtest_live_allocations(void)` | a leak assertion that does not depend on sanitizers |

An `imgtest` image is the four bytes `IMGT`, its width and its height as
`uint32_t` little-endian, and then `width * height` pixel bytes, which
`imgtest_thumbnail` reads and `imgtest_dimensions` does not. For the
four-byte input `NEGW`, `imgtest_dimensions` sets `*w = -3` and `*h = 1`
and returns 0; otherwise it returns -1 for fewer than 12 bytes or another
magic, -2 for the magic `IMGX`, and -3 for a side above 16384. The 12 bytes
`494d475480020000e0010000` (`IMGT`, 640, 480) are therefore a 640 by 480
image to `imgtest_dimensions`. `crates/cpl-cabi/tests/clib/libextra/extra.c`
is a library of one function, which the harness compiles with `-fPIC` into
the static library `libextra.a` of a fixture copy, for the shared link to
take.

Fixtures, under `crates/cpl-cabi/tests/projects/`:

- `cboundary/` (release-clean, locked by the tests): `src/reader/pages.cpl`,
  a copy of the corpus file with `@prove` removed from `paginate` and with
  `divide_exactly`, `clean`, `label`, and `positive_size` made `pub`, each
  changed line marked `// glue:`; `src/reader/images.cpl`, a verbatim
  copy; `adapters/image_codec.c` (a reviewed wrapper, `spec/cabi.md`
  Section 6); and, provided by the harness in each copy, `vendor/imgtest`
  (copied from `clib/imgtest`) and `vendor/libextra/libextra.a` (built from
  `clib/libextra`), which the binding links so that the lock pins a library
  a test can change. Its `cpl.toml` has `targets = ["c"]`, `[interop.c]
  prefix = "cboundary"`, `abi = 1`, and `artifacts = ["shared", "static",
  "executable"]`, an explicit `exports` list (`paginate_ints` in its handle
  and `_i64` forms, then `divide_exactly`, `positive_size`, `clean`,
  `label`, `load_page`, `load_text`, and `image_width`), and the
  backend-`c` binding `image_codec` with `vendor/imgtest/imgtest.c` as its
  source and `libraries = ["extra"]`, `library_dirs = ["vendor/libextra"]`
  (Section 11.1). The release test copies it to a temporary directory and
  runs `cpl build --lock`, then `cpl build --release`.
- `cfaults/` (development only, no lock): `pub` wrappers over the fault
  cases of `host_failures`, among them a copy of `holes.pages.summarize`
  made `pub` (glue), a deep recursion, a loop that never ends, and an
  operation-clause case.
- `cthumbs/` (the declarative binding): `src/cthumbs/thumbs.cpl` (the
  capability `Thumbnails` of cabi.md Section 6 and `pub fn thumbnail_of`),
  `adapters/thumbs.cbind.toml`, `vendor/imgtest` (provided as for
  `cboundary`); one backend-`c` binding, `thumbs`.
- `second/`: a minimal second package (`prefix = "second"`) for
  `host_two_packages`.

`examples/reader` and `examples/holes` are used unchanged, through fresh
committed copies, for the export rows and the release-refusal rows.

Goldens, updated with `UPDATE_GOLDEN=1` as everywhere in the workspace and
normalized as Section 12.4 normalizes plus the digests and the compiler's
`path`, `version`, and `digest`: `crates/cpl-emit-c/tests/golden/reader/include/reader.h`
(the verbatim corpus) and
`golden/cboundary/{include/cboundary.h, include/cboundary_adapter_image_codec.h, src/cboundary.map, build.json}`;
the emitted C of four small modules (`divide_exactly`, `positive_size`, a
loop with an invariant and a measure, a `match` over `StoreError`), so that
printer changes are reviewed as diffs; and `crates/cpl-cli/tests/golden/`
`build_reader.json`, `build_cboundary.json`, `build_release_holes.json`,
`run_paginate_ints_native.json`, `run_image_width_eval.json` (bindings),
and `test_reader_seed1_native.json`. `crates/cpl-cli/tests/build.rs`
compares the `build_*.json` goldens, and `engines.rs` the `run_*` and
`test_*` ones.

### 12.8 Running the native suites (Milestone 2)

- Every test is a plain `#[test]` (Section 12). Tests find the compiler
  with `cpl_emit_c::cc::Compiler::discover` (`CPL_CC`, else `cc`, T72),
  the tests of `cpl-rt` with `cpl_rt::testing::find_cc` (Section 1.12),
  and run it through `std::process::Command`; outputs go to
  `CARGO_TARGET_TMPDIR`, each test working in copies of its own (Section
  12.7), and the library tests of one test binary share one `cache_dir`,
  which concurrent builds may use because every cache entry is written
  atomically (`spec/cabi.md` 1.6). No test uses nested cargo, the `cc`
  crate, or a build script that compiles C (Section 1.12); CLI tests use
  `CARGO_BIN_EXE_cpl` as in Milestone 1.
- Toolchain tests compile the emitted code and the runtime with `-Werror`
  added, since a warning there is a toolchain defect: the library tests set
  `BuildConfig::werror` and `DriverConfig::werror` (Section 1.13), while
  the `cpl` binary, which the CLI tests run, never adds it, and
  `compile.rs` compiles the runtime and every corpus program's emitted C
  with it; user builds never use `-Werror` (`spec/cabi.md` 1.5).
- Skip policy. Without a C compiler, a test that needs one prints
  `skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)` and passes,
  unless `CPL_REQUIRE_CC=1`. A sanitizer the compiler cannot link
  (`Compiler::supports`, a one-line probe) skips its suite with the
  printed reason unless `CPL_REQUIRE_SANITIZERS=1`. The TSan suite runs
  `setarch $(uname -m) -R <host>` on Linux, because a TSan binary aborts on
  some kernels, the WSL2 kernel of the acceptance machine among them
  ("unexpected memory mapping"), and runs clean with address-space
  randomisation disabled; it is skipped with the printed reason when TSan
  or `setarch` is unavailable, unless `CPL_REQUIRE_TSAN=1`. Acceptance and
  CI set all three variables.
- Budgets: D5 draws 200 tuples per function and D7 10,000 inputs per
  operation; drivers compile in parallel, about 1-2 s each at `-O0`, and
  the suites of Milestone 2 should add a few minutes to `cargo test
  --workspace` on a 12-CPU machine. `CPL_DIFF_SCALE=10` is the nightly
  run, and `CPL_DIFF_SEED` reproduces one.
- The four workspace gates stay: `cargo test --workspace`, `cargo clippy
  --workspace --all-targets`, `cargo fmt --all -- --check`, and `cargo doc
  --workspace --no-deps`.

Decision: [D-274] T78. the evidence of Milestone 2 is differential and
sanitized: suites D1-D8 compare the native engine with the evaluator byte
for byte under the comparison rules and the S75 normalization of Section
12.6, gated by its coverage
gate, and a change to the runtime, the emitter, or the evaluator is not
done until they pass; the C host programs run under ASan, UBSan, and LSan
and the thread suite under TSan, launched with `setarch $(uname -m) -R`
on Linux; every test that needs a C compiler or a sanitizer skips with a
printed reason when it is missing, unless `CPL_REQUIRE_CC`,
`CPL_REQUIRE_SANITIZERS`, or `CPL_REQUIRE_TSAN` is set, and acceptance sets
all three. Rationale: the design requires a C host and a C library to be
exercised with their ownership and failure paths (line 645) and native
execution to preserve CPL semantics (line 659), and two implementations
drift where their outputs are compared least, in the fields agents act on
(`input`, `actual`, `bindings`, backtraces) and in the C renderer's
escaping and key order, so whole outputs are compared, and the coverage
gate keeps a suite from passing vacuously; a skipped C test must say so
rather than pass silently, and must fail where the tools are required.
Rejected alternatives: an in-process differential (sanitized code cannot
be loaded into the uninstrumented test process, and a native crash would
kill the runner); comparing only codes, or parsed JSON; host programs
that check only exit codes; LeakSanitizer as the only leak oracle (exact
counts from `cpl_runtime_stats` and `imgtest_live_allocations` do not
depend on its heuristics); a random program generator in Milestone 2 (the
harvested and targeted corpora, the sweeps, and the coverage gate cover
the same ground earlier); editing Milestone 1 tests to fit Milestone 2.

### 12.9 Milestone 2 acceptance checklist

1. **Gates and Milestone 1.** The four workspace gates are clean with
   `CPL_REQUIRE_CC=1`, `CPL_REQUIRE_SANITIZERS=1`, and
   `CPL_REQUIRE_TSAN=1`; every Milestone 1 test (all that `cargo test
   --workspace` runs on the accepted Milestone 1 tree: its 2943 `#[test]`
   functions, Section 13, and its doctests) and every Milestone 1 golden
   passes, and no Milestone 1 test file is edited.
2. **The Export example on the verbatim corpus.** `cpl build --json`
   (development mode) on a fresh committed copy of `examples/reader` exits
   0 with `project_export_skipped_generic` (I) for `paginate`;
   `include/reader.h` equals its golden; `nm -D --defined-only
   lib/libreader.so.0` lists exactly the expected `reader_*` symbols under
   the version node `READER_0`; `host_reader` passes against the shared
   and the static library, in both configurations.
3. **A C host calls CPL.** `host_basic`, `host_ownership`,
   `host_failures`, `host_version`, and `host_caps` pass in both
   configurations with zero live objects and no sanitizer report; every
   value and fault line equals the evaluator's rendering under the
   comparison of Section 12.7.
4. **CPL calls a C library.** `host_imports` and `host_declarative` pass;
   `cpl run reader.images.image_width --arg '{"$type": "Bytes", "hex":
   "494d475480020000e0010000"}'` (Section 12.7) in `cboundary` returns
   `Ok(640)` with `--engine eval` (through `CAdapter`), `native`, and
   `both`, and the `NEGW` input (`"hex": "4e454757"`) gives
   `Err(ForeignError { kind: InvalidResult, .. })` on all three.
5. **Ownership and failure paths.** A double release, a use after
   release, a foreign handle (of the same package and of another), null
   outputs, library-owned buffers, and `drop` are each exercised with
   their defined outcomes; `imgtest_live_allocations() == 0`; the
   failing-allocator sweep (`host_oom`) never crashes or leaks.
6. **Versioning.** A version mismatch is refused at compile time
   (`#error`) and at run time (`CPL_E_VERSION`, through the fields of the
   options struct); the generated headers pass `g++ -std=c++17
   -fsyntax-only`.
7. **Native execution equals the oracle.** D3 passes: every `cpl run` and
   `cpl test` row of Section 12.3 gives byte-identical envelopes with
   `--engine native` and `--engine eval` after the S75 normalization of
   Section 12.6, and
   `cpl test --engine both --seed s` for s in 1..=32 on `examples/reader`
   reports no divergence; D1, D2, and D4-D7 report no divergence at the CI
   budget with the coverage gate passing; and D8 is sanitizer-clean.
8. **Threads.** `host_threads` is TSan-clean under `setarch -R`.
9. **Two packages.** `host_two_packages` passes dynamically and
   statically; two static copies of one package fail the link with a
   multiple-definition error under `-Wl,--whole-archive`, and linked
   plainly give the host one copy.
10. **Release policy.** `cpl build --release` fails on `examples/holes`
    (two `hole_in_release`, one `verification_unsatisfied`,
    `build_lock_missing`) and on `examples/reader` (one
    `verification_unsatisfied`, `build_lock_missing`), and succeeds on a
    locked copy of `cboundary`, whose library information reports
    `release` 1 and `holes` 0.
11. **The lock pins the build.** On a copy of `cboundary`, `cpl build
    --lock` writes the C entries of Section 11.2. Then, under `--release`:
    editing `adapters/image_codec.c` or `vendor/imgtest/imgtest.c` is
    `project_lock_stale`; `--cc` naming a wrapper script around the same
    gcc is `build_lock_mismatch` (`targets.c.compiler_digest`,
    `targets.c.compiler_path`); `--profile debug` is `build_lock_mismatch`
    (`targets.c.flags_digest`); editing `vendor/imgtest/imgtest.h` is
    `build_lock_mismatch` (`foreign.c.image_codec.headers`); and
    rebuilding `vendor/libextra/libextra.a` with other content is
    `build_lock_mismatch` (`foreign.c.image_codec.libraries`). Changing the
    signature of `reader.pages.clean` and running `cpl build --lock` is
    `build_abi_changed` and leaves `cpl.lock` unchanged (T81).
12. **The manifest is validated.** Every new `project_*` code has a
    negative case in `crates/cpl-project/tests/` or in the `cpl-sema`
    corpus.
13. **Native executables.** `bin/cboundary` gives, for each call of
    `host_basic`, the `result`, `result_text`, and `fault` that `cpl run
    --json` gives (`spec/cabi.md` Section 8).
14. **Structure.** The DAG test, the table-fidelity test, the
    emitted-shape test, and the `-Werror` compile test pass on every corpus
    program.
15. **Specifications.** The decisions of Milestone 2 are recorded
    (Appendix A here; semantics S75-S81, stdlib S45-S47, `spec/cabi.md`
    C1-C39), the Milestone 2 row of Section 13 cites these tests, and
    `spec/decisions.md` indexes the new rows.

## 13. Milestone map

| Milestone | Acceptance evidence (design) | Delivered by | Demonstrated by |
| - | - | - | - |
| 1 Core execution | formatter is idempotent | `cpl-syntax` (`fmt`), `cpl fmt --check` | idempotency harness over every corpus (Section 12) |
| | examples execute | `cpl-sema` (`check`), `cpl-eval` (`run_function`, `run_tests`), `cpl run`, `cpl test` | Section 12.3 table; `cpl test` on `examples/reader` passes |
| | native execution | *deferred to Milestone 2* (`cpl-emit-c`, `cpl build`): the Design conflict recorded in the introduction and T41; the reference evaluator is the oracle the backend is validated against | the Section 12.3 table runs on the evaluator in Milestone 1 and must give identical results on the backend in Milestone 2 |
| | invalid types, refinements, and release holes are rejected | `cpl-sema` (`type_mismatch`, `cannot_prove_refinement`, `refinement_false`, `hole_in_release`, `verification_unsatisfied`), `cpl check --release` | negative corpus; `cpl check --release` on `examples/holes` exits 1 |
| | stable IDs | `cpl-index` (`generate_id`, provisional ids, facets), `cpl id assign`, `cpl inspect`, `cpl diff`, `cpl prove` (unknown), `cpl work check`, `cpl-work` (model, validation, patch application) | facets golden; diff corpus; CLI tests |
| 2 C boundary | a C host calls CPL and CPL calls a C library; ownership and failure paths | new crates `cpl-rtspec` (the tables both engines share), `cpl-rt` (the C11 runtime and C API 0.1 of `spec/cabi.md`), `cpl-emit-c` (C emission from the typed HIR, generated headers, package builds), and `cpl-cabi` (the native engine, `CAdapter`, the differential harness); `cpl build --target c`; `--engine eval\|native\|both` on `cpl run` and `cpl test`; `[interop.c]` and `[bindings.*] backend = "c"` active; `ForeignError` mapping (stdlib S45); validating bindings (semantics S77) (T79) | the C host programs of `crates/cpl-cabi/tests/c/` under sanitizers and TSan (Section 12.7: `c_hosts.rs`, `threads.rs`, `oom.rs`), D1-D8 with the coverage gate (Section 12.6: `crates/cpl-cabi/tests/differential.rs`), the other Milestone 2 rows of the Section 12 test table, and the Milestone 2 acceptance checklist of Section 12.9 (`crates/cpl-cli/tests/acceptance_m2.rs`) |
| 3 Python package | Python imports CPL; CPL calls a Python adapter; conversion, exception, worker failure cases | `cpl build --target python`: a generated CPython limited-API extension in C over the package's C API, `.pyi` generation, Python adapters as capability callbacks, the worker transport extending cpl-driver protocol 1 (with which `isolation = "worker"` becomes legal for C bindings too); `[bindings.*] backend = "python"` active; fixtures in test receipts (T79) | pytest suite in `bindings/python/`; receipt `fixtures`/`environment` populated |
| 4 Node package | Node imports CPL; CPL calls a Node adapter; event-loop, BigInt, error mapping | `cpl build --target node`: a generated Node-API addon in C over the C API, a JavaScript wrapper, `.d.ts` generation; `backend = "node"` active (T79) | Node test suite in `bindings/node/` |
| 5 Verification and edits | proved, failed, unknown remain distinct; stale and out-of-scope patches are rejected | new crate `cpl-verify` (pure subset, obligation discharge, receipts with `proved`/`failed`/`unknown`/`conditional`), `cpl prove` real results, `verification_result` diagnostics with `status: "failed"` and a `model` key for abstract solver models (semantics S46), dependency tracking in receipts, `cpl-work` patch validation enforced by `cpl patch apply` (new command) | verifier corpus with one example per status; patch tests (`patch_stale`, `patch_out_of_scope`, `patch_preserved_facet`) |
| 6 Swarm integration | concurrent changes integrate consistently; expired assignments and invalidated reviews cannot commit | `cpl-work` scheduler (Section 10), `cpl work run`, `cpl work integrate`, `CheckRunner` in `cpl-cli`, evidence refs, `[work]` manifest section active | scheduler tests with fake `CheckRunner`: concurrent contract change invalidates dependents; target advancing after validation triggers retry; expired worker submission rejected (`work_stale_submission`); coordinator restart resumes from `.cpl/work/` |

Status (2026-10-29): Milestone 1 is accepted. Two acceptance audits ran
every row of the Section 12.3 table through the built `cpl` binary (debug
and release builds alike) on fresh committed copies of the corpus, every
command form and flag of Section 7.2 with and without `--json`, and the
four workspace gates (`cargo test --workspace`, 2943 tests;
`cargo clippy --workspace --all-targets`; `cargo fmt --all -- --check`;
`cargo doc --workspace --no-deps`), all clean, with every row of the
Section 12 table met; native execution remains deferred to Milestone 2
(T41). Decisions T57 to T67 here and S73 and S74 of the semantics record
the rules that the Milestone 1 implementation and its audits settled.

Status of Milestone 2 (2026-10-30): accepted. Its specification phase
recorded the decisions T68 to T82 here, S75 to S81 of the semantics, S45
to S47 of the standard library, and C1 to C39 of `spec/cabi.md`, and every
item of the checklist of Section 12.9 holds: the four workspace gates are
clean with `CPL_REQUIRE_CC=1`, `CPL_REQUIRE_SANITIZERS=1`, and
`CPL_REQUIRE_TSAN=1` (`cargo test --workspace`: 3235 tests and doctests,
the Milestone 1 tests and goldens among them, all passing); the C host
programs pass in both configurations against the shared and the static
libraries with every line equal to the evaluator's; D1 to D8 report no
divergence, with the coverage gate passing; and
`crates/cpl-cli/tests/acceptance_m2.rs`, `build.rs`, `engines.rs`, and
`executables.rs` run the checklist's commands on fresh committed copies.
No Milestone 1 test was edited; the one Milestone 1 file under a `tests/`
directory that changed is the corpus index
`crates/cpl-sema/tests/corpus/README.md`, which no harness reads and which
gained the rows of the Milestone 2 directory cases of Section 12.2.
Implementation found the specifications incomplete in four places, now
amended: the `declarative` member of `cplrt_grant` (`spec/cabi.md`
Appendix B.1), the driver's `shutdown` answering the final statistics
(`spec/cabi.md` 3.8), `ScriptHost::new` taking the capability and label
(Section 1.14), and a C prefix that may not name a C11 or POSIX header
(Section 11.1).

Decision: [D-275] T79. the Milestone 2, 3, and 4 rows above replace the
plan rows written for Milestone 1: Milestone 2 delivers the native backend
(the C runtime, C emission from the typed HIR, package builds, and the
native engine) together with the C boundary, and Milestones 3 and 4
generate C extensions over the package's C API rather than new Rust crates
over `cpl-eval`. Rationale: the earlier Milestone 2 row put a versioned C
API over the evaluator's values and `Interpreter`, which is no native
execution (design lines 38 and 644) and would have swapped the core out
from under C hosts when native code arrived (the design asks the C
interface to expose the native backend, line 659); the Milestone 3 and 4
rows follow T68's one native core, since a CPython limited-API extension
and a Node-API addon build with the C compiler alone. Rejected
alternatives: the engine first and the boundary in a later milestone (the
C API's handle and error model shapes the runtime's objects and fault
records, so the runtime would be designed twice); PyO3 and napi-rs crates
over `cpl-cabi` or `cpl-eval` (T68).

## Appendix A: decision index

| Id | Rule |
| - | - |
| T1 | `cpl-eval` interprets the typed HIR from `cpl-sema`; no separate evaluation IR |
| T2 | `cpl-work` never depends on `cpl-eval`; checks run through the `CheckRunner` trait implemented in `cpl-cli` |
| T3 | every JSON document is a `serde` type in `cpl-diag::json`; crates emit wire types directly |
| T4 | no further runtime dependencies in Milestone 1; `BTreeMap` wherever order matters |
| T5 | diagnostic payloads are flattened into the S49 diagnostic object; fixed keys are reserved |
| T6 | a lexical or syntax error is fatal for its file (D59): one diagnostic, the earliest (internal resynchronisation permitted, D70), with a semantics 9.4 `parse` code; no error-recovery nodes; other files are still parsed, nothing is typed (S52) |
| T7 | the formatter receives only successfully parsed ASTs and leaves broken files untouched |
| T8 | values use `Rc`; the interpreter and test runner are single-threaded in Milestone 1 |
| T9 | `Int` division follows semantics S37 (truncation) and lives in one function `arith::div_rem` |
| T10 | AST identifiers are `String`s; no interner |
| T11 | comments attach forward as leading trivia, same-line comments backward as trailing, end-of-body comments as dangling; blank lines are recorded on nodes and on leading comments; doc comments are not comments |
| T12 | width-aware breaking at 100 columns that never joins author-broken groups |
| T13 | the formatter reorders only header clauses and `.work` item kinds |
| T14 | `cpl fmt` never inserts, removes, or changes `@id`; only `cpl id assign` adds ids |
| T15 | `FileId`s: std files first, then project files in path order |
| T16 | the HIR is a boxed tree with an explicit block tail |
| T17 | interface dependencies are the targets of references made from signature, contract, or effects |
| T18 | `gen.*` expressions resolve through S54's module alias and the `std.gen` stubs and are lowered to the HIR special form `ExprKind::Gen`, typed `Ty::Gen(T)`, hermetic per S14 |
| T19 | intrinsic-type members are the `@method`/`@property` stubs of `std/prelude.cpl`; `x.len` lowers to a call |
| T20 | `use` imports are file-scoped (with S5) |
| T21 | the id kind prefix is informative; a mismatch is only a warning for generated-format ids |
| T22 | tests and properties are addressed by a quoted final path segment |
| T23 | function `decreases` is in `contract`; loop clauses are in `body` |
| T24 | `name` is a facet separate from `signature`; signature text replaces the name with `_` |
| T25 | member doc comments belong to `docs`, not `signature` |
| T26 | the `effects` facet text is the sorted, deduplicated set |
| T27 | facet hashes are 128 bits; file and tree digests are 256 bits |
| T28 | `dependencies` includes body dependencies; the default list is `interface_dependencies` |
| T29 | `cpl prove` exits 0 only when every obligation is `proved` |
| T30 | value JSON follows S40; `$type` is the qualified name, bare for prelude types |
| T31 | semantic diff is computed from facet maps only |
| T32 | revision trees are materialized with `ls-tree` + `cat-file --batch` |
| T33 | coordinator state is JSON under `.cpl/work/`; durable artifacts are git objects |
| T34 | receipts live under `refs/cpl/evidence/...` by default; `[work] evidence = "tree"` commits them outside the source set |
| T35 | unknown manifest keys are errors (diagnostics, exit 1) |
| T36 | no switch weakens `@prove`: release rejects `unknown`/`failed`; `admit` names dependency assumptions only |
| T37 | `cpl.lock` (sources and verification configuration) is checked under `--release` (stale = error, missing = warning) and written by `--lock` or the integrator |
| T38 | negative tests use trailing `// expect-error: <code>` markers (with `@+N` and `(global)` forms) |
| T39 | refinement proofs are S16's constant evaluation only; `gen.positive_int` typechecks by structural refinement identity (S55) |
| T40 | the coordinator enforces execution boundaries through the `Sandbox` adapter and records only what was enforced |
| T41 | Milestone 1 runs on the reference evaluator; native execution is Milestone 2 (Design conflict, introduction) |
| T42 | file selectors `project.manifest`, `project.lock`, `work.<stem>` make project files grantable; `.patch` files are consumed |
| T43 | a namespace wildcard `m.*` means `m` and every module below it |
| T44 | envelope `status` is `ok`/`unsuccessful`/`error`; test outcomes are `tested`/`failed` |
| T45 | stdlib Section 8 owns distributions and shrink candidates; the stream is semantics S44's (one ChaCha20 stream per property keyed by the 32-byte blake3 digest of run seed and identity); `property_seed` is recorded as 64 hex digits |
| T46 | every diagnostic list is in `Diagnostics::sorted` order |
| T47 | `work_provisional_identity` warns; the integrator assigns ids before the integration checks |
| T48 | one binding diagnostic per key: an invalid `backend` is `project_binding_bad_backend` alone |
| T49 | interpreter corpus cases: closed three-key shape, qualified `function`, value-JSON `args` decoded as `cpl run --arg` (refinement failures are `refinement_violation`), `expect.value` against the declared return type, `expect.fault` by code |
| T50 | the typed HIR and `Program`'s accessors live in `cpl-hir` (between `cpl-syntax` and `cpl-sema`), re-exported as `cpl_sema::hir`; `resolve_selector` is the free function `cpl_sema::resolve_selector`; `cpl-eval` depends on `cpl-hir`, not `cpl-sema` |
| T51 | two bare `type` declarations written on adjacent lines stay adjacent; every other pair of declarations is separated by one blank line |
| T52 | project loading and the std embedding are the crate `cpl-project` (on `cpl-syntax` and `cpl-diag`), re-exported as `cpl_sema::project`; `cpl-index` and `cpl-work` use it directly |
| T53 | every manifest value 11.1 does not admit is the fatal `project_manifest_invalid`; only `[bindings.*]` values are checked later, as diagnostics |
| T54 | a symbolic link to a file is that file, on disk and in a git tree; a link to a directory is never followed; a link that leads to no file is not a member |
| T55 | enforcement also needs write authority over an unchanged facet that uses a name whose import changed (a repointed `use` line or a move between files) |
| T56 | a clause's facet text keeps the block layout of `if`/`match`; its clause text (`Clause::text`, `clause`) is that text on one line |
| T57 | `--cases` and `[test] cases` are at least 1: a zero-case property would be receipted `tested` without one evaluation |
| T58 | `run`, `test`, `prove`, and `work check` act only on a project that checks without errors; their envelopes carry the check's errors and warnings, never its infos |
| T59 | `inspect`, `run`, and `prove` resolve their target by Section 6.1, bare names included; unresolved: exit 1 for `inspect`, 2 for `run` and `prove` |
| T60 | an envelope's `revision` is `null` exactly when no project is involved; a refusal in a located project names its revision |
| T61 | `cpl run` refuses a generator (`usage_not_a_function`); an `internal_error` raised by evaluation is a fault (exit 1) |
| T62 | obligations are numbered over the function's clauses, then its body, in pre-order; every call site counts |
| T63 | a node's further trailing comments print where the next parse attaches them; the round trip holds modulo `strip_layout` |
| T64 | after an `if`/`else if`/`match`/`while` header whose comma group broke, the `{` takes its own line, except inside a comma group |
| T65 | lexing, parsing, and formatting are near-linear in time and, at the D73 minimum limits, under 1 MiB of debug-build stack |
| T66 | `cpl id assign` changes only the inserted lines; formatting follows it, by the user and by the integrator |
| T67 | an `add` namespace whose path names a declaration and no module is `work_selector_empty` |
| T68 | the C API and the Python and Node packages wrap one native core, the C runtime of `cpl-rt` and the C `cpl-emit-c` emits; the Rust crates are the toolchain and the oracle; reopens the fixed decision "the same Rust core" |
| T69 | Milestone 2 adds the crates `cpl-rtspec`, `cpl-rt`, `cpl-emit-c`, and `cpl-cabi`, with the DAG and forbidden edges of Section 1.2, enforced by `workspace_dag.rs` |
| T70 | `cpl-rtspec` is the single source of message templates, White_Space, builtin specifications, limits and versions, the `ForeignError` orders, and the `interop_*` table; the one `div_rem` lives there (amends T9) |
| T71 | no new external crate; `serde_json` gains `unbounded_depth`, used only by `cpl-cabi`'s protocol reader |
| T72 | the `cpl build` contract: the compiler from `--cc`, `[interop.c] compiler`, `CPL_CC`, or `cc`, never `$CC`; a scrubbed compiler environment; the output layout; `build.json`; exit 2 for an environment that cannot build |
| T73 | `--engine eval\|native\|both` on `cpl run` and `cpl test`; the native engine is a driver subprocess; `engine_divergence` exits 2; the output's `engine` and `bindings` keys and a receipt's `engine` and `native` only when not the default |
| T74 | `[interop.c]`, backend-`c` bindings with an explicit `in_process`, `[bindings.*.c]`; the default export set; their validation codes |
| T75 | closed C entries in `[targets]`, `[adapters]`, `[foreign]`, rendered inline; release checks compare the C build inputs, `cpl build --release` everything else too and requires a lock (amends T37's table) |
| T76 | the categories `build` and `interop`; the new `project_*`, `build_*`, and `engine_divergence` codes and `internal_error` kinds |
| T77 | the additive `cpl-eval` API of Section 1.7.4; no Milestone 1 signature, struct, or behaviour changes |
| T78 | the differential suites D1-D8 with their comparison rules, coverage gate, and gate rule; the C host suites under ASan, UBSan, and LSan, TSan under `setarch -R`; the skip policy of `CPL_REQUIRE_*` |
| T79 | the rewritten Milestone 2, 3, and 4 rows of Section 13 |
| T80 | the `cpl-project` and `cpl-sema` additions keep every Milestone 1 struct and function shape |
| T81 | `cpl build --lock` refuses to record an ABI break of `spec/cabi.md` 9.3 (an export, a symbol, the C API major) under an unchanged `prefix` and `abi` (`build_abi_changed`); amends T37 |
| T82 | build inputs stay outside the source set, but a revision's state covers them, `cpl run`'s `bindings` report their digest, and no task can be granted one before Milestone 6, while `ProcessAccess::Toolchain` admits the compiler and the drivers that `cpl` starts |

Design conflicts recorded in this document: the Milestone 1 execution
engine (introduction, with T41). The grammar's D57 (`spec/grammar.md`
Section 6, corpus notes) is inherited, not re-recorded. The divergences
from sibling specifications that earlier revisions listed here (the
`kind~` provisional-id spelling, the 16-hex FNV-1a `property_seed` and
any-failure shrinking, the resolver-level `gen` special form,
`usage_generic_function`, the hole report's `toolchain`/`std` field names,
the wildcard, capability-facet, conventional-location, and canonical-text
wordings of the semantics, and the grammar's G1/G2 and D59) were resolved
by the consistency pass of 2026-10-28: this document now follows semantics
S2, S44, S45, S48, S54, and S39, and the other documents carry the
corresponding changes. No divergence is outstanding; `spec/decisions.md`
indexes every decision of the five documents.

## Appendix B: diagnostic codes owned by this document

The codes of the `parse`, `resolve`, `type`, `effect`, `contract`, `hole`,
`test`, `release_policy`, `runtime`, and `internal` categories are
specified in `spec/semantics.md` 9.4 (the `parse` table there is the
mandatory parse code set, T6) and are not repeated. This appendix lists every code the toolchain adds. Severity: E
error, W warning, I info; "lint" means off unless enabled with `--lint`.
From Milestone 2 (T76) the toolchain also owns the category `build`, whose
codes are listed here, while the codes of the category `interop` are
owned by `spec/cabi.md` Section 4 and listed here by reference; every
`project`, `build`, and `interop` code is span-less.

| Code | Category | Sev | Producer | Meaning |
| - | - | - | - | - |
| `id_kind_mismatch` | resolve | W | sema | generated-format `@id` prefix disagrees with the declaration kind (T21); spans the `@id` annotation, with `expected` (the declaration's kind prefix) and `found` (the id's) |
| `id_missing` | resolve | lint W | sema | public declaration without `@id` (Section 4.3); spans the name token, with `name` |
| `std_builtin_missing` | internal | E | sema | a std stub's `@builtin` id is not in the registry (Section 3.6): a check error spanning the annotation, with `name`, and the definition is `HasErrors`; not a load failure |
| `project_manifest_invalid` | project | E | project | `cpl.toml` is not UTF-8 or not TOML, a required key is missing, or a key is ill-typed or has a value outside its domain (fatal `LoadError::Manifest`, exit 2; T53); `path`, `line` when known, and for a manifest read at a revision (`cpl diff`, `inspect --include changes`) `revision` and `commit` |
| `project_manifest_unknown_key` | project | E | project | unknown table or key, with the dotted `key`; a diagnostic, exit 1 (T35) |
| `project_source_dir_missing` | project | E | project | a `[build] sources` directory does not exist (`path`) |
| `project_no_sources` | project | W | project | no `.cpl` files found |
| `project_duplicate_file_module` | project | E | project | overlapping `[build] sources` directories make a file reachable twice (`path`, `sources`); it is loaded once |
| `project_lock_invalid` | project | E | project | `cpl.lock` is not UTF-8 or does not parse, or has a key the format does not define (fatal `LoadError::Lock`, exit 2); fields as `project_manifest_invalid` |
| `project_lock_stale` | project | E | sema | under `--release`: a present lock's `[sources]`, `[verification]`, or `toolchain.std` differ from the tree or the manifest (T37), or, from Milestone 2 and when the lock has a C entry, the digests of the C build inputs (`adapters`, `foreign.c`, T75); `differences`, `paths` |
| `project_lock_missing` | project | W | sema | under `--release`: no `cpl.lock` (T37) |
| `project_lock_toolchain_mismatch` | project | W | project | lock written by another toolchain version or edition; `expected` and `found` as `{ cpl, edition }` |
| `project_binding_unknown_capability` | project | E | project (`check_bindings`, run by sema after resolution) | `[bindings.*] capability` does not resolve to a `pub capability` |
| `project_binding_target_not_enabled` | project | E | project (`check_bindings`) | binding backend not in `[interop] targets` (`targets`) |
| `project_binding_bad_backend` | project | E | project (`check_bindings`) | backend not one of `python`, `node`, `c` |
| `project_binding_bad_isolation` | project | E | project (`check_bindings`) | isolation not `worker` or `in_process` |
| `project_c_target_not_enabled` | project | E | project (`check_c_bindings`) | `[interop.c]` present while none of `"c"`, `"python"`, and `"node"` is in `[interop] targets` (Section 11.1); `key`, `targets` |
| `project_binding_name_invalid` | project | E | project (`check_c_bindings`, `check_python`) | a backend-`c` or backend-`python` binding whose name does not match `^[a-z][a-z0-9_]*$`, so that its grant would be no C identifier (Section 11.1); `binding` |
| `project_binding_isolation_unsupported` | project | E | project (`check_c_bindings`) | Milestone 2 only (not raised from Milestone 3, `spec/python.md` 6.6): a backend-`c` binding whose `isolation` is absent or not `in_process`; `binding`, `found` |
| `project_binding_adapter_kind` | project | E | project (`check_c_bindings`) | a backend-`c` `adapter` that ends neither in `.c` nor in `.cbind.toml`; `binding`, `found` |
| `project_binding_adapter_missing` | project | E | project (`check_c_bindings`) | a backend-`c` `adapter` that is no project-relative path of a regular file; `binding`, `path` |
| `project_binding_path_missing` | project | E | project (`check_c_bindings`) | a `[bindings.<name>.c]` source or directory that does not exist; `binding`, `key`, `path` |
| `project_binding_c_options_misplaced` | project | E | project (`check_c_bindings`) | a `[bindings.<name>.c]` subtable on a binding whose backend is not `c`; `binding` |
| `project_binding_duplicate_capability` | project | E | project (`check_c_bindings`) | two backend-`c` bindings of one capability; `capability`, `bindings` |
| `project_cbind_invalid` | project | E | project (`check_c_bindings`: the syntax), sema (`interop::plan`: the operations and types) | a `.cbind.toml` outside the closed schema of `spec/cabi.md` Section 6; `binding`, `key`, `line`, `reason` |
| `project_export_unknown`, `project_export_ambiguous` | project | E | sema (`interop::plan`) | an `[interop.c] exports` target that names nothing (`entry`, `index`) or several definitions (`entry`, `candidates`) |
| `project_export_not_function` | project | E | sema (`interop::plan`) | an export target that is no function, or a generator; `entry`, `kind` |
| `project_export_not_public`, `project_export_not_project` | project | E | sema (`interop::plan`) | an export target that is private, or a standard-library declaration; `entry` |
| `project_export_generic` | project | E | sema (`interop::plan`) | a generic export target; `entry`, with a note naming the project's monomorphic callers |
| `project_export_symbol_invalid` | project | E | sema (`interop::plan`) | an export symbol, explicit or defaulted, outside its pattern or in a reserved family; `symbol`, `reason` |
| `project_export_symbol_collision` | project | E | sema (`interop::plan`) | two generated identifiers coincide (`spec/cabi.md` Section 5); `symbol`, `items` |
| `project_export_int_unused` | project | W | sema (`interop::plan`) | `int` given on an entry whose function has no top-level `Int`; `entry` |
| `project_type_name_unknown` | project | E | sema (`interop::plan`) | a `type_names` key that names no type reachable from the exports; `key` |
| `project_export_skipped_generic` | project | I | cli (`cpl build` only) | the default export set left out a generic `pub fn` (T74); `function` |
| `project_python_target_not_enabled` | project | E | project (`check_python`) | Milestone 3: `[interop.python]` present while `"python"` is not in `[interop] targets`; `key`, `targets` |
| `project_python_option_invalid` | project | E | project (`check_python`) | Milestone 3: `[interop.python] package` or `version` outside its domain (`spec/python.md` 1); `key`, `found`, `reason` |
| `project_binding_adapter_invalid` | project | E | project (`check_python`) | Milestone 3: a backend-`python` `adapter` that is not a dotted path of Python identifiers; `binding`, `found` |
| `project_binding_timeout_invalid` | project | E | project (`check_python`) | Milestone 3: a `timeout_ms` integer that is not positive, or `timeout_ms` on a backend-`c` binding with `isolation = "in_process"`; `binding`, `found` |
| `project_fixture_binding_unknown` | project | E | project (`check_python`) | Milestone 3: a `[test.fixtures]` value that names no binding; `capability`, `binding` |
| `project_fixture_capability_mismatch` | project | E | project (`check_python`) | Milestone 3: a `[test.fixtures]` binding whose capability is not the entry's key; `capability`, `binding`, `found` |
| `index_unknown_symbol` | index | E | index | inspect/run/prove target not found (`name`, the target as read; notes per Section 6.1); exit 1 for inspect, 2 for run and prove (T59) |
| `index_ambiguous_symbol` | index | E | index | bare name matches several declarations (Section 6.1): `name`, `candidates` (qualified names in display-path order), a note at each declaration |
| `index_partial` | index | W | index | the program is empty, a definition has `HasErrors`, or the check reported an error; callers and tests may be incomplete |
| `git_unavailable` | git | W | work | `cpl work check` only: `git` not installed or no repository, so `base`/`target` resolution is skipped (Section 7.2); once per file, span-less, with `reason` (`no_repository`, `not_a_repository`, `not_installed`); every other command reports `commit: null` silently (Section 6.3) |
| `git_not_a_repository` | git | E | index | repository required (`path`; `"."` from `cpl diff`) |
| `git_unknown_revision` | git | E | index | `rev-parse --verify` failed, with any exit status, or the name is empty or starts with `-` (Section 1.8); `revision` |
| `git_no_project_at_revision` | git | E | index | no `cpl.toml` at the project prefix in that commit; `revision`, `commit` |
| `git_command_failed` | git | E | index, work | any other git failure, a `git` that cannot be run where a command needs git included (`cpl diff`, `inspect --include changes`), exit 2; for a failed command `args`, `exit_status` (-1 for a signal; `status` is semantics S50's key), and `stderr`, and for a `.work` ref `change`, `item`, `ref` |
| `work_missing_base`, `work_missing_target` | work | E | work | required change items absent |
| `work_bad_workers` | work | E | work | `workers` < 1, or beyond `u32` (`change`, `found`); also `ChangeRun::open`'s refusal of a change with `workers 0` |
| `work_unknown_ref` | work | E | work | `base`/`target` does not resolve |
| `work_target_not_branch` | work | E | work | `target` resolves but is not a branch (grammar D35) |
| `work_selector_empty` | work | E | work | a `write`/`depend`/`preserve` selector matches no declaration and is not a file selector, or an `add` namespace is under `std` or `gen`, is no module path, or names a declaration (T67); `selector`, `set` |
| `work_add_namespace_new` | work | W | work | an `add` namespace under which no module exists yet (Section 7.2) |
| `work_file_selector_shadowed` | work | W | work | a file selector's spelling also names a declaration, which wins (T42) |
| `work_file_not_grantable` | work | E | work | a file selector not listed in `[work.policy] files` (T42) |
| `work_provisional_identity` | work | W | work | a declaration named by a task or requirement has no `@id` (T47); once per declaration, never for a std declaration |
| `work_facet_not_applicable` | work | E | work | facet does not apply to the declaration kind (Section 5.1) |
| `work_identity_not_writable` | work | E | work | `identity` in a `write`/`add` set (Section 10.3) |
| `work_unknown_task` | work | E | work | `after`/`review` names no task |
| `work_cycle` | work | E | work | `after` graph has a cycle |
| `work_overlapping_writes` | work | E | work | unordered tasks write the same facet or add to overlapping namespaces (`cpl work check`, and `ChangeRun::open`) |
| `work_too_many_tasks` | work | E | work | a change declares more than 4,096 tasks (`cpl work check`, and `ChangeRun::open`); `change`, `tasks`, `limit` |
| `work_test_selector_empty` | work | E | work | `require test(sel)` matches no test |
| `work_prove_not_function` | work | E | work | `require prove(sel)` names no function: a declaration that is no function (`kind`), nothing, or a namespace without functions; `selector` |
| `work_preserve_written` | work | E | work | a facet both written and preserved by one task |
| `work_task_without_authority` | work | W | work | task with empty `write` and `add` not used by `review` |
| `work_stale_submission` | work | E | work (M6) | fencing token superseded or never issued, attempt not the lease's, task no longer `Running`, lease expired, or change run ended (Section 10.2) |
| `work_conflict` | work | E | work (M6) | two predecessors changed one facet (Section 10.4) |
| `work_stale_against_target` | work | E | work (M6) | a candidate's `expect base` fails against the target head (Section 10.6) |
| `work_review_stale` | work | E | work (M6) | the review's reviewed candidates are not the current ones |
| `work_not_authorized` | work | E | work (M6) | `cpl work integrate` by an actor without integration authority (Section 10.6); exit 2 |
| `work_boundary_unenforceable` | work | E | work (M6) | the host `Sandbox` cannot enforce a task's granted boundary, or its launch reports a wider one; no lease is issued or recorded (Section 10.8) |
| `patch_facet_unsupported` | patch | E | work | patch targets a facet other than `body` (D39) |
| `patch_unknown_symbol` | patch | E | work | selector not in the baseline |
| `patch_target_missing` | patch | E | work | target not in the tree being patched |
| `patch_stale` | patch | E | work | `expect base` hash differs (Section 5.5); `expected`, `actual` |
| `patch_out_of_scope` | patch | E | work | change outside the grant (Section 10.5), including a rebound name (T55); `path` for a file-level violation |
| `patch_preserved_facet` | patch | E | work | preserved facet changed |
| `patch_identity_changed` | patch | E | work | identity added or removed outside the grant |
| `patch_syntax` | patch | E | work | a submitted `.patch` file does not parse; `notes` carry the parse diagnostic (Section 10.5) |
| `usage_no_project` | usage | E | cli | no `cpl.toml` found |
| `usage_bad_argument` | usage | E | cli | flag or argument invalid; its payload names the offending part where one exists: `argument` (the 0-based index), `pointer` (RFC 6901, in an `--arg` value), `key` (an `--arg` object's duplicate key), `parameters` and `arguments` (the counts of a wrong argument count), `parse_code` (a constant expression that does not parse), `path`, `selector`, `id`, `include`, `lint`; a command line clap refuses carries clap's message, clap's tip, and the command's usage line as notes |
| `usage_not_a_function` | usage | E | cli | `cpl run` target is not a function, or is a generator (T61) (`cannot_run_generic` and `cannot_run_effectful` are semantics S39's; both exit 2) |
| `usage_not_provable` | usage | E | cli | `cpl prove` target is not a function |
| `usage_not_implemented` | usage | E | cli | reserved command (Milestone 6), with a note naming the milestone; no project needed; from Milestone 2 also `cpl build --target python` or `node`, with a note naming Milestone 3 or 4 |
| `build_target_not_enabled` | build | E | cli | `cpl build` on a project without the requested target (`c`, or from Milestone 3 `python`) in `[interop] targets`; `targets`; exit 1 |
| `build_python_unavailable` | build | E | emit-c | Milestone 3: `--target python` without a CPython 3.10 or later that has `Python.h` (`spec/python.md` 1); `interpreter`, `reason`; exit 2 |
| `build_cc_unavailable` | build | E | emit-c | no C compiler by T72's precedence; `searched`, the candidates tried; exit 2 (also `cpl run` and `cpl test` when they need a driver) |
| `build_cc_unsupported` | build | E | emit-c | a compiler that is neither gcc 9 or later nor clang 12 or later; `found`, `minimum`; exit 2 |
| `build_target_unsupported` | build | E | emit-c | the compiler's target is outside the platform matrix of `spec/cabi.md` Section 1; `target`; exit 2 |
| `build_target_untested` | build | W | emit-c | the compiler's target is of the matrix's second tier; `target` |
| `build_sanitizer_unavailable` | build | E | emit-c | a requested sanitizer that the compiler cannot link; `sanitizer`; exit 2 |
| `build_adapter_compile_failed` | build | E | emit-c | an adapter or vendored source of a binding does not compile; `binding`, `file`, `stderr`; exit 1 |
| `build_native_dependency_missing` | build | E | emit-c | a `libraries` entry the linker cannot find; `binding`, `library`; exit 1 |
| `build_link_failed` | build | E | emit-c | the link failed, an unresolved symbol included; `stderr`, `libraries`; exit 1 |
| `build_lock_missing` | build | E | cli (`cpl build --release`, with the release check) | no `cpl.lock`, or one without `targets.c`, replacing `project_lock_missing` in `cpl build --release` (T75); exit 1 |
| `build_lock_mismatch` | build | E | emit-c (`--release`) | the lock's `targets.c`, `foreign.c.<b>.headers`, or `foreign.c.<b>.libraries` differ from the build (T75); `differences`; exit 1 |
| `build_abi_changed` | build | E | emit-c (`--release`, `--lock`) | an ABI break by `spec/cabi.md` 9.3 (an export removed or its ABI digest or integer mapping changed, a locked exported symbol missing, another C API major) under an unchanged `prefix` and `abi` (T75, T81); `symbol`, `id`, `expected`, `found`; one per break; exit 1 |
| `interop_*` | interop | E | runtime (the C API's error objects), cabi (a driver's `error` responses), cli (`interop_bind_failed`, `cpl run`, exit 1) | the codes of `spec/cabi.md` Section 4, which owns their statuses and payloads |
| `engine_divergence` | internal | E | cli, cabi (`compare`) | under `--engine both`, the engines' outputs differ after semantics S75's normalization (Section 7.2 `--engine`); `eval`, `native` (compact JSON), `function` or `test`; exit 2 (T73) |
| `internal_error` | internal | E | any | also used by the CLI for its own unrecoverable conditions, exit 2: `kind` `panic`, `io` (with `path`), `std_unavailable` (`LoadError::Std`); by `cpl-index` for an index without the check's hole reports (`hole_reports_missing`); from Milestone 2 by `cpl-emit-c` (kind `emitted_c_rejected`: the compiler rejected emitted or runtime C, with its `stderr`; kind `emit_failed`: `emit` was given a program with errors) and by `cpl-cabi` and the CLI (kind `native_driver_failed`, with `signal` or `exit_status`; kind `native_protocol`), all exit 2; by the C runtime (kind `runtime_invariant`, the status `CPL_E_INTERNAL` of `spec/cabi.md` 4.6, a toolchain defect: in a driver's `{"error": E}` answer it ends the command with exit 2); an `internal_error` fault raised by evaluated code is a fault and exits 1 (T61), the binding kinds of semantics S80 (`binding_result_ill_typed`, `binding_error_unrepresentable`, `binding_protocol`, `binding_host_failed`) included |

Semantic-diff statements (`identity_added`, ..., `verification_policy_changed`,
Section 9.1) are not diagnostics and never appear as codes.

Where the constants of Milestone 2 live. Each code is defined once, in the
codes module of the crate that produces it: every `project_*` code in
`cpl_project::codes` and its `PROJECT` table, whichever crate raises it
(as for the `project_binding_*` codes, so that `cpl_sema::codes::category`
and the corpus harness of Section 12.2 accept them unchanged); the
`build_*` codes and the kinds `emitted_c_rejected` and `emit_failed` in
`cpl_emit_c::codes`; the `interop_*` codes with their statuses in
`cpl_rtspec::interop`, from which the runtime's generated header takes
them; and `engine_divergence` and the kinds `native_driver_failed` and
`native_protocol` in `cpl_cabi::codes`. `cpl-cli`'s `codes.rs` re-exports
those it reports. Semantics' codes gain producers too: the native runtime
raises `refinement_violation` for C export arguments (semantics S81) and
adapter results (S77) and `resource_exhaustion` with `resource: "memory"`
(S76), and `cannot_run_effectful` carries the notes of S78.

Decision: [D-272] T76. Milestone 2 adds two toolchain categories to
`cpl_diag::Category`, whose `ALL` becomes 18: `build`, for `cpl build` and
the native toolchain, whose codes this appendix owns, and `interop`, for
the C API's error objects and the binding errors a command reports, whose
codes `spec/cabi.md` Section 4 owns; and it adds the codes of the table
above: the `project_*` codes of the C target, the `build_*` codes,
`engine_divergence`, and the `internal_error` kinds `emitted_c_rejected`,
`emit_failed`, `native_driver_failed`, and `native_protocol`, and the kind
`runtime_invariant` that the C runtime raises. Rationale: a
build that the environment cannot run and a C host that misuses the API
are failures of other parties than the program and the command line, for
which no category existed (`usage` means that the command line was wrong,
and `internal` that the toolchain failed), while a divergence of the
engines is the toolchain's own failure, hence `internal`. Rejected
alternatives: the C API's codes in `usage` (a C host's misuse is another
party's error); the `build_*` failures as `internal_error` kinds (a
missing compiler or a broken adapter is no toolchain defect).

Every `patch_*` diagnostic blames its target with `symbol`,
`qualified_name`, and `facet` together, or, when no declaration can be
named (an id in neither facet map that is not provisional), carries the
id in the payload field `target` and no facet (Section 1.9); the payload
fields of the `work_*` codes are those Section 7.2's `cpl work check`
lists.

## Appendix C: wire types

All in `cpl_diag::json` unless noted.

| Type | Defined in | Used by |
| - | - | - |
| `Envelope<T>`, `ToolchainInfo`, `Status` | 1.4.2 | every `--json` output |
| `DiagnosticJson` (semantics S49), `NoteJson`, `BacktraceJson`, `Location` | 1.4.2 | every output |
| `Revision`, `RevisionState`, `Digest`, `FacetHash` | 1.4.2, 6.3 | every output, receipts, facet maps |
| `SymbolVersion` | 1.4.2 | inspect, receipts, hole reports, candidates |
| `SymbolBlock` | 6.4 | inspect |
| `HoleReportJson` (semantics S48 plus `dependencies.symbols`) | 7.2 `check` | check, inspect |
| `CheckOutput`, `VerificationStatusJson` | 7.2 | `cpl check` |
| `FmtOutput` | 7.2 | `cpl fmt` |
| `RunOutput` | 7.2 | `cpl run` |
| `TestReport`, `TestCaseRecord` (semantics S46 `test_result`) | 1.7, 7.2 | `cpl test`, `cpl-eval::run_tests` |
| `InspectOutput` | 6.4 | `cpl inspect` |
| `ProveOutput`, `Obligation`, `Assumption` | 7.2, 8.1 | `cpl prove`, receipts |
| `VerificationReceipt`, `TestReceipt` | 8 | `cpl prove`, `cpl test`, integrator |
| `DiffOutput`, `DiffStatement` | 9.2 | `cpl diff`, candidates, inspect `changes` |
| `IdAssignOutput` | 7.2 | `cpl id assign` |
| `WorkCheckOutput` | 7.2 | `cpl work check` |
| `Baseline` (`cpl_work`) | 5.5, 10.1 | candidates |
| `VerificationConfig`, `VerifierInfo`, `TestRunConfig`, `TestSelectorSpec`, `Fixture`, `ReceiptCore` | below | receipts, `CheckRunner` |
| `ProofStatus`, `BindingKind`, `Include`, `ExecutionBoundary` with `FsAccess`, `ProcessAccess`, `NetworkAccess`, and `Relation` | 1.4.2, 1.6, 1.8, 10.8, 6.4 | the vocabulary enums of other crates' sections, defined here and re-exported there |
| `BuildOutput`, `BuildArtifact`, `BuildInput`, `BuildExport`, `BuildBinding`, `BuildLock`, `CompilerInfo`, `RuntimeInfo` (Milestone 2) | 7.2 `cpl build` | `cpl build`, `build.json` |
| `EngineKind`, `RunBinding`, `EngineRunOutput`, `EngineTestOutput` (Milestone 2) | 7.2 `--engine` | `cpl run`, `cpl test` |

The objects the sections show without a type name have these names in
`cpl_diag::json`: `NoOutput` (an envelope with no keys of its own),
`TestOutput` (the `cpl test` result: `TestReport` flattened plus
`receipt`), `CheckSummary` (`summary` of `CheckOutput`),
`HoleBindingJson`, `HoleCapabilityJson`, `HoleOperationJson`,
`HoleDependenciesJson` (the parts of `HoleReportJson`), `InspectChanges`
(`changes` of an inspect response), `VerificationPolicyJson`, `FmtFileJson`
and `FmtResult` (`files[]` of `FmtOutput`), `TestKind` and `TestStatus`,
`FailingCase` and `ShrunkCase` (of `TestCaseRecord`), `ObligationStatus`,
`AssumptionKind`, `ReceiptKind` (the `receipt` key), `ReceiptInputsJson`
(`inputs`), `TestReceiptEntry` (an entry of `TestReceipt::tests`: the
deterministic part of a `TestCaseRecord`), `TestSummary`,
`ReceiptToolchain`, `ModuleHash`, `DiffStatementKind`, `DiffDuplicates` and
`DuplicateId` (`duplicates` of `DiffOutput`), `IdAssignmentJson` and
`SkippedFileJson` (of `IdAssignOutput`), and `WorkChangeJson`,
`WorkTaskJson`, `WorkSelectorJson`, `WorkTargetJson`, `WorkAddJson`,
`WorkRequirementJson`, `RequireKind` (of `WorkCheckOutput`); `JsonMap` is
`serde_json::Map<String, serde_json::Value>`. `TestCaseRecord::receipt` is
`Option<ReceiptCore>` (Section 1.7), `TestReceipt::tests` is
`Vec<TestReceiptEntry>` (Section 8.2), and `ProveOutput::symbol` is a
`SymbolVersion` (Section 7.2).

Every wire type derives `Clone, Debug, PartialEq, Eq, Serialize,
Deserialize` (the field-less enums also `Copy` and `Hash`), and a key is
written in the field order of its type (Section 7.1). An optional key is
*omitted* when absent, unless a section's example shows it as `null`, in
which case it is always present and `null` when absent: `Envelope::revision`,
`Revision::commit`, `RunOutput::result`, `result_text`, and `fault`,
`Obligation::reason` and `evidence`, `Assumption::facet`,
`VerifierInfo::version`, the three parts of `TestSelectorSpec`,
`ReceiptInputsJson::lock`, `DiffStatement::facet`, and a symbol block's
`docs` when `docs` is included (`Option<Option<String>>`: absent unless
included, then a string or `null`). Everything else optional is omitted:
every optional key of `DiagnosticJson` (semantics S49, never `null`;
`notes` and `backtrace` when empty), the optional keys of `SymbolBlock` and
`InspectOutput`, `HoleBindingJson::mutable` and the `dirty` of
`HoleDependenciesJson` and `ReceiptCore` (present only when `true`),
`ReceiptCore::fixtures` and `environment` (test receipts only), the
property-only keys of `TestCaseRecord` and `TestReceiptEntry`,
`TestCaseRecord::fault`, `counterexamples` (when empty), and `receipt`,
`VerificationStatusJson::reason`, `WorkChangeJson::base_commit`,
`WorkTaskJson::goal`, and the optional keys of `WorkSelectorJson` and
`WorkRequirementJson`, and from Milestone 2 `EngineRunOutput::engine` and
`bindings` (omitted when empty) and `EngineTestOutput::engine`; every key
of `BuildOutput` is always present. Deserializing reads an absent optional
key as `None`.

Definitions of the wire and configuration types that Sections 1.8, 6.4,
7.2, 8, and 9 name without a Rust block (all `Serialize, Deserialize`;
fields mirror the JSON examples, optional fields are `Option` and follow
the rule above), in their field order, which is the key order of their
JSON (Section 7.1):

```rust
pub struct SymbolBlock {                           // Section 6.4
    #[serde(flatten)] pub version: SymbolVersion,  // id, path, provisional, facets, definition
    pub kind: String,                              // "fn" | "ty" | "cap" | "test" | "prop"
    pub module: String, pub public: bool, pub location: Location,
    pub verification_policy: Option<VerificationPolicyJson>,   // fn only
    pub signature: Option<String>, pub contract: Option<Vec<String>>, pub effects: Option<Vec<String>>,
    pub docs: Option<Option<String>>, pub implementation: Option<String>, pub holes: Option<Vec<HoleReportJson>>,
    pub relation: Option<Relation>, pub interface: Option<bool>, pub call_sites: Option<Vec<Location>>,   // Relation: Section 1.4.2
}
pub struct VerificationPolicyJson { pub prove: bool }
pub struct HoleReportJson {                        // semantics S48 plus `dependencies.symbols`; Section 7.2 `cpl check`
    pub hole: String, pub expected_type: String,
    pub bindings: Vec<HoleBindingJson>,            // { name, type, kind, mutable? (only when true) }; kind: BindingKind (Section 1.6)
    pub capabilities: Vec<HoleCapabilityJson>,     // { parameter, capability, operations: [{ name, signature, in_bound }] }
    pub requires: Vec<String>, pub ensures: Vec<String>, pub invariants: Vec<String>,
    pub dependencies: HoleDependenciesJson,        // semantics S48: { source_revision, dirty? (only when true), toolchain: { cpl, edition, std }, modules: [{ module, content_hash }] } plus symbols: [SymbolVersion] (Section 7.2)
}
pub struct VerificationStatusJson { pub symbol: StableId, pub qualified_name: String, pub status: ProofStatus, pub reason: Option<String> }
pub struct Obligation {                            // Section 8.1
    pub id: String, pub kind: String, pub clause: String, pub location: Location,
    pub status: ObligationStatus,                  // proved | failed | unknown
    pub reason: Option<String>, pub evidence: Option<serde_json::Value>, pub assumptions: Vec<String>,   // assumption ids
}
pub struct Assumption {                            // Section 8.1
    pub id: String, pub kind: AssumptionKind, pub symbol: StableId, pub facet: Option<Facet>, pub description: String, pub admitted: bool,
}
#[serde(rename_all = "snake_case")] pub enum AssumptionKind { UncheckedCallee, UnprovedDependency, Hole, ForeignAdapter }
pub struct VerificationConfig { pub release: bool, pub policy: VerificationPolicyJson, pub proof_timeout_ms: u64, pub admitted: Vec<StableId> }
pub struct VerifierInfo { pub name: String, pub version: Option<String>, pub configuration: serde_json::Map<String, serde_json::Value> }
pub struct TestRunConfig {                         // Section 8.2 `configuration`
    pub seed: u64, pub cases: u32, pub max_shrinks: u32, pub timeout_ms: u64,
    pub selector: TestSelectorSpec, pub check_contracts: bool, pub fail_fast: bool,
}
pub struct TestSelectorSpec { pub namespace: Option<Vec<String>>, pub name: Option<String>, pub id: Option<StableId> }   // the wire form of cpl_eval::TestSelector
pub struct Fixture { pub capability: String, pub binding: String, pub digest: Digest }
/// The receipt object of semantics S59 (Section 8 `core`; also the `receipt` field of `verification_result` and `test_result`).
pub struct ReceiptCore {
    pub source_revision: String,               // commit id, or "unversioned"
    #[serde(skip_serializing_if = "Option::is_none")] pub dirty: Option<bool>,   // Some(true) only
    pub toolchain: ReceiptToolchain,           // { cpl, edition, std }
    pub modules: Vec<ModuleHash>,              // [{ module, content_hash }]
    pub check_configuration: serde_json::Value, // { release } or { cases, max_shrinks, max_call_depth, seed }
    pub assumptions: Vec<Assumption>,
    #[serde(skip_serializing_if = "Option::is_none")] pub fixtures: Option<Vec<Fixture>>,              // test receipts only
    #[serde(skip_serializing_if = "Option::is_none")] pub environment: Option<BTreeMap<String, String>>, // test receipts only
}
pub struct ReceiptToolchain { pub cpl: String, pub edition: String, pub std: Digest }
pub struct ModuleHash { pub module: String, pub content_hash: Digest }

// The command results of Section 7.2, flattened into the envelope after its five keys.
pub struct CheckOutput { pub release: bool, pub summary: CheckSummary, pub holes: Vec<HoleReportJson>, pub verification: Vec<VerificationStatusJson> }
pub struct CheckSummary { pub errors: u32, pub warnings: u32, pub holes: u32, pub definitions: u32, pub modules: u32, pub files: u32 }
pub struct FmtOutput { pub check: bool, pub files: Vec<FmtFileJson> }
pub struct FmtFileJson { pub path: String, pub result: FmtResult }   // FmtResult: unchanged | formatted | would_format | error
pub struct RunOutput {
    pub function: SymbolVersion, pub arguments: Vec<serde_json::Value>,   // value JSON, one per positional parameter
    pub result: Option<serde_json::Value>, pub result_text: Option<String>, pub fault: Option<DiagnosticJson>,
}
pub struct TestOutput { #[serde(flatten)] pub report: TestReport, pub receipt: TestReceipt }
pub struct TestReport { pub seed: u64, pub cases: u32, pub selected: u32, pub tested: u32, pub failed: u32, pub results: Vec<TestCaseRecord> }
pub struct TestCaseRecord {                        // the payload of `test_result` (semantics S46) plus `kind` and `counterexamples`
    pub symbol: StableId, pub qualified_name: String, pub kind: TestKind, pub status: TestStatus, pub duration_ms: u64,
    pub seed: Option<u64>, pub property_seed: Option<String>, pub cases: Option<u32>,      // properties only
    pub failing_case: Option<FailingCase>, pub shrunk: Option<ShrunkCase>,                  // a failed property (not one that failed at setup)
    pub fault: Option<DiagnosticJson>, pub counterexamples: Vec<DiagnosticJson>, pub receipt: Option<ReceiptCore>,
}
pub struct FailingCase { pub index: u32, pub input: JsonMap }
pub struct ShrunkCase { pub input: JsonMap, pub shrink_steps: u32, pub shrink_exhausted: bool }
pub struct InspectOutput {
    pub include: Vec<Include>, pub symbol: SymbolBlock, pub interface_dependencies: Option<Vec<SymbolBlock>>,
    pub dependencies: Option<Vec<SymbolBlock>>, pub callers: Option<Vec<SymbolBlock>>, pub obligations: Option<Vec<Obligation>>,
    pub tests: Option<Vec<SymbolBlock>>, pub changes: Option<InspectChanges>,
}
pub struct InspectChanges { pub since: Revision, pub statements: Vec<DiffStatement> }
pub struct ProveOutput { pub symbol: SymbolVersion, pub result: ProofStatus, pub obligations: Vec<Obligation>, pub receipt: VerificationReceipt }
pub struct DiffOutput { pub old: Revision, pub new: Revision, pub statements: Vec<DiffStatement>, pub duplicates: DiffDuplicates }
pub struct DiffStatement {
    pub statement: DiffStatementKind, pub id: StableId, pub path: String, pub kind: String, pub facet: Option<Facet>,
    pub detail: serde_json::Value,                 // written with its keys in declared order (Section 9.2)
}
pub struct DiffDuplicates { pub old: Vec<DuplicateId>, pub new: Vec<DuplicateId> }
pub struct DuplicateId { pub id: StableId, pub files: Vec<String> }
pub struct IdAssignOutput { pub check: bool, pub assignments: Vec<IdAssignmentJson>, pub skipped_files: Vec<SkippedFileJson> }
pub struct IdAssignmentJson { pub path: String, pub line: u32, pub symbol: String, pub kind: String, pub id: StableId }
pub struct SkippedFileJson { pub path: String, pub reason: String }
pub struct WorkCheckOutput { pub file: String, pub changes: Vec<WorkChangeJson> }
pub struct WorkChangeJson {
    pub name: String, pub base: String, pub base_commit: Option<String>, pub target: String, pub workers: u32,
    pub tasks: Vec<WorkTaskJson>, pub integrate: Vec<WorkRequirementJson>,
}
pub struct WorkTaskJson {
    pub name: String, pub after: Vec<String>, pub write: Vec<WorkSelectorJson>, pub add: Vec<WorkAddJson>,
    pub depend: Vec<WorkSelectorJson>, pub preserve: Vec<WorkSelectorJson>, pub goal: Option<String>,
}
pub struct WorkSelectorJson { pub selector: String, pub targets: Option<Vec<WorkTargetJson>>, pub file: Option<String> }
pub struct WorkTargetJson { pub id: StableId, pub facet: FacetSelector }
pub struct WorkAddJson { pub selector: String, pub namespace: String }
pub struct WorkRequirementJson {
    pub require: RequireKind, pub targets: Option<Vec<WorkTargetJson>>, pub namespace: Option<String>,
    pub matched: Option<u32>, pub task: Option<String>,
}

// The receipt documents of Section 8.
pub struct ReceiptInputsJson { pub manifest: Digest, pub lock: Option<Digest>, pub std: Digest, pub sources: BTreeMap<String, Digest> }
pub struct VerificationReceipt {
    pub receipt: ReceiptKind, pub schema: u32, pub toolchain: ToolchainInfo, pub verifier: VerifierInfo, pub revision: Revision,
    pub core: ReceiptCore, pub target: SymbolVersion, pub dependencies: Vec<SymbolVersion>, pub inputs: ReceiptInputsJson,
    pub configuration: VerificationConfig, pub status: ProofStatus, pub obligations: Vec<Obligation>,
    pub assumptions: Vec<Assumption>, pub produced_at: String, pub duration_ms: u64,
}
pub struct TestReceipt {
    pub receipt: ReceiptKind, pub schema: u32, pub toolchain: ToolchainInfo, pub revision: Revision, pub inputs: ReceiptInputsJson,
    pub core: ReceiptCore, pub configuration: TestRunConfig, pub tested: Vec<SymbolVersion>, pub tests: Vec<TestReceiptEntry>,
    pub summary: TestSummary, pub fixtures: Vec<Fixture>, pub assumptions: Vec<Assumption>,
    pub environment: BTreeMap<String, String>, pub status: TestStatus, pub produced_at: String, pub duration_ms: u64,
}
pub struct TestReceiptEntry {
    pub symbol: StableId, pub qualified_name: String, pub kind: TestKind, pub status: TestStatus,
    pub property_seed: Option<String>, pub cases: Option<u32>,   // properties only
}
pub struct TestSummary { pub selected: u32, pub tested: u32, pub failed: u32 }

// Milestone 2: `cpl build` (Section 7.2).
pub struct BuildOutput {
    pub target: String, pub release: bool, pub profile: String, pub sanitize: Vec<String>, pub out_dir: String,
    pub compiler: CompilerInfo, pub runtime: RuntimeInfo, pub program_digest: Digest, pub exports_digest: Digest,
    pub exports: Vec<BuildExport>, pub bindings: Vec<BuildBinding>, pub artifacts: Vec<BuildArtifact>, pub inputs: Vec<BuildInput>,
    pub holes: u32, pub lock: BuildLock, pub cached: bool, pub duration_ms: u64,
}
pub struct CompilerInfo { pub path: String, pub family: String, pub version: String, pub target: String, pub digest: Digest }   // family: "gcc" | "clang"
pub struct RuntimeInfo { pub version: String, pub api: String, pub source: String, pub digest: Digest }   // source: "embedded" | "dir"
pub struct BuildExport { pub symbol: String, pub function: String, pub id: StableId, pub signature: Digest, pub int: String, pub effects: Vec<String> }
pub struct BuildBinding { pub binding: String, pub capability: String, pub form: String, pub isolation: String, pub grant: String }   // form: "wrapper" | "declarative"
pub struct BuildArtifact { pub kind: String, pub path: String, pub digest: Digest }   // kind: "header" | "shared_library" | "static_library" | "executable"
pub struct BuildInput { pub kind: String, pub binding: String, pub path: String, pub digest: Digest }   // kind: "adapter" | "vendored_source"
pub struct BuildLock { pub checked: bool, pub written: bool }

// Milestone 2: `--engine` (Section 7.2). The wrappers flatten the Milestone 1 outputs, which stay unchanged (T73).
#[serde(rename_all = "snake_case")] pub enum EngineKind { Eval, Native, Both }
pub struct RunBinding { pub parameter: String, pub binding: String, pub handle: u32, pub inputs: Digest }   // inputs: T82, Section 7.2
pub struct EngineRunOutput { #[serde(flatten)] pub run: RunOutput, pub engine: Option<EngineKind>, pub bindings: Vec<RunBinding> }
pub struct EngineTestOutput { #[serde(flatten)] pub test: TestOutput, pub engine: Option<EngineKind> }
```
