# CPL specifications, edition 0.1

This directory holds the specifications that turn `design-doc.md`
(the authoritative statement of intent, kept at the repository root and
never edited by the specification work) into something a parser,
typechecker, evaluator, native backend, and toolchain can be built from,
plus two review aids. The specifications are written for implementers
working on different crates concurrently: every rule a crate depends on
is stated in exactly one place, and every judgment call the design leaves
open is recorded as a numbered decision.

## The documents

The 2026-11-01 [state-machine amendment](state-machines.md) defines the
new application model, function roles, constructor invariants, and replay
tooling. It explicitly supersedes conflicting earlier rules while keeping
legacy source compatible. The later [parallelism and performance amendment](parallelism.md) adds structured task expressions, bounded machine mailboxes, and optimization rules.

| File | Covers | Owns |
| - | - | - |
| `parallelism.md` | structured parallel expressions, task cancellation, bounded machine mailboxes, optimization passes | application concurrency and performance amendment |
| `state-machines.md` | the 2026-11-01 language amendment: pure/mutate functions, program roots, scoped reducers, constructor invariants, context dispatch and replay | new syntax, checker rules, runtime guarantees and limits, replay CLI and receipts |
| `grammar.md` | the lexer, the newline and layout rules, and the complete grammar of the three file forms (`.cpl` source, `.work` coordination declarations, `.patch` semantic patches), with a trace of every design-document code block through the productions (Section 6) and the token and nonterminal names the AST mirrors | what parses, the AST shape, the *message text* of every lexical and syntax diagnostic; decisions `D1`..`D74` |
| `semantics.md` | name resolution, the type system (refinements, generics, inference), the effect system, contracts in checked mode, evaluation, faults, tests and properties, holes, development versus release mode, and the structured diagnostics | every diagnostic *code* and field name, the identity format, the value encodings, the `status` vocabulary, the acceptance-corpus expectations (Section 11); decisions `S1`..`S83` |
| `toolchain.md` | the Rust Cargo workspace and the public API of every crate, the AST and typed HIR, the formatter, stable identities and facets, the semantic index and `cpl inspect`, the command-line contract and exit codes, receipts, semantic diff, the work-coordinator data model, `cpl.toml` and `cpl.lock`, the testing strategy, and the milestone map | the CLI, configuration keys, exit codes, receipt documents, wire types, and the toolchain-only diagnostic categories (`project`, `build`, `index`, `git`, `work`, `patch`, `usage`); decisions `T1`..`T82` |
| `stdlib.md` | the prelude and the five `std` modules of 0.1 (`std.prelude`, `std.text`, `std.bytes`, `std.foreign`, `std.gen`): every signature, its contract and laws, its runtime semantics and fault conditions, the property generators and their distributions and shrinking, and the normative stub files (Section 11) | every name under `std` and in the prelude, and the builtin registry; decisions `S1`..`S48` (the library's `S` series is distinct from the semantics' and is cited as `stdlib.md S<n>`) |
| `cabi.md` | the native backend and the C boundary of Milestone 2: how a checked program becomes C11, the C11 runtime that executes it, the mechanisms that keep native execution equal to the reference evaluator, the versioned C API, exports through which a C host calls CPL, imports through which CPL calls a C library, how the evaluator reaches C adapters, native executables, and the package ABI, with the normative headers in its appendices | the runtime model, the code-generation contract, the C API and its `interop` codes, the driver protocol, and the import and export rules; decisions `C1`..`C39` |
| `python.md` | Milestone 3: the Python package that `cpl build --target python` generates (a CPython limited-API extension over the C API, the pure-Python wrapper, stubs, and wheel), Python adapters, the worker transport (cpl-worker protocol 1, for Python adapters and C adapters in workers), `cpl run` with Python bindings, and test fixtures | the Python value mapping and exceptions, the adapter contract, the worker protocol and its failures; decisions `P1`..`P11` |
| `decisions.md` | one table of all 342 `Decision:` and `Design conflict:` lines of the documents, each with its project-wide stable id `D-nnn`, its section, and a one-sentence rule and rationale (`D-226`..`D-237` are the fixture-review rulings of 2026-10-28, `D-238`..`D-240` the rulings on the parser team's report and the `cpl-hir` crate that followed, `D-241`..`D-250` the rulings that folded the core crates' implementation reports into the specifications, `D-251`..`D-263` those that folded the reports of the Milestone 1 integration phase: the connected crates, the `cpl` CLI, their testers, and the acceptance audits; `D-264`..`D-324` the Milestone 2 decisions of the native backend and the C boundary; `D-325`..`D-327` the three that the writing of the Milestone 2 specifications added; and `D-328`..`D-342` the Milestone 3 decisions of the Python package, the worker transport, and test fixtures) | the review index for the project owner |
| this file | orientation | |

When two documents disagree, the precedence is `design-doc.md`, then
`grammar.md`, `semantics.md`, `toolchain.md`, `stdlib.md`, `cabi.md`,
`python.md`, a later document still winning where an earlier one is
silent, except where a later document amends an earlier one explicitly
(`python.md` Section 0.2 lists its amendments). The
ownership boundaries above are themselves decisions (semantics S60 for the
diagnostic contract, grammar D59 and toolchain T6 for parse codes,
toolchain T45 for seeds and shrinking, semantics S55 and stdlib S29 for
refinement identity, `cabi.md` C1 for the C boundary and its place in this
order).

## Reading order

1. `design-doc.md` in full. Everything else cites it by line number and
   never knowingly contradicts it; where it is internally inconsistent the
   chosen reading is marked `Design conflict:` (five such lines exist:
   D-069, D-080, D-132, D-181, D-211 in `decisions.md`).
2. `grammar.md` Sections 1-4, then Section 6 (the design's code blocks
   traced through the grammar), then Section 5 (disambiguation rulings).
3. `semantics.md` Sections 1-3 (program model, resolution, types), then
   4-6 (effects, contracts, evaluation), then 7-9 (tests, holes,
   diagnostics), then 11 (what the corpus must do).
4. `stdlib.md` Sections 0-1 (conventions, prelude), 4 (`List`), 8
   (generators), 10-11 (how stubs load; the stub files). Sections 2-7 are
   reference material per type.
5. `toolchain.md` Sections 0-3 (workspace, crate APIs, pipeline and HIR),
   then whichever of 4-11 your crate implements, then 12-13 (tests,
   milestones).
6. `cabi.md` Section 0 (ownership, terms, precedence), then Sections 1-3
   (code generation, the runtime, the equivalence mechanisms) for the
   native backend, or 4-6 (the C API, exports, imports) for C hosts and
   adapters, then 7-9 (the evaluator and native code, native executables,
   the package ABI). Appendices A-C are the normative headers.
7. `python.md` Sections 0-5 (the package, values, exceptions, adapters),
   then 6 (the worker transport), then 7-9 (`cpl run`, fixtures, the
   lock) for Milestone 3.
8. `decisions.md` when reviewing judgment calls rather than implementing.

Implementers of one crate need the toolchain section for that crate plus
the semantics sections it realizes; the toolchain document names those
sections at each API block.

## Fixed decisions

These were settled before the specifications were written and are not
reopened in them, except where a bullet names the decision that reopened
it:

- **Rust workspace.** The toolchain is a Cargo workspace at the repository
  root with the crates `cpl-diag`, `cpl-syntax`, `cpl-hir`, `cpl-project`,
  `cpl-sema`, `cpl-eval`, `cpl-index`, `cpl-work`, and `cpl-cli`, and from
  Milestone 2 `cpl-rtspec`, `cpl-rt`, `cpl-emit-c`, and `cpl-cabi`
  (toolchain 1.1-1.2; `cpl-hir`, which holds the typed HIR, was added by
  toolchain T50, `cpl-project`, which loads projects and embeds the
  standard library, by T52, and the four Milestone 2 crates by T69); the
  C API and the Python and Node packages of later milestones wrap the same
  native core: the C runtime of `cpl-rt` and the C that `cpl-emit-c` emits
  from the checked program; the Rust crates are the toolchain and the
  reference evaluator, the oracle native execution is tested against
  (reopened by toolchain T68 [D-264] on 2026-10-29: the earlier wording
  was planning text, and design line 38 says "the same native CPL
  core").
- **Interpreter-first Milestone 1.** Milestone 1 executes CPL through the
  *reference evaluator*, a tree-walking interpreter over the typed HIR;
  native execution, which the design lists under Milestone 1, moves to
  Milestone 2 together with every code-emission path. This is a recorded
  deviation from the design (semantics S51, toolchain's introduction and
  T41, stdlib's introduction), and every dynamic rule is stated as a
  requirement on "the runtime" so that the later backend is validated
  against the evaluator.
- **Project layout.** `cpl.toml` is at the project root; sources live
  under `[build] sources` (default `["src"]`); the `module a.b.c` header is
  authoritative and `src/a/b/c.cpl` (or `src/a/b/c/<name>.cpl` for a
  further file of the same module) is only the conventional location,
  warned about when missed; work declarations live in `work/*.work` and
  submitted patches in `work/*.patch`; `cpl.lock` holds resolved inputs
  and is checked only under `--release`; the standard library's stub files
  live in `std/` at the workspace root and are embedded in the binary.
- **Facet names.** Every declaration has exactly the eight facets the
  design names: `identity`, `name`, `signature`, `contract`, `effects`,
  `body`, `docs`, `verification_policy`; `.definition` selects them all.
  The spelling is shared by selectors (grammar 4.4), diagnostics
  (semantics 9.2), the facet model and semantic diff (toolchain 5 and 9),
  and the `Facet` enum of `cpl-diag`.

Other vocabulary that is fixed across all five documents and worth knowing
before reading: the single `Unit` value is spelled `Unit`; `len` is a
property (`xs.len`) and `is_empty`, `take`, `drop`, `drop_last`, `push` are
methods; `flatten`, `check<R>`, `ConstraintError`, and the module alias
`gen` are prelude names; `/` truncates toward zero and `%` takes the
dividend's sign, with a zero divisor the fault `division_by_zero`; the
verification statuses are `proved`, `failed`, `unknown` (and, from
Milestone 5, `conditional`), the test statuses `tested` and `failed`; a
file with a lexical or syntax error yields no tree and exactly one parse
diagnostic, the earliest, whose code comes from the `parse` table of
semantics 9.4 (grammar D59, D70); standard-library operations are
bodiless `@builtin("<id>")` stubs classified by `@method` and `@property`
(an annotation name may be a keyword, grammar D71).

## The examples

`examples/` is the acceptance corpus that all five specifications are
checked against:

| Project | Contents | Role |
| - | - | - |
| `examples/reader/` | `src/reader/pages.cpl`, `src/reader/images.cpl`, `src/tests/pagination.cpl`, `work/pagination.work`, `work/pagination.patch`, `cpl.toml` | every `.cpl`, `.work`, patch, and TOML block of the design document, byte for byte, in the modules the design names |
| `examples/holes/` | `src/holes/pages.cpl`, `cpl.toml` | the design's two hole-bearing declarations (`paginate` with `?build_pages`, `summarize` with `?summary`) in a project of their own, so that holes and `--release` rejection can be exercised without touching `reader` |
| `examples/state_machine/` | `src/app/main.cpl`, `cpl.toml`, `cpl.lock`, `README.md` | the state-machine amendment: an immutable program root, scoped download machine, constructor invariant, typed commands, effect handler, and replay walkthrough |

Rules relating the corpus to the specifications:

- Text that is verbatim from `design-doc.md` is never edited. Everything
  else is *glue* and is marked with a `// glue:` comment on the preceding
  line. The corpus's glue is exactly: `parse_size` delegating to
  `std.text.parse_int` (grammar D58); the module header and `use` line of
  `tests.pagination`, which the design does not show (D57); and the copied
  `PageId`, `Positive`, and `Page` declarations of `holes`. If a verbatim
  example cannot
  typecheck under the specifications, the specifications change, not the
  example; the structural identity of refinements (semantics S55) exists
  for exactly this reason, so that `for size in gen.positive_int` feeds
  `paginate(items, size)` as the design writes it.
- `grammar.md` Section 6 traces every design code block to its corpus file
  and through the productions; every corpus file must parse, including the
  `work/` files, which `cpl check` parses for syntax only (semantics S53).
- `semantics.md` Section 11 fixes what `cpl check`, `cpl check --release`,
  `cpl test`, and `cpl run` must report on each project (for example: both
  projects check clean in development mode, `reader` fails `--release` with
  one `verification_unsatisfied`, `holes` with two `hole_in_release` and
  one `verification_unsatisfied`; the property passes 100 cases; the
  corpus projects carry no `cpl.lock`, so `--release` also warns
  `project_lock_missing`).
- `stdlib.md` Section 12 lists the concrete evaluations the standard
  library must reproduce and the check errors it must reject, and its
  introduction guarantees that every standard-library name the corpus uses
  is defined.
- `toolchain.md` Section 12 turns the corpus into the test suite: golden
  parse and format files, formatter idempotency, the interpreter example
  table (12.3), hole reports, facet hashes, `cpl work check` on
  `pagination.work`, patch application of `pagination.patch` against a
  hole-bodied copy of `reader`, and the CLI integration tests, which copy
  both projects into a temporary Git repository first.

The corpus is executed end to end, and Milestone 1 is accepted (toolchain
Section 13). `cpl-syntax` parses and formats every corpus file (its
golden, syntax-corpus, and idempotency tests); `cpl-sema` checks both
projects with exactly the `cpl check` and `cpl check --release` results
of semantics Section 11, and its negative corpus passes; `cpl-eval` runs
its interpreter corpus on the checker's output and executes every function
of the design's examples, the capability-using ones against scripted
hosts; `cpl-index` answers `cpl inspect` and `cpl prove` and builds the
receipts and hole dependencies; `cpl-work` validates `pagination.work` and
applies `pagination.patch` to a hole-bodied copy of `reader`, getting the
original back byte for byte; and the `cpl` binary implements every command
of toolchain Section 7 (`work run` and `work integrate` reserved for
Milestone 6), through which the CLI tests and the acceptance audits
reproduce every row of toolchain Section 12.3. Milestone 2, the native
backend and the C boundary, is specified (`cabi.md` and the Milestone 2
decisions `D-264`..`D-327` of it, the toolchain, the semantics, and the
standard library) and in implementation; it is accepted when every item
of the checklist of toolchain Section 12.9 holds.
