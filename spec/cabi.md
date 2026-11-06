# CPL native backend and C boundary, edition 0.1

This document specifies Milestone 2 of the implementation order, the C
boundary ("Versioned C imports and exports, generated headers, ownership
rules", design line 645): how a checked program becomes C11 (Section 1),
the C11 runtime that executes it (Section 2), the mechanisms that keep
native execution equal to the reference evaluator (Section 3), the
versioned C API (Section 4), exports through which a C host calls CPL
(Section 5), imports through which CPL calls a C library (Section 6), how
the reference evaluator reaches C adapters and native code (Section 7),
native executables (Section 8), and the package ABI (Section 9). Appendix
A holds the normative public headers `cpl.h` and `cpl_adapter.h`, Appendix
B the internal headers, `cpl_rt.h` (the contract between the runtime and
the emitted code) and `cpl_rt_gen.h` (the generated tables), and Appendix
C two generated headers as normative examples.

It is written for the implementers of the crates `cpl-rt`, `cpl-emit-c`,
and `cpl-cabi` (toolchain 1.1, T69) and for the authors of C hosts and C
adapters. The C blocks of the appendices are the contracts those parties
code against: a header may gain declarations only as Section 4.2 allows,
and no name, field, or value shown may change without updating this
document.

| Document | Role |
| - | - |
| `design-doc.md` | authoritative intent; this document never contradicts it knowingly |
| `spec/grammar.md` | the grammar; nothing here changes it |
| `spec/semantics.md` | the language semantics every runtime satisfies, including the equivalence of native execution with the evaluator (S75) and the boundary rules S76-S81 that Milestone 2 adds |
| `spec/toolchain.md` | the crates and their Rust APIs, the CLI (`cpl build`, `--engine`), the manifest and lock keys, the `project` and `build` diagnostic codes, and the testing strategy |
| `spec/stdlib.md` | the standard library, including how bindings construct `ForeignError` (S45), machine integers at the boundary (S46), and the single source of builtin tables (S47) |
| this document | everything in the first paragraph |

Conventions. Where the design is explicit this document follows it. Where
the design is silent a rule is chosen and marked with a line beginning
`Decision:`, numbered `C1`, `C2`, ... with the rule, the rationale, and the
rejected alternatives. Every `Decision:` line also carries a project-wide
stable identifier `[D-nnn]`, indexed in `spec/decisions.md`; the local
`C<n>` numbers are the citation form (`cabi.md C22`). Rules owned by
another specification are marked `Semantics:`, `Library:`, or `Toolchain:`
and stated only as far as this document needs them. Design lines are cited
by number (`design line 465`), other specifications by document and
section (`semantics 6.7`, `toolchain 11.1`) or decision (`semantics S77`,
`stdlib.md S45`, `toolchain T74`). C blocks are normative: the appendices
are complete headers, and the emitted-code examples of Section 1.4 fix
the shape of generated code (the numeric ids in them are illustrative).
TOML blocks are normative schemas of the keys they show (the keys of
`cpl.toml` are toolchain 11.1's), and JSON blocks normative schemas written
as examples. The Rust functions this document names belong to crates whose
APIs toolchain owns. Statements about the Milestone 1 code cite its files
and were checked against it on 2026-10-29.

## 0. Scope and ownership

### 0.1 What this document owns

| Area | Owned here | Owned elsewhere and cited |
| - | - | - |
| native code generation | emission from the typed HIR, the calling convention, the slot discipline, the lowering of every HIR form, the names of emitted code, the compiler flags and profiles, the compile and link steps, the artifacts, the version script, namespacing, the platform matrix (Section 1) | the `cpl build` command line and its compiler selection (toolchain 7.2, T72) |
| the runtime model | the runtime's sources and how they ship, the value representation, memory management, `Int`, the other values, every builtin in C, equality, the tables generated from `cpl-rtspec` (Section 2) | what every builtin computes (`spec/stdlib.md`); the crate `cpl-rtspec` (toolchain T70) |
| equivalence mechanisms | the facts computed at emit time, the contract regions, fault records and claims, the C renderings, depth, guard, time, cancellation, the driver and cpl-driver protocol 1, and what the native side supplies to the differential suites (Section 3) | the equivalence statement (semantics S75); the differential suites, their comparison rules and coverage gate, the gate rule, and the skip policy (toolchain 12.6, T78) |
| the C API | its headers, dispatch, versioning, lifecycle, evaluation contexts, threading, handles, values, statuses, error objects, the `interop` codes, ownership, unwinding (Section 4, Appendix A) | memory exhaustion as a fault (semantics S76); re-entrant evaluations (semantics S79) |
| exports | export entries, symbols, the generated header, the value mapping, boundary checks on entry, host capabilities and the callback contract, effects (Section 5) | the `[interop.c]` keys and the default export set (toolchain 11.1, T74); boundary values checked as `cpl run` arguments (semantics S81) |
| imports | bindings to C, reviewed wrappers and `cpl_adapter.h`, declarative bindings and the `.cbind.toml` schema, the mechanisms of result validation and `ForeignError` construction, linking and pinning, isolation (Section 6) | the rules of result validation (semantics S77) and of the binding faults (semantics S80); `ForeignError` construction (stdlib.md S45); the binding keys (toolchain 11.1) and the lock entries (toolchain 11.2, T75) |
| the evaluator and native code | `CAdapter` and value exchange (Section 7) | `cpl run` with bindings (semantics S78); the engine selector (toolchain T73) |
| native executables | their command line, output, and exit codes (Section 8) | `cpl run` (toolchain 7.2) |
| the package ABI | soname, version node, the `abi` bump rule, and what `build_abi_changed` compares (Section 9) | the lock (toolchain 11.2); the code (toolchain Appendix B) |
| diagnostic codes | the category `interop` and its codes (Section 4.9) | the categories `project` and `build`, `engine_divergence`, the new `internal_error` kinds (toolchain Appendix B, T76) |

### 0.2 Terms

| Term | Meaning in this document |
| - | - |
| the evaluator | the reference evaluator of `cpl-eval` (semantics S51): the oracle native execution is tested against |
| the runtime | the C11 runtime of `cpl-rt` (Section 2), compiled into every package, driver, and native executable; "a runtime" is one object it creates with `<prefix>_runtime_open` (Section 4.3) |
| emitted code | the C that `cpl-emit-c` writes for one checked program: the program unit `<prefix>_prog.c`, the tables `<prefix>_tables.c`, and the API unit `<prefix>_api.c` (Section 1.6) |
| package | the artifacts `cpl build --target c` produces for one project (Section 1.6); its prefix is `[interop.c] prefix` (toolchain 11.1) |
| host | a program that calls a package through the C API |
| adapter | the code of a backend-`c` binding: a reviewed wrapper or a declarative binding (Section 6) |
| driver | a program built for the toolchain with test bodies compiled in, which runs cpl-driver protocol 1 (Section 3.8) |
| native engine | the driver, as `cpl run --engine native` and `cpl test --engine native` use it (toolchain T73) |
| word | `cplrt_w`, the 64-bit value of the runtime (Section 2.3); never part of the ABI |
| handle | `cpl_value`, `cpl_cap`, or `cpl_cancel` (Section 4.5) |
| top-level evaluation | one evaluation requested from outside CPL code: one export call, one driver request; it has its own evaluation context (Section 4.3) |

### 0.3 Precedence

Decision: [D-287] C1. This document owns the runtime model, the native
code-generation contract, the C API with the `interop` codes, the driver
protocol, and the import and export rules, and it is the last document in
the precedence order: `design-doc.md`, then `grammar.md`, `semantics.md`,
`toolchain.md`, `stdlib.md`, `cabi.md`, a later document still winning
where an earlier one is silent. It states no rule of the language: every
behaviour a CPL program can observe is fixed by `semantics.md` and
`stdlib.md`, and native execution must produce it (semantics S75); this
document fixes how. Rationale: the C boundary is a new layer whose
consumers (C hosts now, the Python and Node packages of Milestones 3 and
4 later) need one owner for every name of the C API and the runtime, while
the language must keep one owner regardless of the engine that runs it;
placing the document last means that a conflict between a mechanism here
and a language rule is resolved in favour of the language. Rejected
alternatives: the C API as a section of `toolchain.md` (that document
owns the Rust crates and the command line, while the C API is a runtime
artifact shared by every package and later binding layer); language rules
for the boundary (validation of adapter results, memory exhaustion,
re-entrancy) stated here (they bind the evaluator too, so they are
semantics S76-S81).

### 0.4 How the C API realizes design line 465

Design line 465: "A versioned CPL C API supplies contexts, value
construction, validation, opaque resources, and explicit release
operations. The internal representation of CPL values is not a public
ABI."

| Element of the sentence | Realized by |
| - | - |
| versioned | `CPL_API_VERSION_MAJOR` and `CPL_API_VERSION_MINOR` in `cpl.h`, checked at compile time by every generated header and at run time by `<prefix>_runtime_open` through `cpl_runtime_options.api_version` and `struct_size`; the function table `struct cpl_api`, append-only within a major version (Section 4.2) |
| contexts | two kinds: the **runtime object** `cpl_runtime`, created by `<prefix>_runtime_open` and released by `cpl_runtime_close`, which holds the handle table, the allocator, the heap budget, the capability registry, the immortal constants, and the default call-depth limit; and the **per-call evaluation context** `cplrt_cx`, which every export creates on its own C stack for one top-level evaluation (depth counter, frames, fault slot, attribution flag, deadline, cancellation token, stack floor, preallocated memory-fault record) and destroys before it returns (Section 4.3). A capability callback receives a third, borrowed context, `cpl_call` (Section 5.7) |
| value construction | the constructors of Section 4.8 (`cpl_int_from_i64`, `cpl_text_new`, `cpl_list_new`, `cpl_struct_new`, `cpl_enum_new`, `cpl_result_ok`, `cpl_foreign_error_new`, ...), which copy host memory in, and `cpl_value_from_json` |
| validation | three levels: the constructors' shallow checks (UTF-8, 0 or 1, arity, variant, each child's kind and declaration, Section 4.5); the deep type and refinement checks of every value entering CPL through an export (Section 5.5, semantics S81); and the validation of every capability result before CPL code sees it (Section 6.4, semantics S77) |
| opaque resources | value, capability, and cancellation handles: 16-byte structs carrying the issuing runtime's identity, a slot, and a generation (Section 4.5); the opaque objects `cpl_runtime`, `cpl_call`, and `cpl_error`; host state behind `void *user` and adapter state behind `void *state`, which CPL code never sees (Sections 5.7, 6.3) |
| explicit release operations | `cpl_value_release`, `cpl_cap_release`, `cpl_cancel_release`, `cpl_buf_release`, `cpl_error_release`, and `cpl_runtime_close`, each the only way to release what the API handed out (Section 4.7); the runtime releases values deterministically by reference counting (Section 2.4) |
| the internal representation is not a public ABI | hosts see only handles, views, and the accessor functions; the word, the object layouts, and the tables are `cpl_rt.h`'s (Appendix B), compiled into each package and never exported (Sections 1.7, 2.3) |

### 0.5 Relation to Milestone 1 and to the other documents

Milestone 1 executes CPL through the evaluator alone and defers native
execution to Milestone 2 (semantics S51, toolchain T41). This document
delivers it without changing a rule of Milestone 1: every amendment that
Milestone 2 needs is a recorded decision of the document that owns the
amended rule (semantics S75-S81, toolchain T68-T82, stdlib.md S45-S47),
and every new behaviour of the evaluator is opt-in (toolchain T77).

Toolchain: T68 reopened the fixed decision "the C API and the Python and
Node packages of later milestones wrap the same Rust core" of
`spec/README.md`: the C API and the packages of Milestones 3 and 4 wrap
one native core, the C runtime of Section 2 and the C that Section 1 emits
from the checked program, and the Rust crates are the toolchain and the
evaluator. This is the design's own wording ("Python and Node bindings
wrap the same native CPL core", design line 38).

### 0.6 Decision index

| Id | Stable id | Section | Rule |
| - | - | - | - |
| C1 | D-287 | 0.3 | this document's ownership; precedence after `stdlib.md` |
| C2 | D-288 | 1.1 | C11 lowered directly from the checked HIR; the calling convention, statuses, slot discipline, and lowering table |
| C3 | D-289 | 1.5 | fixed compiler flags and profiles; user flags only for adapter and vendored sources |
| C4 | D-290 | 1.6 | the compile and link steps, artifacts, explicit version scripts, `CPLRT()`/`CPLPKG()` namespacing |
| C5 | D-291 | 1.7 | the ABI types and the platform matrix |
| C6 | D-292 | 2.1 | the runtime is C11 source embedded in `cpl`, written out and compiled by every build |
| C7 | D-293 | 2.3 | tagged 64-bit words, heap object layouts, immortal statics |
| C8 | D-294 | 2.4 | atomic reference counts with saturation and iterative release |
| C9 | D-295 | 2.5 | `Int` as small immediates and normalized bigints; one `cplrt_int_divrem` |
| C10 | D-296 | 2.6 | `Text`, `Bytes`, `List`, struct, enum, wrapper, and capability references |
| C11 | D-297 | 2.7 | every builtin transcribed from the evaluator with a `mirrors` comment |
| C12 | D-298 | 2.8 | iterative structural equality; `Int` ordering only |
| C13 | D-299 | 3.2 | every reported fact computed at emit time by the evaluator's own helpers |
| C14 | D-300 | 3.3 | one contract region per clause under the runtime flag `check_contracts`, tagged with its obligation id; holes |
| C15 | D-301 | 3.4 | depth, stack guard, time, and cancellation at exactly the evaluator's points |
| C16 | D-302 | 3.3.1 | fault records, S34/S70/S71 claims, the violation record as raised |
| C17 | D-303 | 3.8 | the driver and cpl-driver protocol 1 |
| C18 | D-304 | 3.9 | what the native side supplies to the differential suites: its own renderings, coverage counters, table answers, and a clean shutdown |
| C19 | D-305 | 4.2 | the headers, object-carried dispatch, versioning |
| C20 | D-306 | 4.3 | the runtime lifecycle and per-call evaluation contexts |
| C21 | D-307 | 4.4 | the threading rules |
| C22 | D-308 | 4.5 | 16-byte handles carrying the issuing runtime's identity, a slot, and a generation |
| C23 | D-309 | 4.5 | constructors, accessors, views, buffers |
| C24 | D-310 | 4.6 | statuses, error objects, the `interop` codes |
| C25 | D-311 | 4.7 | allocation ownership, no unwinding, out of memory |
| C26 | D-312 | 5.1 | export entries, symbols, generated identifiers, the refusal of generic functions |
| C27 | D-313 | 5.3 | the generated header's layout; type accessor functions |
| C28 | D-314 | 5.4 | the value mapping |
| C29 | D-315 | 5.5 | boundary checks on entry |
| C30 | D-316 | 5.7 | host capabilities, the callback contract, effects |
| C31 | D-317 | 6.2 | backend-`c` bindings, reviewed wrappers, `cpl_adapter.h` |
| C32 | D-318 | 6.3 | declarative bindings with a closed `.cbind.toml` vocabulary |
| C33 | D-319 | 6.4 | the mechanisms of result validation and `ForeignError` construction |
| C34 | D-320 | 6.6 | linking and native-dependency pinning |
| C35 | D-321 | 6.7 | `in_process` is the only C isolation of edition 0.1 |
| C36 | D-322 | 7.2 | `CAdapter`: the evaluator reaches C adapters through a driver |
| C37 | D-323 | 8 | native executables with `cpl run`'s protocol |
| C38 | D-324 | 9 | the package ABI: soname, version node, the `abi` bump rule |
| C39 | D-325 | 6.3 | CPL code holds no foreign object in edition 0.1: foreign objects are adapter state, one per grant |

## 1. Native code generation

### 1.1 Emission strategy

`cpl-emit-c` lowers the checked, typed HIR (`cpl_hir::Program` as
`cpl_sema::check_with` returns it, toolchain 3.3) to C11. A program with
any definition whose `Def::status` is `DefStatus::HasErrors` is never
emitted: `cpl build` reports the check's errors and exits 1 (toolchain
T58). Generic functions are emitted once: values are uniform words whose
renderings are value-directed (Section 2.3), exactly the erasure the
evaluator relies on, so nothing is monomorphized.

One `static` C function is emitted for each of these:

| Emitted function | For | Signature (`cpl_rt.h`, Appendix B) |
| - | - | - |
| `cpl_f<n>` | every `fn` of DefId `n` with a block body, generic ones included | `static int cpl_f<n>(cplrt_cx *cx, uint32_t site, cplrt_w a0, ..., const cplrt_capref *u0, ..., cplrt_w *out)` |
| `cpl_p<n>` | every refinement declaration of DefId `n` (its `where` predicate) | `static int cpl_p<n>(cplrt_cx *cx, uint32_t site, cplrt_w self, int *holds)` |
| `cpl_o<n>_<i>` | operation `i` (declaration order) of every capability of DefId `n`: its `requires`, the effect point, the host call, result validation (Section 6.4), its `ensures` (semantics S31) | `static int cpl_o<n>_<i>(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, cplrt_w a0, ..., cplrt_w *out)` |
| `cpl_t<n>` | every `test` and `property` body (only under `-DCPL_WITH_TESTS=1`, that is, in drivers) | `static int cpl_t<n>(cplrt_cx *cx, const cplrt_w *binders)` |
| `cpl_b<n>_<binder>_lo`, `cpl_b<n>_<binder>_hi` | the bounds of the `gen.range` of `for` binder `binder` (clause order) of every property `n` (drivers only) | `static int cpl_b<n>_<binder>_lo(cplrt_cx *cx, cplrt_w *out)`, and `_hi` alike |
| `cpl_e<n>` | the entry trampoline of function `n`: of every export (Section 5.5) and, in drivers, of every function with a block body and of every callable `@builtin` stub | `static int cpl_e<n>(cplrt_cx *cx, const cplrt_w *args, const cplrt_capref *const *using, cplrt_w *out)` |

A builtin stub (`FnBody::Builtin`) has no emitted function: its calls go
to the runtime's builtin functions `cplrt_bi(<id>)` (Section 2.7). In a
driver, a callable stub's entry trampoline is the one statement `return
cplrt_bi(<id>)(cx, CPLRT_NO_SITE, CPL_D<n>, args[0], ..., out);`, so that
`cpl run` and the sweeps can name a standard-library function on either
engine (toolchain 7.2): it pushes no frame and is neither checked against
nor counted by the depth limit (semantics S72), and a fault leaving it
gains the frame `{stub}` without a span, as the evaluator's
`call_builtin` gives a top-level call (Section 1.2). A package emits the
functions of every definition, also those that no export reaches and
nothing then references; the base flags of Section 1.5 therefore silence
`-Wunused-function`.

Decision: [D-288] C2. `cpl-emit-c` lowers the checked, typed HIR directly
to C11, with no lowered IR between them. Inside the emitter each body is
put in A-normal form, every subexpression in its own temporary in the
evaluation order of semantics 6.2 and every operand marked owned or
borrowed, and is printed under the calling convention of Section 1.2 and
the slot discipline of Section 1.3, each HIR form as the lowering table of
Section 1.4 fixes. Rationale: the two properties hardest to get right are
evaluation order and the release of every value on every path; the
A-normal form fixes the order explicitly, so C's unspecified order of
argument evaluation never decides CPL's left-to-right order, and the slot
discipline makes release on every path a property of each function's text
that a shape test checks, so a second representation of every body with
its own ownership verifier would mostly re-prove what the discipline
guarantees while being one more representation to keep in sync with the
HIR, which toolchain T1 rejects. Rejected alternatives: direct LLVM
lowering (a large, version-pinned native dependency on every developer
machine, emitted code that the sanitizers do not instrument for free, and
no C headers or adapters, which the C boundary needs anyway; the design
names C emission first, line 659); Cranelift (it still needs a system
linker and a runtime, has a narrower C-ABI reach, and no header story); a
lowered IR with a dataflow ownership verifier (correct, but a new crate, a
second representation of every body, and a verifier whose main property
the discipline makes structural); C printed straight from the AST (it
loses the types and the resolution the evaluator relies on, T1); an
embedded interpreter or a bytecode VM (not native execution, design lines
38 and 644; it would put the parser and the checker into every library and
make the comparison with the oracle vacuous); `setjmp`/`longjmp` for faults
(a long jump skips the releases of the frames it crosses and would need a
second, dynamic ownership mechanism); one translation unit per CPL module
(inter-unit linkage for nothing in edition 0.1; the emitter can split
units later without an ABI change).

### 1.2 Calling convention and statuses

- Values are words (`cplrt_w`, Section 2.3). Positional arguments are
  **borrowed**: the callee never releases them and may hand them on as
  borrowed arguments. `using` handles are `const cplrt_capref *`, after
  the positional arguments, in `FnDef::using` order (semantics S28).
- `*out` is an **owned** slot of the caller. The callee stores its result
  into it with store semantics: whatever the slot held is released, and
  the slot then holds exactly one reference. Every function returns a
  word, a `Unit`-typed one the immortal `Unit`.
- The internal status is `CPLRT_OK` (0) or `CPLRT_FAULT` (1). A fault is
  held in `cx->fault` (Section 3.3.1); a cancellation request unwinds
  through the same path (`cx->cancelled`) and becomes `CPL_E_CANCELLED` at
  the API (Section 3.6).
- `site` is the index of the calling expression in the site table
  (`CPL_S<n>`, Section 1.4). The callee records it in its frame, which is
  how a backtrace frame carries "the span of the call that entered it"
  (semantics 6.7). A top-level activation has `CPLRT_NO_SITE`.
- `cplrt_enter_fn(cx, &fr, CPL_D<n>, site, params, n)` at the head of
  every `cpl_f` runs the evaluator's checks in the evaluator's order
  (`call_fn` and `enter_activation`, `crates/cpl-eval/src/interp/eval.rs`
  and `crates/cpl-eval/src/interp.rs`): the depth limit (`calls >=
  max_call_depth` refuses the call, raised against the caller at `site`),
  then the stack guard, then the time budget and cancellation, then the
  frame push with the borrowed positional parameters (for `input`), and
  only then the increment of the call count. A refused call returns
  before any frame of the callee exists. `cplrt_enter_act` does the same
  for predicates, test bodies, and range bounds without the depth check
  and count (semantics S72).
- The emitted code calls the inline `cplrt_fast_enter_fn`, which pushes the
  frame without a call when every check passes trivially (depth below the
  limit, the frame above the stack floor, no deadline, no cancellation or
  failed parallel sibling) and is `cplrt_enter_fn` otherwise; likewise
  `cplrt_fast_leave` (`cplrt_leave` on a fault) and
  `cplrt_fast_header_measure` (a direct recursion's small, smaller,
  nonnegative measure; `cplrt_header_measure` otherwise). The observable
  behavior is the out-of-line functions'.
- `cplrt_leave(cx, &fr, st)` pops the frame and, on a fault, appends
  `{def, site}` to the backtrace, which is the evaluator's `leaving`. A
  builtin pushes no frame; when its fault leaves it, the runtime sets the
  fault's span to the call's site if it has none and appends `{stub,
  site}` (`call_builtin`); with `CPLRT_NO_SITE` it sets no span and appends
  `{stub}`. A capability operation pushes no frame either
  (`call_operation`).
- The functions of emitted code never return a value through C's return
  value other than the status, and never unwind.

### 1.3 The slot discipline

Every emitted `cpl_f`, `cpl_p`, `cpl_o`, `cpl_t`, and `cpl_b` function
obeys five rules, each checkable by reading that function alone; an entry
trampoline `cpl_e<n>` is a single `return` statement and is exempt:

1. Every word variable the function owns, a *slot* (locals `l<k>_<name>`
   with `k` the `LocalId.0`, temporaries `t<k>`, clause temporaries
   `c<k>`, loop measures `m<k>`, the exit value `rv`), is declared at the
   top of the function and initialized to 0, which holds nothing.
2. A slot owns at most one reference, and only four macros write it:
   `CPLRT_SET(slot, owned)` (release the old content, store),
   `CPLRT_COPY(slot, borrowed)` (retain, then `CPLRT_SET`),
   `CPLRT_MOVE(dst, src)` (transfer `src` into `dst`, releasing `dst`'s old
   content, and zero `src`), and `CPLRT_GIVE(slot)` (zero a temporary whose
   reference a constructor took: `cplrt_struct_new_m` and
   `cplrt_enum_new_m` take the references of the temporaries among their
   fields instead of retaining them). Runtime operations that produce a value take
   `cplrt_w *out` and have `CPLRT_SET` semantics. Arrays of borrowed words
   built for a call (`params`, `args`, `visible`) are not slots and are
   never released.
3. There are at most three labels. `exit:` is reached by every normal
   exit (the tail, `return e`, a bare `return`, and a `?` that propagates
   an `Err`, each of which `CPLRT_MOVE`s into `rv` first); it runs the
   `ensures` regions with `result` bound to `rv` and then
   `CPLRT_MOVE(*out, rv)`. A function without `ensures` regions omits
   `exit:` when nothing jumps to it (`cpl_p23` and `cpl_o61_0` of Section
   1.4). `fail:` is reached by every fault and skips the `ensures` regions
   (semantics 5.2). Both lead to `done:`, which releases every slot
   (releasing 0 is a no-op) and returns through `cplrt_leave` (an
   operation function, which has no frame, returns its status).
4. No `return` appears before `done:` except the refusal return directly
   after `cplrt_enter_fn` or `cplrt_enter_act`, before any slot can hold
   anything.
5. No `setjmp`/`longjmp`, no recursion proportional to the depth of a
   value (deep work is iterative in the runtime), and no mutable global
   state.

Together the rules give "no leak and no double release" for the function.
Toolchain: the emitted-shape test checks them mechanically on the emitted
text of every corpus program, and every differential corpus also runs
under ASan, UBSan, and LSan (toolchain 12.6, T78).

### 1.3a The scalar tier

In the optimized profile every eligible function also has a speculative
C form, `cpl_q<n>`, on unboxed values: `Int` as `int64_t`, `Bool` as
`int`, `Text` as the runtime's text word, a record (a struct without type
parameters whose fields are `Int`, `Bool`, `Text`, or records) as a C
struct (a struct with an invariant included: a scalar form never builds
one, its literal being outside the subset, so each value it holds is a
copy of one the generic form checked), an enum without type parameters whose payloads
are `Int` and `Bool` as a C struct of its tag and every variant's
payload, `List<E>` as the
runtime's list words for `E` an `Int`, a `Bool`, a record, or a list of
`Int` or `Bool`, and `Option<E>` (locals and expressions only, never of a
list) as `cplrt_qopt` or a C struct with a flag. It has no frame and no
slot discipline, allocates no `Option`, makes a record a struct object
only to put it in a list (filled in place when its `Int` fields are
small), and reads list elements in place: an element list of a list of
lists that an operation only reads (`a.get(k).unwrap_or([]).get(j)`) is
borrowed from its list, with no reference taken. A value of any other
struct or enum (a tree's node, an option of one, a syntax tree) is a
*word*: the generic form's own word, borrowed, never retained or released.
A word is only a parameter (never a result), a local, a field read (an
`Int`, a `Bool`, or a word out of a struct word: an `Int` field holding a
bigint bails), a `match` scrutinee (its variant tested, its payloads
bound: each an `Int`, a `Bool`, or a word, an `Int` holding a bigint
bailing), the operand of `is_some`/`is_none`, or an argument of an
eligible function; the scalar form never builds, compares, returns, or
keeps one in a list or record. Every word is held by a parameter's value
(the parameter itself, or reached from it), which the caller keeps alive
for the call, so a borrowed word outlives every use. A read-only
traversal (a tree's size, an evaluator over a syntax tree, a walk down a
linked list) runs in the scalar tier this way. A function is eligible when it has a block body,
no type parameters and no `using`, every parameter, its result, and every
local has one of these types (an `Option` never at the boundary), its body
and contracts use only literals, locals, arithmetic, comparisons, the
logical operators, `if`, `match` on an `Int`, a `Bool`, a `Text`, an
enum (a variant with literal, binding, and `_` sub-patterns), or an
`Option` (literal, binding, `Some`, `None`, and `_` patterns), `while` without
clauses, `let`, assignment, `return`, `assert`, widening, record literals
and field reads, list literals, `++` of lists, `Some`/`None`, the list
builtins `len`, `is_empty`, `get`, `first`, `last`, `push`, `take`,
`drop`, `drop_last`, the option builtins `unwrap_or`, `is_some`,
`is_none`, the text operations `++`, `==`, `len`, `is_empty`, and `Int`'s
`to_text`, enum constructors and `==`, and calls of eligible functions;
and, when it has a
`decreases` clause, it is its own recursive group, so the activation its
measure is compared with is its caller or none.

The scalar form returns a result or *bails*: it bails wherever the generic
form could do anything but compute the same value (an `int64_t` overflow,
a division by zero, a contract, assertion, or measure that does not hold,
a negative count, a list element that is a bigint, an element value
outside the small range, an allocation that fails, the depth limit, the
stack floor, a deadline, a cancellation, a failed parallel sibling). The
generic form `cpl_f<n>` calls it first, with its `Int` arguments when they
are small and its records when every leaf is, boxes a result, and on a
bail runs its own body, which raises exactly what it always raised: an
eligible function is pure, so running it again computes the same thing.
An evaluation stops trying the scalar tier after `CPLRT_Q_MAX_BAILS`
bails. The depth limit is checked at every speculative call; the stack
floor every 16 levels; the time budget, a cancellation, and a failed
sibling at every call and loop iteration while the evaluation has any of
them. A poll that sees one of those sets `q_late`, and the next entry check
(the bailing function's generic re-entry) skips its time poll once: the
time ran out inside that body, past the entry the evaluator found in time,
so the fault is raised at the body's next poll, as the evaluator raises it.
Each list or text a speculative local or temporary holds is a reference
of its own, released on return and on a bail, and so is each text a
record variable holds (taken when a record is copied, moved out of a
temporary, given back when the variable is overwritten); a parameter's
is the caller's. A text grows in place on `t = t ++ a ++ b ...` (every
part evaluated first, so each sees the old `t`) where it is unique and has
room; a field of a local is read without copying the record; and
the default of `unwrap_or` is evaluated only when it is the value, when
it can neither fault nor allocate (a literal, a local, or records, enums,
and options of them). A list local that never leaves its function (it
is assigned only `[]` and its own `push`, and read only by `len`,
`is_empty`, `get`, `first`, and `last`) is a flat C array of its elements
(`cplrt_qbuf`: an `int64_t`, an `int`, or a record's C struct each),
grown in place within the heap budget and at most 64 MiB (past that the
form bails, so that the generic form's list grows at the pace of its own
steps), and freed on return and on a bail with the texts its records
hold.
Nothing a speculative form does is observable but its result.

### 1.4 Shape of generated code

The examples below are normative for their shape; the numeric ids of
DefIds, sites, clauses, and constants are illustrative. The design's
`divide_exactly` (two `requires`, one `ensures`):

```c
/* reader.pages.divide_exactly  [identity reader.pages.divide_exactly, DefId 41]
   fn divide_exactly(a: Int, b: Int) -> Int  requires b != 0  requires a % b == 0  ensures result * b == a */
static int cpl_f41(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    int st = CPLRT_OK, eq = 0;
    cplrt_w t0 = 0, t1 = 0, t2 = 0, rv = 0;            /* owned slots: 0 holds nothing */
    cplrt_frame fr;
    const cplrt_w params[2] = { a, b };                 /* borrowed: the `input` of a contract fault */
    if (cplrt_enter_fn(cx, &fr, CPL_D41, site, params, 2) != CPLRT_OK)
        return CPLRT_FAULT;                             /* depth, guard, time, or cancellation: no frame of ours */
    if (cx->check_contracts) {                          /* obl requires/1: b != 0 */
        if (!cplrt_int_ne_small(b, 0)) { cplrt_requires_failed(cx, CPL_CL17); goto fail; }
    }
    if (cx->check_contracts) {                          /* obl requires/2: a % b == 0 */
        if (cplrt_int_rem(cx, CPL_S88, a, b, &t0) != CPLRT_OK) { cplrt_claim(cx, CPL_CL18); goto fail; }
        if (!cplrt_int_eq_small(t0, 0)) { cplrt_requires_failed(cx, CPL_CL18); goto fail; }
    }
    if (cplrt_int_div(cx, CPL_S91, a, b, &t1) != CPLRT_OK)  /* a / b: division_by_zero at its span */
        goto fail;
    CPLRT_MOVE(rv, t1);
exit:
    if (cx->check_contracts) {                          /* obl ensures/1: result * b == a */
        if (cplrt_int_mul(cx, CPL_S93, rv, b, &t2) != CPLRT_OK) { cplrt_claim(cx, CPL_CL19); goto fail; }
        if (cplrt_equal(cx, t2, a, &eq) != CPLRT_OK) { cplrt_claim(cx, CPL_CL19); goto fail; }
        if (!eq) { cplrt_ensures_failed(cx, CPL_CL19, rv); goto fail; }
    }
    CPLRT_MOVE(*out, rv);                               /* ownership moves to the caller */
    goto done;
fail:
    st = CPLRT_FAULT;
done:
    cplrt_release(t0); cplrt_release(t1); cplrt_release(t2); cplrt_release(rv);
    return cplrt_leave(cx, &fr, st);                    /* pop; on a fault append {CPL_D41, site} */
}
```

The refinement `Positive`, and `positive_size` with a call, a `?`, and a
`check<R>` over `Positive`'s predicate:

```c
/* reader.pages.Positive = Int where self > 0  [DefId 23] */
static int cpl_p23(cplrt_cx *cx, uint32_t site, cplrt_w self, int *holds)
{
    int st = CPLRT_OK;
    cplrt_frame fr;
    if (cplrt_enter_act(cx, &fr, CPL_D23, site) != CPLRT_OK)   /* an activation, not a call (S72) */
        return CPLRT_FAULT;
    *holds = cplrt_int_gt_small(self, 0);               /* a fault here: cplrt_claim(cx, CPL_CL22); goto fail */
    goto done;
fail:
    st = CPLRT_FAULT;
done:
    return cplrt_leave(cx, &fr, st);
}
/* reader.pages.positive_size(text: Text) -> Result<Positive, ConstraintError>
   { let n = parse_size(text)?  check<Positive>(n) }  [DefId 44; parse_size is DefId 43] */
static int cpl_f44(cplrt_cx *cx, uint32_t site, cplrt_w text, cplrt_w *out)
{
    int st = CPLRT_OK, holds = 0;
    cplrt_w l1_n = 0, t0 = 0, rv = 0;
    cplrt_frame fr;
    const cplrt_w params[1] = { text };
    if (cplrt_enter_fn(cx, &fr, CPL_D44, site, params, 1) != CPLRT_OK)
        return CPLRT_FAULT;
    if (cpl_f43(cx, CPL_S120, text, &t0) != CPLRT_OK)   /* parse_size(text) */
        goto fail;
    if (!cplrt_is_ok(t0)) { CPLRT_MOVE(rv, t0); goto exit; }        /* `?`: the Err is the exit value */
    CPLRT_COPY(l1_n, cplrt_payload(t0, 0));              /* let n = the Ok payload */
    if (cpl_p23(cx, CPL_S124, l1_n, &holds) != CPLRT_OK) /* check<Positive>(n): the chain, root first */
        goto fail;
    if (!holds) {
        if (cplrt_check_failure(cx, CPL_R23, l1_n, &rv) != CPLRT_OK) goto fail;
        goto exit;
    }
    if (cplrt_result_ok(cx, l1_n, &rv) != CPLRT_OK)      /* Ok(n) */
        goto fail;
exit:
    CPLRT_MOVE(*out, rv);                               /* no ensures clauses */
    goto done;
fail:
    st = CPLRT_FAULT;
done:
    cplrt_release(l1_n); cplrt_release(t0); cplrt_release(rv);
    return cplrt_leave(cx, &fr, st);
}
```

A loop with an invariant and a measure (the function is a corpus example,
not design text):

```c
/* corpus.loops.count_down  [DefId 50]
   fn count_down(n: Int) -> Int  requires n >= 0
   { var i = n  while i > 0 invariant i >= 0 decreases i { i = i - 1 }  i } */
static int cpl_f50(cplrt_cx *cx, uint32_t site, cplrt_w n, cplrt_w *out)
{
    int st = CPLRT_OK;
    uint64_t iteration = 0;
    cplrt_w l1_i = 0, t0 = 0, m0 = 0, rv = 0;            /* m0: the previous measure of the loop */
    cplrt_frame fr;
    const cplrt_w params[1] = { n };
    if (cplrt_enter_fn(cx, &fr, CPL_D50, site, params, 1) != CPLRT_OK)
        return CPLRT_FAULT;
    if (cx->check_contracts) {                          /* obl requires/1: n >= 0 */
        if (!cplrt_int_ge_small(n, 0)) { cplrt_requires_failed(cx, CPL_CL60); goto fail; }
    }
    CPLRT_COPY(l1_i, n);                                 /* var i = n */
    if (cx->check_contracts) {                          /* obl loop/1/invariant/1/init: i >= 0 */
        if (!cplrt_int_ge_small(l1_i, 0)) {
            const cplrt_w visible[2] = { l1_i, n };      /* innermost first: the binding site CPL_A5 */
            cplrt_invariant_failed(cx, CPL_CL61, visible, 2, iteration);
            goto fail;
        }
    }
    for (;;) {
        if (cplrt_tick(cx, CPL_S70) != CPLRT_OK)        /* the time budget and cancellation, before the condition */
            goto fail;
        if (!cplrt_int_gt_small(l1_i, 0))                /* while i > 0 */
            break;
        if (cx->check_contracts) {                      /* obl loop/1/decreases: i */
            const cplrt_w visible[2] = { l1_i, n };
            if (cplrt_loop_measure(cx, CPL_CL62, visible, 2, iteration, l1_i, &m0) != CPLRT_OK) goto fail;
        }
        if (cplrt_int_sub(cx, CPL_S71, l1_i, cplrt_small(1), &t0) != CPLRT_OK)   /* i - 1 */
            goto fail;
        CPLRT_MOVE(l1_i, t0);                            /* i = i - 1 */
        iteration += 1;
        if (cx->check_contracts) {                      /* obl loop/1/invariant/1/preserve: i >= 0 */
            if (!cplrt_int_ge_small(l1_i, 0)) {
                const cplrt_w visible[2] = { l1_i, n };
                cplrt_invariant_failed(cx, CPL_CL61, visible, 2, iteration);
                goto fail;
            }
        }
    }
    CPLRT_COPY(rv, l1_i);                                /* the tail i */
exit:
    CPLRT_MOVE(*out, rv);
    goto done;
fail:
    st = CPLRT_FAULT;
done:
    cplrt_release(l1_i); cplrt_release(t0); cplrt_release(m0); cplrt_release(rv);
    return cplrt_leave(cx, &fr, st);
}
```

A `match` over `Result<Page, StoreError>` (`label` of the corpus):

```c
/* reader.pages.label(result: Result<Page, StoreError>) -> Text  [DefId 46]
   match result { Ok(page) => page.text  Err(Missing(_)) => "Missing page"
                  Err(Unavailable) => "Storage unavailable" } */
static int cpl_f46(cplrt_cx *cx, uint32_t site, cplrt_w result, cplrt_w *out)
{
    int st = CPLRT_OK;
    cplrt_w l1_page = 0, rv = 0;
    cplrt_frame fr;
    const cplrt_w params[1] = { result };
    if (cplrt_enter_fn(cx, &fr, CPL_D46, site, params, 1) != CPLRT_OK)
        return CPLRT_FAULT;
    if (cplrt_variant(result) == CPL_RESULT_OK) {                        /* Ok(page) */
        CPLRT_COPY(l1_page, cplrt_payload(result, 0));
        CPLRT_COPY(rv, cplrt_field(l1_page, 1));                         /* page.text */
    } else if (cplrt_variant(result) == CPL_RESULT_ERR
               && cplrt_variant(cplrt_payload(result, 0)) == 0u) {      /* Err(Missing(_)) */
        CPLRT_COPY(rv, CPL_K2);                                         /* "Missing page" */
    } else if (cplrt_variant(result) == CPL_RESULT_ERR
               && cplrt_variant(cplrt_payload(result, 0)) == 1u) {      /* Err(Unavailable) */
        CPLRT_COPY(rv, CPL_K3);                                         /* "Storage unavailable" */
    } else {
        cplrt_no_match(cx, CPL_S99, result);         /* internal_error no_matching_arm: "no arm of the match matches <value text>" */
        goto fail;
    }
exit:
    CPLRT_MOVE(*out, rv);
    goto done;
fail:
    st = CPLRT_FAULT;
done:
    cplrt_release(l1_page); cplrt_release(rv);
    return cplrt_leave(cx, &fr, st);
}
```

A capability operation, a call of it, and the entry trampoline of an
export:

```c
/* reader.pages.PageRead.get(id: PageId) -> Result<Page, StoreError>  [capability DefId 61, operation 0] */
static int cpl_o61_0(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, cplrt_w id, cplrt_w *out)
{
    int st = CPLRT_OK;
    cplrt_w rv = 0;
    const cplrt_w args[1] = { id };                     /* borrowed: the operation's `input` */
    /* the requires regions would run here, with the parameters bound and no frame; then the effect point db.get */
    if (cplrt_cap_invoke(cx, site, h, 0, args, 1, &rv) != CPLRT_OK)   /* host call, 6.5, then validation, 6.4 */
        goto fail;
    /* the ensures regions would run here with result = rv (attributed_to binding) */
    CPLRT_MOVE(*out, rv);
    goto done;
fail:
    st = CPLRT_FAULT;
done:
    cplrt_release(rv);
    return st;                                          /* an operation pushes no frame */
}
/* reader.pages.load_page(id: PageId, using db: PageRead) -> Result<Page, StoreError>  [DefId 62] */
static int cpl_f62(cplrt_cx *cx, uint32_t site, cplrt_w id, const cplrt_capref *db, cplrt_w *out)
{
    int st = CPLRT_OK;
    cplrt_w t0 = 0, rv = 0;
    cplrt_frame fr;
    const cplrt_w params[1] = { id };                   /* using parameters are not part of `input` */
    if (cplrt_enter_fn(cx, &fr, CPL_D62, site, params, 1) != CPLRT_OK)
        return CPLRT_FAULT;
    if (cpl_o61_0(cx, CPL_S130, db, id, &t0) != CPLRT_OK)   /* db.get(id) */
        goto fail;
    CPLRT_MOVE(rv, t0);
exit:
    CPLRT_MOVE(*out, rv);
    goto done;
fail:
    st = CPLRT_FAULT;
done:
    cplrt_release(t0); cplrt_release(rv);
    return cplrt_leave(cx, &fr, st);
}
/* the entry trampoline of the export reader_load_page (5.5 step 4) */
static int cpl_e62(cplrt_cx *cx, const cplrt_w *args, const cplrt_capref *const *using, cplrt_w *out)
{
    return cpl_f62(cx, CPLRT_NO_SITE, args[0], using[0], out);
}
```

The other forms lower exactly as `crates/cpl-eval/src/interp/eval.rs`
evaluates them:

| HIR | Emitted C |
| - | - |
| `Lit` | an `Int` in the small range and a `Bool` are immediates (`cplrt_small(n)`, `CPLRT_TRUE`); every other literal is an immortal constant of the program unit (`CPL_K<n>`), borrowed |
| `Local` | the slot, borrowed; `CPLRT_COPY` when stored |
| `Ctor`, `Wrap`, `List`, `StructLit` | the payloads, the elements, or the spread base and then the fields in source order, each into a temporary (semantics 6.2); then `cplrt_enum_new`, `cplrt_wrap`, `cplrt_list_new`, or `cplrt_struct_new`, which retain the borrowed words they are given; a field the literal does not list is the base's `cplrt_field(base, i)`, borrowed; a payload-less variant (`None`, `Unavailable`, `Unit`) is the program's immortal static, which `cplrt_enum_new` returns for an empty payload; the optimized and debug profiles name it directly (`cplrt_word_of(&CPLPKG(s<def>_<variant>))`, defined in the tables unit and declared `extern` in the program unit), so building one is a constant |
| `Field` | `cplrt_field(v, i)`, borrowed; `CPLRT_COPY` when kept |
| `Call` of a `fn` with a block body | `cpl_f<n>(cx, CPL_S<site>, args..., using..., &t)` |
| `Call` of a `@builtin` stub | `cplrt_bi(<id>)(cx, CPL_S<site>, CPL_D<stub>, args..., &t)` (Section 2.7) |
| `xs.get(i).unwrap_or(d)`, `xs.first().unwrap_or(d)`, `xs.last().unwrap_or(d)` (optimized profile) | the element read in place, or `d`: `cplrt_items(xs)[k]` when `0 <= k < cplrt_len(xs)`, retained; no `Option` is built (reading an element cannot fault, so `d` evaluated after the read is not observable) |
| `let x = xs.get(i)` (or `first()`, `last()`) where `x` is only matched on (`Some(..)`, `None`, `_` arms), unwrapped with `unwrap_or`, or tested with `is_some`/`is_none` (optimized profile) | no `Option`: `x`'s slot holds the element, retained, or 0 for `None`, and those uses read it; where a binding site reports `x` (an assertion, a hole, a loop clause), the `Option` is built for the report |
| `xs.len`, `xs.is_empty` (optimized profile) | the list header's length, inline |
| `xs = xs.push(x)`, `xs = xs.drop(k)` on a `var`'s own list | `cplrt_list_push_fast` (inline while the list is a unique node with room), `cplrt_list_drop_owned` (a unique view moves along in place); a unique node or text that outgrows its block grows it (`realloc` with the default allocator, so a large one moves without a copy) |
| `Call` of a `@builtin` stub whose id is not callable (`cpl_rtspec::builtins::spec(id).callable` is false), which only a hand-built HIR state contains | `cplrt_bi_uncallable(cx, CPL_S<site>, CPL_D<stub>, "<id>", args, n)` (Appendix B.1): for `gen_range` the precondition `lo <= hi` of its stub first, then `internal_error` kind `builtin_type_called` for a type id or `generator_not_a_value` for a generator id, with the detail of `crates/cpl-eval/src/builtins/gen.rs`, spanning the call, and the frame `{stub, site}`, as `call_builtin` reports it |
| `Call` of a definition that is no function, or with an argument or handle count other than its declaration's (unreachable in a checked program) | `cplrt_internal(cx, CPL_S<site>, "not_a_function", ...)` or `cplrt_internal(cx, CPL_S<site>, "call_mismatch", ...)` with the detail that the evaluator's `call_fn` formats, computed at emit time (Section 3.2) |
| `CapCall` | `cpl_o<cap>_<op>(cx, CPL_S<site>, handle, args..., &t)` |
| `Check` | the `check_order(R)` chain unrolled as `cpl_p<r>` calls at the site of the `check`, root first; the first false gives `cplrt_check_failure(cx, CPL_R<r>, v, &t)`, else `cplrt_result_ok` |
| `Propagate` | `cplrt_is_ok(v)`, then the payload, borrowed; else `CPLRT_MOVE(rv, v); goto exit` |
| `Unary`, `Binary` | `cplrt_int_add`, `_sub`, `_mul`, `_neg`, and `/` and `%` through the one `cplrt_int_divrem` (T9); the orderings through `cplrt_int_cmp` or, against a small constant, the inline `cplrt_int_lt_small` family; `==` and `!=` through `cplrt_equal`, except on `Int`, `Bool`, and `Text` (refinements stripped), which compare inline (`cplrt_int_eq`, word equality, `cplrt_text_eq`: none can fail); `++` through `cplrt_concat`; in the optimized profile the arithmetic is the inline `cplrt_add_w` .. `cplrt_rem_w` (small operands in place, else `cplrt_arith_slow`, which returns the word or 0 with the fault raised, so a temporary never has its address taken), and a `var` read directly as an operand is borrowed, not copied, when no later operand of the same expression holds an `if` or a `match` (the only expressions that can reassign it); `not`, `and`, `or` as C `!`, `if`, short-circuiting exactly as semantics 6.2 says; a `Bool` subexpression that feeds control flow is a C `int` |
| `Forall` | the collection into a temporary; a loop over its items (`cplrt_len`, `cplrt_item`, borrowed) that binds the binder, evaluates the predicate, and stops at the first false; no time check (the evaluator has none in `forall`) |
| `Hole` | `cplrt_hole(cx, CPL_H<n>, visible, n)` with the locals of the hole's binding site, innermost first (Sections 3.2 and 3.5), a `using` parameter's entry being its capability reference as a word (Appendix B.1) |
| `If`, `Match` | C `if`; the arms tried in source order with inline pattern tests (`cplrt_variant`, `cplrt_payload`, `cplrt_inner`, `cplrt_int_eq_small` or `cplrt_int_cmp` against a constant, `cplrt_text_eq`, the `Bool` words) binding into slots; no matching arm is `cplrt_no_match(cx, CPL_S<match>, <the scrutinee's slot>)`: `internal_error` kind `no_matching_arm` with the detail `no arm of the match matches <value text of the scrutinee>`, spanning the `match` expression, as the evaluator's `eval_match` raises it |
| `Widen` | the operand itself |
| `Refine` | the operand, plus, inside `if (cx->check_contracts)`, the chain re-checked by `cpl_p<r>` calls; a false predicate is `cplrt_refine_unsound` (`internal_error` kind `refinement_proof_unsound`, T39) |
| `Gen`, `Error` | `Gen` is `cplrt_internal(cx, site, "generator_not_a_value", ...)` (unreachable in a checked program); `Error` is never emitted (no program with errors is emitted) |
| `Stmt::Let`, `Assign` | the initializer, then its value into the local's slot: `CPLRT_MOVE` from the temporary that owns it, or `CPLRT_COPY` of a borrowed word (a parameter, a field, a payload) |
| `Stmt::Discard`, `Stmt::Expr` | the expression into a temporary, released at `done:` or reused |
| `Stmt::Return` | `CPLRT_MOVE(rv, v); goto exit` (a bare `return`: the immortal `Unit`) |
| `Stmt::Assert` | the expression; a false result is `cplrt_assert_failed(cx, CPL_A<n>, visible, n)` |
| `Stmt::While` | the invariants at iteration 0, each in its own contract region; then `for (;;) { cplrt_tick(cx, CPL_S<cond>); the condition; the measure (only after the condition held, cplrt_loop_measure); the body; the iteration count; the invariants }`: semantics 5.3 and the evaluator's `exec_while`, step for step |
| header `decreases` | after the `requires` regions, `cplrt_header_measure(cx, &fr, CPL_CL<n>, m)`, which compares the measure with the one recorded by the nearest enclosing activation of the same function, when that activation recorded one, and records it on this frame (semantics S35, the evaluator's `enter_measure`) |

The names emitted code uses, besides the functions of Section 1.1, are
`#define` constants, which each emitted unit defines for the names it uses
(the API unit, for example, those of its exports, type accessors,
capability constructors, and grants):

| Name | Value | Names |
| - | - | - |
| `CPL_D<n>` | `n`, the `DefId.0` | definition `n`, std included: `cplrt_program.defs[n]` |
| `CPL_S<n>` | the site index | every span a fault can report: call expressions, operator expressions, clause spans, parameter spans, hole spans, the clause spans of builtin stubs (`cplrt_program.sites`) |
| `CPL_CL<n>` | the clause index | every clause: a function's `requires`, `ensures`, and `decreases`, a loop's `invariant` and `decreases`, a refinement's `where` predicate, an operation's `requires` and `ensures`, a property's `for` range (`cplrt_program.clauses`) |
| `CPL_H<n>` | `n`, the `HoleId.0` | hole `n` (`cplrt_program.holes`) |
| `CPL_A<n>` | the binding-site index | the locals visible at an `assert`, at a `while` loop, or at a hole (`cplrt_program.bsites`) |
| `CPL_K<n>` | an expression: the word of constant `n` | an immortal literal of the program unit (`Text`, `Int` outside the small range) |
| `CPL_R<n>` | the refinement-table index of refinement DefId `n` | its chain, predicate text, `ConstraintError.type_name` (`cplrt_program.refinements`) |
| `CPL_X<n>` | `n`, a package type id (`cpl_type`) | type `n` of the type table: what `<prefix>_type_*` returns, and what boundary checks and result validation walk (`cplrt_program.types`) |

The program unit defines its constants as `static` objects with the count
`CPLRT_RC_IMMORTAL` (Section 2.4), for example `static struct { cplrt_obj
hdr; size_t bytes, scalars; char utf8[13]; } cpl_k2 = { { CPLRT_TEXT, 0,
0, 0, { { CPLRT_RC_IMMORTAL, 0 } } }, 12, 12, "Missing page" };` with
`#define CPL_K2 cplrt_word_of(&cpl_k2.hdr)`, and its only external symbol
is the entry table `CPLPKG(code)` (Appendix B).

Every contract region of emitted code is the body of one `if
(cx->check_contracts)` and carries, in a comment, the obligation id of
toolchain 8.1 (T62) that it checks, without the function's identity:
`ensures/<i>`, `decreases`, `loop/<n>/invariant/<i>/init`,
`loop/<n>/invariant/<i>/preserve`, `loop/<n>/decreases`, `refine/<n>`, and,
for the function's own `requires` clauses, whose obligations are their
callers' `call/<n>/requires/<i>`, `requires/<i>` (1-based, source order).
Milestone 5 drops proved regions by these tags (Section 3.3).

**Coverage.** Under `EmitConfig::coverage` (the drivers of the coverage
gate, toolchain 12.6) the lowering of every HIR node (an expression, a
statement, a pattern test) begins with `CPLRT_COV(k)` (Appendix B.1),
where `k` indexes the coverage tables of `<prefix>_tables.c`:
`CPLPKG(cov_kinds)[k]` is the node's HIR variant as text
(`ExprKind::Call`, `Stmt::While`, `PatternKind::Variant`) and
`CPLPKG(cov)[k]` counts its executions. Compiled with
`-DCPLRT_COVERAGE=1`, the runtime itself counts the calls of each builtin
by id and the claims by the kind of their claim site (Section 3.8,
`coverage`). Without that configuration the emitted text holds no
`CPLRT_COV`.

### 1.5 C standard and compiler flags

**Language.** ISO C11 (`<stdatomic.h>`, `_Static_assert`, `_Alignas`,
compound literals). Compiler builtins appear only behind portable
fallbacks (`__builtin_add_overflow`, `__builtin_mul_overflow`,
`__builtin_expect`, `__builtin_frame_address`). Emitted code uses no VLAs,
no statement expressions, and no `#pragma`. The runtime is C11 plus POSIX
threads and clocks. Under `-std=c11` glibc declares `clock_gettime`,
`pthread_attr_getstack`, and `pthread_getattr_np` only when a feature-test
macro precedes the first `#include` of the translation unit, so the
prologue of every runtime unit opens with `#define _GNU_SOURCE 1`
(Section 2.2); only `cpl_rt_platform_linux.c` uses what it enables, no
runtime file defines a feature-test macro itself, and no emitted unit
defines one. Right shift of a negative integer is implementation-defined,
so the untagging of small integers uses a portable formula
(`cplrt_small_value`, Appendix B.1).

| Set | Flags |
| - | - |
| base (the runtime and emitted code) | `-std=c11 -fPIC -fvisibility=hidden -fno-strict-aliasing -fno-common -ffunction-sections -fdata-sections -ffile-prefix-map=<out>=. -I<out>/include -Wall -Wextra -Wno-unused-parameter -Wno-unused-label -Wno-unused-variable -Wno-unused-function`; gcc adds `-frandom-seed=<unit digest>` (the blake3 of the unit's text) |
| profile `optimized` (the default of `cpl build`) | `-O2 -DNDEBUG` |
| profile `debug` (the default of drivers) | `-O0 -g3 -DCPLRT_DEBUG=1` (invariant checks on every handle and object header access) |
| `--sanitize address,undefined` | `-O1 -g -fno-omit-frame-pointer -fsanitize=address,undefined -fno-sanitize-recover=all`, at compile and at link time |
| `--sanitize thread` | `-O1 -g -fsanitize=thread`, at compile and at link time; run under `setarch $(uname -m) -R` on Linux, where the WSL2 kernel's address layout otherwise aborts it (toolchain 12, T78) |
| adapter and vendored sources | the base set without its four `-Wno-*` flags, plus `[interop.c] cflags` and the binding's `include_dirs` (as `-I`, after `-I<out>/include`, in their order), `defines`, and `cflags` (toolchain 11.1) |
| a declarative binding's generated wrapper (`src/<prefix>_bind_<binding>.c`) | the adapter set with the binding's `include_dirs` passed as `-isystem` instead of `-I`, plus `-Werror=conversion -Werror=sign-conversion -Werror=incompatible-pointer-types` (Section 6.3): a conversion at a call of the library is an error, while the library's own headers, which the binding does not control, are not checked |
| toolchain tests | `-Werror` added: a warning in emitted code or in the runtime is a toolchain bug; user builds never use `-Werror` |
| shared link | `-shared -Wl,-soname,lib<prefix>.so.<abi> -Wl,--version-script=<prefix>.map -Wl,-z,defs -Wl,-z,now -Wl,--gc-sections -Wl,-Bsymbolic -Wl,--trace`, then the bindings' `-L` and `-l` options, `[interop.c] ldflags`, `-lpthread`; with clang and a sanitizer `-Wl,-z,defs` is left out, because clang leaves the sanitizer runtime's symbols of a shared object to the executable |

**The flags digest.** `targets.c.flags_digest` (toolchain 11.2) is the
blake3 of the effective flags of the locked profile and sanitizers, one
flag per line, each ended by LF: the base set, the profile's set, and the
sanitizers' sets in the order of the table, with `<out>` written as `.`
and the per-unit `-frandom-seed`, `-MD`, and `-MF` left out; then the
shared link's flags; then `[interop.c] cflags` and `ldflags`; then, per
backend-`c` binding in name order, its `include_dirs`, `defines`,
`cflags`, `libraries`, and `library_dirs` as the manifest writes them. An
output root or a checkout elsewhere therefore gives the same digest.

Decision: [D-289] C3. The toolchain fixes the flags of the runtime and of
emitted code by profile and sanitizer set, as the table above says; the
manifest's `cflags` and a binding's options apply only to adapter and
vendored sources, `ldflags` only to links, and the effective flag set is
recorded in the lock (`targets.c.flags_digest`, toolchain 11.2). Rationale:
emitted code relies on invariants a user flag could break (no
strict-aliasing assumptions between the object layouts, hidden visibility
of every internal symbol, no common symbols), and the equivalence evidence
of Section 3 is evidence about these flags. Rejected alternatives: user
`cflags` applied to emitted code (a `-fstrict-aliasing` or
`-fvisibility=default` would silently change what the evidence covers);
reading `CC`, `CFLAGS`, or `LDFLAGS` from the environment (hidden context,
which toolchain T35 rejects; toolchain T72 selects the compiler
explicitly).

### 1.6 How a program is compiled and linked

`cpl build --target c` runs these steps through
`cpl_emit_c::package::build`, and the driver build of the native engine
through `cpl_emit_c::package::driver` (Section 3.8), each running the
compiler with `std::process::Command` only; their order, and the target
check and the lock steps around them, are toolchain 7.2's (T72):

1. **Check** the project (development mode, or the release rules under
   `--release`); any error exits 1 (toolchain T58).
2. **Plan** the C surface: the interop plan of `cpl_sema::interop`
   (exports, the types the header declares, capabilities, bindings),
   computed by the check (toolchain 1.6, T74).
3. **Probe the compiler** that toolchain T72 selects, in the scrubbed
   environment it defines: `-dumpfullversion`, `-dumpmachine`, the
   predefined macros, and the blake3 of the resolved driver binary; its
   target is then checked against the platform matrix of Section 1.7.
4. **Emit** the units of the table below into the output root of toolchain
   7.2: the runtime amalgamation and its headers, written out of the `cpl`
   binary behind their prologues (Section 2.2), with the main that the
   build needs (`cpl_rt_exe.c` for the executable artifact,
   `cpl_rt_driver.c` in a driver), the generated runtime tables
   `cpl_rt_gen.h` (Section 2.9), the program, tables, and API units, the
   declarative-binding wrappers (Section 6.3), the version script, and the
   headers.
5. **Compile** each unit, in parallel, with `-MD -MF <obj>.d`. An object
   is cached under `<cache>/obj/` (`<cache>` the per-user cache of
   `cpl build`, toolchain 7.2, never inside the project) by the blake3 of the
   unit's text, the compiler digest, and the unit's flags, together with
   its depfile and the blake3 of every file the depfile lists, and it is
   reused only while each of those files still has its recorded digest:
   a changed header (`cpl_rt.h`, `cpl.h`, a generated header, a vendored
   or a system header) recompiles every unit that read it.
6. **Link** the artifacts of `[interop.c] artifacts` (the second table
   below) with `-Wl,--trace`.
7. **Record** `build.json` (toolchain 7.2) with every input and output
   digest, the headers each unit read (from the depfiles), and the files
   the linker opened (from the trace), which the lock pins (Section 6.6).

Every cache entry (an object with its depfile and digests, a driver) is
written under a temporary name in its own directory and renamed into
place, and a build reads only entries that were renamed into place, so
builds of one key that run in parallel (the tests of one test binary,
toolchain 12.8) may both compile, and the later rename wins harmlessly.

The *program digest* (`cpl_library_info.program_digest`, `build.json`,
the banners of Sections 3.8 and 5.3) is the blake3 of `path LF content
LF` for each emitted program unit in path order (`src/<prefix>_api.c`,
every `src/<prefix>_bind_<binding>.c`, `src/<prefix>_prog.c`,
`src/<prefix>_tables.c`), with the program digest that
`<prefix>_tables.c` itself records written as 64 `0` characters.

| Unit (relative to the output root) | Contents |
| - | - |
| `src/cpl_rt.c`; `src/cpl_rt.h`, `src/cpl_rt_internal.h`, `src/cpl_rt_platform.h`, `src/cpl_rt_gen.h` | the runtime amalgamation behind its prologue (Section 2.2); the internal contract (Appendix B.1), the runtime's own headers (Section 2.1), and the generated tables (Section 2.9, Appendix B.2) |
| `src/cpl_rt_exe.c` | the native executable's `main` (with the `executable` artifact, Section 8) |
| `src/<prefix>_prog.c` | every emitted function (all `static`), the program's constants, and the entry table `CPLPKG(code)` |
| `src/<prefix>_tables.c` | `CPLPKG(program)`: definitions and names, sites with their locations, clauses, binding sites, holes, refinements, types, stub facts, the immortal statics of payload-less variants, the export and grant tables, the library information (Section 3.2) |
| `src/<prefix>_api.c` | the exports, `<prefix>_runtime_open`, `<prefix>_library_info`, the type accessors, the capability constructors, the grants (Section 5) |
| `src/<prefix>_bind_<binding>.c` | the wrapper generated from each declarative binding (Section 6.3) |
| `src/<prefix>.map` | the ELF version script |
| `include/cpl.h`, `include/cpl_adapter.h` | Appendix A, identical in every package |
| `include/<prefix>.h` | the package header (Section 5.3) |
| `include/<prefix>_adapter_<binding>.h` | the contract of each reviewed-wrapper binding (Section 6.2) |

Every unit that includes `cpl_rt.h` (the amalgamation, the two mains, and
the program, tables, and API units) begins with the namespace prologue of
Appendix B.1 for the package's prefix, before its first `#include`; the
runtime's units put `#define _GNU_SOURCE 1` before that (Section 2.2).

| Artifact | Contents |
| - | - |
| `lib/lib<prefix>.so.<abi>` and the link `lib/lib<prefix>.so` | the runtime, the program, the API unit, the adapters and vendored sources; only the names the version script lists are dynamic symbols, under the version node `<PREFIX>_<abi>` (`<PREFIX>` is the prefix in upper case) |
| `lib/lib<prefix>.a` | one object, `<prefix>.o`: the objects of the shared library joined by a relocatable link (the compiler with `-r -nostdlib`) and passed through `objcopy --keep-global-symbols` with the names the version script lists, so that, as in the shared library, only those names stay global and every other symbol, those of the adapters and the vendored sources included, is local; then `ar rcsD`. The `objcopy` is the one the compiler names (`-print-prog-name=objcopy`, toolchain T72) |
| `bin/<prefix>` (`executable`) | the objects of the shared library plus `cpl_rt_exe.c`: a native executable (Section 8) |
| `<cache>/driver/<key>/<prefix>-driver` (`<cache>` the per-user cache of `cpl build`, toolchain 7.2) | the program compiled with `-DCPL_WITH_TESTS=1` (test bodies, range bounds, the entry trampoline of every function and every callable builtin stub, every backend-`c` adapter of the project) plus `cpl_rt_driver.c`, in the debug profile unless a sanitizer is selected: toolchain-internal (Section 3.8). `<key>` is the blake3 of the program digest, the runtime digest, the compiler digest, the effective flags of every compile and of the link (profile, sanitizers, coverage, and the manifest's options), and the digests of the backend-`c` bindings' adapter files and vendored sources (`c_input_digests`, toolchain 11.2); a cached driver is reused only while every object it links would be reused, so a changed header rebuilds it too. Its `<prefix>` is the effective prefix when that is in the prefix domain of toolchain 11.1, and `driver` otherwise (a project without the C target may be named `cpl`): a driver exports nothing, so the name reaches no host |

The version script lists the exact exported names, never a pattern:
`<prefix>_runtime_open`, `<prefix>_library_info`, every export symbol,
every `<prefix>_type_*` accessor, every `<prefix>_<capability>_new`, and
every `<prefix>_bind_<binding>`. The API unit defines each of them with
`CPLRT_EXPORT` (default visibility, Appendix B), because the base flags
make every other symbol hidden and a version script cannot export a
hidden symbol. Everything else is local. A host may also vendor a package
by compiling every `src/*.c` but `cpl_rt_exe.c`, with the adapters and
vendored sources that `build.json` lists, using a supported C11 compiler
with `-fno-strict-aliasing` (the one flag the units depend on),
`-Iinclude`, and `-pthread`; every unit carries its own prologue, so no
`-D` is needed. A vendored package keeps the names of its adapters and
vendored sources global, so two vendored packages that vendor one library
fail to link with a duplicate symbol; the archive of the table above has
no such limitation.

**Namespacing.** Every external-linkage symbol of the runtime is spelled
`CPLRT(name)`, which expands to `cplrt_<prefix>__name`, and every
external-linkage symbol of emitted code `CPLPKG(name)`, which expands to
`cplpkg_<prefix>__name` (Appendix B.1); emitted code writes the runtime's
functions by their plain names (`cplrt_enter_fn`), which `cpl_rt.h`
defines as macros over `CPLRT()`. The prefix contains no `__`, does not
end with `_`, and never starts with `cpl` (toolchain 11.1), so the first
`__` after `cplrt_` ends it, two prefixes never give one internal name,
and these names never meet a package symbol. The prologue writes the
prefix inside the tokens `cplrt_<prefix>__` and `cplpkg_<prefix>__`, so a
macro of the same name (`linux` and `unix` are predefined in GNU modes)
cannot replace it. Two packages linked statically into one host therefore
keep two runtimes, also when one prefix is another followed by `_` and
more (`p` and `p_op`). Their exported names can still coincide in that
case (package `p`'s export `op_f` and package `p_op`'s export `f` are both
`p_op_f`), so a host links no two packages one of whose prefixes begins
with the other followed by `_`. Two copies of one package (one prefix) are not
detected by a static link: the linker extracts the archive member of the
first copy, which resolves every reference, and never the second's, so
the host links exactly one copy; linked with `-Wl,--whole-archive`, or as
objects, the copies fail with a multiple-definition error.

Decision: [D-290] C4. A package is built by the steps above into the
units and artifacts of the two tables; its dynamic symbols are exactly
those the explicit version script lists, under the version node
`<PREFIX>_<abi>`, defined with default visibility while everything else is
compiled hidden; the object of its static archive keeps exactly those
names global; and every other external symbol of the runtime and of
emitted code is namespaced by the prefix through `CPLRT()` and `CPLPKG()`.
Rationale: two plain static archives that carry the same runtime silently
collapse to one copy at link time, so a host linking two packages would
get one runtime serving both; namespacing removes the collapse for every
artifact kind and for a vendored package, the relocatable link of the
archive localizes the symbols of adapters and vendored sources, which
namespacing cannot reach (two packages that vendor one library otherwise
share whichever copy the linker extracts first), and the explicit list
keeps the dynamic surface reviewable. Rejected alternatives: global
`cpl_*` runtime symbols exported by every package (two packages interpose
on each other, and with static archives one runtime serves both); a glob
in the version script (it would export whatever a future emitter names
alike); a shared system `libcpl.so` (every package would depend on a
separately installed runtime version); hiding archive symbols with
`--exclude-libs` instead of namespacing (it acts on shared links only, so
it neither covers a vendored package nor a host's own static link of the
archive); a plain archive of the objects (it leaves the names of adapters
and vendored sources global).

### 1.7 Target ABI and platform matrix for edition 0.1

The ABI is the platform C calling convention. The public API uses only
fixed-width integers, `size_t`, pointers to incomplete or one-member
structs, function pointers that take `void *user`, and the 16-byte handle
structs of Section 4.5, which are two INTEGER eightbytes passed in two
registers on SysV AMD64 and AAPCS64. It never uses C `enum` types, `bool`,
bit-fields, or `long`, whose sizes are implementation-defined. No struct
of the ABI has implicit padding on an LP64 target (a 32-bit member is
followed by another one or by an explicit `reserved` member), so a
`#pragma pack` that another header leaves active cannot change a layout,
and `cpl.h` and every generated header assert the sizes of their structs
on LP64 targets (`CPL_LAYOUT_ASSERT`, Appendix A.1).

| Tier | Targets | Commitment in Milestone 2 |
| - | - | - |
| 1 | `x86_64-linux-gnu` with gcc 11.4.0 and glibc 2.35, the acceptance run; gcc 9 or later, clang 12 or later, and glibc 2.31 or later are accepted by `cpl build` (toolchain T72) but untested in Milestone 2 | acceptance runs here, with the acceptance run's compiler and C library: every suite, every sanitizer, ThreadSanitizer under `setarch -R` |
| 2 | `aarch64-linux-gnu` | builds: the runtime and the link recipe are portable; `cpl build` warns `build_target_untested`; tested before Milestone 3 ships wheels |
| 3 | macOS (Mach-O, `pthread_get_stackaddr_np`), Windows x64 (MSVC 17.5 or later, C11 atomics, `.def` files, `GetCurrentThreadStackLimits`) | refused in Milestone 2 with `build_target_unsupported` (exit 2): the platform file and the link recipe exist only for Linux; the dispatch through function tables (Section 4.1) assumes no symbol interposition, so nothing in the API changes when they arrive |

The lock records the target (`-dumpmachine`) as `targets.c.target`
(toolchain 11.2); the codes are toolchain's (Appendix B).

Decision: [D-291] C5. The C ABI of edition 0.1 is the platform calling
convention over fixed-width integers, `size_t`, pointers, function
pointers with `void *user`, and 16-byte handle structs, never a C `enum`,
`bool`, bit-field, or `long`; Tier 1 is `x86_64-linux-gnu`, committed
with the compiler and C library of the acceptance run (gcc 11.4.0, glibc
2.35) and accepting gcc 9 or later, clang 12 or later, and glibc 2.31 or
later untested, Tier 2 `aarch64-linux-gnu` (built, warned, untested), and
every other target is refused. Rationale: the sizes of `enum`, `bool`, and
`long` differ between compilers and platforms (`long` is 32 bits on
Win64), and only the target where the acceptance suites run can carry a
commitment, as can only the versions they run with: the design selects "a
documented set of runtime and platform versions" that is "tested in CI"
(design line 441), so a version becomes a commitment when a CI job runs
the suites with it. Rejected alternatives: C `enum` and `bool` in the ABI
(their sizes are implementation-defined); claiming macOS and Windows
without a platform layer or a test run.

## 2. Runtime

### 2.1 Implementation language and files

The runtime is ISO C11 (Section 1.5), about 7,500 lines, in
`crates/cpl-rt/c/`. It is one native core with the emitted C: the C API,
the native executables, the drivers, and the CPython and Node-API packages
of Milestones 3 and 4 all wrap it. The Rust crates remain the toolchain
and the evaluator, which stays the oracle (Section 3).

| File | Contents |
| - | - |
| `include/cpl.h` | the public API, normative (Appendix A.1) |
| `include/cpl_adapter.h` | the adapter-side declarations and helpers (Appendix A.2) |
| `cpl_rt.h` | the internal contract with emitted code (Appendix B.1) |
| `cpl_rt_internal.h` | the runtime's own cross-file contract, which emitted code never includes: the layouts of `cplrt_fault` (the fields of Section 3.3.1) and `cplrt_runtime` (handle table, allocator, heap counters, capability registry, defaults), and every function one runtime file calls in another (the allocation wrappers, the value builders, the JSON reader and writer, the S49 writer, the `{name}` renderer, the evaluation-thread launcher); included by every runtime `.c` file and by both mains |
| `cpl_rt_platform.h`, `cpl_rt_platform_linux.c` | threads, the monotonic clock, per-thread cached stack bounds |
| `cpl_rt_value.c` | heap objects, atomic counts, iterative release, immortals, the handle table |
| `cpl_rt_int.c` | arbitrary-precision `Int`; `cplrt_int_divrem`, the only division |
| `cpl_rt_text.c` | `Text` and `Bytes`, UTF-8 validation with `valid_up_to` offsets, scalar counts, the `std.text` kernels |
| `cpl_rt_list.c` | `List`, every list builtin, `flatten` |
| `cpl_rt_builtins.c` | the remaining callable builtins, the `check<R>` failure values |
| `cpl_rt_json.c` | the strict value-JSON reader and writer, the S49 diagnostic writer, the protocol JSON reader |
| `cpl_rt_render.c` | value text and `quote_text` |
| `cpl_rt_fault.c` | fault records, the S34/S70/S71 claims, backtraces, the violation record as raised |
| `cpl_rt_eval.c` | evaluation contexts, frames, the depth counter, the stack guard, the time budget, cancellation |
| `cpl_rt_boundary.c` | the type-directed shape checks and the refined positions in S74 order (export arguments, capability results) |
| `cpl_rt_api.c` | the `struct cpl_api` table, handles, the runtime lifecycle, error objects, options |
| `cpl_rt_cap.c` | capability objects, callbacks, result validation (Section 6.4), `ForeignError` construction (Section 6.5) |
| `cpl_rt_driver.c` | the driver's `main` and cpl-driver protocol 1 (drivers only, Section 3.8) |
| `cpl_rt_exe.c` | the native executable's `main` (executables only, Section 8) |
| generated `cpl_rt_gen.h` | the tables of Section 2.9, written at emit time and never checked in |

Every function of the runtime that transcribes a function of the
evaluator carries a `/* mirrors cpl_eval::<path> */` comment naming it;
`crates/cpl-rt/c/README.md` indexes them.

Decision: [D-292] C6. The runtime is C11 source embedded in the `cpl`
binary and written out and compiled, with the emitted code, by every
build; it is part of one native core with the emitted C, which the C API
and the later Python and Node packages wrap, and no prebuilt static or
shared runtime exists. Rationale: a C runtime needs only the C compiler
the C boundary needs anyway, cross-compiles with any cross compiler, and
builds a CPython limited-API extension and a Node-API addon with gcc
alone, as Milestones 3 and 4 need; it is instrumented together with the
emitted code by ASan, UBSan, LSan, and ThreadSanitizer; it reports
allocation failure as a fault instead of aborting (semantics S76), with no
panics to contain at every boundary call; and an independent second
implementation compared on every case finds bugs in the evaluator too,
which shared code would hide. The cost, a second implementation that must
stay equal to the evaluator, is paid by the single source of every
spec-owned text and table (Section 2.9), by computing every reported fact
with the evaluator's own helpers at emit time (Section 3.2), and by the
differential suites with their coverage gate (toolchain 12.6). Rejected
alternatives: one Rust core shared by the evaluator and the C API (every
generated package would carry a prebuilt Rust static library per target
and per host kind, or need cargo at install time; a Rust runtime is not
instrumented by the sanitizers on stable; safe Rust aborts on allocation
failure; shared builtins make a shared bug invisible to the oracle; and it
needs the reshaping of accepted Milestone 1 APIs, `Rc` to `Arc` among
them); a prebuilt runtime archive (per target triple, and it could drift
from the emitter that writes against `cpl_rt.h`); the `cc` crate in a
build script (Cargo would compile C, with environment-driven defaults).

### 2.2 How the runtime ships and how every build finds it

- **Embedding.** `crates/cpl-rt/build.rs` reads every `.c` and `.h` file
  under `crates/cpl-rt/c/` (the *runtime files*; `README.md` and any other
  file is neither embedded nor digested, so editing it changes no lock) in
  sorted path order and writes `$OUT_DIR/runtime_files.rs`, which defines
  `pub static RUNTIME_FILES: &[(&str, &str)]` (relative path,
  `include_str!`), printing `cargo:rerun-if-changed` for the directory and
  every file and failing on a file that is not UTF-8. This is the
  mechanism `crates/cpl-project` uses for `std/` (`STD_FILES`, toolchain
  3.6). Cargo compiles no C.
- **Digest.** The runtime digest is the blake3 of `path LF content LF` for
  every runtime file in that order, followed by `cpl_rt_gen.h LF` and the
  generated header's content and `LF`: the construction of `std_digest`
  (`crates/cpl-project/src/stdlib.rs`). It is recorded in `build.json`, in
  the lock (`targets.c.runtime_digest`), in `cpl_library_info.runtime_digest`,
  and in the banner of every generated header. Toolchain: the Rust API is
  `cpl_rt::runtime_digest` (toolchain 1.12, T69).
- **Override.** `CPL_RUNTIME_DIR` replaces the embedded files with a
  directory, for runtime development only, as `CPL_STD_DIR` does for
  `std/`. `build.json` then reports the runtime's source as `"dir"`, and
  the digest covers the files used.
- **Amalgamation.** `cpl_rt::amalgamation` writes `src/cpl_rt.c` as
  `#define _GNU_SOURCE 1`, then the namespace prologue of Appendix B.1 for
  the package's prefix, then `#include "cpl_rt.h"`, then the text of every
  runtime `.c` file except `cpl_rt_driver.c` and `cpl_rt_exe.c`, in sorted
  path order, each preceded by `#line 1 "<file name>"`; it writes the
  runtime's headers beside it (Section 1.6). Every runtime header is
  include-guarded, so each file keeps its own `#include` lines, and every
  file-scope name with internal linkage begins with its file's stem after
  `cpl_rt_` (`value_`, `int_`, `json_`, ...), so the concatenation cannot
  define one twice. The two mains, `cpl_rt_driver.c` (drivers) and
  `cpl_rt_exe.c` (the executable artifact), are separate units with the
  same two prologues.
- **`cpl build` locates nothing.** It writes the embedded files into the
  output root and compiles them, so the runtime can never be stale or
  mismatched with the emitter.
- **`cargo test`** obtains a compiled runtime by calling the library code
  `cpl build` uses (`cpl_emit_c::package::build`), which runs the system
  compiler through `std::process::Command` into
  `env!("CARGO_TARGET_TMPDIR")`. `cpl-rt`'s own C unit tests, which may
  not depend on `cpl-emit-c` (toolchain 1.2), compile the runtime files
  with the flag sets of Section 1.5 through test helpers of `cpl-rt`
  (toolchain 1.12). There is no nested cargo, no `cc` crate, and no build
  script that compiles C. Toolchain: a test that needs a compiler and
  finds none is skipped unless `CPL_REQUIRE_CC=1` (toolchain 12, T78).

### 2.3 Value representation

`typedef uint64_t cplrt_w;` is internal to the runtime and emitted code;
it is never an ABI (design line 465).

| Word pattern | Meaning |
| - | - |
| `...xxx1` | a small `Int` *v* in [-2^62, 2^62 - 1], stored as `(v << 1) \| 1` |
| `0b0010`, `0b0110` | `false`, `true` |
| `0` | no value (an empty slot); never a value of the language |
| any other, 8-aligned | a pointer to a heap object |

Every heap object begins with a 16-byte header (`cplrt_obj`, Appendix B):
`kind` (one of `CPLRT_BIGINT`, `_TEXT`, `_BYTES`, `_LIST`, `_STRUCT`,
`_ENUM`, `_WRAPPER`), `flags`, a 16-bit `variant`, a 32-bit `len` (the
number of children or limbs), and a union of the live count with the
`DefId.0` of a struct, enum, or wrapper and, once the object is dead, the
link of the pending-release list.

| CPL value | Representation |
| - | - |
| `Int` | a small immediate, or a bigint: the sign in `flags` (`CPLRT_F_NEGATIVE`) and a normalized array of `uint32_t` limbs, least significant first; a value in the small range is always small, so each value has exactly one representation |
| `Bool` | an immediate |
| `Text` | `{hdr, size_t bytes, size_t scalars, char utf8[bytes + 1]}`: valid UTF-8 without surrogates, the scalar count cached (an O(1) `len`), a NUL after the last byte |
| `Bytes` | `{hdr, size_t len, uint8_t data[len]}` |
| `List<T>` | `{hdr, cplrt_w item[len]}`, contiguous, or a *view* (`CPLRT_F_VIEW`): `{hdr, cplrt_w base}`, the `len` items of the list `base` from the index in the header's `def` word, `base` never a view itself (stdlib.md S15: O(1) `len` and `get`; `take`, `drop`, and `drop_last` are views of the operand's buffer and copy nothing, as the evaluator's are, unless the result has at most 8 items, which are copied; `push` and `++` copy in O(result)). `cplrt_item` and `cplrt_items` read both forms |
| struct | `{hdr(def), cplrt_w item[n]}`, the fields in declaration order |
| enum, `Unit`, `Option`, `Result` | `{hdr(def, variant), cplrt_w item[n]}`, the payload in order; every payload-less variant (`None`, `Unavailable`, `Unit`, ...) is one immortal static of the program tables |
| wrapper | `{hdr(def), cplrt_w item[1]}` |
| refinement | its base value (semantics 6.1) |
| capability | not a word: a `const cplrt_capref *` (`{impl, user, capability DefId, handle number}`), passed only in `using` positions; semantics S27 guarantees that no value contains one. In a `visible` array (Section 1.4) a `using` parameter's entry is its reference cast to a word, which the runtime recognizes by the binding site's table and renders as `{"$handle": n, "$type": "<capability>"}` (Appendix B.1) |
| `""`, `[]`, empty `Bytes` | immortal statics of the runtime |

An enum value whose variant index exceeds 65,535 (possible only in an
enum with more than 65,536 variants) sets `CPLRT_F_WIDE` and stores the
index as the small `Int` in `item[0]`, its payload following;
`cplrt_variant` and `cplrt_payload` (Appendix B) read both forms, so the
limit is not observable. A composite whose child count, or a bigint whose
limb count, would not fit `len` (2^32 - 1) cannot be allocated: the
allocation fails, as Section 2.4 says.

Values carry their declaration, so every rendering (`$type`, field and
variant names, value text) is value-directed, as `value_to_json_untyped`
and `value_text_untyped` of `crates/cpl-eval/src/json.rs` are. The one
typed case, a capability handle's `$type` in `bindings`, takes the
capability from the binding site's table (Section 3.2).

Decision: [D-293] C7. A runtime value is a 64-bit word: a tagged small
`Int` in [-2^62, 2^62 - 1], one of two `Bool` immediates, or a pointer to a
heap object with the 16-byte header and the bodies of the table above;
payload-less variants and the empty `Text`, `List`, and `Bytes` are
immortal statics; capability handles are never words. Rationale: the
representation is internal, so it can be chosen for speed and for the
value-directed renderings the equivalence needs; immediates avoid an
allocation per `Int` result and per `Bool`, a normalized bigint gives each
value one representation (so equality is a word or limb comparison), and
carrying the declaration in every composite makes every rendering
independent of static types, as in the evaluator. Rejected alternatives:
boxed small integers (simpler, but one allocation per `Int` result; the
tagged word costs a few hundred lines and is clean under UBSan on the
boundary cases); a linked or persistent list (it breaks stdlib.md S15's
O(1) `len` and `get`).

### 2.4 Memory management

- **Ownership.** Each word in a slot, an object's child, or a handle-table
  entry owns one reference; parameters are borrowed (Section 1.2). The
  slot discipline (Section 1.3) releases every owned slot exactly once.
- **Acyclic.** CPL values are immutable after construction; there are no
  mutable cells, closures, or references (semantics 6.1), and a `var`
  rebinds a name, never mutates a value. An object can only refer to
  objects that existed when it was built, so the heap is a DAG and
  reference counting reclaims all of it, without a cycle collector.
- **No host cycles.** Capability handles are never stored in values
  (semantics S27). Host state (`void *user`) belongs to its capability
  object and is dropped exactly once. A host can create a cycle only by
  keeping a CPL handle in its own `user` data, which is the host's
  responsibility (Section 5.7).
- **Threads.** Increments are `memory_order_relaxed`; a decrement is
  `memory_order_acq_rel`, and the thread that takes the count to zero
  releases the object. No standalone fence is used: ThreadSanitizer does
  not model `atomic_thread_fence`, so a release decrement followed by an
  acquire fence would make it report the release of a shared value as a
  data race. Values are immutable, so sharing one across threads needs
  nothing more. Capability objects are counted the same way (Section 5.7).
- **Saturation.** Every count at or above `CPLRT_RC_SATURATED` (2^31)
  means immortal. An increment that brings a count to
  `CPLRT_RC_SATURATED` stores `CPLRT_RC_IMMORTAL` (3 * 2^30), the count of
  statics, and both `cplrt_retain` and `cplrt_release` leave a count in
  that range alone, so increments and decrements that race with the
  saturation stay inside it, never wrap to 0, and never free an immortal
  object.
- **Depth.** Release never recurses: a dying composite is linked into an
  intrusive pending list through its header (`u.next`), and its children
  are decremented from that list. Equality and both renderings also run on
  explicit work stacks (semantics S72).
- **Out of memory.** Every allocation is checked; the runtime never calls
  `abort` or `exit`. Inside an evaluation a failure is the fault
  `resource_exhaustion` with `resource: "memory"` (semantics S76, Section
  4.7); outside one (the value functions of the API) it is `CPL_E_NOMEM`
  with a static error object. `cpl_runtime_options.max_heap_bytes` makes
  the same paths a budget: it bounds the bytes the runtime has allocated
  through its allocator after the runtime object and not yet freed
  (values, the handle table, fault records, buffers, error objects), the
  quantity `cpl_stats.live_bytes` reports. The budget is deterministic for
  a runtime that one thread uses at a time; concurrent calls share it, so
  which of them meets it depends on their interleaving.
- **The object pool.** While an evaluation runs, the objects of up to 64
  bytes it makes (16-byte size classes) are slots of 64 KiB pages, one
  page per class at a time: a slot comes from its page's free slots or its
  untouched rest and goes back to its page's free slots, with no call to
  the allocator and no atomic update. A page counts whole in `live_bytes`
  from its allocation to its release (so the budget meets a page at once);
  the pool's objects count in `live_objects` when the evaluation that
  owns the pool ends, so between calls the statistics are exact. A page
  left with no objects is freed, or kept as one of 4 spares (uncounted)
  for the next pages; `cpl_runtime_close` frees them all. One thread at a
  time owns a runtime's pool: the first evaluation to begin while no other
  thread holds it, until its outermost evaluation ends, or, outside any
  evaluation, a release for its own duration. Another thread that frees a
  slot (a parallel section's worker, a concurrent call) pushes it onto
  its page's remote stack, which the owner takes back when the page's
  other slots run out and when its ownership ends; `cpl_runtime_stats`
  takes them back first when no evaluation runs. The pool makes nothing
  for a runtime with a host allocator (which sees every block) or with a
  `max_heap_bytes` below 64 MiB (whose page-sized steps would move where a
  small budget is met). Under AddressSanitizer a free slot stays poisoned
  until it is reused, so a use after free is still reported.

Decision: [D-294] C8. The runtime manages values by atomic reference
counts, with relaxed increments, acquire-release decrements, saturation
into a range of immortal counts, and iterative release through an
intrusive pending list, and it reports every allocation failure.
Rationale: values are immutable and acyclic, so counting is complete and
releases each value deterministically at its last use, which is what the
design's "explicit release operations" (line 465) promise a host; atomic
counts make a value shareable between the threads of a host (Section 4.4)
at no other cost. Rejected alternatives: a tracing collector or arenas
(release would become non-deterministic, and arenas would keep every
intermediate of a `push` loop alive, O(n^2) memory for `paginate`);
non-atomic counts (unsound as soon as a host shares a value across
threads: free-threaded CPython, Node worker threads, C thread pools);
release decrements with an acquire fence before the release (equivalent
on the hardware, but ThreadSanitizer, which the acceptance suites run,
does not model fences and reports false races); a single immortal count
at `UINT32_MAX` (an increment racing with the saturation wraps it to 0).

### 2.5 Arbitrary-precision `Int`

- Small immediates, then bigints of `uint32_t` limbs in sign and
  magnitude, normalized (no high zero limb; never in the small range).
- Schoolbook addition, subtraction, comparison, and multiplication; Knuth's
  algorithm D for division (stdlib.md 2.2 binds only the reference
  complexity of `*`, `/`, `%`); decimal conversion in chunks of base 10^9;
  parsing in O(n^2), as the reference's is.
- Division and remainder exist once, in `cplrt_int_divrem`, which mirrors
  the single function of toolchain T9 (`cpl_eval::arith::div_rem`): the
  quotient truncates toward zero and the remainder is `a - q * b`
  (semantics S37); `-2^62 / -1` promotes to a bigint; a zero divisor raises
  `division_by_zero` with `input: {"a", "b"}` at the operator's site.
- Small-path overflow is decided by `__builtin_add_overflow` and
  `__builtin_mul_overflow` plus a range test, with a portable fallback,
  clean under UBSan with `-fno-sanitize-recover=all`.
- Conversions: the decimal text is the canonical one (an optional `-`,
  then digits without leading zeros, `0` for zero); one decimal reader
  implements stdlib.md S25 for `parse_int` and for `cpl_int_from_decimal`;
  the JSON reader rejects fractions and exponents (toolchain 7.3); checked
  conversions to and from `int64_t`, `uint64_t`, and little-endian arrays
  of 64-bit words serve the C API (Section 4.8) and, later, Node `BigInt`
  and Python `int`.

Decision: [D-295] C9. `Int` is a small immediate or a normalized
sign-and-magnitude bigint of 32-bit limbs, and division and remainder
exist only in `cplrt_int_divrem`, which computes the truncated quotient
and derives the remainder as `a - q * b`. Rationale: one representation
per value makes equality and hashing structural; one division function
mirrors the evaluator's single `div_rem` (T9), so semantics S37 is
implemented once on each side and fuzzed between them (toolchain 12.6,
D7);
32-bit limbs keep every partial product within 64 bits portably. Rejected
alternatives: GMP (a native build dependency on every platform, which
toolchain 1.3 rejects for the Rust side as well); separate division and
remainder routines (two places to get S37's signs right).

### 2.6 Text, Bytes, List, struct, enum, wrapper, and capability values

- **Text.** Construction validates UTF-8 with the automaton of Unicode
  Table 3-7 (the well-formedness of Rust's `str::from_utf8`), reporting the
  first invalid offset exactly as `Utf8Error::valid_up_to` does. `==` is
  byte equality, which is scalar-sequence equality for valid UTF-8. `++`
  copies, and returns an operand when the other is empty (unobservable, as
  in `Value::concat`).
- **Bytes.** Counted byte buffers. `from_list` checks the values left to
  right and faults at the first one outside 0..255 with `input` the whole
  list, as `crates/cpl-eval/src/builtins/bytes.rs` does.
- **List.** `take(n)` and `drop(n)` compare the `Int` with `len` before
  any conversion to a machine integer, so a huge count clamps (stdlib.md
  4.1); a negative `n` is the precondition fault; `get(i)` is `None` for a
  negative or huge `i`. A result equal to the operand shares it, as the
  evaluator's does (unobservable).
- **Struct literals.** The spread base first, then the fields in source
  order (semantics 6.2); a spread keeps the base's other fields.
- **Capabilities.** `cpl_o<cap>_<op>` evaluates the operation's `requires`
  with its parameters bound and no frame pushed (`attributed_to:
  "caller"`), invokes the host through `cplrt_cap_invoke` (the binding
  layer of Sections 5.7 and 6.2-6.3, the foreign-failure mapping of
  Section 6.5, the validation of Section 6.4), then evaluates its
  `ensures` with `result` bound (`attributed_to: "binding"`): the
  evaluator's `call_operation`. The effect point (an `EffectEvent` in the
  evaluator when `trace_effects` is on) is not traced natively in
  Milestone 2 (Section 5.8).

Decision: [D-296] C10. `Text` is validated UTF-8 with a cached scalar
count and a trailing NUL, `Bytes` a counted buffer, `List` a contiguous
array, structs, enums, and wrappers arrays of children carrying their
`DefId`, and a capability a reference passed only in `using` positions;
every operation reproduces the evaluator's order of checks and its
results, including the offsets and inputs it reports. Rationale: the
cached count gives `len` in O(1) and the NUL lets a host pass a view to C
functions without copying (with `len` authoritative, since U+0000 is a
scalar); the representations are those the evaluator's semantics needs,
so its functions can be transcribed operation by operation. Rejected
alternatives: capabilities as words in values (semantics S27 forbids
storing them, and a word would make them forgeable by copying); views or
ropes for lists and text in Milestone 2 (performance work, deferred and
ABI-neutral).

### 2.7 Every std builtin

The 59 ids of the builtin registry (stdlib.md 10.6; `REGISTRY` in
`crates/cpl-eval/src/builtins/mod.rs`) are listed in
`cpl_rtspec::builtins::BUILTINS` with their C entry points (toolchain T70).
The 13 that are not callable, six types and seven generators, are never
called by a checked program: a builtin type is never called, and a
generator is `ExprKind::Gen`, drawn by the Rust runner for both engines
(toolchain T18); a call that names one, which only a hand-built HIR state
contains, lowers to `cplrt_bi_uncallable` (Section 1.4). The 46 callable
ones are `cplrt_bi(<id>)(cx, site, stub, args..., out)` (the external
symbol `cplrt_<prefix>__bi_<id>`, Appendix B.1), where `stub` is the
`DefId.0` of the `@builtin` stub the call names. Each transcribes the Rust
function of the same id with a `/* mirrors cpl_eval::builtins::<path> */`
comment, and the runtime fuzz suite (D7, toolchain 12.6) compares the
two.

| Ids | C semantics (what must match) |
| - | - |
| `int_to_text`, `text_from_int` | the canonical decimal: `-`, then digits, no leading zeros, `0` for zero |
| `int_abs`, `int_min`, `int_max` | `min` is `b < a ? b : a` and `max` is `b > a ? b : a`, so ties return `a` (`prelude::int_min`, `int_max`) |
| `text_len`, `text_is_empty`, `bytes_len`, `bytes_is_empty`, `list_len`, `list_is_empty` | O(1); `text_len` counts scalars |
| `text_trim`, `text_trim_start`, `text_trim_end` | strip leading and trailing scalars of the generated `CPLRT_WHITE_SPACE` ranges: the 25 scalars of stdlib.md S22 (U+0009-000D, U+0020, U+0085, U+00A0, U+1680, U+2000-200A, U+2028, U+2029, U+202F, U+205F, U+3000); not U+200B, not U+FEFF; the operand when nothing is stripped |
| `text_to_upper`, `text_to_lower` | ASCII `a-z` or `A-Z` only, every other byte copied unchanged (stdlib.md S23); the operand when nothing changes; no case table exists in edition 0.1 |
| `text_chars` | one single-scalar `Text` per scalar, in order |
| `text_starts_with`, `text_ends_with`, `text_contains` | byte prefix, suffix, and substring on valid UTF-8, exact at the scalar level because UTF-8 is self-synchronizing (the evaluator's argument, `crates/cpl-eval/src/builtins/text.rs`) |
| `text_split` | the precondition `not sep.is_empty()` first; then leftmost-first, non-overlapping pieces, empty pieces kept: `split("", s) == [""]`, `split("aaa", "aa") == ["", "a"]` (stdlib.md S24) |
| `text_join` | the parts with `sep` between consecutive parts; `""` for `[]` |
| `text_parse_int` | `("+" \| "-")? [0-9]+` (stdlib.md S25), else `Err(ConstraintError { type_name: "Int", message: "<quote_text(t)> is not a decimal integer" })` |
| `bytes_from_list`, `bytes_to_list`, `bytes_from_text` | stdlib.md 3.4 |
| `bytes_to_text` | strict UTF-8, else `Err(ConstraintError { type_name: "Text", message: "invalid UTF-8 at byte <valid_up_to>" })` |
| `flatten` | concatenation in order; `[]` for `[]` |
| `list_take`, `list_drop`, `list_drop_last`, `list_push`, `list_get`, `list_first`, `list_last`, `list_reverse`, `list_contains` | as `crates/cpl-eval/src/builtins/prelude.rs`; `contains` uses structural `==` (Section 2.8) |
| `option_*`, `result_*` | variant tests and constructions with the prelude's variant indices (`CPL_OPTION_SOME`, `CPL_RESULT_OK`, ... of `cpl.h`) |

**Preconditions.** `list_take`, `list_drop`, `text_split`, and
`bytes_from_list` check their `requires` first, in every contract mode
(stdlib.md S39). The fault's `symbol`, `clause`, clause span, and `input`
names (`self` for a member's receiver) are the stub's facts in the program
tables, computed at emit time by `cpl_eval::builtins::requires_spec`
(Section 3.2), the function `requires_violation` itself uses; `caller` is
the innermost frame's declaration (absent at the top level).

**`check<R>`** is no builtin: emitted code evaluates the predicates of
`check_order(R)` root first, each in an activation of its refinement
(Section 1.4), and `cplrt_check_failure` builds `Err(ConstraintError {
type_name, message: "<value text> does not satisfy <predicate text>" })`
with the `type_name` precomputed by `constraint_type_name` and the fields
placed as `builtins::constraint_error` places them.

Decision: [D-297] C11. Every callable builtin is a C function of the
runtime that transcribes the evaluator's function of the same registry id,
marked with a `mirrors` comment, and fuzzed against it; its precondition
facts (symbol, clause text and span, input names) come from the program
tables, computed at emit time by the evaluator's own helper. Rationale:
the evaluator is the oracle, so the C side must follow it function by
function, and the facts a precondition fault reports are exactly where two
implementations drift, so they are computed once, in Rust. Rejected
alternatives: generating the builtins' C from their Rust code (no such
generator exists, and the kernels are short); using the C library's or a
host language's Unicode classification or case mapping (stdlib.md S47:
neither engine may, so `trim` uses the generated ranges and the case
functions are ASCII-only).

### 2.8 Equality and ordering

`==` and `!=` are structural, computed on an explicit work stack with the
algorithm of `Value::structurally_equal`
(`crates/cpl-eval/src/value.rs`): equal words are equal; then kind,
`def`, and variant; then the children pairwise, left to right, stopping at
the first difference; a child array shared by both sides is equal without
being walked. A value of another kind or declaration is unequal. `Int`
ordering is a sign-and-magnitude comparison and cannot fail. `Text` and
`Bool` have no ordering (stdlib.md 1.7, S7), and capability equality is
rejected statically (`capability_equality`), so neither is implemented.
The work stack's allocation can fail, so `cplrt_equal` returns a status
and its failure is `resource_exhaustion` (`memory`).

Decision: [D-298] C12. Structural equality is iterative, left to right,
with the evaluator's short cuts, and returns a status; ordering exists only
for `Int`. Rationale: values may nest arbitrarily deep (semantics S72), so
any recursion on value depth could exhaust the host stack, and the
evaluator's algorithm is the specification of which comparisons happen.
Rejected alternatives: recursive equality (a crash on deep values); an
infallible `cplrt_equal` (its work stack must be allocated, and an
allocation failure inside an evaluation is a fault, semantics S76).

### 2.9 The generated runtime tables

No text or table that a specification owns is written by hand in the C
runtime. `cpl-emit-c` writes `cpl_rt_gen.h` at emit time from `cpl-rtspec`
(toolchain T70; stdlib.md S47), so the C side cannot drift from the Rust
side; Appendix B.2 gives its exact declarations, and only the runtime's
units include it:

| Macro family | Contents |
| - | - |
| `CPLRT_MSG_*` | every fault and value message as a `{name}` template: `"{clause_kind} clause failed: {clause}"`, `"assertion failed: {clause}"`, `"evaluation reached the unresolved hole ?{hole}"`, `"division by zero"`, `"the value does not satisfy {expected_type}: {clause}"`, `"the call depth limit of {limit} activations was exceeded"`, the stack-guard message, `"the time budget of {limit} ms was exceeded"`, `"memory was exhausted"`, `"internal error ({kind}): {detail}"` and every `internal_error` detail template, `"{value} does not satisfy {predicate}"`, the `parse_int` and `bytes_to_text` messages, the binding messages of semantics S77 and S80 (the `detail` of a scripted host's unmatched call among them) and stdlib.md S45 |
| `CPLRT_WHITE_SPACE` | the ranges of the 25 White_Space scalars (stdlib.md S22) |
| `CPLRT_BUILTINS` | the registry: id, callable, fallback symbol, precondition clauses and input names (the C entry points are the emitter's, Section 2.7) |
| `CPLRT_LIMIT_*` | the default call-depth limit 10,000, the evaluation stack of 1 GiB, the guard margin of 64 MiB, the C API version 0.1, the driver protocol 1 |
| `CPLRT_FOREIGN_*` | the `ForeignErrorKind` names in declaration order and the fixed order of the detail keys (stdlib.md S45) |
| `CPLRT_INTEROP_*` | the codes of Section 4.9 with their statuses |

A `{name}` substitution routine of about 40 lines renders the templates in
C. Toolchain: a test checks that the header is what `cpl-rtspec`
generates and that the C renderer renders every template identically to
the Rust renderer (toolchain 12, T78). What cannot be generated (the
bigint, UTF-8, value text, `quote_text`, the JSON reader and writer, the
text kernels) is transcribed function by function with a `mirrors` comment
and fuzzed against the Rust originals (toolchain 12.6, D7).

## 3. Equivalence mechanisms

### 3.1 The equivalence statement

Semantics: S75 fixes what native execution must produce: for the same
checked program, entry, arguments, evaluation configuration, and bindings,
the same value or the same fault byte for byte, the same test reports and
receipts but for the keys it exempts, and C renderings byte-identical to
the evaluator's; it also lists the sanctioned divergences and what must
still agree in each.

This section fixes the mechanisms that make S75 hold. The native engine
evaluates on a thread with 1 GiB of stack reserved, the evaluator's
`EVAL_STACK_SIZE` (`crates/cpl-eval/src/stack.rs`), so under the default
limit of 10,000 the depth counter, not the stack guard, decides in both
engines. A guard fault arises only for a `max_call_depth` beyond what the
evaluator's stack holds, and there the engines may differ in whether and
where the guard trips, since an emitted activation uses less stack than
the evaluator's (semantics S75).

### 3.2 Every reported fact is computed at emit time

The native side never forms a name, a clause text, or a location.
`cpl-emit-c` computes every fact a fault or a rendering reports with the
Rust function the evaluator uses for it, and writes it into the program
tables (`<prefix>_tables.c`, Appendix B):

| Fact | Computed at emit time by | Used for |
| - | - | - |
| `symbol`, `qualified_name` of a declaration | `Def::stable_id`, `Program::display_path`; the `qualified_name` of an identity no definition has is the identity itself (`Program::by_stable_id` finds none), as `Fault::to_diagnostic` does | faults, backtrace frames, `caller` |
| operation identities (`reader.pages.PageRead.get`) | `cpl_eval::types::operation_identity` (toolchain T77; today the private `Interpreter::operation_identity` of `crates/cpl-eval/src/interp.rs`) | operation clause faults, the faults and details of Section 6.4 |
| `$type` names, bare names, field and variant names | `cpl_eval::types::type_name`, `bare_name`; `FieldDef::name`, `VariantDef::name` | value JSON, value text, `ConstraintError.type_name` |
| clause and assertion texts | `Clause::text`, `Stmt::Assert::text` (T56) | `clause` |
| spans | the `Span { file, start, end }` of each HIR site and its location from the source map (`SourceMap::location`), with the file's project-relative path | `span`, backtrace spans; the protocol of Section 3.8 carries `(FileId, start, end)` |
| binding sites | `cpl_eval::interp::scope_at` (toolchain 1.7.4), against which the interpreter's own bindings are tested: for each `assert`, `while` loop, and hole, the locals in the order in which the evaluator brings them into scope (the positional parameters in order, then the `using` parameters, then each binder as evaluation binds it: `let` and `var`, pattern bindings, `forall` and `for` binders, a block's binders dropped at its end), listed innermost (last bound) first, each with its name, `Program::display_ty_in` text, `BindingKind`, `mutable`, and, for a `using` parameter, its capability; so a hole of `load_page(id, using db)` lists `db` before `id` | `bindings`, the `actual.bindings` of loop violations |
| holes | the name and `display_ty_in` of the expected type with the enclosing definition's generics (`raise_hole`) | `hole`, `expected_type` |
| refinement chains | `cpl_eval::builtins::check_order`, `constraint_type_name`, the predicate's `Clause::text`, the refinement's `DeclName::display` (the `expected_type` of `run_function`'s argument check, `crates/cpl-eval/src/run.rs`), and its root base `cpl_eval::types::strip` | `check<R>`, boundary checks |
| builtin stub facts | `cpl_eval::builtins::requires_spec` (toolchain T77; extracted from today's `requires_violation`) and `stub_of` | builtin precondition faults |
| std layout | `Program::std_defs` (`StdDefs`: `unit`, `option`, `result`, `constraint_error`, `foreign_error`) and the declarations `std.foreign.ForeignErrorKind` and `std.foreign.ForeignDetail` | constructing `Unit`, `Option`, `Result`, `ConstraintError`, `ForeignError` |
| types | for every type of the type table (Section 5.3), its qualified type text (`cpl_sema::interop::type_text`, toolchain 1.6) and its `Program::display_ty` text (semantics 3.3) | the qualified text: `cpl_type_name`, `cpl_type_lookup`, and the `interop_*` payloads; the display text: the `<expected_type>` of the messages and details of stdlib.md S45 and of semantics S80 |

The only texts the runtime owns are the message templates, and those are
generated from `cpl-rtspec` (Section 2.9). Toolchain: the table-fidelity
test asserts, for every definition, site, clause, binding site, hole, and
stub of every corpus program, that the emitted tables give the program's
own answers, and that the compiled tables return the same answers through
the driver's `tables` request (toolchain 12, T78).

Decision: [D-299] C13. Every name, text, and location the native side
reports is computed at emit time by the same Rust function the evaluator
uses for it and emitted as data; the runtime renders data and templates
only. Rationale: two implementations drift exactly where they form names,
clause texts, and spans (a qualified name for a member stub, a provisional
identity, a scalar column), and forming each once, in Rust, removes that
drift by construction instead of testing for it. Rejected alternatives:
forming names and locations in C from a serialized HIR (a second
implementation of display paths and source maps); moving the naming and
stub helpers into `cpl-hir` (it changes a crate that toolchain T50 keeps
minimal, while calling `cpl-eval`'s own pure functions guarantees the same
answers with no Milestone 1 change).

### 3.3 Checked-mode contracts: the same points, in the same order

| Event | Evaluator | Native |
| - | - | - |
| function entry | `call_fn`: the depth check; `enter_activation`: the stack guard, the time budget, the frame push; `run_fn`: each `requires` in source order (a later one only after the earlier held), then the header `decreases` (semantics S35) | `cplrt_enter_fn` (depth, guard, time, cancellation, the frame with the parameters); one `requires` region per clause; `cplrt_header_measure` |
| every exit | `ensures` in order with `result` bound, on the tail, a `return`, and a propagated `Err` | the `exit:` block; one `ensures` region per clause |
| `while` | the invariants at iteration 0; each time around: the time check, the condition, the measure (only after the condition held), the body, the invariants | the same, in the emitted loop (Section 1.4) |
| builtin call | no clause evaluated; the builtin checks its own `requires` in every mode | `cplrt_bi(<id>)` checks first, in every mode |
| capability operation | the operation's `requires` with no frame; the effect event; the host call; its `ensures` with `attributed_to: "binding"` | `cpl_o<cap>_<op>`: the same, with the validation of Section 6.4 between the host call and the `ensures` (in the evaluator too, for every Milestone 2 binding path, semantics S77) |
| `check<R>(x)` | the predicates of `check_order(R)` root first, each an activation; the first false gives `Err(ConstraintError)` | `cpl_p<r>` calls in the same order |
| `Refine` | re-checked when contracts are on; `internal_error` kind `refinement_proof_unsound` | the same, with the same template |
| `assert` | `assertion_failed` with the expression text and the visible bindings | `cplrt_assert_failed` with the binding site's table and the live slots |
| a fault escaping a clause | `eval_contract_expr` claims it once (the `attributed` flag), never an `internal_error` | `cplrt_claim` honours `cx->attributed`, which every top-level evaluation starts cleared, as `begin_evaluation` does |
| an `ensures`, `invariant`, or `decreases` violation | recorded as raised for `contract_counterexample` (the evaluator's `violation`) | `cx->violation` holds a copy of the record before any claim; the driver returns it with `run_body` and `range_bound` (Section 3.8) |

A contract region is the body of one `if (cx->check_contracts)` around
the evaluation and the test of one clause, and is tagged with its
obligation id (Section 1.4). A fault escaping a region's evaluation is
claimed there with `cplrt_claim(cx, CPL_CL<n>)` before the jump to `fail:`.

Decision: [D-300] C14. Emitted code checks every clause in a region of
its own, at exactly the evaluator's point and in its order, guarded by
the runtime flag `cx->check_contracts` rather than by a compile-time
constant, and tags each region with the obligation id of toolchain T62
that it checks; holes compile to the fault of Section 3.5. Rationale: a
runtime flag lets one compiled program run in both contract modes, so the
differential suites test both (Section 3.7), and per-clause regions with
obligation tags are the unit in which Milestone 5 drops proved checks.
Rejected alternatives: a compile-time constant for the contract mode (two
builds of every driver, and an untested unchecked mode); one region per
function (a proved clause could not be dropped alone).

#### 3.3.1 Fault records, claims, and the violation record

A fault is held in a *fault record* (`cplrt_fault`, whose layout is the
runtime's), owned by the evaluation context until the top-level evaluation
ends. A record holds what `cpl_eval::Fault` holds
(`crates/cpl-eval/src/fault.rs`):

| Field of `Fault` | Native source |
| - | - |
| `kind` | the fault kind (`contract_violation` with its clause kind, `assertion_failed`, `unresolved_hole`, `division_by_zero`, `refinement_violation`, `resource_exhaustion` with its resource, `internal_error`); `code` and `category` follow from it as in `FaultKind::code` and `FaultKind::category` |
| `message` | rendered once, from the template of Section 2.9 for the kind, when the raising helper creates the record; a claim never renders it again, as the evaluator's `eval_contract_expr` changes only `symbol`, `facet`, and `clause` |
| `symbol` | the blamed declaration: the clause's owner (`requires`, `ensures`, `decreases`, operation clauses), the innermost frame's declaration for a fault raised by an operator, a builtin, a hole, an `assert`, a loop clause, or a refusal by the depth limit, the guard, or the time budget (none at the top level, semantics S72); the stub for a builtin precondition; the function for a boundary `refinement_violation`; the operation identity for the faults of Section 6.4; never one for `internal_error` |
| `facet` | set to `contract` by every contract violation, every builtin precondition fault, and every claim; otherwise unset, and the diagnostic then says `body` when there is a symbol (`Fault::to_diagnostic`) |
| `span` | the site the raising helper is given: the clause's site for a violation, the operator's for `division_by_zero`, the hole's, the asserted expression's, the parameter's for a boundary violation, the stub clause's for a builtin precondition, the operation call's for a fault of Section 6.4 and for every binding fault of semantics S80; a builtin's fault without a span takes the call's site as it leaves the builtin |
| `backtrace` | `{def, site}` appended by `cplrt_leave` as the fault leaves each activation, and `{stub, site}` as it leaves a builtin; a refused call appends nothing |
| `data` | the payload keys of toolchain 1.7.3 for the kind, each rendered as value JSON or text by the C writers of Section 3.3.2: `clause_kind`, `attributed_to`, `clause`, `status`, `input` (the frame's or the operation's parameters by name), `actual`, `caller`, `iteration`, `bindings`, `hole`, `expected_type`, `resource`, `limit`, `depth`, `kind`, `detail`, `parameter`, `pointer`, and the binding fields of semantics S77 and S80 (`binding`, `operation`) |

- **Claims.** `cplrt_claim(cx, CPL_CL<n>)` implements semantics S34 and
  S70 exactly as `eval_contract_expr` does: unless the fault is an
  `internal_error` or `cx->attributed` is already set, it sets the flag,
  replaces the symbol by the clause's owner identity, sets the facet to
  `contract`, and sets `clause` to the clause's text, leaving every other
  field as raised. A violation raised by a callee's own clause reaches the
  calling clause's region as a fault and is claimed there (S70). A fault
  escaping a property's range bound is claimed by `cplrt_claim_range` with
  the range text the request carries (semantics S71, Section 3.8).
- **The violation record.** When a function's `ensures` or `decreases`, or
  a loop's `invariant` or `decreases`, is violated, the raising helper
  stores a copy of the record in `cx->violation` before any claim can
  rewrite it, replacing an earlier one, exactly as the evaluator's
  `violation` field records it. An operation's `ensures` violation is
  never recorded there (toolchain 1.7.3).
- **The memory record.** Every evaluation context owns a preallocated
  record for `resource_exhaustion` with `resource: "memory"` and `limit`
  the heap budget in bytes (0 when none was set). An allocation failure
  inside an evaluation fills it without allocating: `symbol` the innermost
  frame's declaration, no span, and the backtrace frames that can be
  recorded without allocating, possibly none (semantics S76); a record
  being built when the failure happens is dropped. Every partially built
  value is released on the way out.

Decision: [D-302] C16. A native fault is a record holding exactly the
fields of `cpl_eval::Fault`, filled at the points where the evaluator
fills them; claims follow S34, S70, and S71 through the same attribution
flag, the violation record is copied as raised, and memory exhaustion
uses a preallocated record. Rationale: a record of the same fields can be
rebuilt as a `cpl_eval::Fault` and rendered by the same Rust code
(Section 3.8), and its own C rendering, the one a C host sees, can be
compared byte for byte with the evaluator's; a preallocated record makes
reporting an allocation failure need no allocation. Rejected
alternatives: rendering only a code and a message natively (the fields
agents act on, `input`, `actual`, `bindings`, the backtrace, would be lost
at the C boundary); building the S49 text directly instead of a record
(the driver protocol needs the fields to rebuild the fault, and a claim
rewrites fields of a fault already raised).

#### 3.3.2 The C renderings

The runtime renders four things byte for byte as the Rust side does:
value JSON (`serde_json::to_string` of `value_to_json_untyped`), value
text (`value_text_untyped`), `quote_text` (`crates/cpl-diag/src/id.rs`),
and the S49 diagnostic of a fault (`serde_json::to_string` of
`Fault::to_diagnostic(p).to_json(sm)`). Those functions are normative;
the rules they imply for a C writer are:

- JSON is compact: no whitespace outside strings.
- Every object the Rust side builds as a `serde_json::Map` (every value
  JSON object, every payload value, `input`, `actual`, each `bindings`
  entry) writes its keys in byte order of their UTF-8: the workspace builds
  `serde_json` without `preserve_order`, so every `serde_json` map is a
  sorted map (toolchain 7.1, the `--json` row, and T4). The diagnostic
  object writes `code`, `category`, `severity`, `message`, `span`,
  `symbol`, `qualified_name`, `facet`, `notes`, `backtrace` in that order,
  each optional one omitted when absent or empty, then the payload keys in
  byte order (`DiagnosticJson`, `crates/cpl-diag/src/json.rs`); a span is
  `{"file", "line", "col", "end_line", "end_col"}` in that order (the
  struct `Location`), and a backtrace entry `{"qualified_name", "span"}`.
- A string escapes `"` as `\"`, `\` as `\\`, U+0008, U+0009, U+000A,
  U+000C, and U+000D as `\b`, `\t`, `\n`, `\f`, and `\r`, every other
  scalar below U+0020 as `\u00` and two lowercase hex digits, and nothing
  else: `/`, U+007F, and every non-ASCII scalar are written as their UTF-8
  (the escaping of `serde_json`'s compact formatter).
- An `Int` is its canonical decimal, as `int_to_json` and `serde_json`'s
  `arbitrary_precision` write it.
- Value text is semantics S40's form with `quote_text`'s escapes: `\\`,
  `\"`, `\n`, `\r`, `\t`, every other scalar below U+0020 and U+007F as
  `\u{h}` with the minimal number of lowercase hex digits.

### 3.4 The call-depth limit and the host-stack guard

- **Counter.** `cx->calls` counts active CPL function calls only;
  predicates, test bodies, range bounds, and builtins are frames but not
  calls (semantics S72). A refused call is raised against the innermost
  frame at the call's site; at the top level it has no symbol.
  `max_call_depth = 0` refuses every function call and still runs
  predicates and bodies.
- **Guard.** `cplrt_enter_fn` and `cplrt_enter_act` compare the frame
  address (`__builtin_frame_address(0)`, behind a portable fallback that
  takes the address of a local) with `cx->stack_floor`; the frame address
  is used because the address of a local may lie in a heap-allocated fake
  frame under ASan's `detect_stack_use_after_return`. The floor is
  computed when the top-level evaluation begins: from the calling
  thread's stack bounds, cached in a `_Thread_local` record
  (`pthread_getattr_np` on Linux costs about 19 µs on the main thread, so
  it runs once per thread), or, when `cpl_call_options.stack_bytes` is
  non-zero, from the entry stack pointer and that size. The margin is the
  evaluator's: `min(GUARD_MARGIN, size / 8)` with `GUARD_MARGIN` 64 MiB
  (`crates/cpl-eval/src/stack.rs`). Tripping the guard is
  `resource_exhaustion` with `resource: "stack"`, `limit`, and `depth`
  (the call count), with the stack-guard message template, never a crash.
- **Floors.** A host that evaluates on a stack other than its thread's
  own (a fiber, a coroutine) passes `stack_bytes`, which must not exceed
  the stack remaining at the call. An evaluation that starts on a thread
  where an evaluation of the same runtime copy is active (a re-entrant
  export, Section 4.3) uses the higher of its own floor and that
  evaluation's. The floor is computed with saturating arithmetic (an
  entry stack pointer below `stack_bytes` gives the stack end 0), and so
  is the deadline of Section 3.6 (a budget whose deadline would overflow
  is no budget). When the thread's bounds cannot be queried (the query
  fails, as `pthread_getattr_np` can with `ENOMEM`), the export returns
  `CPL_E_NOMEM` with `interop_out_of_memory` before any CPL frame exists,
  and the failure is not cached.
- **Threads.** The driver and the native executable evaluate on a pthread
  with 1 GiB reserved; a C host runs evaluations on its own threads, so a
  host that needs depth 10,000 under ASan runs them on a thread it creates
  with a large stack.

Decision: [D-301] C15. Native code checks the call-depth limit, the stack
guard, the time budget, and cancellation at exactly the points where the
evaluator checks the first three, in its order, with the evaluator's
guard margin and per-thread cached stack bounds. Rationale: the order
decides which fault a refused call reports and whom it blames (semantics
S38, S72), so it must be the evaluator's; caching the bounds per thread
keeps the cost of a top-level evaluation small. Rejected alternatives:
relying on the host stack or on guard pages (a crash instead of the fault
S38 requires); querying the thread's bounds on every call (about 19 µs on
the main thread).

### 3.5 Holes

A development build compiles `?name` to `cplrt_hole(cx, CPL_H<n>,
visible, n)`: the fault `unresolved_hole` with `hole`, `expected_type`
(rendered with type parameters by name), `bindings` with their values,
innermost first, the hole's span, and the enclosing declaration as
`symbol` (the evaluator's `raise_hole`). Toolchain: `cpl build --release`
applies the release rules of semantics S47 first (toolchain 7.2, T72), so
no function-body hole reaches a release artifact (`hole_in_release`). Test
and property bodies exist only in drivers. `cpl_library_info.holes` counts
the holes linked.

### 3.6 The time budget and cancellation

- `cpl run` and the C API default to no budget; `cpl test --timeout-ms`
  and `cpl_call_options.time_budget_ms` set one for each top-level
  evaluation, for exactly the evaluations toolchain 7.2 budgets (the
  runner's `Budget::Fresh` and `Budget::None`,
  `crates/cpl-eval/src/interp.rs`, travel in the driver protocol, Section
  3.8). Through the C API, the deadline is set when the checks of Section
  5.5 step 3 begin and covers them and the call.
- When a budget is active, the monotonic clock is read at exactly the
  evaluator's points: every activation entry and before every loop
  condition, never inside a builtin or a `forall`. Without a budget and a
  cancellation token the check is one predictable branch. An expired
  budget is `resource_exhaustion` with `resource: "time"` and `limit` the
  budget in milliseconds, raised at the activation's call site or at the
  loop condition.
- A cancellation token (`cpl_call_options.cancel`, Section 4.8) is polled
  at the same points with a relaxed atomic load. A request unwinds the
  evaluation through the fault path, releasing every slot, and the export
  returns `CPL_E_CANCELLED` (`interop_cancelled`). A cancellation unwind
  passes every claim, result validation, substitution, and violation
  record unchanged: `cplrt_claim`, `cplrt_claim_range`, and the validation
  of Section 6.4 test `cx->cancelled` first and return. Cancellation is a
  host request, not a CPL fault: no `cpl run` or `cpl test` can observe
  it, so it takes no part in equivalence, and it never implies rollback
  (design line 459). A capability callback can see the request with
  `cpl_call_cancelled` (Section 5.7).

### 3.7 `check_contracts = false`

Every contract region of emitted code is guarded by the runtime flag
`cx->check_contracts` (Section 3.3). With the flag off these are skipped:
function and operation `requires` and `ensures`, loop invariants, both
kinds of `decreases`, and the `Refine` re-checks. These still run: builtin
preconditions (stdlib.md S39), the boundary checks (export arguments,
capability results, `cpl run` arguments), and `check<R>`. This is
`EvalConfig::check_contracts` exactly.

The C API refuses `CPL_CALL_UNCHECKED_CONTRACTS` with `CPL_E_UNSUPPORTED`
(`interop_option_unsupported`) in edition 0.1. Only the toolchain-internal
driver accepts the unchecked mode (`configure` with `check_contracts:
false`, Section 3.8), so the interpreter corpus runs in both modes on both
engines and Milestone 5 inherits a tested switch.

### 3.8 The native engine: the driver and cpl-driver protocol 1

A driver is the program's emitted C compiled with `-DCPL_WITH_TESTS=1`,
the runtime, every backend-`c` adapter of the project with its vendored
sources, and `cpl_rt_driver.c`. `cpl_emit_c::package::driver` builds it
at the driver path of Section 1.6, keyed by everything it is built from,
in the debug profile, with the sanitizers for the sanitizer suite, and
with the coverage counters of Section 1.4 under `-DCPLRT_COVERAGE=1` for
the coverage gate. It evaluates on a thread with 1 GiB of stack.

**Protocol.** One compact JSON request per LF-terminated line on stdin,
one response line on stdout; stderr carries only crash and sanitizer
reports. Objects are read by key, so key order is not part of the
protocol. The driver's first line is `{"protocol": 1, "runtime": "0.1.0",
"api": "0.1", "program": "<64 hex>"}`, and the client refuses a program
digest other than the one it built. A `def`, a `file`, and a position are
the program's own `DefId.0`, `FileId.0`, and byte offset.

| Request | Response |
| - | - |
| `{"op": "configure", "max_call_depth": n, "time_budget_ms": n \| null, "check_contracts": b, "max_heap_bytes": n}` | `{"ok": null}`: the driver closes its runtime, releasing every capability it bound, and opens a new one with this configuration; a value still live in the old runtime (a leak) gives `{"error": E}` with `interop_runtime_in_use`. Handles are numbered from 0 again |
| `{"op": "bind", "binding": "<name>"}` | `{"handle": n}`: grants the compiled-in backend-`c` adapter of that binding (Section 6.1), in process or, from Milestone 3, with `isolation = "worker"` in a re-executed driver; and from Milestone 3 a backend-`python` binding in a Python worker (`spec/python.md` Section 6); handles are numbered from 0 in bind order. A grant that fails answers `{"error": E}` with `interop_bind_failed` (Section 6.2) and numbers no handle |
| `{"op": "workers", "python": "<path>", "env": {"KEY": "value", ...}}` | `{"ok": null}`: Milestone 3; the interpreter and the environment additions that later grants of Python workers use (`spec/python.md` 6.5); without it the driver uses `CPL_PYTHON`, else `python3`, and its own environment |
| `{"op": "bind_scripted", "capability": def, "label": "<label>", "responses": [...]}` | `{"handle": n}`: a scripted host. Each response is `{"op": "<name>", "args": [V...], "result": V}` or `{"op": "<name>", "args": [V...], "foreign_error": {"kind": "<ForeignErrorKind>", "message": "...", "details": [["key", "value"], ...]}}`; an operation call takes the first unused response whose `op` and `args` equal the call's (structural equality) and answers with its `result` (which may be ill-typed on purpose) or reports its foreign error; a call no response matches is the fault `internal_error` kind `binding_protocol` with `operation`, `binding` (the label), the operation call's span, and the `detail` that semantics S80 gives a scripted host without a response for the call. `label` is the handle's binding name (semantics 6.9); the host's results are validated as every Milestone 2 binding's are (Section 6.4) |
| `{"op": "call", "function": def, "args": [V...], "using": [n...], "check_arguments": b}` | `{"ok": V}`, `{"fault": R}`, or `{"error": E}`: one top-level evaluation of the function with no calling declaration; with `check_arguments` the refined positions of the arguments are checked first, as `cpl run` checks them (semantics S39 (a), S74) |
| `{"op": "run_body", "test": def, "binders": [V...], "using": [n...]}` | `{"ok": null}` or `{"fault": R, "violation": {"def": n, "fault": R} \| null}`; `using` (Milestone 3, default `[]`) holds the handles of the test's fixtures in parameter order (semantics S83) |
| `{"op": "failing_refinement", "refined": def, "value": V, "budget": "fresh" \| "none"}` | `{"ok": def \| null}` (the first refinement of the chain, root first, whose predicate is false) or `{"fault": R}` |
| `{"op": "range_bound", "property": def, "binder": i, "side": "lo" \| "hi", "clause": "<range text>"}` | `{"ok": V}` or `{"fault": R, "violation": ...}`; the runner computes the range text in Rust (`clause::range_text`, `crates/cpl-eval/src/test/clause.rs`) |
| `{"op": "cap_call", "handle": n, "operation": i, "args": [V...]}` | `{"ok": V}`, `{"foreign_error": F}`, `{"fault": R}` (a fault of the binding layer: `binding_protocol`, memory), or `{"error": E}`: the binding layer only, with no clauses and no validation, for the evaluator's `CAdapter` (Section 7.2). `F` is `{"kind": "<ForeignErrorKind>", "message": "...", "details": [["key", "value"], ...]}`: the reported kind; the final message (the adapter's, the one stdlib.md S45 gives a declarative binding, or the binding layer's own); and, in stdlib.md S45's order, every detail after the runtime's keys `adapter`, `capability`, and `operation`: the declarative pair, the adapter's own entries in the order it added them, and, for an `InvalidResult` of the binding layer, `pointer` and `expected_type`. The `CAdapter` prepends the runtime's three keys as `cplrt_cap_invoke` does (Section 6.5) |
| `{"op": "tables", "table": "<table>", "index": i}` | `{"ok": {...}}`: entry `i` of the compiled table `<table>` (`defs`, `files`, `sites`, `clauses`, `bsites`, `holes`, `refinements`, `types`, `exports`, or `grants` of `cplrt_program`, Appendix B.1) as an object with its struct's member names: strings as strings, integers as numbers, `CPLRT_NONE` and `NULL` as `null`, a pointer to a static value as `true`, arrays (the counted pointers) as arrays, nested structs as objects, function pointers left out; a `files` entry is its path. Without `index`, `{"ok": n}`: the table's length. The table-fidelity test reads every entry (Section 3.2) |
| `{"op": "coverage"}` | `{"ok": {"nodes": {"<HIR variant>": n, ...}, "builtins": {"<id>": n, ...}, "claims": {"<claim site>": n, ...}}}`: the counters of Section 1.4 summed by HIR variant, the calls of each builtin, and the claims by the kind of their claim site (`requires`, `ensures`, `header_decreases`, `invariant`, `loop_decreases`, `where`, `op_requires`, `op_ensures`, `range`) with the S70 path that finds a fault already claimed as `already_claimed`; a driver built without coverage answers `{"error": E}` with `interop_option_unsupported` (`option` `"coverage"`) |
| `{"op": "from_json", "type": "<qualified type text>", "json": "<text>"}` | `{"ok": V}` or `{"error": E}`: `cpl_value_from_json` (Section 4.8) of the text at the type that `cpl_type_lookup` finds (`interop_type_unknown`, `interop_json_invalid`), for the round-trip tests of the C reader (toolchain 12.6) |
| `{"op": "render", "value": V}` | `{"ok": V}`: the value with the C writer's renderings (`V` below) |
| `{"op": "stats"}` | `{"ok": {...}}`: the counters of the driver runtime's `cpl_stats` by member name, without `struct_size` and `reserved` (Section 4.5) |
| `{"op": "shutdown"}` | `{"ok": {...}}`: the counters of `stats`, taken after the driver dropped every handle it numbered (its grants and scripted hosts hold values until then), so that anything still live is a leak (rule 3 of toolchain 12.6); the driver then closes its runtime and exits 0 |

- **`V` (a value)** is a flat post-order node list, `{"nodes": [[tag,
  ...], ...]}` with the root last and children referenced by the index of
  an earlier node, so a shared child array is sent once and neither side
  recurses on value depth. The nodes: `["i", "<canonical decimal>"]`,
  `["t"]`, `["f"]`, `["s", "<text>"]`, `["y", "<lowercase hex>"]`, `["l",
  k...]` (a list's elements), `["r", def, k...]` (a struct's fields),
  `["e", def, variant, k...]` (an enum value's payload), `["w", def, k]`
  (a wrapper), and `["c", def, handle]` (a capability handle, in outputs
  only). An output value adds `"json"` and `"text"`: the C writer's value
  JSON and value text of it, as JSON strings.
- **`R` (a fault record)** is `{"kind": "<code>", "clause_kind"?: "...",
  "resource"?: "...", "message": "...", "symbol"?: "<stable id>",
  "facet"?: "contract", "span"?: [file, start, end], "backtrace":
  [[def, file, start, end] | [def], ...], "data": {...}, "diagnostic":
  "<S49 JSON>"}`, the fields of Section 3.3.1 innermost frame first;
  `data` holds the payload as the C runtime renders it, and `diagnostic`
  the C runtime's S49 rendering of the fault.
- **`E` (a usage error)** is the S49 object of an `interop_*` error
  (Section 4.9) or, when the driver's runtime hit `CPL_E_INTERNAL`, of
  `internal_error` kind `runtime_invariant` (Section 4.6); the runtime is
  then poisoned, and every later request but `configure`, which replaces
  the runtime, and `shutdown` answers `interop_runtime_poisoned`.
- **Malformed requests.** A line that is not JSON, an unknown `op`, a
  missing or ill-typed key, a binding name that no backend-`c` binding
  has, or a `def`, `handle`, `binder`, `side`, `operation`, table, or
  index out of range answers `{"error": E}` with `interop_json_invalid`
  (`pointer` the position of the offending key, `detail`), and the driver
  reads the next line.
- **Defaults and budgets.** Until the first `configure` the driver runs
  with `max_call_depth` 10,000, no time budget, contracts checked, and no
  heap budget. `call` and `run_body` are each one top-level evaluation
  with a fresh deadline (the evaluator's `Budget::Fresh`); with
  `check_arguments` each refined position is first checked in an
  evaluation of its own with a fresh deadline, as `run_function` checks it
  (`crates/cpl-eval/src/run.rs`); `range_bound` runs with no deadline even
  when `configure` set a budget (`Budget::None`), as the evaluator's
  range bounds do; and `failing_refinement` uses its `budget`.

Toolchain: `cpl_cabi::fault_from_record(p, &record)` rebuilds the
evaluator's `cpl_eval::Fault` from `R`: the kind from `kind`,
`clause_kind`, and `resource`, the symbol by `StableId::parse`, spans and
frames from the indices, and `data` parsed on the evaluation thread with
`serde_json`'s recursion limit disabled (the feature `unbounded_depth`,
toolchain T71), since fault payloads can be as deep as the values they
hold. From there the CLI and the runner use the evaluator's
`to_diagnostic`, human renderer, and envelopes. The `diagnostic` string is
compared separately, byte for byte, because it is what C hosts see.

Decision: [D-303] C17. The native engine is a driver process that runs
the program's emitted C with the runtime and speaks cpl-driver protocol 1:
line-delimited JSON requests and responses, values as post-order node
lists, faults as records the toolchain rebuilds as `cpl_eval::Fault`.
Rationale: out of process, a crash in emitted code or an adapter cannot
take the test runner or `cpl` down, a sanitized driver can serve an
uninstrumented test process (an ASan-instrumented shared library cannot be
loaded into one), and the toolchain needs no `unsafe` FFI; node lists keep
both sides free of recursion on value depth and send a shared value once;
a round trip costs about 20 µs. Rejected
alternatives: loading the package into the toolchain with `dlopen`
(`unsafe` FFI, a native crash kills `cpl`, no sanitized builds); comparing
only codes or parsed JSON (it misses the fields agents act on and the C
writer's escaping and key order, which are where two implementations
drift).

### 3.9 The differential suites

Toolchain: the differential suites D1-D8, their comparison rules and
normalization, the coverage gate, the gate rule, the seeds and budgets,
and the skip policy are toolchain 12.6 (T78). The native side supplies
what they compare and count: the C writer's value JSON and value text of
every output value and the C runtime's S49 rendering of every fault (`V`
and `R`, Section 3.8), which the suites compare byte for byte with the
Rust renderings of the same outcome because they are what a C host
receives; the counters of the coverage drivers (Section 1.4, the
`coverage` request); the compiled tables (the `tables` request), which
the table-fidelity test compares with the program (Section 3.2); and, at
`shutdown`, a runtime with no live handle, capability, buffer, or object
(the `stats` request).

Decision: [D-304] C18. The native side renders every outcome it produces
itself, in C, and supplies the coverage counters, the compiled tables,
and a clean shutdown that the differential suites of toolchain 12.6
compare and count. Rationale: S75 is a byte-level statement, and a C host
receives the C runtime's renderings rather than a Rust rendering of a
rebuilt outcome, so only the C renderings test the C writer's escaping
and key order; a coverage gate needs counts of the lowered forms,
builtins, and claim sites that executed, which only the compiled code can
produce, and the table-fidelity test must read the tables back from the
compiled program to cover what was compiled. Rejected alternatives:
rendering native outcomes only through the toolchain's Rust code after
`fault_from_record` (the C writer, on which every C host depends, would
go untested); deriving coverage from the compared outcomes alone (they
do not show which lowered forms and claim sites executed); comparing in
process (sanitized code cannot be loaded into the test process, and a
native crash would kill the runner).

## 4. The versioned C API

### 4.1 Headers and dispatch

| Header | Source | Contents |
| - | - | - |
| `cpl.h` | `crates/cpl-rt/c/include/cpl.h`, identical in every package | C API 0.1: statuses, handles, options, `struct cpl_api`, and one `static inline` wrapper per function of Section 4.8 (Appendix A.1) |
| `cpl_adapter.h` | `crates/cpl-rt/c/include/cpl_adapter.h` | `CPL_ADAPTER` (hidden visibility) and the adapter helpers of Section 6.2; includes `cpl.h` (Appendix A.2) |
| `<prefix>.h` | generated | the package: `<prefix>_runtime_open`, `<prefix>_library_info`, the type accessors, the field and variant constants, the typed capability operations, the grants, the exports (Section 5.3) |
| `<prefix>_adapter_<binding>.h` | generated | the functions a reviewed wrapper must define (Section 6.2) |

**Dispatch.** `cpl_runtime`, `cpl_call`, and `cpl_error` are opaque except
for a public first member, `const cpl_api *api`. `cpl.h` defines `struct
cpl_api` in full: `uint32_t struct_size` and `uint32_t api_version`
followed by one function pointer per function `cpl.h` declares (those of
Section 4.8 but the two package functions), named like the function, in
the order of Appendix A.1, append-only within a major version. Every such
function is a `static inline` wrapper that calls the entry of the same
name through the table of the object it takes, so a call always reaches
the runtime copy that owns the object, whichever package's header the host
included. A wrapper for an entry added in a later minor version first
checks the library's table with `CPL_API_HAS(api, member)` and returns
`CPL_E_UNSUPPORTED` when the table is shorter; every entry of 0.1 is
present in every library. A package therefore exports only its own
`<prefix>_*` symbols and never a global `cpl_*` symbol, so two packages,
and two runtime versions, coexist in one process. Because the wrapper
reads the object's first member, a NULL `cpl_runtime *`, `cpl_call *`, or
`const cpl_error *` is undefined behaviour (`cpl_error_release(NULL)`
excepted). Toolchain: the entry order is frozen in
`crates/cpl-rt/api_v0.txt`, and a test fails when `struct cpl_api`
reorders or removes an entry, or adds one without raising
`CPL_API_VERSION_MINOR` (toolchain 12, T78).

### 4.2 Versioning and the compatibility rule

- **Compile time.** `cpl.h` defines `CPL_API_VERSION_MAJOR` 0,
  `CPL_API_VERSION_MINOR` 1, and `CPL_API_VERSION` (`major << 16 |
  minor`). Every `<prefix>.h` begins with `#if CPL_API_VERSION_MAJOR != 0
  || CPL_API_VERSION_MINOR < 1` and an `#error`, so a translation unit
  that combines a package header with an older `cpl.h` fails to compile.
  `<PREFIX>_ABI` gives the package ABI major (Section 9).
- **Run time.** `<prefix>_library_info()` works before anything is opened
  and reports the API version, the ABI, the digests, the target, and the
  compiler. `<prefix>_runtime_open(options, &rt, &error)` checks
  `options->struct_size` and `options->api_version`.
- **Rule.** The majors must be equal, and the library's minor must be at
  least the requested minor. Every options, table, and statistics struct
  (`cpl_runtime_options`, `cpl_allocator`, `cpl_call_options`,
  `cpl_cap_ops`, the typed operation structs of Section 5.3, `cpl_stats`,
  and the library's own `cpl_library_info` and `struct cpl_api`) begins
  with `uint32_t struct_size` and grows only at its end, and `struct_size`
  and `api_version` are the first two members of `cpl_runtime_options` in
  every major version, so that a library of any major reads them safely.
  A member counts as covered only when `struct_size` is at least its
  offset plus its size (a `struct_size` of 12 covers half of `allocator`,
  which then takes its default). For a struct the host passes in, a
  `struct_size` smaller than the library's own is accepted when it covers
  the struct's required members (for the options structs, the members
  before `allocator` or `time_budget_ms`: 8 bytes; for `cpl_allocator`,
  `cpl_cap_ops`, and a typed operation struct, every member of 0.1 but
  `drop`), and the members it does not cover take their defaults; a
  `struct_size` below that minimum, or above the library's own size, is
  `interop_struct_size_unsupported`. For a struct the library fills or
  owns, each side reads only what both know: `cpl_runtime_stats` writes
  only the members that the host's `struct_size` covers, and a host reads
  a member of `cpl_library_info` or of `struct cpl_api` only when that
  struct's `struct_size` covers it (`CPL_API_HAS` for the table). Major 0
  lasts through edition 0.1.
- **Newer libraries.** A host built against one minor version runs
  against a library of a later one, which may return statuses and kinds
  that the host's `cpl.h` does not name: the host treats an unknown
  status as a failure that carries an error object (when `error` is not
  NULL), and a value of an unknown `cpl_kind` as one it can only pass on,
  duplicate, compare, render, and release.
- **Mismatch.** `CPL_E_VERSION` with `interop_version_mismatch` (`library`
  and `requested` as `{"major", "minor"}`) or
  `interop_struct_size_unsupported` (`struct`, `size`, `minimum`,
  `maximum`), and no runtime is created. Toolchain: the version tests
  drive this through the options' fields, since redefining with `-D` a
  macro that the header defines unconditionally changes nothing
  (toolchain 12, T78).
- **Package ABI.** Section 9.

Decision: [D-305] C19. The C API is versioned by a major and a minor in
`cpl.h`, checked at compile time by every generated header and at run
time by `<prefix>_runtime_open`, and dispatched through the function table
the API's objects carry as their first member, behind `static inline`
wrappers, so that no package exports a global `cpl_*` symbol; the table
and every size-prefixed struct grow only at their end within a major
version.
Rationale: per-package runtime copies are what lets two packages, or two
versions of one, live in one process (Section 1.6), and only object-carried
dispatch reaches the copy that owns an object whichever header a host
compiled against; size-prefixed structs let an older host run against a
newer library of the same major. Rejected alternatives: global `cpl_*`
symbols exported by every package (two packages interpose on each other,
and with static archives one runtime serves both); version checks through
macros alone (a host compiled against one header and run against another
library would go undetected); a separate version query the host must
remember to call (a host that forgets it runs against an incompatible
table).

### 4.3 Runtime lifecycle and evaluation contexts

1. **Open.** `<prefix>_runtime_open(options, &rt, &error)` creates a
   runtime: its handle table, a copy of its allocator
   (`*options->allocator`, whose contract is Section 4.7, or the C
   library's `malloc` and `free`), its heap budget, its capability
   registry, the immortal constants of the program, and its default
   `max_call_depth`. A process may open many runtimes of one package and
   runtimes of many packages. With `options` NULL every default applies
   (`CPL_RUNTIME_OPTIONS_INIT`).
2. **Call.** Every export takes `cpl_runtime *rt` and an optional `const
   cpl_call_options *options` (NULL: the defaults of
   `CPL_CALL_OPTIONS_INIT`). The export creates the evaluation context
   (`cplrt_cx`, Appendix B: the depth counter, the frame chain, the fault
   slot, the attribution flag, the deadline, the cancellation token, the
   stack floor, the preallocated memory-fault record) on its own C stack,
   runs one top-level evaluation, and destroys it before it returns. There
   is no context object to create, share, or lock.
3. **Close.** `cpl_runtime_close(rt, 0, &error)` returns `CPL_E_STATE`
   with `interop_runtime_busy` (`active_calls`) while calls are active,
   and with `interop_runtime_in_use` (`live_handles`, `live_caps`,
   `live_buffers`) while handles, capabilities, or buffers are live;
   nothing is freed then. `cpl_runtime_close(rt, CPL_CLOSE_FORCE, &error)`
   also refuses while calls are active; otherwise it first runs every
   outstanding `drop` and adapter `fini`, each exactly once and in reverse
   order of creation, while the runtime is still fully usable (Section
   5.7), and only then releases every live handle, frees every live
   buffer, and frees everything else. After a successful close `rt` and
   every handle and buffer it issued are invalid, and using one is
   undefined behaviour. Error objects stay valid after the close; the
   allocator the runtime was opened with must outlive them (Section 4.7).
   Error objects and their views, `cpl_library_info`, and the views of
   `cpl_type_name` are valid only while the library that produced them is
   loaded. A poisoned runtime (Section 4.6) can still be closed.

**Re-entrancy.** Semantics: S79 makes every evaluation a capability
callback starts a fresh top-level evaluation. A callback may call any
runtime function and any export of any package, the running one included;
each export call creates its own context as step 2 says: no calling
declaration (a top-level `requires` violation carries no `caller`), its
own depth counter, its own attribution flag, its own deadline and token
from its own options. Its outcome is returned to the callback (a
`CPL_FAULT` with its own error object), never merged into the outer
evaluation, which sees only what the callback returns (Section 5.7). The
stack guard still applies, because the bounds are per thread: an
unbounded recursion through export, callback, and export ends in
`resource_exhaustion` (`stack`), not in a crash.

Decision: [D-306] C20. A runtime is the object that owns handles,
allocation, the heap budget, and capabilities; every export creates the
evaluation context of its call on its own C stack and destroys it before
returning, so that no context object exists in the API; closing refuses
while calls are active and, without `CPL_CLOSE_FORCE`, while handles,
capabilities, or buffers are live. Rationale: a per-call context makes
re-entrance a fresh top-level evaluation by construction (semantics S79),
needs no locking or thread affinity, and keeps the API's object count at
three; refusing to close under a running call is the only safe answer,
since the call's frames are on another stack. Rejected alternatives:
explicit context objects (`cpl_ctx`, one more object to create, lock, and
migrate between threads, with a busy flag instead of a structural
guarantee for re-entry, and nothing the per-call context lacks); a forced
close that frees under running calls (undefined behaviour the API could
not report).

### 4.4 Threading rules

| Object | Rule |
| - | - |
| `<prefix>_library_info`, `<prefix>_runtime_open` | any thread, concurrently |
| `cpl_runtime` | thread-safe: any thread may call any function and any export at any time, concurrently; `cpl_runtime_close` must not race with another use of the runtime |
| values | immutable, with atomic reference counts; one value may be reached from any number of threads through different handles |
| one handle | may be used from several threads; releasing it concurrently with another use of the same handle is a host error that cannot corrupt memory, since every lookup retains the object under the handle-table lock, and the loser gets `CPL_E_ARGUMENT` (`interop_handle_released`) |
| views (`cpl_strview`, `cpl_bytesview`) | readable from any thread until the handle they came from is released |
| `cpl_error` | owned by its receiver; may move between threads; never shared concurrently |
| `cpl_buf` | owned by its receiver; released with the runtime it came from |
| capabilities | usable by concurrent calls; a callback runs synchronously on the thread of the call that invoked it, so a `user` shared across threads must be thread-safe, as an adapter's `state` must be (Section 6.2) |
| `cpl_call` | usable from any thread until its callback returns, but by one thread at a time: the host serializes the uses of one `cpl_call` and completes them before the callback returns |
| a callback's borrowed arguments (handles and spans) | readable from any thread until the callback returns |
| `cpl_allocator` | its functions are called concurrently from every thread that uses the runtime (Section 4.7) |
| `cpl_cancel` | `cpl_cancel_request` may be called from any thread (it sets an atomic flag); it takes the handle-table mutex to find the token, so it is not async-signal-safe: a signal handler sets a flag of the host's own, and a thread of the host then calls `cpl_cancel_request` |

The runtime starts no threads in edition 0.1. The handle table is guarded
by one mutex per runtime; sharding it later changes no API, since handles
are opaque.

Decision: [D-307] C21. A runtime is thread-safe for every function and
export; values are immutable and shared freely; a callback runs on the
calling thread; the runtime starts no thread. Rationale: the hosts of
Milestones 3 and 4 call from many threads (free-threaded CPython, the
libuv pool behind Node's Promises), and immutable values with atomic
counts and per-call contexts make thread safety cost one mutex around the
handle table. Rejected alternatives: single-threaded runtimes (every
binding layer would have to serialize calls); one context per thread
(thread-local state that re-entrant callbacks and pooled threads would
have to manage).

### 4.5 The value-handle model

- **Handles.** `cpl_value`, `cpl_cap`, and `cpl_cancel` are 16-byte
  structs `{ uint64_t bits[2]; }`, owned by the receiver, never pointers.
  `bits[0]` holds a 32-bit slot and a 32-bit generation of the issuing
  runtime's handle table; `bits[1]` holds the identity of the issuing
  runtime: the address of its *identity cell*, 16 bytes that
  `<prefix>_runtime_open` allocates with the C library's `malloc` and
  never frees (the cell stays on a static list of its runtime copy, so
  LeakSanitizer does not report it). No two runtimes of one process, open
  or closed, of one package or of several, share an identity, so a handle
  of a closed runtime passed to a live one is `interop_handle_foreign`;
  passing anything to a closed runtime itself remains undefined behaviour
  (Section 4.3). `{{0, 0}}` is "no value". A function that takes a
  handle checks it in this order: `{{0, 0}}`
  (`interop_handle_invalid`, except that the three release functions
  accept it as a no-op), the identity (`interop_handle_foreign`), the slot
  (`interop_handle_invalid`), and the generation
  (`interop_handle_released`), each with `parameter` the C parameter's
  name, so a handle of another runtime, of another package, or a released
  one is a defined `CPL_E_ARGUMENT` that changes nothing. A NULL output
  pointer is `interop_null_pointer`.
- **Release.** `cpl_value_release` frees the slot and drops the reference;
  a second release, or any use after release, is
  `interop_handle_released`. A slot whose 32-bit generation would wrap is
  retired and never reused (16 bytes of table per 2^32 uses of one slot),
  and free slots are reused in first-in, first-out order, so a released
  handle is refused for the life of the runtime. A borrowed handle (a
  callback's argument, Section 5.7) cannot be released:
  `interop_handle_invalid`.
- **Constructors** copy host memory in: `cpl_text_new` validates strict
  UTF-8 (`interop_utf8_invalid` with `offset`, the `valid_up_to` of the
  first invalid sequence), `cpl_bytes_new` copies, `cpl_bool_new` accepts
  only 0 and 1 (`interop_bool_out_of_range`), and `cpl_int_from_decimal`
  accepts stdlib.md S25's grammar. Composite constructors retain their
  child handles (the caller keeps and still releases its own) and check
  the arity (`interop_arity_mismatch`), the variant index
  (`interop_variant_out_of_range`), and each child's shallow kind and
  declaration against the declared member type, a type parameter
  accepting any value (`interop_kind_mismatch`). Deep types and
  refinements are checked once, when a value enters CPL (export
  arguments, Section 5.5; capability results, Section 6.4), so building a
  nested value stays linear.
- **Accessors** (`cpl_list_get`, `cpl_struct_field`, `cpl_enum_payload`,
  `cpl_wrapper_inner`) return new owned handles; `interop_kind_mismatch`
  and `interop_index_out_of_range` report misuse.
- **Views.** `cpl_text_view` and `cpl_bytes_view` return borrowed pointers
  into the immutable object, valid until the handle they came from is
  released. A text view is followed by a NUL byte for convenience, but
  `len` is authoritative (U+0000 is a scalar).
- **Buffers.** The functions that produce text (`cpl_value_to_json`,
  `cpl_value_to_text`, `cpl_int_to_decimal`) return an owned `cpl_buf`,
  NUL-terminated after `len` bytes, released with `cpl_buf_release`; a
  live buffer, like a live handle, keeps `cpl_runtime_close` from closing
  (Section 4.3).
- **Leak accounting.** `cpl_runtime_stats` reports the live handles (value
  handles and cancellation tokens), the live capabilities, the live heap
  objects (immortals excluded), the bytes of Section 2.4 live and at their
  peak, the active calls, and the live buffers, so tests assert zero
  without depending on LeakSanitizer.

Decision: [D-308] C22. A handle is 16 bytes: a slot and a generation of
the issuing runtime's handle table and the identity of that runtime, and
every use checks all three before it touches anything. Rationale: each
package carries its own runtime copy, so a tag drawn from a per-copy
counter collides between packages (two copies both hand out tag 1) and
cannot detect a foreign handle, while the address of an identity cell that
is never freed is unique for the life of the process, among the runtimes
of all copies, closed ones included; a 16-byte struct of two 64-bit
integers is still passed in two registers on SysV AMD64 and AAPCS64, and
the generation, with every slot retired before its generation wraps, turns
a double release or a use after release into a defined error for the life
of the runtime. Rejected alternatives: 8-byte handles with a 16-bit
runtime tag (the tags collide across packages); the address of the
runtime object as the identity (the allocator hands it out again after a
close, so a stale handle of a closed runtime would pass as one of the next
runtime's); raw pointers as handles (a double release becomes a
use-after-free that only sanitizers might catch, while the acceptance
criterion asks for the failure paths to have defined outcomes).

Decision: [D-309] C23. Constructors copy host memory in and check shallow
shapes only; accessors return new owned handles; views borrow from the
immutable object until its handle is released; text the API produces is
an owned, NUL-terminated buffer. Rationale: copying keeps ownership simple
at the boundary ("Initial array and byte transfer copies data for clear
ownership", design line 451), shallow checks keep the construction of a
nested value linear while the deep check runs once where the value enters
CPL, and immutable objects make borrowed views safe without locking.
Rejected alternatives: deep checks in every constructor (quadratic for
nested values); views into host memory for constructed values (the host
would have to keep its memory alive for the value's lifetime).

### 4.6 Statuses and the error object

Every fallible function returns `cpl_status`. Its last parameter, `cpl_error
**error`, may be NULL. When it is not NULL and the status is neither
`CPL_OK` nor `CPL_ERR`, `*error` receives an error object owned by the
caller and released with `cpl_error_release`; on `CPL_OK` and `CPL_ERR` it
receives NULL. Under `CPL_E_NOMEM` it receives a static object whose
release is a no-op. When the error object of an outcome cannot be
allocated (the allocator fails, or the heap budget would be exceeded),
the function returns `CPL_E_NOMEM` instead, `*error` receives that static
`interop_out_of_memory` object, and the outputs are written as for any
failure; the one exception is a memory fault, which stays `CPL_FAULT` with
the static `resource_exhaustion` object of Section 4.7. The error object
exposes its status, a static `code` and `category`, a `message`, and the
full S49 diagnostic as compact JSON (`cpl_error_json`). A function without
an `error` parameter
(`cpl_value_release`, `cpl_cap_release`, `cpl_cancel_request`,
`cpl_cancel_release`, the `cpl_call_*` functions) reports by its status
alone.

| Outcome | Status | Carrier | Meaning |
| - | - | - | - |
| a value, or `Ok(v)` | `CPL_OK` | `*out` or `*ok_out` | success |
| a typed `Err(e)` of a `Result` return | `CPL_ERR` | `*err_out`, a value of `E`; no error object | the program's ordinary recoverable outcome (semantics 6.6): the design's "defined error status in C" (design line 594) |
| a fault | `CPL_FAULT` | `*error`: `code` one of the seven fault codes of semantics 6.7, `cpl_error_json` byte-identical to the evaluator's diagnostic (semantics S75) | the program faulted: a violated contract, a hole, a division by zero, an exhausted resource, an `internal_error`, or the `refinement_violation` of an export argument (semantics S81) |
| a usage error | `CPL_E_ARGUMENT`, `CPL_E_RANGE`, `CPL_E_STATE`, `CPL_E_VERSION`, `CPL_E_UNSUPPORTED` | `*error`: an S49-shaped object of category `interop` with an `interop_*` code (Section 4.9) | the host misused the API or passed an ill-typed value, and no CPL code ran; except `CPL_E_RANGE` on a result, where the call completed and its result does not fit the selected machine type |
| resources and the runtime | `CPL_E_NOMEM`, `CPL_E_CANCELLED`, `CPL_E_INTERNAL` | a static or owned error object: `interop_out_of_memory`, `interop_cancelled`, and for `CPL_E_INTERNAL` the code `internal_error` (category `internal`) with `kind` `runtime_invariant` and a `detail` | as named; after `CPL_E_INTERNAL`, a toolchain bug, the runtime is poisoned: every later status-returning function on it but the three release functions (`cpl_value_release`, `cpl_cap_release`, `cpl_cancel_release`) and `cpl_runtime_close` returns `CPL_E_STATE` (`interop_runtime_poisoned`) and changes nothing; `cpl_runtime_info`, `cpl_runtime_stats`, `cpl_buf_release`, the `cpl_call_*` functions, and the `cpl_error_*` functions keep working, and calls already in flight are not interrupted |

`CPL_FOREIGN_ERROR` (11) is only a capability callback's return value
(Section 5.7), and 12 is reserved for an asynchronous completion status
of a later minor version (`CPL_PENDING`). An error object of an
`interop_*` code has severity `error`, no span, no symbol, a message of
the runtime's wording (not a contract, semantics S49), and the payload of
Section 4.9.

**Outputs.** A function that fails writes `{{0, 0}}` to each handle
output, NULL to a `cpl_runtime **` output, and `{NULL, 0}` to a `cpl_buf`
output, and leaves its other outputs unchanged, except that
`cpl_int_to_words` and `cpl_list_to_i64` write `*needed` (and
`*negative`) whenever they return `CPL_OK` or
`interop_index_out_of_range`, which is how a host learns the size to
allocate. An export writes every output: on success those of Section 5.4,
and otherwise `{{0, 0}}` to each handle output and 0 to each scalar output
(`*ok_out` and `*err_out` included), so a host can release its outputs
unconditionally. The output pointers of one call must not alias one
another; otherwise the behaviour is undefined.

Decision: [D-310] C24. Every fallible function returns one of the
statuses of `cpl.h` and reports through an optional error object owned by
the caller: a fault with its S49 diagnostic, every misuse with an S49
object of the category `interop` and a code of Section 4.9; a typed `Err`
is a status of its own with its value in `*err_out`; and an export writes
every output, the null handle or 0 where the outcome has none. Rationale:
the design asks for "a defined status/error channel" (design line 469) and
for the `Err` of a `Result` to be "a defined error status in C" (line
594), and the same structured object a command reports is the failure
evidence a C host can act on; a category of its own tells a host's misuse
from a command line's; outputs that are always written let a host release
them on every path without tracking which call succeeded. Rejected
alternatives: an `errno`-style last error (invisible in signatures,
overwritten by nested calls in callbacks, and unsafe for thread-safe
functions); the category `usage` for C API errors (`usage` means that the
command line was wrong, toolchain Appendix B); `Err` as `CPL_FAULT` (a
recoverable outcome is no fault, semantics 6.6).

### 4.7 Allocation ownership, unwinding, out of memory

- **Ownership.** Every object the API hands out (runtime, value handle,
  capability handle, cancellation token, error object, buffer) is released
  through the function that owns it, which frees with the runtime's
  allocator: "An allocation must be released through the API that owns
  it" (design line 469). Hosts never `free()` runtime memory. An error
  object of `<prefix>_runtime_open`, which no runtime owns, is allocated
  with the C library's `malloc` and freed with `free` by
  `cpl_error_release`; the options' allocator is never called before every
  check of the options has passed.
- **The allocator.** The runtime copies `*options->allocator` at open, so
  the host's struct need not outlive the call. `alloc` and `free` may be
  called from any thread that uses the runtime, concurrently, and, to
  release error objects, after the runtime closed, so they must be
  thread-safe. `alloc` is never called with size 0; `align` is a power of
  two no greater than `alignof(max_align_t)`, and the result must be
  aligned to it (or NULL, a failure). `free` receives the size and the
  alignment of the allocation and is never given NULL. Both may be called
  while the runtime holds its handle-table mutex, so they must not wait for
  anything that a thread inside the API may hold (a host language's
  global lock, for one), must not call a function of the API, and must
  return normally. `user` and both functions stay valid until the runtime
  is closed and every error object it produced is released.
- **Host memory.** The runtime never frees host memory: spans passed in
  are copied, and `void *user` belongs to the host until the runtime calls
  `drop(user)` exactly once. `cpl_cap_new` and the typed constructors take
  ownership of `user` only on success.
- **No unwinding.** The runtime uses no `setjmp`, `longjmp`, or exceptions.
  A callback must return normally. A C++ adapter catches everything at its
  `extern "C"` entry and reports it with `cpl_call_foreign_error(...,
  CPL_FOREIGN_EXCEPTION, ...)`. Unwinding through the runtime is undefined
  behaviour, as the design requires ("Exceptions and language-runtime
  unwinding must not cross the C boundary", design line 469), and `cpl.h`
  says so in its banner: a C++ exception, a Rust panic, `longjmp` or
  `siglongjmp`, and also `pthread_exit` or a POSIX thread cancellation
  acting inside a callback, which glibc implements as a forced unwind that
  would skip every release and could leave the handle-table mutex locked;
  a host that uses thread cancellation disables it around its calls. The
  generated Python and Node layers of Milestones 3 and 4 convert their
  exceptions inside their callbacks.
- **Out of memory.** Every allocation is checked; the runtime never
  aborts. Semantics: S76 makes an allocation failure inside an evaluation
  (the allocator returned NULL, or `max_heap_bytes` would be exceeded) the
  fault `resource_exhaustion` with `resource: "memory"` and `limit` the
  heap budget in bytes (0 when none was set), blamed on the innermost
  declaration, with a possibly empty backtrace. It is built in the
  context's preallocated record (Section 3.3.1). The export then allocates
  the error object; if that fails too, `*error` is a static
  `resource_exhaustion` object without symbol or span. Outside an
  evaluation, allocation failure is `CPL_E_NOMEM`.

Decision: [D-311] C25. Everything the API hands out is released only
through the API, with the runtime's allocator (an error object of
`<prefix>_runtime_open`, which no runtime owns, with the C library's);
the allocator is a thread-safe, non-reentrant pair of functions that the
runtime copies at open and that outlives the runtime's error objects; the
runtime never frees host memory except through `drop` once, never unwinds
and must never be unwound through, and reports every allocation failure,
as a fault inside an evaluation and as `CPL_E_NOMEM` outside one.
Rationale: these are the design's rules for the C boundary (line 469), a
library that aborts on allocation failure takes its host down, which a
structured report avoids, and an allocator called from every thread that
uses the runtime, sometimes under its handle-table mutex, must say what it
may do there. Rejected alternatives: aborting on out of memory (simpler,
but it makes a library abort its host); exceptions or `longjmp` for faults
(unwinding must not cross the boundary, and a long jump skips releases);
the options' allocator for the error objects of `<prefix>_runtime_open`
(the checks that fail there are those of the very fields the allocator
would be read from); a promise never to allocate under the handle-table
mutex (growing the table needs it).

### 4.8 The functions of C API 0.1

Appendix A.1 is the normative declaration of every function. The rules of
Sections 4.5-4.7 apply to every row: handle arguments are checked in
parameter order, a NULL output pointer or a NULL input pointer with a
nonzero length is `interop_null_pointer`, a span of `const char *` must be
strict UTF-8 (`interop_utf8_invalid`), and an argument of the wrong kind
is `interop_kind_mismatch`. Its `expected` and `found` are kind words
(`int`, `bool`, `text`, `bytes`, `list`, `struct`, `enum`, `wrapper`,
`capability`, and, for a type id, `refinement`), except in a composite
constructor's shallow check of a member, where `expected` is the member's
declared qualified type text. A type id of another form than the function
needs (an enum's id given to `cpl_struct_new`, a struct's to `cpl_cap_new`,
a refinement's to `cpl_wrapper_new`) is `interop_kind_mismatch` with
`parameter` the id's parameter, `expected` the form needed, and `found` the
id's form; an id outside the type table is `interop_type_unknown`
(`type`).

| Function | Semantics |
| - | - |
| `<prefix>_library_info()` | the package's `cpl_library_info`, a static object valid while the library is loaded; callable before any runtime is open: `struct_size` its size, `api_version` `CPL_API_VERSION` of the library, `abi` the package ABI major, `release` 1 for a `cpl build --release` artifact, `holes` the unresolved holes linked, `reserved` 0, then the package name, prefix, edition, toolchain version, runtime version, the runtime, program, and exports digests (64 lowercase hex), the target, the compiler (`gcc 11.4.0`), and the profile (`optimized` or `debug`) |
| `<prefix>_runtime_open(options, out, error)` | Section 4.3; checks, in order: `out` (`interop_null_pointer`), `options->struct_size` against the minimum of Section 4.2, `options->api_version` (`interop_version_mismatch`), `options->struct_size` against the library's size, `options->flags` (`interop_option_unsupported`, `option` `"flags"`), and a non-NULL allocator's `struct_size` (as `options->struct_size` is checked, `struct` `"cpl_allocator"`), `alloc`, and `free` (`interop_null_pointer`) |
| `cpl_runtime_close(rt, flags, error)` | Section 4.3; flags other than 0 and `CPL_CLOSE_FORCE` are `interop_option_unsupported` |
| `cpl_runtime_info(rt)` | the `cpl_library_info` of the runtime's package |
| `cpl_runtime_stats(rt, out)` | writes the counters of Section 4.5 that `out->struct_size` covers (Section 4.2; the host initializes `*out` with `CPL_STATS_INIT`); `out` must not be NULL |
| `cpl_type_lookup(rt, name, len, out, error)` | the id of the type whose qualified type text (Section 5.3) is `name`: a well-known type of `cpl.h` or a type of the package's type table; else `interop_type_unknown` (`name`) |
| `cpl_type_name(rt, type, out, error)` | a view of the type's qualified type text, valid while the library is loaded; an unknown id is `interop_type_unknown` (`type`) |
| `cpl_value_dup(rt, v, out, error)` | a new handle to the same value |
| `cpl_value_release(rt, v)` | Section 4.5 |
| `cpl_value_kind(rt, v, out, error)` | the value's `CPL_KIND_*` |
| `cpl_value_type(rt, v, out, error)` | for a struct, enum, or wrapper value, the type id of its declaration (an instance of a generic declaration gives the declaration's id: values carry no type arguments); any other kind is `interop_kind_mismatch` |
| `cpl_value_equal(rt, a, b, out, error)` | 1 when the values are `==` (Section 2.8), else 0; values of different kinds or declarations are unequal |
| `cpl_value_to_json(rt, v, out, error)` | the value JSON of the value (semantics S40, toolchain 7.3), byte-identical to the evaluator's (Section 3.3.2) |
| `cpl_value_from_json(rt, type, json, len, out, error)` | the value that the strict reading of toolchain 7.3 decodes from the JSON text at `type`, a duplicate key included as a failure, and without a nesting limit; refined positions are decoded at their root base and their predicates are not evaluated (a value enters CPL only through the checks of Section 5.5); a JSON or decoding failure is `interop_json_invalid` (`pointer`, `detail`); a type with no value JSON (a capability, a declaration with type parameters) is `interop_kind_mismatch` |
| `cpl_value_to_text(rt, v, out, error)` | the value text of the value (semantics S40) |
| `cpl_buf_release(rt, buf)` | frees the buffer and sets it to `{NULL, 0}`; a `{NULL, 0}` buffer is a no-op |
| `cpl_int_from_i64`, `cpl_int_from_u64` | the `Int` of the machine integer |
| `cpl_int_from_decimal(rt, s, len, out, error)` | the `Int` of a text in stdlib.md S25's grammar (`("+" \| "-")? [0-9]+`); any other text is `interop_argument_type_mismatch` (`parameter` `"s"`, `pointer` `""`, `expected` `"Int"`, `found` the `quote_text` of the text) |
| `cpl_int_from_words(rt, negative, words, n, out, error)` | the `Int` whose magnitude is the `n` little-endian 64-bit `words` and whose sign is `negative` (0 or 1, else `interop_bool_out_of_range`); `n` 0 is 0, and a negative zero is 0 |
| `cpl_int_to_i64`, `cpl_int_to_u64` | the machine integer; a value outside the type is `CPL_E_RANGE` with `interop_int_out_of_range` (`value` the decimal text, `type` `int64` or `uint64`, `position` `"out"`) |
| `cpl_int_to_decimal(rt, v, out, error)` | the canonical decimal (Section 2.5) in a buffer |
| `cpl_int_to_words(rt, v, negative, words, cap, needed, error)` | `*needed` the number of 64-bit words of the magnitude (0 for 0) and `*negative` 1 for a negative value; with `words` NULL nothing else is written; otherwise the magnitude, little-endian, into `words`, and a `cap` below `*needed` is `interop_index_out_of_range` (`index` `*needed - 1`, `len` `cap`) with nothing written to `words` |
| `cpl_bool_new(rt, b, out, error)`, `cpl_bool_get(rt, v, out, error)` | `b` must be 0 or 1 (`interop_bool_out_of_range`, `parameter` `"b"`, `found`); the getter writes 0 or 1 |
| `cpl_text_new(rt, utf8, len, out, error)` | a `Text` copied from strict UTF-8 |
| `cpl_text_view`, `cpl_text_scalars` | Section 4.5; the scalar count is `Text.len` |
| `cpl_bytes_new`, `cpl_bytes_view` | a copied `Bytes`; Section 4.5 |
| `cpl_list_new(rt, items, n, out, error)` | a `List` of the `n` values, retained; elements are not checked against each other (a list's element type is checked when it enters CPL) |
| `cpl_list_len`, `cpl_list_get(rt, v, index, out, error)` | the length; element `index`, an `index` at or past the length being `interop_index_out_of_range` (`index`, `len`) |
| `cpl_list_from_i64(rt, items, n, out, error)` | a `List` of the `Int`s of the `n` machine integers |
| `cpl_list_to_i64(rt, v, buf, cap, needed, error)` | `*needed` the length; with `buf` NULL nothing else is written; otherwise every element is checked first (an element that is no `Int` is `interop_kind_mismatch`; one outside `int64_t` is `CPL_E_RANGE` with `interop_int_out_of_range`, `position` `"buf[<i>]"`; a `cap` below the length is `interop_index_out_of_range`) and only then are the elements written |
| `cpl_struct_new(rt, type, fields, n, out, error)` | a struct of the type (a struct declaration or an instantiation of a generic one) from its `n` fields in declaration order; `interop_arity_mismatch` (`type`, `expected`, `found`) for another `n`; each field's shallow check (`interop_kind_mismatch` with `parameter` `"fields[<i>]"`) |
| `cpl_struct_field(rt, v, index, out, error)` | field `index` (`interop_index_out_of_range`) |
| `cpl_enum_new(rt, type, variant, payload, n, out, error)` | an enum value of the type; `interop_variant_out_of_range` (`type`, `variant`) for a variant past the last; the payload's arity and shallow checks as for structs; a payload-less variant is the program's immortal static |
| `cpl_enum_variant`, `cpl_enum_payload` | the variant index; payload value `index` (`interop_index_out_of_range`) |
| `cpl_wrapper_new(rt, type, inner, out, error)`, `cpl_wrapper_inner` | a wrapper of the type around `inner` (shallow check against the wrapped type); the wrapped value |
| `cpl_unit`, `cpl_option_some`, `cpl_option_none`, `cpl_result_ok`, `cpl_result_err` | the prelude's values; `Unit` and `None` are immortal statics |
| `cpl_foreign_error_new(rt, kind, message, len, keys, values, ndetails, out, error)` | `ForeignError { kind, message, details }` with one `ForeignDetail { key, value }` per detail, in order; `kind` must be a `CPL_FOREIGN_*` value (`interop_variant_out_of_range`, `type` `std.foreign.ForeignErrorKind`); the message, keys, and values must be strict UTF-8 (`parameter` `"message"`, `"keys[<i>]"`, `"values[<i>]"`) |
| `cpl_cancel_new(rt, out, error)` | a new cancellation token, not requested |
| `cpl_cancel_request(rt, token)` | requests cancellation, permanently for that token, from any thread; every evaluation using the token observes it at its next check (Section 3.6) |
| `cpl_cancel_release(rt, token)` | releases the token's handle; a call in flight keeps its own reference until it ends |
| `cpl_cap_new(rt, capability, ops, user, out, error)` | Section 5.7: a capability object of the capability type over a generic operation table; checks `capability` (an id that is no capability type of the package is `interop_kind_mismatch` with `expected` `"capability"`), `ops` (`interop_null_pointer`), `ops->struct_size` (below the members before `drop`, or above the library's size: `interop_struct_size_unsupported` with `struct` `"cpl_cap_ops"`), `ops->nops` against the operation count (`interop_arity_mismatch`), and every entry of `ops->ops` (`interop_null_pointer`, `parameter` `"ops->ops[<i>]"`); copies the table |
| `cpl_cap_release(rt, cap)` | drops the host's reference (Section 5.7) |
| `cpl_call_runtime(call)`, `cpl_call_operation(call)`, `cpl_call_cancelled(call)` | the runtime of the running call; the operation's index in declaration order; 1 when the token of the running top-level call was requested, else 0 |
| `cpl_call_foreign_detail(call, key, key_len, value, value_len)` | appends a detail, in call order, to the foreign failure the callback may report, before or after `cpl_call_foreign_error`; `CPL_OK`, `CPL_E_ARGUMENT` (not strict UTF-8, or a NULL with a nonzero length), or `CPL_E_NOMEM` |
| `cpl_call_foreign_error(call, kind, message, len)` | records the foreign failure's kind and a copy of its message (a later call replaces both) and returns `CPL_FOREIGN_ERROR`, which the callback then returns; `CPL_E_ARGUMENT` for a kind that is no `CPL_FOREIGN_*` value or a message that is not strict UTF-8, and `CPL_E_NOMEM` when the message cannot be copied |
| `cpl_error_status`, `cpl_error_code`, `cpl_error_category`, `cpl_error_message`, `cpl_error_json` | the error object's status, its static NUL-terminated code and category, views of its message and of its compact S49 JSON, valid until the object is released; callable after the runtime closed |
| `cpl_error_release(e)` | frees the object; NULL and static objects are no-ops |

### 4.9 C API error codes

Category `interop`, severity error, span-less. They are the `code` of
usage-class error objects; only `interop_bind_failed` is also reported by
a command (`cpl run`, semantics S78). Toolchain: the table is also
`cpl_rtspec::interop`, from which the runtime's generated header takes it
(toolchain T70).

| Code | Status | Payload |
| - | - | - |
| `interop_version_mismatch` | `CPL_E_VERSION` | `library`, `requested` (`{"major", "minor"}`) |
| `interop_struct_size_unsupported` | `CPL_E_VERSION` | `struct`, `size`, `minimum`, `maximum` (the library's size) |
| `interop_handle_invalid` | `CPL_E_ARGUMENT` | `parameter` |
| `interop_handle_released` | `CPL_E_ARGUMENT` | `parameter` |
| `interop_handle_foreign` | `CPL_E_ARGUMENT` | `parameter` (a handle of another runtime or package) |
| `interop_null_pointer` | `CPL_E_ARGUMENT` | `parameter` |
| `interop_argument_type_mismatch` | `CPL_E_ARGUMENT` | `parameter`, `pointer` (RFC 6901), `expected`, `found` |
| `interop_utf8_invalid` | `CPL_E_ARGUMENT` | `parameter`, `offset` (`valid_up_to`) |
| `interop_bool_out_of_range` | `CPL_E_ARGUMENT` | `parameter`, `found` |
| `interop_int_out_of_range` | `CPL_E_RANGE` | `value` (decimal), `type` (`int64`, `int32`, `uint64`, `uint32`), `position` |
| `interop_index_out_of_range` | `CPL_E_ARGUMENT` | `index`, `len` |
| `interop_variant_out_of_range` | `CPL_E_ARGUMENT` | `type`, `variant` |
| `interop_arity_mismatch` | `CPL_E_ARGUMENT` | `type`, `expected`, `found` |
| `interop_kind_mismatch` | `CPL_E_ARGUMENT` | `expected`, `found` (the kind words of Section 4.8), and `parameter` when the value is an argument or an element of an argument array |
| `interop_type_unknown` | `CPL_E_ARGUMENT` | `name` or `type` |
| `interop_capability_mismatch` | `CPL_E_ARGUMENT` | `parameter`, `expected`, `found` (capability qualified names) |
| `interop_json_invalid` | `CPL_E_ARGUMENT` | `pointer`, `detail` (the strict reading of toolchain 7.3, duplicate keys included) |
| `interop_runtime_busy` | `CPL_E_STATE` | `active_calls` |
| `interop_runtime_in_use` | `CPL_E_STATE` | `live_handles`, `live_caps`, `live_buffers` |
| `interop_runtime_poisoned` | `CPL_E_STATE` | |
| `interop_option_unsupported` | `CPL_E_UNSUPPORTED` | `option` |
| `interop_out_of_memory` | `CPL_E_NOMEM` | |
| `interop_cancelled` | `CPL_E_CANCELLED` | |
| `interop_bind_failed` | `CPL_E_STATE` (from `<prefix>_bind_<binding>`; `cpl run`: exit 1) | `binding`; `kind` `"Unavailable"` when a declarative binding's `[state]` could not be created (stdlib.md S45); the adapter's own diagnostic, when it gave one, as a note |

## 5. Exports: a C host calls CPL

### 5.1 The export set

Toolchain: T74 (toolchain 11.1) defines the export set: when `[interop.c]
exports` is absent, every `pub fn` of the project's modules that is
neither generic nor a generator (a function whose declared return type is
a `Gen<T>`), each in its handle form with its default symbol; when it is
present, exactly its entries, each a target string or an inline table `{
function = "<target>", symbol = "<C name>", int = "<mapping>" }`, resolved
by toolchain 6.1 (T59) after name resolution; and the `project_export_*`
codes that reject an entry. With the default set the verbatim
`examples/reader` exports `reader_image_width`, `reader_load_page`,
`reader_load_text`, and `reader_paginate_ints` and skips the generic
`paginate` (`project_export_skipped_generic`, an info of `cpl build`).

Each entry of the export set is one C entry point: a function listed twice
(in its handle form and with `int = "int64"`, say) has two. The *export
order* is the manifest's order for an explicit list and the byte order of
the functions' qualified names for the default set; the generated header
(Section 5.3), the export table of the program tables, and `build.json`
list exports in it.

Decision: [D-312] C26. An export is one C entry point per entry of the
export set, for a `pub`, non-generic, non-generator function declared in
the project, under the symbol of Section 5.2; generated identifiers never
change silently (a collision is an error that names the colliding items,
and a parameter or member name that is no valid or no unique C name is
suffixed with `_`, or a reserved spelling prefixed with `x`, by the rule of
Section 5.2), and a generic function is
never exported (Section 5.6). Rationale: one
entry point per entry lets a project offer a function in its handle form
and in a machine-integer form side by side, as the design's "unless the
API explicitly selects a checked conversion to a machine type" (line 469)
needs; symbols derived from readable names keep the header readable, and
refusing collisions keeps every symbol a deliberate one, recorded in the
lock (Section 9). Rejected alternatives: module-path-mangled names
(`reader_pages__Page`: unreadable, and they still collide, `a_b.c`
against `a.b_c`); renaming a colliding symbol silently (a host compiled
against the old name would bind to another function); an `@export`
annotation (a language change to a closed annotation vocabulary for
project configuration, and the C name would become a facet of the
declaration).

### 5.2 Symbol naming and generated identifiers

| Item | Name | Notes |
| - | - | - |
| prefix | `[interop.c] prefix`, default `[project] name` | `^[a-z][a-z0-9]*(_[a-z0-9]+)*$`, at most 31 characters, not starting with `cpl` (toolchain 11.1): no `__` and no trailing `_`, which the namespacing of Section 1.6 relies on |
| export, handle form | `<prefix>_<function name>` (`reader_paginate_ints`) | an explicit `symbol` must match `^<prefix>_[a-z0-9_]{1,56}$` |
| package functions | `<prefix>_runtime_open`, `<prefix>_library_info` | reserved |
| type accessor | `cpl_type <prefix>_type_<name>(void)` | `<name>` is the snake case of the type's bare name (`page_id`), the snake cases of an instantiation's bare names in pre-order joined by `_` (`list_list_int`, `result_page_store_error`), or an alias from `[interop.c] type_names` |
| field and variant constants | `<PREFIX>_<NAME>_F_<FIELD>`, `<PREFIX>_<NAME>_V_<VARIANT>` | `<NAME>` the accessor's `<name>` in upper case; `<FIELD>` the field name and `<VARIANT>` the snake case of the variant name, in upper case; the values are declaration-order indices |
| capability | `<prefix>_<name>_ops` (the typed operation struct), `<prefix>_<name>_new` (its constructor) | `<name>` the snake case of the capability's bare name |
| grant of a backend-`c` binding | `<prefix>_bind_<binding>` | `<binding>` the name of the `[bindings.<binding>]` table, which must match `^[a-z][a-z0-9_]*$` |
| adapter contract | `<prefix>_adapter_<binding>_init`, `_fini`, `_<operation>` | hidden visibility (`CPL_ADAPTER`); never exported; `<operation>` the operation's name with `_` appended while it is `init`, `fini`, or the name that an earlier operation of the capability received (`init` gives `init_`) |
| internal | `cplrt_<prefix>__*` (runtime), `cplpkg_<prefix>__*` (emitted code) | hidden in the shared library, local in the static archive's object, and namespaced in every artifact and in a vendored package (Section 1.6) |

The snake case of a PascalCase name puts `_` before every upper-case
letter that follows a lower-case letter or a digit, and before an
upper-case letter that is followed by a lower-case letter and follows an
upper-case letter, then lowers every letter (`PageId` gives `page_id`,
`IOError` gives `io_error`).

The C names of parameters and members in generated declarations are the
CPL names, with `_` appended while a name is a keyword of C11 or C++17; a
C++ alternative token (`and_eq`, `bitand`, `bitor`, `compl`, `not_eq`,
`or_eq`, `xor`, `xor_eq`, which `<iso646.h>` also defines as macros in
C); an identifier that `<stddef.h>` or `<stdint.h>` declares or defines
(`size_t`, `ptrdiff_t`, `max_align_t`, `int8_t` to `uint64_t`, `intptr_t`,
...) or that begins with `cpl_`; a lowercase object-like macro of a C11
standard header (`errno`, `stdin`, `stdout`, `stderr`, `math_errhandling`,
`noreturn`, `complex`, `imaginary`, `alignas`, `alignof`, `bool`, `true`,
`false`, `static_assert`, `thread_local`) or one that a GNU-mode compiler
predefines (`linux`, `unix`); one of the fixed names of the declaration
(`rt`, `options`, `error`, `out`, `ok_out`, `err_out`, `user`, `call`,
`result`, `state`, and the members `struct_size`, `reserved`, and
`drop`); or the name of an earlier parameter of the same declaration. A
name that begins with `_` and an upper-case letter or a second `_`, which
C reserves, is first prefixed with `x` (`_Tmp` gives `x_Tmp`). The length
parameter of a span is the parameter's C name followed by `_len`, renamed
by the same rule. Without these renamings a parameter named `size_t`
would hide the type from the rest of the prototype, and one named `errno`
would be replaced by the macro's expansion.

Two generated identifiers that coincide (two exports of one name in
different modules, an export named like a type accessor or a capability
constructor, two types with one bare name, two adapter contract names of
different bindings, such as binding `x`'s operation `y_init` and binding
`x_y`'s `init`) are `project_export_symbol_collision` (`symbol`, `items`);
an explicit or defaulted export symbol in a reserved family is
`project_export_symbol_invalid` (`symbol`, `reason`); and a backend-`c`
binding whose name is outside its pattern is
`project_binding_name_invalid` (`binding`); `cpl check` reports all three
(toolchain 11.1 and Appendix B). The author resolves a collision with an
explicit `exports` entry with a `symbol`, with `[interop.c] type_names`,
or by renaming a binding. Nothing is renamed silently. The lock records
the symbol, the identity, and the signature digest of every export
(Section 9), so a rename that changes a defaulted symbol is visible under
`--release` (`build_abi_changed`).

### 5.3 The generated header

`<prefix>.h` consists of, in this order:

1. the banner: `/* <prefix>.h: generated by cpl <toolchain> (edition
   <edition>, C API <api>) for project "<name>"; do not edit.`, then
   ``Regenerate with `cpl build --target c`; the CPL sources and cpl.toml
   are the editing authority.``, then `runtime <digest>; program <digest>;
   exports <digest>; target <target> */`;
2. the include guard `<PREFIX>_H`, `#include "cpl.h"`, the version guard
   of Section 4.2, `extern "C"` for C++, and `#define <PREFIX>_ABI <abi>u`;
3. `/* ---- package ---- */` and the declarations of
   `<prefix>_library_info` and `<prefix>_runtime_open`;
4. `/* ---- types; ids are valid only with this package's runtimes ---- */`
   and one accessor per listed type, sorted by the types' qualified type
   texts in byte order, each preceded by its type comment and, for a
   struct or enum declaration, followed by its field or variant constants
   (an instantiation has its declaration's members, so it has none: for
   the prelude enums the `CPL_OPTION_*` and `CPL_RESULT_*` of `cpl.h`
   apply, and the declaration of every listed instantiation of a project
   type is listed);
5. `/* ---- capabilities ---- */` and, for every listed capability, in
   qualified-name order, its typed operation struct, the struct's layout
   assertion `CPL_LAYOUT_ASSERT(sizeof(<prefix>_<name>_ops) == <8 + 8 *
   (operations + 1)>u)` (Section 1.7), and its constructor;
6. `/* ---- grants ---- */` and, for every backend-`c` binding, in
   binding-name order, a comment and its grant;
7. `/* ---- exports ---- */` and every export in export order, each
   preceded by its export comment;
8. for the default set, one comment `/* not exported: <qualified name> is
   generic (project_export_skipped_generic) */` per skipped `pub fn`, in
   qualified-name order;
9. the closing of `extern "C"` and of the include guard.

A section with no entries is omitted with its heading. Every comment
stands on lines of its own before what it describes, and declarations use
single spaces. An export's prototype puts each group of its parameters on
a line of its own (`rt` with the positional and `using` parameters; the
outputs, when there are any; `options` with `error`), a capability
constructor's two groups (`rt`, `ops`, `user`; `out`, `error`), and every
other declaration is one line; continuation lines are aligned after the
opening parenthesis.

The *qualified type text* of a type is its CPL type syntax with every
declaration named as `$type` names it (toolchain T30: the bare name of a
prelude type, the qualified name of every other declaration): `List<Int>`,
`Result<reader.pages.Page, reader.pages.StoreError>`,
`reader.pages.Positive`; `cpl_sema::interop::type_text` (toolchain 1.6)
renders it for the plan and the emitter alike. The *listed capabilities*
are those that appear in an export signature or that a backend-`c` binding
binds. The *listed types* are every declaration reachable from an export's
or a listed capability's signature (transitively through fields and
payloads) and every instantiation that appears directly in one of those
signatures (a top-level `Result` return of an export is split into a
status and outputs and is not listed, but its payload types are), except
the well-known types of `cpl.h`. The package's type table holds the
well-known types (ids 1-11) and, each with every type nested in it, the
listed types, the parameter and return types of every operation of every
capability the program declares (Section 6.4 validates against them), the
root base of every refinement, and, in a driver, the parameter types of
every function (the `call` request of Section 3.8). The ids from 12 on are
assigned in byte order of the qualified type texts over that whole set,
and all ids are obtained only at run time, through the accessors, which
the header declares for the listed types only, and `cpl_type_lookup`, so
the larger table changes no host.

| Comment | Text |
| - | - |
| an instantiation | its qualified type text |
| a struct, an enum, a wrapper, a refinement | `struct <qualified name> { <field>: <type>, ... }`, `enum <qualified name> { <Variant>(<types>), <Variant>, ... }`, `type <qualified name>(<type>)`, `type <qualified name> = <type> where <predicate>`: the declaration's signature facet (toolchain 5.2) on one line, with the types as written and the members joined by `, `, the name in place of `_`, without `pub`, and for a refinement followed by its contract facet |
| a capability's accessor | `capability <qualified name>` |
| a capability's typed struct | `capability <qualified name>` before the struct; before `struct_size`, `sizeof(<prefix>_<name>_ops)`; before `reserved`, `padding; never read`; before each operation's member, three lines: the operation's line of the capability's signature facet (without its indentation), then its parameters as `<p>: borrowed for the call` (a handle), `<p>: a borrowed byte span, valid for the call` (`Bytes`), `<p>: a borrowed UTF-8 span, valid for the call` (`Text`), or `<p>: 0 or 1` (`Bool`), joined by `; ` (the line is omitted without parameters), then `on CPL_OK *result: an owned <return type>, validated by the runtime before CPL sees it`; before `drop`, `optional; called once, after the last reference is released and no call is in flight` |
| a grant | `[bindings.<binding>]: backend c, isolation in_process, adapter <adapter path>.` and `The manifest requests the binding; the host grants it by calling this function.` |
| an export | the qualified name followed by the function's signature facet after its `_`; then `identity <identity> (provisional)` (without the parenthesis for an `@id`) and `; effects { ... }` (the effects facet, `effects {}` when empty), with `; <mapping>` when the entry's mapping is not `handle`; then, when the function has span or handle parameters, a line of notes in parameter order: `<p> is copied in.` for a span and `<p> is borrowed.` for a handle (a `cpl_value` or a `cpl_cap`, Section 5.4); then the outcome (`CPL_OK: *ok_out is an owned <T>. CPL_ERR: *err_out is an owned <E>.` for a split `Result`, `CPL_OK: *out is an owned <T>.` otherwise; `is the value as <c type>` for a machine-integer output, `is 0 or 1` for a `Bool`, `CPL_OK: no output.` for `Unit`); then `CPL_FAULT and CPL_E_*: *error, when error is not NULL.` |

The generated header of the verbatim `examples/reader` is Appendix C.1;
`crates/cpl-emit-c/tests/golden/reader/include/reader.h` pins it with the
digests normalized (toolchain 12). Its binding `image_codec` has backend
`python`, so it has no grant. The fixture `cboundary` (toolchain 12)
shows the machine-integer form and a grant; an excerpt of its
`cboundary.h`:

```c
/* reader.pages.paginate_ints(items: List<Int>, size: Int) -> Result<List<List<Int>>, ConstraintError>
   identity reader.pages.paginate_ints (provisional); effects {}; int64
   items is borrowed.
   CPL_OK: *ok_out is an owned List<List<Int>>. CPL_ERR: *err_out is an owned ConstraintError.
   CPL_FAULT and CPL_E_*: *error, when error is not NULL. */
cpl_status cboundary_paginate_ints_i64(cpl_runtime *rt, cpl_value items, int64_t size,
                                       cpl_value *ok_out, cpl_value *err_out,
                                       const cpl_call_options *options, cpl_error **error);
/* [bindings.image_codec]: backend c, isolation in_process, adapter adapters/image_codec.c.
   The manifest requests the binding; the host grants it by calling this function. */
cpl_status cboundary_bind_image_codec(cpl_runtime *rt, cpl_cap *out, cpl_error **error);
```

Decision: [D-313] C27. The generated header lists, in a fixed order and
with comments derived from the facets, the package functions, one accessor
function per listed type with the field and variant constants of each
struct and enum declaration, the typed capability structs with their
layout assertions and constructors, the grants, and the exports; type
ids are values the accessors return at run time, never constants compiled
into hosts. Rationale: adding an export or a type renumbers sorted ids, so
ids compiled into hosts would silently break every host built against the
old header, while accessors bind by name; comments taken from the facets
show each export's CPL signature, identity, and effect bound at the point
of use, and a fixed order makes the header a reviewable golden. Rejected
alternatives: type ids as compile-time constants (they shift when a type
is added); hand-written comments in the generator (they could disagree
with the source they describe).

### 5.4 The value mapping

At the top level of an export signature and of a typed callback's
parameters; nested positions are always handles.

| CPL type | C parameter | C result | Design row (lines 493-502) |
| - | - | - | - |
| `Int` and refinements of `Int`, mapping `handle` (the default) | `cpl_value` | `cpl_value *out` | "Managed integer handle" |
| `Int` with `int = "int64"`, `"int32"`, `"uint64"`, `"uint32"` | `int64_t`, `int32_t`, `uint64_t`, `uint32_t` | `int64_t *out`, ... (the `Ok` payload of an `Int`-based `Result` too) | "checked machine integer": an input always fits an `Int`; a result outside the type is `CPL_E_RANGE` (`interop_int_out_of_range` with the decimal `value` and `position` the output's name); the handle form of the same function always returns it (stdlib.md S46) |
| `Bool` | `int32_t`, 0 or 1 (else `interop_bool_out_of_range`) | `int32_t *out` | "Defined integer representation" |
| `Text` | `const char *<p>, size_t <p>_len`: strict UTF-8 (else `interop_utf8_invalid` with `offset`), copied in; NULL only with length 0 | `cpl_value *out`, read with `cpl_text_view` | "UTF-8 byte span with length" |
| `Bytes` | `const uint8_t *<p>, size_t <p>_len`, copied in; NULL only with length 0 | `cpl_value *out`, read with `cpl_bytes_view` (valid until released) | "Byte span with explicit lifetime" |
| `Unit` | no parameter | no output | its one value |
| `List<T>` | `cpl_value` (deep-checked, Section 5.5) | `cpl_value *out`; bulk access with `cpl_list_from_i64` and `cpl_list_to_i64` | "managed handle" |
| struct | `cpl_value` | `cpl_value *out` | "opaque handle", with the generated field constants: no C struct layout is promised (the representation is not an ABI, design line 465) |
| enum, `Option<T>` | `cpl_value` | `cpl_value *out` | "Tagged representation": `cpl_enum_variant` and the generated `_V_` constants; `Some(None)` and `None` stay distinct |
| wrapper | `cpl_value` | `cpl_value *out` | an opaque handle (`cpl_wrapper_inner`) |
| refinement `R` of base `B` | as `B`, plus the boundary check of `chain(R)` (Section 5.5) | as `B`: CPL produced it, so nothing is checked | |
| `Result<T, E>` as the return type | n/a | `CPL_OK` with `*ok_out` mapped as `T`, or `CPL_ERR` with `*err_out` mapped as `E`; a `Unit` payload has no output | "Status plus result/error outputs" |
| `Result<T, E>` elsewhere | `cpl_value` | `cpl_value *out` | an ordinary enum |
| capability (`using`) | `cpl_cap`, after the positional parameters, in `using` order | n/a | Section 5.7 |

Handle and `cpl_cap` arguments are borrowed: the export retains what it
needs for the call, and the host keeps, and still releases, its handles.
Every export ends with `const cpl_call_options *options, cpl_error
**error`, and writes its outputs as Section 4.6 says.

Decision: [D-314] C28. Exports map CPL types to C as the table above says:
spans for `Text` and `Bytes` parameters, `int32_t` for `Bool`, handles for
`Int` unless the entry selects a checked machine-integer mapping, opaque
handles for every composite, and a status split for a top-level `Result`.
Rationale: this is the design's mapping table read for C (lines 493-502):
`Int` is "a managed integer handle or checked machine integer", text and
bytes are spans with a length, and a `Result` is "status plus
result/error outputs"; opaque handles keep the representation private
(line 465). Rejected alternatives: generated C structs mirroring CPL
structs (they freeze field layout into the ABI and need deep copies of
nested lists; the design allows "generated struct or opaque handle");
`Int` as `int64_t` by default (it narrows silently, while the design
requires an explicit selection); `Text` and `Bytes` parameters as handles
only (every C caller would build a handle first, while the design's
mapping is a span).

### 5.5 Boundary checks on entry

The API unit implements every export as a call of
`cplrt_export_call` (Appendix B) with the export's index into the export
table of the program tables, which describes each parameter's form and
type. For the verbatim `reader` (normative for its shape):

```c
/* reader_api.c (excerpt): the package functions, a type accessor, a capability constructor, an export */
CPLRT_EXPORT const cpl_library_info *reader_library_info(void)
{
    return &CPLPKG(program).info;
}
CPLRT_EXPORT cpl_status reader_runtime_open(const cpl_runtime_options *options, cpl_runtime **out,
                                         cpl_error **error)
{
    return cplrt_runtime_open(&CPLPKG(program), options, out, error);
}
CPLRT_EXPORT cpl_type reader_type_list_int(void)        /* List<Int> */
{
    return CPL_X12;
}
/* reader.pages.PageRead: the binding layer over the typed ops struct */
static cpl_status cpl_c61_invoke(const cplrt_capref *cap, cpl_call *call, uint32_t op, const cplrt_w *args,
                                 cplrt_w *result)
{
    const reader_page_read_ops *ops = (const reader_page_read_ops *)cplrt_cap_ops(cap);
    cpl_value r = { { 0, 0 } };
    cpl_status st;
    switch (op) {
    case 0:                                             /* fn get(id: PageId) -> Result<Page, StoreError> */
        st = ops->get(cap->user, call, cplrt_call_borrow(call, args[0]), &r);
        break;
    default:
        return CPL_E_INTERNAL;
    }
    return cplrt_call_take(call, st, r, result);        /* the callback contract of 5.7 */
}
static void cpl_c61_drop(const cplrt_capref *cap)
{
    const reader_page_read_ops *ops = (const reader_page_read_ops *)cplrt_cap_ops(cap);
    if (ops->drop != NULL)
        ops->drop(cap->user);
}
static const cplrt_capimpl cpl_c61_impl = { 1, cpl_c61_invoke, cpl_c61_drop };
static const size_t cpl_c61_ops[1] = { offsetof(reader_page_read_ops, get) };
CPLRT_EXPORT cpl_status reader_page_read_new(cpl_runtime *rt, const reader_page_read_ops *ops, void *user,
                                             cpl_cap *out, cpl_error **error)
{
    return cplrt_cap_create(rt, CPL_D61, &cpl_c61_impl, ops, sizeof(reader_page_read_ops), cpl_c61_ops, user,
                            out, error);
}
/* reader.pages.paginate_ints: export 3 */
CPLRT_EXPORT cpl_status reader_paginate_ints(cpl_runtime *rt, cpl_value items, cpl_value size,
                                             cpl_value *ok_out, cpl_value *err_out,
                                             const cpl_call_options *options, cpl_error **error)
{
    cplrt_arg a[2];
    cplrt_out o[2];
    a[0].value = items;
    a[1].value = size;
    o[0].value = ok_out;
    o[1].value = err_out;
    return cplrt_export_call(rt, 3, a, NULL, o, options, error);
}
/* reader.images.image_width: export 0 (a Bytes span and a capability) */
CPLRT_EXPORT cpl_status reader_image_width(cpl_runtime *rt, const uint8_t *image, size_t image_len, cpl_cap codec,
                                           cpl_value *ok_out, cpl_value *err_out,
                                           const cpl_call_options *options, cpl_error **error)
{
    cplrt_arg a[1];
    cplrt_out o[2];
    a[0].span.ptr = image;
    a[0].span.len = image_len;
    o[0].value = ok_out;
    o[1].value = err_out;
    return cplrt_export_call(rt, 0, a, &codec, o, options, error);
}
```

`cplrt_export_call` does, in this order, before the first CPL frame
exists:

1. checks every C argument without running CPL code: the `options`, when
   not NULL (its `struct_size` per Section 4.2; its `flags`, where
   `CPL_CALL_UNCHECKED_CONTRACTS` and every unknown bit are
   `interop_option_unsupported`; its `cancel` token unless `{{0, 0}}`);
   then each positional parameter in order (a handle's identity, slot, and
   generation; a span's pointer and, for `Text`, its UTF-8; a `Bool`'s 0
   or 1); then each `cpl_cap` (a handle check, then
   `interop_capability_mismatch` for a capability of another declaration);
   then each output pointer (`interop_null_pointer`);
2. deep-checks every handle argument against its parameter's declared type
   in the order `value_from_json` decodes (struct fields in declaration
   order, list elements by index, payloads by position); the first
   mismatch is `CPL_E_ARGUMENT` with `interop_argument_type_mismatch`
   (`parameter`, `pointer`, `expected` the qualified type text at the
   position, `found` the value's kind or, for a struct, enum, or wrapper,
   its declaration's qualified name): a usage error, as an ill-typed `cpl
   run` argument is `usage_bad_argument`;
3. checks every refined position of every argument in semantics S74's
   order, parameter by parameter, by calling the compiled predicates, each
   an activation entered with `CPLRT_NO_SITE`, whose frame therefore has
   no span, as `run_function` enters it. A position whose type is a
   refinement is one position: the positions of its root base (`strip(R)`)
   come first, and then its whole chain is checked root first, so a
   refinement whose base is a refinement adds no position of its own, and
   the position's `expected_type` is the declaration named there
   (`cpl_eval::json::refined_positions` and `check_arguments` of
   `crates/cpl-eval/src/run.rs`). The first failure is the fault
   `refinement_violation` (`CPL_FAULT`) with `cpl run`'s exact diagnostic
   (`symbol` the function, `span` the parameter, `expected_type`,
   `actual`, `clause`, `parameter`, `pointer`), and a predicate that
   faults is reported per S34;
4. calls the function's entry trampoline as a top-level evaluation with no
   calling declaration (semantics S39 (c)), under the runtime's
   `max_call_depth` and the call's options, and maps the result back by
   Section 5.4.

Semantics: S81 makes values entering through the C API subject to the
checks of `cpl run` arguments, with a type mismatch a usage error and a
refinement failure the fault `refinement_violation`. This is the design's
Export example: "The wrapper checks foreign inputs even if an internal
CPL caller could establish the refinement statically" (line 594). For
`paginate_ints` an invalid `size` is no boundary failure: `size` is an
`Int`, the body returns `Err(ConstraintError)`, and the host sees
`CPL_ERR` with `*err_out`, "a defined error status in C".

Decision: [D-315] C29. Every export checks its C arguments, then the deep
types of its handle arguments, then their refined positions in S74 order,
before any CPL frame exists, and only then calls the function as a
top-level evaluation; a type mismatch is a usage status, a refinement
failure the fault `cpl run` reports. Rationale: a value entering CPL must
have the type its parameter declares before CPL code relies on it, and
constructors check only shallow shapes (Section 4.5), so the deep check
runs exactly once, at entry; refinements are checked as `cpl run` checks
them so that the C host and the command line see the same diagnostic.
Rejected alternatives: a refinement failure as a usage status (it is a
fault in `cpl run`, semantics 6.7, exit 1, so it is a fault here);
trusting handles built through the API (the constructors' shallow checks
cannot see a list of the wrong element type).

### 5.6 Generic functions

A generic function is never exported: listing one is
`project_export_generic`, whose note names the project's monomorphic
callers as `cannot_run_generic`'s note does, and the default set skips it
with the info `project_export_skipped_generic` (toolchain 11.1). This is
the design's "CPL generic functions require concrete export wrappers"
(line 504); `paginate_ints` is the design's wrapper of `paginate`.

### 5.7 Functions with `using` capability parameters

A host supplies each capability as a `cpl_cap`, obtained in one of two
ways:

- **Its own callbacks.** It fills the generated typed operation struct
  (one function pointer per operation in declaration order, parameters
  mapped by Section 5.4: spans for `Text` and `Bytes`, `int32_t` for
  `Bool`, borrowed `cpl_value` handles otherwise) and calls
  `<prefix>_<name>_new(rt, &ops, user, &cap, &error)`, or uses the generic
  `cpl_cap_new` with a `cpl_cap_ops` table, whose operations receive every
  argument as a borrowed handle. The runtime copies the struct (and the
  generic table's `ops` array), so the host's copy need not outlive the
  call; `struct_size` must cover every operation pointer (else
  `interop_struct_size_unsupported` with `struct` the typed struct's
  name), and a NULL operation pointer is `interop_null_pointer`.
- **The project's adapter.** It calls `<prefix>_bind_<binding>(rt, &cap,
  &error)`, which instantiates the backend-`c` adapter compiled into the
  package (Section 6.1): the adapter's `init` runs then, and its `fini`
  runs once, when the last reference to the capability is released. The
  manifest requests the binding; the host grants it by this call (design
  line 559). A grant that cannot be made (the adapter's `init` returned a
  status other than `CPL_OK`, or a declarative `[state] create` returned
  NULL) is `CPL_E_STATE` with `interop_bind_failed`, except that an `init`
  returning `CPL_E_NOMEM` gives `CPL_E_NOMEM`; Section 6.2 fixes what
  `init` receives and what the runtime does with what it leaves.

**The callback contract.**

- A callback runs synchronously on the thread of the call that invoked
  it. `cpl_call_runtime(call)` gives the runtime for building values,
  `cpl_call_operation(call)` the operation's index, and
  `cpl_call_cancelled(call)` whether the running call's token was
  requested. `call` is valid only until the callback returns.
- The arguments are borrowed for the call: a callback may duplicate an
  argument handle with `cpl_value_dup` to keep the value, and must not
  release it. Before it calls the binding layer, `cplrt_cap_invoke`
  reserves one handle-table slot for each argument that the binding layer
  passes as a handle; a failure to reserve one is the fault
  `resource_exhaustion` (`memory`) and no callback runs, so borrowing an
  argument cannot fail, and the slots are freed when the callback
  returns. `*result` is `{{0, 0}}` on entry. On `CPL_OK`, `*result` must
  be an owned value handle of the runtime, which passes to the runtime; on
  any other status the runtime releases a valid handle that the callback
  left in `*result` and reads nothing else from it.
- It returns `CPL_OK`; or `CPL_FOREIGN_ERROR` after
  `cpl_call_foreign_error` (with optional `cpl_call_foreign_detail` calls,
  before or after it, which are kept in call order); or `CPL_E_NOMEM`,
  which becomes `resource_exhaustion` (`memory`). With any status but
  `CPL_FOREIGN_ERROR` a recorded failure and its details are discarded.
  Semantics: S80 makes any other status, `CPL_OK` without a valid owned
  result handle, or `CPL_FOREIGN_ERROR` without a reported failure the
  fault `internal_error` with kind `binding_protocol`.
- The runtime evaluates the operation's `requires` before invoking the
  callback (`attributed_to: "caller"`), validates the result (Section 6.4,
  semantics S77), and then evaluates the `ensures` (`attributed_to:
  "binding"`), as the evaluator does for a validating `CapabilityImpl`.
- **Re-entrancy** is allowed: a callback may call any runtime function and
  any export, each a fresh top-level evaluation (Section 4.3, semantics
  S79).
- **User data.** `user` stays the host's: the runtime never reads or frees
  it, and calls `drop(user)` exactly once, after the last `cpl_cap`
  referring to it is released and no call that holds it is in flight, on
  the thread that ends the last of them. A capability object's count is
  decremented with acquire-release ordering (Section 2.4), so `drop`
  happens after every callback of the capability has returned. `drop`,
  like an adapter's `fini` (Section 6.2), runs with no runtime lock held
  and may call every function of `cpl.h` on the owning runtime but
  `cpl_runtime_close`, though no export, so it can release the handles it
  kept; under `CPL_CLOSE_FORCE` every outstanding `drop` and `fini` runs,
  in reverse order of creation, before any handle is released (Section
  4.3). When the constructor fails, the runtime has not taken `user` and
  never calls `drop` for it. A host that keeps a CPL handle in its `user`
  data creates a reference cycle the runtime cannot see, which is the
  host's to break.
- **Handle numbers.** A capability's `$handle` number in value JSON (the
  `bindings` of holes, asserts, and loop violations) is its creation order
  in the runtime, from 0, as the evaluator numbers its `HandleId`s; `cpl
  run` and the driver bind in `using`-parameter order (semantics S78).

Decision: [D-316] C30. A host passes capabilities as `cpl_cap` handles,
built from its own typed or generic callbacks or granted from the
project's adapter; a callback runs synchronously on the calling thread
with borrowed arguments and an owned result, reports foreign failures
through `cpl_call_foreign_error`, may re-enter the API, and must return
normally; `drop` runs exactly once. Rationale: this is the synchronous
protocol the design's open callback decision asks to define (line 663:
"ownership, event-loop affinity, reentrancy, and cancellation without
implying rollback"), with every outcome a defined status, and the
asynchronous form Node needs is an addition of a later minor version, not
a change. Rejected alternatives: asynchronous callbacks in Milestone 2 (no
Milestone 2 host needs them); refusing re-entry (a callback that needs
another export would have to be split by the host); trusting callback
results (Section 6.4).

### 5.8 Effects

The generated comment and `build.json`'s `exports[].effects` (toolchain
7.2) record each export's declared effect bound. An export with an empty
bound receives no capability and therefore performs no capability
operation: the runtime has no ambient channel. Any authority an export
exercises is authority the host passed in as a `cpl_cap`; adapters remain
the trusted boundary (design line 213). The native runtime does not trace
effects in Milestone 2 (no command reports them).

## 6. Imports: CPL calls a C library

### 6.1 Binding a capability to C

Foreign code is reached through a capability (design line 475): a
backend-`c` binding says which C implementation the toolchain and the
runtime use when they must provide a handle for the capability. Toolchain:
the keys, their domains, and their validation codes are toolchain 11.1
(T74). A binding over vendored sources reads, for example:

```toml
[interop]
targets = ["c"]

[bindings.image_codec]
capability = "reader.images.ImageCodec"   # a pub capability
backend = "c"                             # active from Milestone 2
adapter = "adapters/image_codec.c"        # a reviewed wrapper (.c) or a declarative binding (.cbind.toml)
isolation = "in_process"                  # Milestone 2: required and the only value; from Milestone 3 also
                                          # "worker" (the default), the C adapter host of spec/python.md 6.6

[bindings.image_codec.c]                  # optional, backend c only: how to compile and link the adapter
sources = ["vendor/imgtest/imgtest.c"]    # vendored C sources compiled into the package and pinned by digest
include_dirs = ["vendor/imgtest"]
defines = ["IMGTEST_STATIC=1"]
libraries = []                            # system libraries by name: -l<name>, pinned by the file the linker opened
library_dirs = []
cflags = []
```

- **`adapter`** names the project-relative file that implements the
  capability's operations: a `.c` reviewed wrapper written against
  `cpl.h`, `cpl_adapter.h`, and the generated adapter header (Section
  6.2), or a `.cbind.toml` declarative binding from which `cpl-emit-c`
  generates such a wrapper (Section 6.3). Adapter files and vendored
  sources are build inputs, not members of the source set (toolchain 0),
  and the lock pins them (Section 6.6).
- **`isolation = "in_process"`** means that the adapter and the library
  run in the host process with its privileges (Section 6.7).
- **Where handles come from.** A binding provides the handle when the
  toolchain must: `cpl run` on a function with `using` parameters
  (semantics S78, Section 7), and a host that asks the package for its
  adapter through the grant `<prefix>_bind_<binding>` (Section 5.7). A host
  that implements the capability itself needs no binding. At most one
  backend-`c` binding exists per capability
  (`project_binding_duplicate_capability`, toolchain 11.1), so `cpl run`
  has one source of handles.
- **Compiling.** Every backend-`c` adapter of the project, with its
  vendored sources, is compiled into every artifact of the package and
  into its drivers, with the adapter flag set of Section 1.5, and linked
  with the binding's libraries.

### 6.2 Reviewed wrappers

For each binding whose adapter is a `.c` file, `cpl build` generates
`<prefix>_adapter_<binding>.h`, the contract the wrapper must satisfy;
Appendix C.2 is the one of the `cboundary` fixture. It declares, all with
`CPL_ADAPTER` (hidden visibility), never exported:

| Function | Contract |
| - | - |
| `cpl_status <prefix>_adapter_<binding>_init(cpl_runtime *rt, void **state, cpl_error **error)` | called once per grant (Section 5.7), on the granting thread, after the runtime has allocated the capability object and its handle, so that a grant never fails after a successful `init`; `*state` and `*error` are NULL on entry. It stores the adapter's state for this capability object in `*state` and returns `CPL_OK`; any other status fails the grant (`interop_bind_failed`, or `CPL_E_NOMEM`), after which `fini` is never called, so a failing `init` has released what it allocated. Whatever the status, the runtime takes an error object that `init` stored in `*error` (one it received from an API call): on a failure its diagnostic becomes a note of `interop_bind_failed`, and either way the runtime releases it |
| `void <prefix>_adapter_<binding>_fini(void *state)` | called once, after the last reference to the granted capability is released and no call is in flight, or by a forced close (Section 4.3), never after a failed `init`; it may run on any thread, runs with no runtime lock held, and may call the API as `drop` may (Section 5.7) |
| `cpl_status <prefix>_adapter_<binding>_<operation>(void *state, cpl_call *call, <parameters>, cpl_value *result)` | one per operation, in declaration order and named as Section 5.2 says, with the parameters of a typed callback (Section 5.7) and its contract, `state` in place of `user`. The operations of one granted capability may run concurrently on several threads with one `state` (Section 4.4), so a reviewed wrapper makes them thread-safe, as the generated wrapper of Section 6.3 does |

The header's layout follows Appendix C.2: a banner naming the binding,
the capability, the backend, the isolation, and the adapter file; the
include guard `<PREFIX>_ADAPTER_<BINDING>_H`; `cpl_adapter.h` and
`<prefix>.h`; `extern "C"` for C++; then `init`, `fini`, and the
operation functions, each preceded by its comment, an operation's
comment being that of its member in the typed capability struct (Section
5.3).

The wrapper of the fixture, over the test library `imgtest` (normative
for its use of the API):

```c
/* adapters/image_codec.c of the cboundary fixture: a reviewed wrapper over the test library imgtest. */
#include "cboundary_adapter_image_codec.h"
#include "imgtest.h"

cpl_status cboundary_adapter_image_codec_init(cpl_runtime *rt, void **state, cpl_error **error)
{
    (void)rt;
    (void)error;
    *state = imgtest_decoder_new();                    /* an opaque foreign object owned by the adapter */
    return *state != NULL ? CPL_OK : CPL_E_NOMEM;
}

void cboundary_adapter_image_codec_fini(void *state)
{
    imgtest_decoder_free(state);
}

cpl_status cboundary_adapter_image_codec_dimensions(void *state, cpl_call *call, const uint8_t *image,
                                                   size_t image_len, cpl_value *result)
{
    cpl_runtime *rt = cpl_call_runtime(call);
    int32_t w = 0, h = 0;
    int rc = imgtest_dimensions(state, image, image_len, &w, &h);              /* the span is borrowed */
    if (rc != 0) {                                                             /* foreign_code, then the error */
        cpl_foreign_kind kind = rc == IMGTEST_EUNSUPPORTED ? CPL_FOREIGN_UNAVAILABLE : CPL_FOREIGN_EXCEPTION;
        return cpl_adapter_fail_code(call, kind, rc, imgtest_strerror(rc));
    }
    cpl_value f[2] = { { { 0, 0 } }, { { 0, 0 } } }, dims = { { 0, 0 } };
    cpl_status st = cpl_int_from_i64(rt, w, &f[0], NULL);
    if (st == CPL_OK)
        st = cpl_int_from_i64(rt, h, &f[1], NULL);
    if (st == CPL_OK)
        st = cpl_struct_new(rt, cboundary_type_dimensions(), f, 2, &dims, NULL);   /* Positive is checked in 6.4 */
    if (st == CPL_OK)
        st = cpl_adapter_ok(rt, dims, result);                                     /* Ok(dims); releases dims */
    (void)cpl_value_release(rt, f[0]);                                             /* {{0, 0}}: a no-op */
    (void)cpl_value_release(rt, f[1]);
    return st;
}
```

`cpl_adapter.h` (Appendix A.2) is `cpl.h` plus `CPL_ADAPTER` and four
`static inline` helpers built only on the functions of Section 4.8 (it
adds no table entry):

| Helper | Does |
| - | - |
| `cpl_status cpl_adapter_int_arg(cpl_call *call, cpl_value v, int64_t lo, int64_t hi, int64_t *out)` | a checked conversion of an `Int` argument; outside `[lo, hi]` it reports `InvalidArgument` and returns `CPL_FOREIGN_ERROR` |
| `cpl_status cpl_adapter_cstr(cpl_call *call, const char *ptr, size_t len, char *buf, size_t cap)` | a NUL-terminated copy of a UTF-8 span into a caller buffer; an interior U+0000 or a short buffer reports `InvalidArgument` |
| `cpl_status cpl_adapter_fail_code(cpl_call *call, cpl_foreign_kind kind, int64_t code, const char *message)` | adds the detail `foreign_code` (the decimal code) and reports the failure with `message`, which must be NULL or strict UTF-8 (a C library's locale-encoded `strerror` text may not be); it returns `CPL_FOREIGN_ERROR`, or the status of the API call that failed (`CPL_E_NOMEM`, or `CPL_E_ARGUMENT` for a message that is not UTF-8), which the operation function returns in turn |
| `cpl_status cpl_adapter_ok(cpl_runtime *rt, cpl_value v, cpl_value *result)` | `Ok(v)` into `*result`, releasing `v` |

Decision: [D-317] C31. A backend-`c` binding's adapter is a reviewed C
wrapper, or a declarative binding that generates one, written against
`cpl.h`, `cpl_adapter.h`, and a generated header that declares exactly
the functions the wrapper must define (`init` once per grant, `fini`
once, one function per operation with the typed callback contract), all
with hidden visibility; the adapter is compiled into the package and its
drivers. Rationale: the design's C row is "Generated or reviewed bindings
to C-compatible functions" (line 439) and its starting point is that
"header parsing can generate signatures but cannot establish pointer
ownership, mutation, callback behavior, or semantic contracts" (line
467), so the reviewed code is where those are established, against a
contract the C compiler checks; hidden visibility keeps the adapter out of
the package's ABI. Rejected alternatives: header-parsing generation in the
style of bindgen (signatures without ownership or contracts; the design
rules it insufficient, and the machine has no libclang); dynamic calls
through `libffi` (no compile-time check of a binding against the real
header, no ownership model, and a native dependency); exporting the
adapter's functions (they would become part of the package ABI).

### 6.3 Declarative bindings

`cpl-emit-c` generates from a `.cbind.toml` file a wrapper,
`src/<prefix>_bind_<binding>.c`, that defines the adapter contract of
Section 6.2 and includes the library's own headers, so the C compiler
checks every declared call against the real prototypes: the wrapper is
compiled with the flags of Section 1.5 for generated wrappers, so a
declared scalar type that does not fit the real parameter, out-parameter,
or result, or a pointer type that differs from it, fails the build with
`build_adapter_compile_failed` (toolchain Appendix B) instead of
converting silently, while a warning inside the library's own headers,
which the binding does not control, is not reported. No header is
parsed. The vocabulary covers exactly the design's starting shapes
("fixed-width scalars, byte spans, explicit structs, and opaque handles",
line 467). The fixture `cthumbs` (toolchain 12) declares:

```toml
# adapters/thumbs.cbind.toml, for
#   pub capability Thumbnails { fn thumbnail(image: Bytes, max_side: Int) -> Result<Bytes, ForeignError>
#                               fn decoder_version() -> Int }
[library]
headers = ["imgtest.h"]                          # included by the generated wrapper, in order

[state]                                          # one opaque foreign object per grant, owned by the adapter
type = "imgtest_decoder *"
create = "imgtest_decoder_new"                   # imgtest_decoder *imgtest_decoder_new(void); NULL fails the grant
destroy = "imgtest_decoder_free"                 # called exactly once, when the last reference is released

[ops.thumbnail]
c = "imgtest_thumbnail"   # int imgtest_thumbnail(imgtest_decoder *, const uint8_t *, size_t, int32_t, uint8_t **, size_t *)
args = ["state", "image: bytes_span", "max_side: int32"]
out = [{ name = "data", type = "owned_bytes", release = "imgtest_free" }]   # library-allocated; copied, released once
returns = { type = "int", ok = [0] }
errors = { "-1" = "InvalidArgument", "-2" = "Unavailable", default = "Exception" }
message = "imgtest_strerror"                     # const char *imgtest_strerror(int)
result = "data"                                  # Ok(data)

[ops.decoder_version]
c = "imgtest_version"                            # int32_t imgtest_version(void)
args = []
returns = { type = "int32", result = true }      # an infallible scalar; the operation needs no ForeignError arm
```

The schema is closed: a key it does not name, a value outside its domain,
or a form the vocabulary cannot express is `project_cbind_invalid` with
`binding`, `key`, `line`, and `reason` (toolchain 11.1), reported by `cpl
check`; the checks against the capability's operations and types run
after resolution.

| Key | Domain |
| - | - |
| `[library] headers` | required, a non-empty array of header names, included as `#include "<name>"` in order, found through the binding's `include_dirs` |
| `[state]` | optional; when present `type` (a C object pointer type: an identifier or `struct <identifier>`, then ` *`), `create` (a C function `<type> create(void)`), and `destroy` (a C function `void destroy(<type>)`), all required. The generated wrapper creates a mutex with the object and holds it around every library call that receives `state`, because the operations of one grant may run concurrently (Section 6.2); a library whose state object allows concurrent calls needs a reviewed wrapper to make them. A mutex that cannot be created fails the grant with `CPL_E_NOMEM` after `destroy` has released the object, and `destroy` may run on any thread |
| `[structs.<name>]` | optional, one per explicit C struct: `c_type` (required: the C type, an identifier or `struct <identifier>`) and one entry `<field> = "<member>: <scalar>"` per field of every CPL struct the mapping is used for, exactly those fields, with a scalar binding type |
| `[ops.<operation>]` | exactly one table per operation of the capability, keyed by the operation's name |
| `c` | required: the C function the operation calls |
| `args` | required: the C arguments in order; each is `"state"` (the `[state]` object, which `[state]` must declare) or `"<parameter>: <type>"` with an argument binding type; every parameter of the operation appears exactly once |
| `out` | optional: out-parameters appended after `args`, each `{ name, type, release? }` with an out binding type; `release` (a C function `void release(void *)`) is required for `owned_bytes` and `owned_text` and forbidden otherwise. The wrapper initializes every out-parameter before the call: a pointer to NULL, a size or a scalar to 0, a struct to zeros |
| `returns` | required: `{ type = "<scalar>", ok = [<integers>] }` (a status: the listed values succeed), `{ type = "<scalar>", result = true }` (an infallible scalar result), or `{ type = "void" }` |
| `errors` | required with `ok` and forbidden otherwise: a table from the decimal text of a status to a `ForeignErrorKind` name, with `default` required |
| `message` | optional with `ok`: a C function `const char *message(<returns type>)` giving the failure's message; the wrapper passes its result on only when it is not NULL and is strict UTF-8, and otherwise reports the message that stdlib.md S45 gives for that case |
| `result` | with `ok`: the name of the out-parameter whose value is the `Ok` payload; omitted when the payload is `Unit` |

| Binding type | C side | CPL side | Conversion failure |
| - | - | - | - |
| `int8`, `int16`, `int32`, `int64`, `uint8`, `uint16`, `uint32`, `uint64`, `int` (the C `int`); as an argument, an out-parameter, or a return | the integer type | `Int` or a refinement of `Int` | an argument outside the C type: `InvalidArgument`, before the call, with the message and details that stdlib.md S45 gives a conversion failure of the binding layer; a result failing a declared refinement: Section 6.4 |
| `bool`; as an argument, an out-parameter, or a return | `int`, 0 or 1 | `Bool` | a result other than 0 or 1: `InvalidResult` |
| `bytes_span` (argument) | `const uint8_t *, size_t`, borrowed for the call | `Bytes` | none |
| `text_span`, `cstr` (argument) | `const char *, size_t`, or NUL-terminated (an interior U+0000 is `InvalidArgument`) | `Text` | as stated |
| `owned_bytes`, `owned_text` (out) | `uint8_t **, size_t *` or `char **, size_t *`, with a named `release` function | `Bytes`, `Text` | copied when the status is an `ok` value; whatever the status, released with its `release` function exactly once when it is not NULL after the call, and never when it is NULL; a text result must be strict UTF-8, else `InvalidResult` |
| `struct:<name>` (argument, out) | a pointer to the C struct of `[structs.<name>]`: filled from the CPL struct argument field by field, or, as an out-parameter, on the wrapper's stack and converted field by field | a CPL struct of the mapped fields | per field |
| `state` (argument) | the opaque object of `[state]` | none: CPL 0.1 has no foreign object type (C39) | `create` returning NULL at the grant: `interop_bind_failed` (Section 5.7) |

The operation's CPL types must agree: an operation with `ok` returns
`Result<T, std.foreign.ForeignError>` with `T` the `result`
out-parameter's CPL type (or `Unit`); one with `result = true` returns the
return's CPL type or that type inside `Result<_, ForeignError>`; one with
`void` returns `Unit` or `Result<Unit, ForeignError>`; a fallible
operation whose return type has no `ForeignError` arm is
`project_cbind_invalid`. Callbacks, variadic functions, unions, arrays
inside structs, and pointer graphs cannot be expressed and need a reviewed
wrapper ("Variadic APIs, unusual layouts, and complex callbacks are
outside automatic binding support initially", line 467). A foreign object
that lives across calls lives in `[state]`, one per grant, and one local
to a call is created and destroyed inside it; CPL code never holds a
foreign object (C39). A failure of a fallible call takes its kind from
`errors` and its message as stdlib.md S45 says, from `message` when the
binding names one (Section 6.5).

Decision: [D-318] C32. Milestone 2 ships declarative bindings with the
closed vocabulary above, from which the emitter generates a wrapper that
defines the reviewed-wrapper contract and is compiled against the
library's real headers. Rationale: the design names generated bindings
beside reviewed ones (line 439) and its starting shapes are exactly
scalars, byte spans, explicit structs, and opaque handles (line 467); a
closed vocabulary keeps every generated conversion and release
reviewable, and generating the same contract as a reviewed wrapper means
one runtime path serves both. Rejected alternatives: deferring declarative
bindings to Milestone 3 (the vocabulary is small and off the critical
path); header parsing (line 467).

Decision: [D-325] C39. In edition 0.1 CPL code holds no foreign object: a
foreign object that lives across calls is its adapter's state, one per
grant (a reviewed wrapper's `*state`, a declarative binding's `[state]`),
and one that lives within a call is created and destroyed inside it. A
foreign handle type in CPL source, the design's opaque handles of foreign
objects seen from CPL (design lines 467, 480, and 504), is deferred to a
later edition; at the C boundary of edition 0.1 the design's opaque
handles are the API's handles of CPL values and capabilities (Section
4.5) and the adapter state that CPL code never sees. Rationale: a handle
that CPL values could carry needs ownership and aliasing rules that
semantics S27 avoids by keeping capability handles out of values, and an
`Int` or `Text` id of a foreign object would be forgeable, contradicting
"code cannot manufacture one from a string or a record" (design line
207); one state per grant covers the design's adapter pattern, a library
object behind a capability (design lines 512-540). Rejected alternatives:
resource ids in CPL values (forgeable); one capability per foreign object
(CPL code cannot create a capability, so a program could never obtain a
second object); a foreign handle type in edition 0.1 (a language feature
with an aliasing analysis of its own, which semantics owns).

### 6.4 Validating foreign results

Semantics: S77 requires every value a capability implementation returns
on a Milestone 2 path to be validated after the host returns and before
the operation's `ensures` runs: its shape against the operation's
declared return type in value order, then every refined position in S74
order; it fixes the three outcomes of a failure (a substitution
`Err(ForeignError { kind: InvalidResult, ... })` when the return type is
`Result<T, std.foreign.ForeignError>`, else the fault
`refinement_violation` blamed on the operation with `attributed_to:
"binding"`, or for a shape failure `internal_error` kind
`binding_result_ill_typed`), and it keeps the Milestone 1 API
`Interpreter::bind_capability` trusted (semantics 6.9).

The mechanisms:

- **Natively**, `cplrt_cap_invoke` validates the word the binding layer
  returned against the type-table entry of the operation's return type
  (`cplrt_op.ret`, Appendix B.1), shape first and then the refined
  positions, one per refinement as at an export's entry (Section 5.5 step
  3), each predicate a call of the compiled `cpl_p<def>` as an activation
  entered at the operation call's site, and builds the outcome: the
  substitute value from the templates of Section 2.9 and the detail order
  of stdlib.md S45 (the original result is then released), or the fault
  record of Section 3.3.1 with `symbol` the operation's identity (so that
  the diagnostic's facet is `body`), `span` the operation call,
  `expected_type`, `actual`, `clause`, `pointer`, `attributed_to:
  "binding"`, and `binding`. Every `<expected_type>` it renders is the
  display text of the type at the failing position (`cplrt_type.display`)
  or, for a refinement, the declaration named there
  (`cplrt_refinement.declared_name`).
- **In the evaluator**, `cpl_eval::boundary::validate_result` does the
  same for every capability bound with `BindOptions { validate_results:
  true }` (toolchain T77), with the interpreter's predicates; the CLI binds
  every `CAdapter` that way (Section 7.2).
- **The binding name.** `binding` and the detail `adapter` carry the
  binding name of semantics 6.9: the binding's name for a grant, `"host"`
  for a capability created through the C API, the label of a scripted host
  of the driver.

For `Dimensions { width: Positive, height: Positive }` inside `Ok`, the
refined positions are `/$payload/0/width` and then `/$payload/0/height`:
this is the design's "The adapter validates returned dimensions before
constructing Positive values" (line 540), done by the runtime even when
the adapter forgot.

Decision: [D-319] C33. Result validation and the construction of
`ForeignError` values are runtime mechanisms in `cplrt_cap_invoke`,
table-driven by the operation's return type and built from the shared
templates and detail order, with the evaluator's
`cpl_eval::boundary::validate_result` as their counterpart; the rules they
implement are semantics S77 and S80 and stdlib.md S45. Rationale: one
mechanism per engine, placed between the host call and the `ensures`,
serves every binding path (host callbacks, grants, declarative wrappers,
scripted hosts) and makes the two engines comparable on every outcome.
Rejected alternatives: validation in each adapter (the runtime could not
rely on it, and the design requires validation before values become CPL
values); trusting adapter results as Milestone 1 trusts its test hosts.

### 6.5 Foreign failures to `ForeignError`

Library: stdlib.md S45 fixes how a C binding produces each
`ForeignErrorKind`, the message rule, and the fixed order of the detail
keys. The mechanisms: a reviewed wrapper reports a failure with
`cpl_call_foreign_error` and its details with `cpl_call_foreign_detail`
(Section 5.7); a declarative wrapper maps its status through `errors` and
its message through `message`, and adds `foreign_type` (the C function's
name) and `foreign_code` (the decimal status); the binding layer converts
arguments before the call and reports `InvalidArgument` when one does not
fit; a failed `[state] create` fails the grant. `cplrt_cap_invoke` then
builds the `ForeignError` value with the details in the order stdlib.md
S45 fixes, taking the keys the runtime supplies from the binding name
(Section 6.4) and the program tables, and returns `Err(ForeignError)` when
the operation's return type is `Result<T, std.foreign.ForeignError>`.
Semantics: S80 makes a foreign failure of an operation whose return type
cannot carry a `ForeignError` the fault `internal_error` kind
`binding_error_unrepresentable`. A host time limit (`Timeout`) is not
produced in Milestone 2: an in-process C call cannot be preempted. The
runtime never retries an operation (design line 489).

### 6.6 Linking and native-dependency pinning

- **Vendored sources** (`[bindings.<name>.c] sources`) are compiled with
  the package's compiler and flags and pinned by source digest: what the
  design calls "reproducible build recipes" (line 600), and the preferred
  form. A file
  listed by several bindings is compiled once; listing it with different
  `defines` is `project_cbind_invalid` for a declarative binding and
  `build_adapter_compile_failed` otherwise.
- **System libraries** (`libraries`, `library_dirs`) are linked with `-L`
  and `-l`; the files the linker actually opened are taken from
  `-Wl,--trace` and pinned by path and blake3 digest, the libraries behind
  linker scripts included, because "native headers alone do not identify
  the linked library version" (design line 600). A name that does not
  resolve is `build_native_dependency_missing`; an unresolved symbol is
  `build_link_failed` (`-Wl,-z,defs`).
- **Headers** actually read while compiling each binding's files (its
  adapter or generated wrapper and its vendored sources) are taken from
  the `-MD` depfiles and pinned as one digest per binding,
  `foreign.c.<binding>.headers`: the blake3 of `path LF digest LF` for
  every header read, sorted by path, the path project-relative under the
  project and absolute elsewhere, the digest the header's blake3 in hex.
  The headers under `<out>/include` are left out: the runtime digest and
  the program cover them.
- **The platform files** that the final link opened (the shared library's
  link, else the executable's, else none) and that are neither objects of
  this build nor files a binding's `-l` resolved (the crt objects,
  `libc`, `libgcc`, `libpthread`) are pinned as one digest,
  `targets.c.system_digest`, built as the headers' digest is.
- **When checked.** Toolchain: the lock entries and when each is written
  and checked are toolchain 11.2 (T75): adapter files and vendored sources
  under `cpl check --release` and `cpl build --release` (they need no
  build step), the compiler, flags, headers, libraries, and platform files
  under `cpl build --release` only, after compiler discovery.

Decision: [D-320] C34. A package pins its native inputs by what the build
actually used: vendored sources by digest, the headers each binding's
compile read from the compiler's depfiles, the libraries and platform
files the linker opened from its trace. Rationale: the design requires
native dependencies to be pinned artifacts or reproducible recipes and
observes that headers alone do not identify a library (line 600); asking
the compiler and the linker what they opened is exact where reading
search paths is a guess. Rejected alternatives: recording headers but not
libraries (the design's own counterexample); recording the search paths
or the `-l` names only (they do not identify files).

### 6.7 What isolation means for C in edition 0.1

`in_process` is the only C isolation. The adapter and every library it
calls share the host's address space and privileges: the capability
declaration bounds what CPL code can request (every call is a tracked
effect), not what native code can do (design line 487). A crash in them is
a crash of the host process: of a C host, of a driver (which the CLI
reports as `internal_error` kind `native_driver_failed`), or of the
evaluator's adapter host, where it becomes `Transport` (Section 7.2).
Receipts list each adapter as a `foreign_adapter` assumption from
Milestone 5 (semantics S59). Worker isolation for C arrives with the
worker transport of Milestone 3, which extends cpl-driver protocol 1.

Decision: [D-321] C35. Edition 0.1 has one isolation for C bindings,
`in_process`, which the manifest must state explicitly
(`project_binding_isolation_unsupported` otherwise, toolchain 11.1).
Rationale: running native code in the host's process is a trust decision
the manifest should state rather than inherit from a default whose meaning
differs (`worker` is the default of `isolation`, toolchain 11.1, and no
default silently changes meaning, T35); a worker transport designed for C
alone would be designed again for Python. Rejected alternatives: worker
isolation for C in Milestone 2 (it would design Milestone 3's transport
twice); accepting an absent `isolation` as `in_process` (the default would
change meaning by backend).

## 7. The evaluator and native code

### 7.1 `cpl run` with C-bound capabilities

Semantics: S78 lets `cpl run` accept a function with `using` parameters
when every `using` parameter's capability is the capability of exactly
one binding whose backend is available (in Milestone 2, `backend = "c"`
with `"c"` in `[interop] targets`), binds the handles in
`using`-parameter order, numbered from 0, before the arguments are
checked, and fixes the refusals (`cannot_run_effectful` with its notes, a
grant that fails as `interop_bind_failed`). Tests and properties stay
pure: `cpl test` binds nothing (semantics S30). This section provides the
handles: the evaluator obtains them through `CAdapter` (Section 7.2), and
the native engine through its driver's `bind` request (Section 3.8),
which grants the same adapter in the driver's own process.

### 7.2 `CAdapter`: the evaluator reaches C adapters through a driver

`cpl_cabi::adapter::CAdapter` implements `cpl_eval::CapabilityImpl`
(`crates/cpl-eval/src/interp.rs`) for one backend-`c` binding. The
evaluator never loads C into its own process:

1. `manifest_hosts` (toolchain 1.14) creates the `CAdapter`s of one `cpl
   run` when the run binds its handles, before the arguments are checked
   (semantics S78). The first one builds (or takes from the cache) the
   program's driver, which contains every backend-`c` adapter of the
   project (Section 3.8), spawns it, and sends `configure`; then each, in
   `using`-parameter order, grants its binding with `{"op": "bind",
   "binding": "<name>"}`. The `CAdapter`s of one run therefore share one
   driver process, as the bindings of the native engine do, and their
   handles there are numbered from 0 in parameter order, so state that an
   adapter or its library keeps for the whole process is shared alike in
   both engines. An `{"error": E}` answer is the error
   `interop_bind_failed` of S78, and nothing runs.
2. Each `call(op, args, ctx)` becomes `{"op": "cap_call", "handle": h,
   "operation": i, "args": [V...]}`, `h` the `CAdapter`'s handle: the
   binding layer only (the adapter's function and the foreign-failure
   mapping), with no clauses and no validation. The answer `{"ok": V}` is
   rebuilt as a `cpl_eval::Value`; `{"foreign_error": F}` becomes
   `Err(ForeignError { kind, message, details })`, with `adapter`,
   `capability`, and `operation` put before `F`'s details (Section 3.8),
   when the operation's return type carries a `ForeignError`, else the
   fault `internal_error` kind `binding_error_unrepresentable`, by the
   rules of stdlib.md S45 and semantics S80 applied in Rust; `{"fault":
   R}` (a `binding_protocol` or memory fault of the binding layer, which
   runs at no site and so carries no span and no frame) becomes the
   rebuilt fault, to which the evaluator gives the operation call's span
   and its own frames, as to any fault a host returns (semantics S76,
   S80).
3. The evaluator then performs the validation of semantics S77 and the
   operation's `ensures` itself, because the CLI binds every `CAdapter`
   through `bind_capability_with` with `validate_results: true`
   (toolchain T77); the binding name its faults and `ForeignError`
   details carry is the binding's own name, as natively (semantics 6.9),
   so the two engines report the same `binding` and `adapter`.
4. If the driver process dies, the result is `Err(ForeignError { kind:
   Transport, .. })`, with the message and details of stdlib.md S45, when
   the return type carries a `ForeignError`, else the fault
   `internal_error` kind `binding_host_failed` of semantics S80.

Consequences: no `unsafe` FFI and no loader crate enter the toolchain, a
crashing adapter cannot take `cpl` down, and one adapter, compiled once,
serves both engines and runs under ASan in the sanitizer suites and in
the C host suites. A round trip costs about 20 µs, negligible for `cpl
run`.

Decision: [D-322] C36. The evaluator reaches a C adapter only through a
driver process, by `CAdapter`, which forwards each operation to the
driver's binding layer and leaves validation and `ensures` to the
evaluator; the `CAdapter`s of one `cpl run` share one driver, which grants
their bindings in parameter order before the arguments are checked; values
cross as node lists. Rationale: the comparison of the engines needs
CPL-side evidence that CPL calls C, which only `cpl run` with bindings
provides, and the out-of-process path gives it without `unsafe` code in
the toolchain, survives an adapter's crash, and loads sanitized builds;
one driver per run, granting where semantics S78 binds, gives the adapters
the process, the handle numbers, and the grant failures they have
natively. Rejected alternatives: a driver per `CAdapter`, granting at the
first operation call (a failing grant would surface mid-evaluation or not
at all, and process-wide state of an adapter's library would differ
between the engines); `dlopen` of the project's library into the `cpl`
process (`unsafe` FFI, a native crash kills `cpl`, no sanitized builds,
and in Milestone 3 it would load libpython into the toolchain); keeping
`cannot_run_effectful` for every function with `using` parameters in
Milestone 2 (no evidence comparable with the oracle that CPL calls C);
Rust reimplementations of C adapters for the evaluator (two adapters per
binding, and the evidence would not be about the C library).

### 7.3 `cpl test` on both engines

Toolchain: the property runner is generic over `cpl_eval::test::TestEngine`
(toolchain 1.7.2, T77): seeds, generation, shrinking, the `gen.range`
precondition (`builtins::check_gen_range_at`), and the assembly of records
stay in Rust for both engines, and only body evaluation, refinement
checks, range bounds, and violation records come from the engine, which
for the native engine are the driver's `run_body`, `failing_refinement`,
and `range_bound` requests (Section 3.8).

### 7.4 Value exchange

- **The driver and the adapter host.** Values cross as the flat
  post-order node lists of Section 3.8, so neither side recurses on value
  depth and a shared child array is sent once; `cpl_cabi::wire` converts
  them to and from `cpl_eval::Value` iteratively. Output values also carry
  the C writer's value JSON and value text, which the suites compare byte
  for byte with the Rust renderings (toolchain 12.6).
- **Fault records** carry their payload as the C runtime's JSON, parsed on
  the evaluation thread with the recursion limit disabled (Section 3.8).
- **C hosts** exchange values through the API's value JSON
  (`cpl_value_to_json`, and `cpl_value_from_json`, the strict codec of
  `cpl run --arg` and `result` without its nesting limit, Section 4.8);
  toolchain 12.7 fixes which lines of the host programs the C host suites
  compare with the evaluator's rendering of the same call and which with
  goldens.
- **Round trips.** Toolchain: a test checks, for generated values of every
  kind (nests 10^6 deep and DAGs with sharing included), that node-list
  encoding and decoding, and the C reading and writing of value JSON
  through the driver's `from_json` and `render` requests (Section 3.8),
  give structurally equal values with identical renderings (toolchain
  12.6, T78).

## 8. Native executables

The `executable` artifact (Section 1.6), `bin/<prefix>`, runs one
exported function with the semantics of `cpl run`:

```text
bin/<prefix> <function> [--arg JSON]... [--max-call-depth N]
```

- `<function>` is the qualified name of an exported function; a function
  exported several times (in its handle form and with an integer mapping)
  is one function here, since an entry's integer mapping does not apply to
  value JSON. Arguments are value JSON only, decoded strictly at the
  parameter types by the rules of `cpl run --arg` (toolchain 7.2 and 7.3:
  a duplicate key and a value nested more than 127 levels deep are usage
  errors); constant expressions would need the CPL parser, which a native
  artifact does not carry.
- The refusals, the binding, and the checks keep `cpl run`'s order
  (semantics S39 and S78): a `<function>` that names no export is
  `index_unknown_symbol`; a `using` parameter whose capability has no
  backend-`c` grant in the package is `cannot_run_effectful` with the note
  of semantics S78; a wrong argument count or an argument that does not
  decode is `usage_bad_argument`; then the `using` parameters are bound
  from the package's grants in parameter order, a grant that fails being
  `interop_bind_failed`; then the refined positions of the arguments are
  checked (semantics S39 (a)); and the call runs with no calling
  declaration on a thread with 1 GiB of stack, under `--max-call-depth`,
  whose default is the project's `[run] max_call_depth` when the package
  was built (toolchain 11.1; `cplrt_program.max_call_depth`, Appendix
  B.1), so 10,000 when the manifest sets none.
- The output is one JSON line on stdout, `{"function": "<qualified
  name>", "arguments": [...], "result": <value JSON> | null,
  "result_text": "<value text>" | null, "fault": <S49 object> | null}`,
  with the encodings of `RunOutput` (toolchain 7.2). The exit status is
  `cpl run`'s (toolchain 7.4): 0 for a value, `Err(...)` values included;
  1 for a fault and for a grant that fails (`interop_bind_failed`,
  semantics S78); and 2 for a refusal or a usage error. A refusal, a
  usage error, and a failed grant print `{"error": <S49 object>}` with
  their code: an `index_*`, `usage_*`, or `resolve` code (toolchain
  Appendix B, semantics 9.4) or an `interop_*` code (Section 4.9).

Toolchain: the acceptance test compares, for the calls of the C host test
programs, the executable's `result`, `result_text`, and `fault` with those
of `cpl run --json` (toolchain 12).

Decision: [D-323] C37. A package can be built as a native executable that
runs one exported function from value-JSON arguments with `cpl run`'s
checks, binding, and output encoding, exiting 0, 1, or 2 as `cpl run`
does. Rationale: the design names native executables as the first
compilation target (line 38), and an executable that speaks `cpl run`'s
protocol is the one whose results can be compared with the evaluator's
without a C host. Rejected alternatives: constant-expression arguments (the
CPL parser in every artifact; value JSON is the round-trip form, toolchain
7.3); a `main` function in CPL (edition 0.1 has no I/O capability, semantics
S39's rationale).

## 9. Package ABI

### 9.1 Soname and version node

A package's ABI is its major number `[interop.c] abi` (0..=65535, default
0, toolchain 11.1). The shared library is `lib<prefix>.so.<abi>`, with
the ELF soname `lib<prefix>.so.<abi>` and every dynamic symbol under the
version node `<PREFIX>_<abi>` (Section 1.6); `<prefix>.h` defines
`<PREFIX>_ABI` as `<abi>u`, and `cpl_library_info.abi` reports it. The
soname and the node change only when `abi` changes, so a host linked
against one major never binds to another.

### 9.2 The `abi` bump rule

Under one `abi`, a package may only grow: adding exports, types,
capabilities, and grants is compatible, because hosts obtain type ids
through accessor functions (Section 5.3) and never compile them in.
Removing an export or any other name that the version script exports (a
type accessor, a capability constructor, a grant: renaming a type or its
`type_names` alias, say), changing an export's signature or integer
mapping, changing the members of a declaration an export reaches (its
fields or variants: their names, order, or types), changing the operations
of a capability an export uses (their names, order, parameter types, or
return types), or building with a toolchain whose C API has another major
version is an ABI change, which requires raising `abi`. Toolchain: under
`cpl build --release` such a change without a raised `abi` is
`build_abi_changed` (toolchain Appendix B; T75).

### 9.3 What `build_abi_changed` compares

The lock records, per export symbol, the function, its identity, its
integer mapping, and a `signature` digest (toolchain 11.2); for the
package an `exports_digest`; and `targets.c.symbols`, the sorted list of
every name that the version script exports (Section 1.6: the package
functions, the exports, the type accessors, the capability constructors,
and the grants). The two digests are blake3 digests, as 64 lowercase hex
characters, of a text of lines, each ended by LF:

- An export's **signature** digest is that of the lines: its symbol; its
  integer mapping (`handle`, `int64`, `int32`, `uint64`, `uint32`); the
  content hash of its signature facet (toolchain 5.3); and, for every
  declaration its signature reaches transitively through fields,
  payloads, and capability operations, in byte order of their qualified
  names, the qualified name followed by the declaration's signature facet
  text (toolchain 5.2), its *shape*, which holds the field and variant
  names, orders, and types, and a capability's operations with their
  parameter and return types.
- The **exports digest** is that of the lines: every export's symbol and
  signature digest, in export order, then the qualified type text of every
  listed type, in id order.

`cpl build --release` compares a built package with its lock's
`targets.c` and reports `build_abi_changed` (`symbol`, `id`, `expected`,
`found`) for every export of `targets.c.exports` that the package lacks
(`found` null) or whose `signature` digest or integer mapping differs; for
every other name of `targets.c.symbols` that the package's version script
lacks (`symbol` the name, `id` null, `expected` the name, `found` null);
and for a C API major version other than that of the lock's `api`
(`symbol` and `id` null, `expected` the lock's `api`, `found` the
package's). `cpl build --lock` makes the same comparison against the lock
it would replace and, on a difference, leaves `cpl.lock` unchanged
(toolchain T81). Neither compares when the package's `prefix` or `abi`
differs from the lock's: a new prefix is a new library, with a new soname
and new symbols, and a new `abi` a declared break. A refinement's
predicate is not part of a shape: a changed predicate changes which values
the checks of Section 5.5 accept, not the C interface.

Decision: [D-324] C38. A package's ABI major `abi` names its soname and
its version node, and under one `abi` the package may only add to its C
interface; the export signature digests in the lock, over symbols,
mappings, signature facets, and the shapes of the reached declarations,
with the list of exported names and the C API major, are what `cpl build
--release` and `cpl build --lock` compare to enforce it. Rationale: hosts
link against a soname and bind versioned symbols, so an incompatible
change must change both, and a digest of exactly what a C caller depends
on (names, mappings, member orders, operation lists) makes a silent break
detectable at release time while additions stay free. Rejected
alternatives: a new soname per build (every host would relink for every
change); comparing generated headers textually (comment changes would
count as breaks, while a type renumbering, which is compatible, would too).

## Appendix A: `cpl.h` and `cpl_adapter.h`

### A.1 `cpl.h` (C API 0.1)

The complete header. It compiles as C11 (`gcc -std=c11 -Wall -Wextra
-Wconversion -Wsign-conversion -Werror -pedantic`) and as C++17 (`g++
-std=c++17 -Wall -Wextra -Wconversion -Wsign-conversion -Wold-style-cast
-Werror -pedantic`), and a host may test its version in `#if`.

```c
/* cpl.h: the CPL C API 0.1 (spec/cabi.md 4 and Appendix A.1). One file, identical in every package.
   Only fixed-width integers, size_t, and pointers appear in the ABI: no C enum, no bool, no bit-field, no long.
   Every function below is a static inline wrapper that calls the entry of the same name in the function table
   (struct cpl_api) that the object it takes carries as its first member, so a call always reaches the runtime
   copy that owns the object. No unwinding may cross a function of this header or a callback it registers:
   a callback returns normally (spec/cabi.md 4.7). */
#ifndef CPL_H
#define CPL_H 1
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* ---- versions --------------------------------------------------------------------------------------------- */
#define CPL_API_VERSION_MAJOR 0u
#define CPL_API_VERSION_MINOR 1u
#define CPL_API_VERSION ((CPL_API_VERSION_MAJOR << 16) | CPL_API_VERSION_MINOR)   /* usable in #if */

/* Layout assertions (spec/cabi.md 1.7): no ABI struct has implicit padding on an LP64 target, and this header and
   every generated header assert their structs' sizes there, so a #pragma pack left active cannot go unnoticed. */
#if defined(__cplusplus)
#if __cplusplus >= 201103L && defined(__LP64__)
#define CPL_LAYOUT_ASSERT(e) static_assert(e, "CPL ABI struct layout (spec/cabi.md 1.7)");
#endif
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L && defined(__LP64__)
#define CPL_LAYOUT_ASSERT(e) _Static_assert(e, "CPL ABI struct layout (spec/cabi.md 1.7)");
#endif
#ifndef CPL_LAYOUT_ASSERT
#define CPL_LAYOUT_ASSERT(e)
#endif

/* ---- statuses (spec/cabi.md 4.6) ---------------------------------------------------------------------------- */
typedef int32_t cpl_status;
#define CPL_OK             0   /* success; outputs set */
#define CPL_ERR            1   /* the export returned Err(e): *err_out holds e; no error object */
#define CPL_FAULT          2   /* an execution fault (semantics 6.7): *error holds its S49 diagnostic */
#define CPL_E_ARGUMENT     3   /* host misuse or ill-typed input: bad, released, or foreign handle; NULL; bad UTF-8 */
#define CPL_E_RANGE        4   /* a checked machine-integer conversion does not fit */
#define CPL_E_NOMEM        5   /* allocation failed outside a CPL evaluation */
#define CPL_E_VERSION      6   /* API version or struct size not supported */
#define CPL_E_STATE        7   /* runtime busy, still in use, or poisoned; an adapter that cannot be granted */
#define CPL_E_CANCELLED    8   /* the call observed its cancellation token */
#define CPL_E_UNSUPPORTED  9   /* option or feature not available in this library or edition */
#define CPL_E_INTERNAL    10   /* a runtime invariant failed: a toolchain bug; the runtime is poisoned */
#define CPL_FOREIGN_ERROR 11   /* only as a capability callback's return value, after cpl_call_foreign_error */
/* 12 is reserved for CPL_PENDING (asynchronous completion, a later minor version) */

/* ---- handles and plain types (spec/cabi.md 4.5) ------------------------------------------------------------- */
typedef struct cpl_value  { uint64_t bits[2]; } cpl_value;    /* owned value handle; {{0, 0}} is "no value" */
typedef struct cpl_cap    { uint64_t bits[2]; } cpl_cap;      /* owned capability handle */
typedef struct cpl_cancel { uint64_t bits[2]; } cpl_cancel;   /* owned cancellation token */
typedef uint32_t cpl_type;                          /* type id: well-known below, package ids from <prefix>_type_* */
typedef uint32_t cpl_kind;
#define CPL_KIND_INT     1u
#define CPL_KIND_BOOL    2u
#define CPL_KIND_TEXT    3u
#define CPL_KIND_BYTES   4u
#define CPL_KIND_LIST    5u
#define CPL_KIND_STRUCT  6u
#define CPL_KIND_ENUM    7u
#define CPL_KIND_WRAPPER 8u
typedef int32_t cpl_foreign_kind;                             /* std.foreign.ForeignErrorKind, declaration order */
#define CPL_FOREIGN_EXCEPTION        0
#define CPL_FOREIGN_INVALID_ARGUMENT 1
#define CPL_FOREIGN_INVALID_RESULT   2
#define CPL_FOREIGN_TRANSPORT        3
#define CPL_FOREIGN_TIMEOUT          4
#define CPL_FOREIGN_CANCELLED        5
#define CPL_FOREIGN_UNAVAILABLE      6
/* well-known type ids, equal in every package (declarations; instantiations such as List<Int> get package ids) */
#define CPL_T_INT                1u
#define CPL_T_BOOL               2u
#define CPL_T_TEXT               3u
#define CPL_T_BYTES              4u
#define CPL_T_UNIT               5u
#define CPL_T_OPTION             6u
#define CPL_T_RESULT             7u
#define CPL_T_CONSTRAINT_ERROR   8u
#define CPL_T_FOREIGN_ERROR      9u
#define CPL_T_FOREIGN_ERROR_KIND 10u
#define CPL_T_FOREIGN_DETAIL     11u
#define CPL_OPTION_SOME 0u                                    /* variant indices of the prelude enums */
#define CPL_OPTION_NONE 1u
#define CPL_RESULT_OK   0u
#define CPL_RESULT_ERR  1u
#define CPL_CONSTRAINT_ERROR_F_TYPE_NAME 0u                   /* field indices, declaration order */
#define CPL_CONSTRAINT_ERROR_F_MESSAGE   1u
#define CPL_FOREIGN_ERROR_F_KIND    0u
#define CPL_FOREIGN_ERROR_F_MESSAGE 1u
#define CPL_FOREIGN_ERROR_F_DETAILS 2u
#define CPL_FOREIGN_DETAIL_F_KEY    0u
#define CPL_FOREIGN_DETAIL_F_VALUE  1u
typedef struct cpl_strview   { const char *ptr; size_t len; } cpl_strview;      /* borrowed UTF-8; ptr[len] == '\0' */
typedef struct cpl_bytesview { const uint8_t *ptr; size_t len; } cpl_bytesview;  /* borrowed */
typedef struct cpl_buf       { char *ptr; size_t len; } cpl_buf;                 /* owned text; cpl_buf_release */
typedef struct cpl_api cpl_api;                                                  /* the function table (4.1) */
typedef struct cpl_runtime { const cpl_api *api; } cpl_runtime;                  /* only the first member is public */
typedef struct cpl_call    { const cpl_api *api; } cpl_call;                     /* a capability callback's context */
typedef struct cpl_error   { const cpl_api *api; } cpl_error;

/* ---- options, information, statistics (spec/cabi.md 4.2, 4.3, 4.8) ----------------------------------------- */
typedef struct cpl_allocator {    /* copied at open; called from any thread, concurrently (spec/cabi.md 4.7) */
    uint32_t struct_size;            /* sizeof(cpl_allocator) in the host's cpl.h */
    uint32_t reserved;               /* padding; never read */
    void *user;
    void *(*alloc)(void *user, size_t size, size_t align);                   /* NULL on failure */
    void  (*free)(void *user, void *ptr, size_t size, size_t align);
} cpl_allocator;
typedef struct cpl_runtime_options {
    uint32_t struct_size;            /* sizeof(cpl_runtime_options) in the host's cpl.h */
    uint32_t api_version;            /* CPL_API_VERSION in the host's cpl.h */
    const cpl_allocator *allocator;  /* NULL: the C library's malloc and free */
    uint64_t max_heap_bytes;         /* 0: unlimited; otherwise a heap budget (spec/cabi.md 2.4) */
    uint32_t max_call_depth;         /* semantics S38; 10000 in CPL_RUNTIME_OPTIONS_INIT; 0 refuses every call */
    uint32_t flags;                  /* none in 0.1; nonzero is CPL_E_UNSUPPORTED */
} cpl_runtime_options;
#define CPL_RUNTIME_OPTIONS_INIT { sizeof(cpl_runtime_options), CPL_API_VERSION, NULL, 0u, 10000u, 0u }
typedef struct cpl_call_options {
    uint32_t struct_size;            /* sizeof(cpl_call_options) */
    uint32_t flags;                  /* CPL_CALL_UNCHECKED_CONTRACTS is reserved for Milestone 5: CPL_E_UNSUPPORTED */
    uint64_t time_budget_ms;         /* 0: none; the budget of this top-level evaluation */
    uint64_t stack_bytes;            /* 0: the calling thread's own bounds */
    cpl_cancel cancel;               /* {{0, 0}}: none */
} cpl_call_options;
#define CPL_CALL_OPTIONS_INIT { sizeof(cpl_call_options), 0u, 0u, 0u, { { 0u, 0u } } }
#define CPL_CALL_UNCHECKED_CONTRACTS 1u
#define CPL_CLOSE_FORCE 1u
typedef struct cpl_library_info {
    uint32_t struct_size, api_version, abi, release;  /* release: built by cpl build --release */
    uint32_t holes, reserved;                         /* holes: unresolved holes linked (0 in a release build) */
    const char *package, *prefix, *edition, *toolchain, *runtime_version, *runtime_digest,
               *program_digest, *exports_digest, *target, *compiler, *profile;
} cpl_library_info;
typedef struct cpl_stats {
    uint32_t struct_size;            /* sizeof(cpl_stats) in the host's cpl.h, set before the call */
    uint32_t reserved;               /* padding; never read or written */
    uint64_t live_handles, live_caps, live_objects, live_bytes, peak_bytes, active_calls, live_buffers;
} cpl_stats;
#define CPL_STATS_INIT { sizeof(cpl_stats), 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u }

/* ---- capability callbacks (the generic form; <prefix>.h has a typed ops struct per capability; 5.7) -------- */
typedef cpl_status (*cpl_op_fn)(void *user, cpl_call *call,
                                const cpl_value *args, size_t nargs,    /* borrowed for the call */
                                cpl_value *result);                     /* owned; transferred on CPL_OK */
typedef struct cpl_cap_ops {
    uint32_t struct_size;
    uint32_t nops;                   /* must equal the capability's operation count */
    const cpl_op_fn *ops;            /* declaration order */
    void (*drop)(void *user);        /* optional; once, after the last reference is gone and no call is in flight */
} cpl_cap_ops;
CPL_LAYOUT_ASSERT(sizeof(cpl_value) == 16 && sizeof(cpl_cap) == 16 && sizeof(cpl_cancel) == 16)
CPL_LAYOUT_ASSERT(sizeof(cpl_strview) == 16 && sizeof(cpl_bytesview) == 16 && sizeof(cpl_buf) == 16)
CPL_LAYOUT_ASSERT(sizeof(cpl_allocator) == 32 && sizeof(cpl_runtime_options) == 32 && sizeof(cpl_call_options) == 40)
CPL_LAYOUT_ASSERT(sizeof(cpl_library_info) == 112 && sizeof(cpl_stats) == 64 && sizeof(cpl_cap_ops) == 24)

/* ---- the function table (spec/cabi.md 4.1): append-only within a major version ---------------------------- */
struct cpl_api {
    uint32_t struct_size;            /* sizeof(struct cpl_api) in the library */
    uint32_t api_version;            /* CPL_API_VERSION of the library */
    /* runtime */
    cpl_status (*cpl_runtime_close)(cpl_runtime *rt, uint32_t flags, cpl_error **error);
    const cpl_library_info *(*cpl_runtime_info)(cpl_runtime *rt);
    void       (*cpl_runtime_stats)(cpl_runtime *rt, cpl_stats *out);
    cpl_status (*cpl_type_lookup)(cpl_runtime *rt, const char *name, size_t len, cpl_type *out, cpl_error **error);
    cpl_status (*cpl_type_name)(cpl_runtime *rt, cpl_type type, cpl_strview *out, cpl_error **error);
    /* any value */
    cpl_status (*cpl_value_dup)(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_value_release)(cpl_runtime *rt, cpl_value v);
    cpl_status (*cpl_value_kind)(cpl_runtime *rt, cpl_value v, cpl_kind *out, cpl_error **error);
    cpl_status (*cpl_value_type)(cpl_runtime *rt, cpl_value v, cpl_type *out, cpl_error **error);
    cpl_status (*cpl_value_equal)(cpl_runtime *rt, cpl_value a, cpl_value b, int32_t *out, cpl_error **error);
    cpl_status (*cpl_value_to_json)(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error);
    cpl_status (*cpl_value_from_json)(cpl_runtime *rt, cpl_type type, const char *json, size_t len,
                                      cpl_value *out, cpl_error **error);
    cpl_status (*cpl_value_to_text)(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error);
    void       (*cpl_buf_release)(cpl_runtime *rt, cpl_buf *buf);
    /* Int and Bool */
    cpl_status (*cpl_int_from_i64)(cpl_runtime *rt, int64_t n, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_int_from_u64)(cpl_runtime *rt, uint64_t n, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_int_from_decimal)(cpl_runtime *rt, const char *s, size_t len, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_int_from_words)(cpl_runtime *rt, int32_t negative, const uint64_t *words, size_t n,
                                     cpl_value *out, cpl_error **error);
    cpl_status (*cpl_int_to_i64)(cpl_runtime *rt, cpl_value v, int64_t *out, cpl_error **error);
    cpl_status (*cpl_int_to_u64)(cpl_runtime *rt, cpl_value v, uint64_t *out, cpl_error **error);
    cpl_status (*cpl_int_to_decimal)(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error);
    cpl_status (*cpl_int_to_words)(cpl_runtime *rt, cpl_value v, int32_t *negative, uint64_t *words, size_t cap,
                                   size_t *needed, cpl_error **error);
    cpl_status (*cpl_bool_new)(cpl_runtime *rt, int32_t b, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_bool_get)(cpl_runtime *rt, cpl_value v, int32_t *out, cpl_error **error);
    /* Text and Bytes */
    cpl_status (*cpl_text_new)(cpl_runtime *rt, const char *utf8, size_t len, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_text_view)(cpl_runtime *rt, cpl_value v, cpl_strview *out, cpl_error **error);
    cpl_status (*cpl_text_scalars)(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error);
    cpl_status (*cpl_bytes_new)(cpl_runtime *rt, const uint8_t *data, size_t len, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_bytes_view)(cpl_runtime *rt, cpl_value v, cpl_bytesview *out, cpl_error **error);
    /* List */
    cpl_status (*cpl_list_new)(cpl_runtime *rt, const cpl_value *items, size_t n, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_list_len)(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error);
    cpl_status (*cpl_list_get)(cpl_runtime *rt, cpl_value v, size_t index, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_list_from_i64)(cpl_runtime *rt, const int64_t *items, size_t n, cpl_value *out,
                                    cpl_error **error);
    cpl_status (*cpl_list_to_i64)(cpl_runtime *rt, cpl_value v, int64_t *buf, size_t cap, size_t *needed,
                                  cpl_error **error);
    /* struct, enum, wrapper, and the prelude values */
    cpl_status (*cpl_struct_new)(cpl_runtime *rt, cpl_type type, const cpl_value *fields, size_t n,
                                 cpl_value *out, cpl_error **error);
    cpl_status (*cpl_struct_field)(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_enum_new)(cpl_runtime *rt, cpl_type type, uint32_t variant, const cpl_value *payload, size_t n,
                               cpl_value *out, cpl_error **error);
    cpl_status (*cpl_enum_variant)(cpl_runtime *rt, cpl_value v, uint32_t *out, cpl_error **error);
    cpl_status (*cpl_enum_payload)(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_wrapper_new)(cpl_runtime *rt, cpl_type type, cpl_value inner, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_wrapper_inner)(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_unit)(cpl_runtime *rt, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_option_some)(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_option_none)(cpl_runtime *rt, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_result_ok)(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_result_err)(cpl_runtime *rt, cpl_value e, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_foreign_error_new)(cpl_runtime *rt, cpl_foreign_kind kind, const char *message, size_t len,
                                        const cpl_strview *keys, const cpl_strview *values, size_t ndetails,
                                        cpl_value *out, cpl_error **error);
    /* cancellation */
    cpl_status (*cpl_cancel_new)(cpl_runtime *rt, cpl_cancel *out, cpl_error **error);
    cpl_status (*cpl_cancel_request)(cpl_runtime *rt, cpl_cancel token);
    cpl_status (*cpl_cancel_release)(cpl_runtime *rt, cpl_cancel token);
    /* capabilities and callbacks */
    cpl_status (*cpl_cap_new)(cpl_runtime *rt, cpl_type capability, const cpl_cap_ops *ops, void *user,
                              cpl_cap *out, cpl_error **error);
    cpl_status (*cpl_cap_release)(cpl_runtime *rt, cpl_cap cap);
    cpl_runtime *(*cpl_call_runtime)(cpl_call *call);
    uint32_t   (*cpl_call_operation)(cpl_call *call);
    int32_t    (*cpl_call_cancelled)(cpl_call *call);
    cpl_status (*cpl_call_foreign_detail)(cpl_call *call, const char *key, size_t key_len,
                                          const char *value, size_t value_len);
    cpl_status (*cpl_call_foreign_error)(cpl_call *call, cpl_foreign_kind kind, const char *message, size_t len);
    /* errors */
    cpl_status  (*cpl_error_status)(const cpl_error *e);
    const char *(*cpl_error_code)(const cpl_error *e);
    const char *(*cpl_error_category)(const cpl_error *e);
    cpl_strview (*cpl_error_message)(const cpl_error *e);
    cpl_strview (*cpl_error_json)(const cpl_error *e);
    void        (*cpl_error_release)(cpl_error *e);
};
CPL_LAYOUT_ASSERT(sizeof(struct cpl_api) == 8 + 8 * 63)   /* the 63 entries of C API 0.1 */

/* True when the library's table has the entry `member` (for entries added after 0.1; every 0.1 entry is present). */
#define CPL_API_HAS(api, member) \
    ((api)->struct_size >= offsetof(struct cpl_api, member) + sizeof((api)->member))

/* ---- runtime ------------------------------------------------------------------------------------------------ */
/* opened by the package:
   cpl_status <prefix>_runtime_open(const cpl_runtime_options *, cpl_runtime **, cpl_error **) */
static inline cpl_status cpl_runtime_close(cpl_runtime *rt, uint32_t flags, cpl_error **error)
{ return rt->api->cpl_runtime_close(rt, flags, error); }
static inline const cpl_library_info *cpl_runtime_info(cpl_runtime *rt)
{ return rt->api->cpl_runtime_info(rt); }
/* writes the counters that out->struct_size covers: initialize *out with CPL_STATS_INIT */
static inline void cpl_runtime_stats(cpl_runtime *rt, cpl_stats *out)
{ rt->api->cpl_runtime_stats(rt, out); }
static inline cpl_status cpl_type_lookup(cpl_runtime *rt, const char *name, size_t len, cpl_type *out,
                                         cpl_error **error)
{ return rt->api->cpl_type_lookup(rt, name, len, out, error); }
static inline cpl_status cpl_type_name(cpl_runtime *rt, cpl_type type, cpl_strview *out, cpl_error **error)
{ return rt->api->cpl_type_name(rt, type, out, error); }

/* ---- any value ---------------------------------------------------------------------------------------------- */
static inline cpl_status cpl_value_dup(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_value_dup(rt, v, out, error); }
/* CPL_E_ARGUMENT for a stale or foreign handle */
static inline cpl_status cpl_value_release(cpl_runtime *rt, cpl_value v)
{ return rt->api->cpl_value_release(rt, v); }
static inline cpl_status cpl_value_kind(cpl_runtime *rt, cpl_value v, cpl_kind *out, cpl_error **error)
{ return rt->api->cpl_value_kind(rt, v, out, error); }
/* struct, enum, wrapper */
static inline cpl_status cpl_value_type(cpl_runtime *rt, cpl_value v, cpl_type *out, cpl_error **error)
{ return rt->api->cpl_value_type(rt, v, out, error); }
static inline cpl_status cpl_value_equal(cpl_runtime *rt, cpl_value a, cpl_value b, int32_t *out, cpl_error **error)
{ return rt->api->cpl_value_equal(rt, a, b, out, error); }
/* value JSON, semantics S40 */
static inline cpl_status cpl_value_to_json(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ return rt->api->cpl_value_to_json(rt, v, out, error); }
/* strict, toolchain 7.3 */
static inline cpl_status cpl_value_from_json(cpl_runtime *rt, cpl_type type, const char *json, size_t len,
                                             cpl_value *out, cpl_error **error)
{ return rt->api->cpl_value_from_json(rt, type, json, len, out, error); }
/* value text, semantics S40 */
static inline cpl_status cpl_value_to_text(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ return rt->api->cpl_value_to_text(rt, v, out, error); }
static inline void cpl_buf_release(cpl_runtime *rt, cpl_buf *buf)
{ rt->api->cpl_buf_release(rt, buf); }

/* ---- Int and Bool ------------------------------------------------------------------------------------------- */
static inline cpl_status cpl_int_from_i64(cpl_runtime *rt, int64_t n, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_int_from_i64(rt, n, out, error); }
static inline cpl_status cpl_int_from_u64(cpl_runtime *rt, uint64_t n, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_int_from_u64(rt, n, out, error); }
/* the grammar of stdlib.md S25 */
static inline cpl_status cpl_int_from_decimal(cpl_runtime *rt, const char *s, size_t len, cpl_value *out,
                                              cpl_error **error)
{ return rt->api->cpl_int_from_decimal(rt, s, len, out, error); }
/* little-endian */
static inline cpl_status cpl_int_from_words(cpl_runtime *rt, int32_t negative, const uint64_t *words, size_t n,
                                            cpl_value *out, cpl_error **error)
{ return rt->api->cpl_int_from_words(rt, negative, words, n, out, error); }
/* CPL_E_RANGE */
static inline cpl_status cpl_int_to_i64(cpl_runtime *rt, cpl_value v, int64_t *out, cpl_error **error)
{ return rt->api->cpl_int_to_i64(rt, v, out, error); }
/* CPL_E_RANGE */
static inline cpl_status cpl_int_to_u64(cpl_runtime *rt, cpl_value v, uint64_t *out, cpl_error **error)
{ return rt->api->cpl_int_to_u64(rt, v, out, error); }
static inline cpl_status cpl_int_to_decimal(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ return rt->api->cpl_int_to_decimal(rt, v, out, error); }
/* words NULL: count only */
static inline cpl_status cpl_int_to_words(cpl_runtime *rt, cpl_value v, int32_t *negative, uint64_t *words,
                                          size_t cap, size_t *needed, cpl_error **error)
{ return rt->api->cpl_int_to_words(rt, v, negative, words, cap, needed, error); }
/* 0 or 1 */
static inline cpl_status cpl_bool_new(cpl_runtime *rt, int32_t b, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_bool_new(rt, b, out, error); }
static inline cpl_status cpl_bool_get(cpl_runtime *rt, cpl_value v, int32_t *out, cpl_error **error)
{ return rt->api->cpl_bool_get(rt, v, out, error); }

/* ---- Text and Bytes ----------------------------------------------------------------------------------------- */
/* copied */
static inline cpl_status cpl_text_new(cpl_runtime *rt, const char *utf8, size_t len, cpl_value *out,
                                      cpl_error **error)
{ return rt->api->cpl_text_new(rt, utf8, len, out, error); }
/* until v is released */
static inline cpl_status cpl_text_view(cpl_runtime *rt, cpl_value v, cpl_strview *out, cpl_error **error)
{ return rt->api->cpl_text_view(rt, v, out, error); }
/* Text.len */
static inline cpl_status cpl_text_scalars(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error)
{ return rt->api->cpl_text_scalars(rt, v, out, error); }
static inline cpl_status cpl_bytes_new(cpl_runtime *rt, const uint8_t *data, size_t len, cpl_value *out,
                                       cpl_error **error)
{ return rt->api->cpl_bytes_new(rt, data, len, out, error); }
static inline cpl_status cpl_bytes_view(cpl_runtime *rt, cpl_value v, cpl_bytesview *out, cpl_error **error)
{ return rt->api->cpl_bytes_view(rt, v, out, error); }

/* ---- List --------------------------------------------------------------------------------------------------- */
static inline cpl_status cpl_list_new(cpl_runtime *rt, const cpl_value *items, size_t n, cpl_value *out,
                                      cpl_error **error)
{ return rt->api->cpl_list_new(rt, items, n, out, error); }
static inline cpl_status cpl_list_len(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error)
{ return rt->api->cpl_list_len(rt, v, out, error); }
static inline cpl_status cpl_list_get(cpl_runtime *rt, cpl_value v, size_t index, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_list_get(rt, v, index, out, error); }
static inline cpl_status cpl_list_from_i64(cpl_runtime *rt, const int64_t *items, size_t n, cpl_value *out,
                                           cpl_error **error)
{ return rt->api->cpl_list_from_i64(rt, items, n, out, error); }
/* buf NULL: count only */
static inline cpl_status cpl_list_to_i64(cpl_runtime *rt, cpl_value v, int64_t *buf, size_t cap, size_t *needed,
                                         cpl_error **error)
{ return rt->api->cpl_list_to_i64(rt, v, buf, cap, needed, error); }

/* ---- struct, enum, wrapper, and the prelude values ---------------------------------------------------------- */
/* declaration order */
static inline cpl_status cpl_struct_new(cpl_runtime *rt, cpl_type type, const cpl_value *fields, size_t n,
                                        cpl_value *out, cpl_error **error)
{ return rt->api->cpl_struct_new(rt, type, fields, n, out, error); }
static inline cpl_status cpl_struct_field(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out,
                                          cpl_error **error)
{ return rt->api->cpl_struct_field(rt, v, index, out, error); }
static inline cpl_status cpl_enum_new(cpl_runtime *rt, cpl_type type, uint32_t variant, const cpl_value *payload,
                                      size_t n, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_enum_new(rt, type, variant, payload, n, out, error); }
static inline cpl_status cpl_enum_variant(cpl_runtime *rt, cpl_value v, uint32_t *out, cpl_error **error)
{ return rt->api->cpl_enum_variant(rt, v, out, error); }
static inline cpl_status cpl_enum_payload(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out,
                                          cpl_error **error)
{ return rt->api->cpl_enum_payload(rt, v, index, out, error); }
static inline cpl_status cpl_wrapper_new(cpl_runtime *rt, cpl_type type, cpl_value inner, cpl_value *out,
                                         cpl_error **error)
{ return rt->api->cpl_wrapper_new(rt, type, inner, out, error); }
static inline cpl_status cpl_wrapper_inner(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_wrapper_inner(rt, v, out, error); }
static inline cpl_status cpl_unit(cpl_runtime *rt, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_unit(rt, out, error); }
static inline cpl_status cpl_option_some(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_option_some(rt, v, out, error); }
static inline cpl_status cpl_option_none(cpl_runtime *rt, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_option_none(rt, out, error); }
static inline cpl_status cpl_result_ok(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_result_ok(rt, v, out, error); }
static inline cpl_status cpl_result_err(cpl_runtime *rt, cpl_value e, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_result_err(rt, e, out, error); }
static inline cpl_status cpl_foreign_error_new(cpl_runtime *rt, cpl_foreign_kind kind, const char *message,
                                               size_t len, const cpl_strview *keys, const cpl_strview *values,
                                               size_t ndetails, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_foreign_error_new(rt, kind, message, len, keys, values, ndetails, out, error); }

/* ---- cancellation ------------------------------------------------------------------------------------------- */
static inline cpl_status cpl_cancel_new(cpl_runtime *rt, cpl_cancel *out, cpl_error **error)
{ return rt->api->cpl_cancel_new(rt, out, error); }
/* any thread; not async-signal-safe (spec/cabi.md 4.4) */
static inline cpl_status cpl_cancel_request(cpl_runtime *rt, cpl_cancel token)
{ return rt->api->cpl_cancel_request(rt, token); }
static inline cpl_status cpl_cancel_release(cpl_runtime *rt, cpl_cancel token)
{ return rt->api->cpl_cancel_release(rt, token); }

/* ---- capabilities and callbacks ----------------------------------------------------------------------------- */
/* takes user only on success */
static inline cpl_status cpl_cap_new(cpl_runtime *rt, cpl_type capability, const cpl_cap_ops *ops, void *user,
                                     cpl_cap *out, cpl_error **error)
{ return rt->api->cpl_cap_new(rt, capability, ops, user, out, error); }
static inline cpl_status cpl_cap_release(cpl_runtime *rt, cpl_cap cap)
{ return rt->api->cpl_cap_release(rt, cap); }
static inline cpl_runtime *cpl_call_runtime(cpl_call *call)
{ return call->api->cpl_call_runtime(call); }
/* operation index, declaration order */
static inline uint32_t cpl_call_operation(cpl_call *call)
{ return call->api->cpl_call_operation(call); }
/* the running call's token was requested */
static inline int32_t cpl_call_cancelled(cpl_call *call)
{ return call->api->cpl_call_cancelled(call); }
static inline cpl_status cpl_call_foreign_detail(cpl_call *call, const char *key, size_t key_len, const char *value,
                                                 size_t value_len)
{ return call->api->cpl_call_foreign_detail(call, key, key_len, value, value_len); }
/* returns CPL_FOREIGN_ERROR; CPL_E_ARGUMENT or CPL_E_NOMEM when the failure cannot be recorded */
static inline cpl_status cpl_call_foreign_error(cpl_call *call, cpl_foreign_kind kind, const char *message,
                                                size_t len)
{ return call->api->cpl_call_foreign_error(call, kind, message, len); }

/* ---- errors (usable after their runtime closed) ------------------------------------------------------------- */
static inline cpl_status cpl_error_status(const cpl_error *e)
{ return e->api->cpl_error_status(e); }
/* "contract_violation", "interop_handle_released", ... (static) */
static inline const char *cpl_error_code(const cpl_error *e)
{ return e->api->cpl_error_code(e); }
/* "contract", "interop", ... (static) */
static inline const char *cpl_error_category(const cpl_error *e)
{ return e->api->cpl_error_category(e); }
static inline cpl_strview cpl_error_message(const cpl_error *e)
{ return e->api->cpl_error_message(e); }
/* the S49 diagnostic object, compact JSON */
static inline cpl_strview cpl_error_json(const cpl_error *e)
{ return e->api->cpl_error_json(e); }
/* NULL and static error objects: no-op */
static inline void cpl_error_release(cpl_error *e)
{ if (e != NULL) e->api->cpl_error_release(e); }

#ifdef __cplusplus
}
#endif
#endif
```

### A.2 `cpl_adapter.h`

```c
/* cpl_adapter.h: what the adapter of a backend-"c" binding is written against (spec/cabi.md 6.2 and
   Appendix A.2). It is cpl.h plus CPL_ADAPTER and four static inline helpers built only on the functions of
   cpl.h; it adds no entry to struct cpl_api. */
#ifndef CPL_ADAPTER_H
#define CPL_ADAPTER_H 1
#include <string.h>
#include "cpl.h"
#ifdef __cplusplus
extern "C" {
#endif

/* The visibility of the adapter contract's functions: hidden, so they are never exported from the package. */
#if defined(__GNUC__) || defined(__clang__)
#define CPL_ADAPTER __attribute__((visibility("hidden")))
#else
#define CPL_ADAPTER
#endif

/* A checked conversion of the Int argument v to [lo, hi]. Outside it (or outside int64_t) the call reports
   InvalidArgument and the helper returns CPL_FOREIGN_ERROR, which the operation function returns in turn.
   A bad handle returns that status (a binding protocol error, spec/cabi.md 5.7). */
static inline cpl_status cpl_adapter_int_arg(cpl_call *call, cpl_value v, int64_t lo, int64_t hi, int64_t *out)
{
    static const char message[] = "an integer argument is outside the range of the C parameter";
    int64_t n = 0;
    cpl_status st = cpl_int_to_i64(cpl_call_runtime(call), v, &n, NULL);
    if (st == CPL_OK && n >= lo && n <= hi) {
        *out = n;
        return CPL_OK;
    }
    if (st != CPL_OK && st != CPL_E_RANGE)
        return st;
    return cpl_call_foreign_error(call, CPL_FOREIGN_INVALID_ARGUMENT, message, sizeof message - 1);
}

/* A NUL-terminated copy of the UTF-8 span [ptr, ptr + len) into buf (cap bytes). An interior U+0000, which a C
   string cannot carry, or a buffer shorter than len + 1 reports InvalidArgument and returns CPL_FOREIGN_ERROR. */
static inline cpl_status cpl_adapter_cstr(cpl_call *call, const char *ptr, size_t len, char *buf, size_t cap)
{
    static const char interior[] = "a text argument contains U+0000, which a C string cannot carry";
    static const char too_long[] = "a text argument is longer than the C buffer";
    if (len > 0 && memchr(ptr, 0, len) != NULL)
        return cpl_call_foreign_error(call, CPL_FOREIGN_INVALID_ARGUMENT, interior, sizeof interior - 1);
    if (cap == 0 || len > cap - 1)
        return cpl_call_foreign_error(call, CPL_FOREIGN_INVALID_ARGUMENT, too_long, sizeof too_long - 1);
    if (len > 0)
        memcpy(buf, ptr, len);
    buf[len] = '\0';
    return CPL_OK;
}

/* Adds the detail foreign_code (the decimal text of code) and reports a failure of kind `kind` with the
   NUL-terminated message, which must be NULL (an empty message) or strict UTF-8. Returns CPL_FOREIGN_ERROR, or the
   status of the API call that failed (CPL_E_NOMEM; CPL_E_ARGUMENT for a message that is not UTF-8), which the
   operation function returns in turn. */
static inline cpl_status cpl_adapter_fail_code(cpl_call *call, cpl_foreign_kind kind, int64_t code,
                                               const char *message)
{
    static const char digit[] = "9876543210123456789";     /* digit[9 + r] is the digit of a remainder r in -9..9 */
    char digits[21];
    size_t at = sizeof digits;
    int64_t rest = code;
    cpl_status st;
    do {
        digits[--at] = digit[9 + rest % 10];
        rest /= 10;
    } while (rest != 0);
    if (code < 0)
        digits[--at] = '-';
    st = cpl_call_foreign_detail(call, "foreign_code", sizeof "foreign_code" - 1, digits + at, sizeof digits - at);
    if (st != CPL_OK)
        return st;
    return cpl_call_foreign_error(call, kind, message, message != NULL ? strlen(message) : 0);
}

/* Ok(v) into *result, releasing v (the caller's handle) in every case; returns the status of cpl_result_ok. */
static inline cpl_status cpl_adapter_ok(cpl_runtime *rt, cpl_value v, cpl_value *result)
{
    cpl_status st = cpl_result_ok(rt, v, result, NULL);
    (void)cpl_value_release(rt, v);
    return st;
}

#ifdef __cplusplus
}
#endif
#endif
```

## Appendix B: the internal headers

### B.1 `cpl_rt.h`, the internal contract

`cpl_rt.h` is the contract between the runtime and emitted code (Section
1): the word, the object layouts, the slot macros, capability references,
frames, the evaluation context, the program tables that `cpl-emit-c`
writes and the runtime reads, the entry points of the program unit, and
the prototypes of every helper that emitted code and the API unit call.
It is internal: compiled only as C11 into a package's own units (it
refuses C++), never installed for hosts, never an ABI, and changed only
with the runtime digest. Every name, field, and value it shows is
normative for Milestone 2; the runtime may add private functions, and
private members after the ones shown, and the layouts of a fault record
and of the runtime object are its own (`cplrt_fault`, `cplrt_runtime`,
declared for the runtime's files in `cpl_rt_internal.h`, Section 2.1). A
unit includes it after the namespace prologue for its package's prefix
`<p>`:

```c
#define CPLRT_NS <p>
#define CPLRT(name) cplrt_<p>__##name
#define CPLPKG(name) cplpkg_<p>__##name
```

It compiles with `gcc -std=c11 -Wall -Wextra -Werror -pedantic`, and the
examples of Sections 1.4 and 5.5 compile against it with the base flags of
Section 1.5 and `-Werror -pedantic`.

```c
/* cpl_rt.h: the internal contract between the CPL C runtime and the C that cpl-emit-c emits (spec/cabi.md
   Appendix B.1). Internal: never installed for hosts and never an ABI; it changes with the runtime digest.
   C11 only. Every external-linkage name is namespaced by CPLRT() or CPLPKG() (spec/cabi.md 1.6). */
#ifndef CPL_RT_H
#define CPL_RT_H 1
#ifdef __cplusplus
#error "cpl_rt.h is C11: the runtime and the emitted code are compiled as C (spec/cabi.md 1.5)"
#endif
#if !defined(CPLRT_NS) || !defined(CPLRT) || !defined(CPLPKG)
#error "begin the unit with the namespace prologue of spec/cabi.md Appendix B.1 before including cpl_rt.h"
#endif
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include "cpl.h"

/* ---- names (1.6) -------------------------------------------------------------------------------------------- */
/* The prologue defines CPLRT(name) as cplrt_<prefix>__##name and CPLPKG(name) as cplpkg_<prefix>__##name, the prefix
   inside one token so that no macro named like it (linux, unix) can replace it; CPLRT_NS only marks the prologue. */
/* On the definition of every function the version script lists (<prefix>_api.c): the base flags make every
   other symbol hidden, and a version script cannot export a hidden symbol. */
#if defined(__GNUC__) || defined(__clang__)
#define CPLRT_EXPORT __attribute__((visibility("default")))
#else
#define CPLRT_EXPORT
#endif

/* ---- statuses and sentinels (1.2) --------------------------------------------------------------------------- */
#define CPLRT_OK 0                     /* the internal status of every emitted function and helper */
#define CPLRT_FAULT 1                  /* a fault is held in cx->fault (cancellation included, 3.6) */
#define CPLRT_NO_SITE UINT32_MAX       /* the site of a top-level activation */
#define CPLRT_NONE UINT32_MAX          /* "no entry" in a table reference */

/* ---- the word (2.3) ----------------------------------------------------------------------------------------- */
typedef uint64_t cplrt_w;
#define CPLRT_EMPTY ((cplrt_w)0)       /* a slot that holds nothing; never a value of the language */
#define CPLRT_FALSE ((cplrt_w)2)
#define CPLRT_TRUE ((cplrt_w)6)
#define CPLRT_SMALL_MIN (-(INT64_C(1) << 62))
#define CPLRT_SMALL_MAX ((INT64_C(1) << 62) - 1)
static inline int cplrt_is_small(cplrt_w w) { return (int)(w & 1u); }
static inline int cplrt_is_obj(cplrt_w w) { return w != 0u && (w & 7u) == 0u; }
static inline cplrt_w cplrt_small(int64_t v) { return ((uint64_t)v << 1) | 1u; }   /* v in [SMALL_MIN, SMALL_MAX] */
static inline int64_t cplrt_small_value(cplrt_w w)          /* portable: no implementation-defined shift */
{ return (int64_t)((w >> 1) ^ (UINT64_C(1) << 62)) - (INT64_C(1) << 62); }
static inline cplrt_w cplrt_bool(int b) { return b ? CPLRT_TRUE : CPLRT_FALSE; }
static inline int cplrt_truth(cplrt_w w) { return w == CPLRT_TRUE; }

/* ---- heap objects (2.3) ------------------------------------------------------------------------------------- */
#define CPLRT_BIGINT 1u
#define CPLRT_TEXT 2u
#define CPLRT_BYTES 3u
#define CPLRT_LIST 4u
#define CPLRT_STRUCT 5u
#define CPLRT_ENUM 6u
#define CPLRT_WRAPPER 7u
#define CPLRT_F_NEGATIVE 0x01u         /* a negative bigint */
#define CPLRT_F_WIDE 0x02u             /* an enum value whose variant index is the small Int in item[0] (2.3) */
#define CPLRT_F_VIEW 0x08u             /* a List that views len items of another list from u.live.def (2.3) */
#define CPLRT_RC_SATURATED UINT32_C(0x80000000)   /* every count at or above it is immortal (2.4) */
#define CPLRT_RC_IMMORTAL UINT32_C(0xC0000000)    /* the count of statics and of objects whose count saturated */
typedef struct cplrt_obj {
    uint8_t kind;                      /* CPLRT_BIGINT, _TEXT, _BYTES, _LIST, _STRUCT, _ENUM, _WRAPPER */
    uint8_t flags;
    uint16_t variant;                  /* enums */
    uint32_t len;                      /* children or limbs; Text and Bytes keep size_t lengths in their bodies */
    union {
        struct {
            _Atomic uint32_t rc;       /* CPLRT_RC_IMMORTAL for statics; >= CPLRT_RC_SATURATED: immortal */
            uint32_t def;              /* DefId.0 of a struct, enum, or wrapper */
        } live;
        struct cplrt_obj *next;        /* once dead: the pending-release link */
    } u;
} cplrt_obj;
_Static_assert(sizeof(cplrt_obj) == 16, "the object header is 16 bytes");
typedef struct cplrt_bigint { cplrt_obj hdr; uint32_t limb[]; } cplrt_bigint;        /* len limbs, least first */
typedef struct cplrt_text { cplrt_obj hdr; size_t bytes; size_t scalars; char utf8[]; } cplrt_text; /* NUL after */
typedef struct cplrt_bytes { cplrt_obj hdr; size_t len; uint8_t data[]; } cplrt_bytes;
typedef struct cplrt_node { cplrt_obj hdr; cplrt_w item[]; } cplrt_node;          /* List, struct, enum, wrapper */
typedef struct cplrt_listview { cplrt_obj hdr; cplrt_w base; } cplrt_listview;    /* a List view: base is never one */
static inline cplrt_obj *cplrt_obj_of(cplrt_w w) { return (cplrt_obj *)(uintptr_t)w; }
static inline cplrt_w cplrt_word_of(const cplrt_obj *o) { return (cplrt_w)(uintptr_t)o; }

/* ---- reference counts and the slot macros (1.3, 2.4) -------------------------------------------------------- */
void CPLRT(release_obj)(cplrt_obj *o);                     /* the last reference is gone: iterative release */
static inline void cplrt_retain(cplrt_w w)
{
    if (cplrt_is_obj(w)) {
        cplrt_obj *o = cplrt_obj_of(w);
        if (atomic_load_explicit(&o->u.live.rc, memory_order_relaxed) < CPLRT_RC_SATURATED &&
            atomic_fetch_add_explicit(&o->u.live.rc, 1u, memory_order_relaxed) >= CPLRT_RC_SATURATED - 1u)
            atomic_store_explicit(&o->u.live.rc, CPLRT_RC_IMMORTAL, memory_order_relaxed);   /* saturation */
    }
}
/* 0, immediates, and immortals: nothing. An acquire-release decrement and no standalone fence, which
   ThreadSanitizer does not model (2.4). */
static inline void cplrt_release(cplrt_w w)
{
    if (cplrt_is_obj(w)) {
        cplrt_obj *o = cplrt_obj_of(w);
        if (atomic_load_explicit(&o->u.live.rc, memory_order_relaxed) < CPLRT_RC_SATURATED &&
            atomic_fetch_sub_explicit(&o->u.live.rc, 1u, memory_order_acq_rel) == 1u)
            CPLRT(release_obj)(o);
    }
}
/* The only writes of a slot: store an owned word, store a retained borrowed word, move between slots. */
#define CPLRT_SET(slot, owned) \
    do { cplrt_w cplrt_set_ = (owned); cplrt_release(slot); (slot) = cplrt_set_; } while (0)
#define CPLRT_COPY(slot, borrowed) \
    do { cplrt_w cplrt_copy_ = (borrowed); cplrt_retain(cplrt_copy_); CPLRT_SET(slot, cplrt_copy_); } while (0)
#define CPLRT_MOVE(dst, src) \
    do { cplrt_w cplrt_move_ = (src); (src) = 0; cplrt_release(dst); (dst) = cplrt_move_; } while (0)

/* ---- borrowed access (2.3, 2.6); every result is borrowed from v ---------------------------------------------- */
static inline uint32_t cplrt_len(cplrt_w v) { return cplrt_obj_of(v)->len; }
/* The children of a composite; a List view's are the items of its base it views. */
static inline const cplrt_w *cplrt_items(cplrt_w v)
{
    const cplrt_obj *o = cplrt_obj_of(v);
    if (o->flags & CPLRT_F_VIEW)
        return ((const cplrt_node *)cplrt_obj_of(((const cplrt_listview *)o)->base))->item + o->u.live.def;
    return ((const cplrt_node *)o)->item;
}
static inline cplrt_w cplrt_item(cplrt_w v, uint32_t i) { return cplrt_items(v)[i]; }
/* A struct, enum, or wrapper child: never a view. */
static inline cplrt_w cplrt_node_item(cplrt_w v, uint32_t i) { return ((const cplrt_node *)cplrt_obj_of(v))->item[i]; }
static inline cplrt_w cplrt_field(cplrt_w v, uint32_t i) { return cplrt_node_item(v, i); }
static inline cplrt_w cplrt_inner(cplrt_w v) { return cplrt_node_item(v, 0); }
static inline uint32_t cplrt_variant(cplrt_w v)
{
    const cplrt_obj *o = cplrt_obj_of(v);
    return (o->flags & CPLRT_F_WIDE) ? (uint32_t)cplrt_small_value(cplrt_node_item(v, 0)) : o->variant;
}
static inline uint32_t cplrt_payload_len(cplrt_w v)
{ return cplrt_len(v) - ((cplrt_obj_of(v)->flags & CPLRT_F_WIDE) ? 1u : 0u); }
static inline cplrt_w cplrt_payload(cplrt_w v, uint32_t i)
{ return cplrt_node_item(v, i + ((cplrt_obj_of(v)->flags & CPLRT_F_WIDE) ? 1u : 0u)); }
static inline int cplrt_is_ok(cplrt_w v) { return cplrt_variant(v) == CPL_RESULT_OK; }
static inline const cplrt_text *cplrt_text_of(cplrt_w v) { return (const cplrt_text *)cplrt_obj_of(v); }
static inline const cplrt_bytes *cplrt_bytes_of(cplrt_w v) { return (const cplrt_bytes *)cplrt_obj_of(v); }
static inline int cplrt_int_sign_big(cplrt_w v) { return (cplrt_obj_of(v)->flags & CPLRT_F_NEGATIVE) ? -1 : 1; }
/* An Int against a small constant b (normalized bigints lie outside the small range, 2.5). */
static inline int cplrt_int_eq_small(cplrt_w a, int64_t b) { return a == cplrt_small(b); }
static inline int cplrt_int_ne_small(cplrt_w a, int64_t b) { return a != cplrt_small(b); }
static inline int cplrt_int_lt_small(cplrt_w a, int64_t b)
{ return cplrt_is_small(a) ? cplrt_small_value(a) < b : cplrt_int_sign_big(a) < 0; }
static inline int cplrt_int_gt_small(cplrt_w a, int64_t b)
{ return cplrt_is_small(a) ? cplrt_small_value(a) > b : cplrt_int_sign_big(a) > 0; }
static inline int cplrt_int_le_small(cplrt_w a, int64_t b) { return !cplrt_int_gt_small(a, b); }
static inline int cplrt_int_ge_small(cplrt_w a, int64_t b) { return !cplrt_int_lt_small(a, b); }
static inline int cplrt_text_eq(cplrt_w a, cplrt_w b)     /* byte equality: scalar equality for valid UTF-8 */
{
    const cplrt_text *x = cplrt_text_of(a), *y = cplrt_text_of(b);
    return a == b || (x->bytes == y->bytes && memcmp(x->utf8, y->utf8, x->bytes) == 0);
}

/* ---- capability references (2.6, 5.7) ----------------------------------------------------------------------- */
typedef struct cplrt_capref cplrt_capref;
typedef struct cplrt_capimpl {                  /* how the operations of one capability object reach their code */
    uint32_t nops;
    /* the binding layer of operation op (5.7, 6.2, 6.3): args are borrowed words in parameter order; on CPL_OK
       *result is an owned word; any other status follows the callback contract of 5.7 */
    cpl_status (*invoke)(const cplrt_capref *cap, cpl_call *call, uint32_t op, const cplrt_w *args, cplrt_w *result);
    void (*drop)(const cplrt_capref *cap);      /* once, after the last reference (5.7): the host's drop or fini */
} cplrt_capimpl;
struct cplrt_capref {                           /* the runtime's capability object begins with it */
    const cplrt_capimpl *impl;
    void *user;                                 /* the host's user data, or the adapter's state */
    uint32_t capability;                        /* DefId.0 of the capability declaration */
    uint32_t handle;                            /* the $handle number: creation order in the runtime, from 0 */
};

/* ---- activations and the evaluation context (1.2, 3.3-3.6, 4.3) --------------------------------------------- */
typedef struct cplrt_frame {
    struct cplrt_frame *prev;                   /* the enclosing activation; NULL at the top */
    uint32_t def;                               /* DefId.0 of the activation */
    uint32_t site;                              /* CPL_S of the call that entered it; CPLRT_NO_SITE at the top */
    const cplrt_w *params;                      /* function activations: the borrowed positional parameters */
    uint32_t nparams;
    uint32_t call;                              /* 1 for a CPL function call, which the depth limit counts */
    cplrt_w measure;                            /* the header decreases measure recorded on entry (S35), 0: none */
} cplrt_frame;
typedef struct cplrt_fault cplrt_fault;         /* a fault record (3.3.1); its layout is the runtime's */
typedef struct cplrt_runtime cplrt_runtime;     /* begins with the public cpl_runtime (4.1) */
typedef struct cplrt_program cplrt_program;
typedef struct cplrt_cx {                       /* one top-level evaluation, on the caller's C stack (4.3) */
    cplrt_runtime *rt;
    const cplrt_program *program;
    cplrt_frame *top;                           /* the innermost activation; NULL when none */
    uint32_t calls;                             /* active CPL function calls (S72) */
    uint32_t max_call_depth;
    int check_contracts;                        /* contract regions run (3.7) */
    int attributed;                             /* the propagating fault was claimed by a clause (S34, S70) */
    int cancelled;                              /* the fault path is a cancellation unwind (3.6) */
    uintptr_t stack_floor;                      /* the lowest frame address an activation may start at (3.4) */
    uint64_t deadline_ns;                       /* monotonic deadline (3.6); 0: no time budget */
    uint64_t budget_ms;                         /* the `limit` of a time fault */
    const _Atomic uint32_t *cancel;             /* the call's cancellation flag; NULL: none */
    const char *range_text;                     /* drivers: the clause text of the range bound being evaluated */
    cplrt_fault *fault;                         /* the fault being raised; NULL when none */
    cplrt_fault *violation;                     /* a copy of the last ensures, invariant, or decreases violation */
    cplrt_fault *memory;                        /* the preallocated resource_exhaustion (memory) record (4.7) */
    int q_ok;                                   /* the scalar tier may be tried (1.3a); off after CPLRT_Q_MAX_BAILS */
    uint32_t q_bails;                           /* scalar-tier attempts that bailed in this evaluation */
    int q_late;                                 /* a scalar-tier poll saw an interruption: the next entry skips its own */
} cplrt_cx;

/* ---- the program tables, written by cpl-emit-c into <prefix>_tables.c (3.2) ---------------------------------- */
#define CPLRT_DEF_FN 1u
#define CPLRT_DEF_WRAPPER 2u
#define CPLRT_DEF_REFINEMENT 3u
#define CPLRT_DEF_STRUCT 4u
#define CPLRT_DEF_ENUM 5u
#define CPLRT_DEF_CAPABILITY 6u
#define CPLRT_DEF_TEST 7u
#define CPLRT_DEF_PROPERTY 8u
#define CPLRT_DEF_BUILTIN_TYPE 9u
typedef struct cplrt_op {                       /* one capability operation */
    const char *name;
    const char *identity;                       /* operation_identity: reader.pages.PageRead.get */
    uint32_t nparams;
    const char *const *params;                  /* parameter names: the `input` keys */
    const uint32_t *sorted;                     /* parameter indices in byte order of their names */
    uint32_t ret;                               /* CPL_X of the declared return type (6.4) */
    uint32_t ret_foreign;                       /* 1 when the return type is Result<T, std.foreign.ForeignError> */
    const uint32_t *ptypes;                     /* CPL_X of each parameter: a request's arguments are checked */
} cplrt_op;
typedef struct cplrt_def {                      /* CPL_D: one per DefId, std included */
    const char *stable_id;                      /* Def::stable_id: `symbol` */
    const char *qualified_name;                 /* Program::display_path: `qualified_name`, backtraces, `caller` */
    const char *type_name;                      /* types::type_name: `$type`, ConstraintError.type_name (S44) */
    const char *bare_name;                      /* types::bare_name: value text */
    uint32_t kind;                              /* CPLRT_DEF_* */
    uint32_t nmembers;                          /* fields, variants, positional parameters, operations, or binders */
    const char *const *members;                 /* their names; a member stub's receiver is "self" */
    const uint32_t *sorted;                     /* member indices in byte order of their names (JSON keys) */
    cplrt_obj *const *statics;                  /* enum: per variant, its immortal value if payload-less, else NULL */
    const cplrt_op *ops;                        /* capability: nmembers operations */
    uint32_t nrequires;                         /* fn: its requires clauses (a builtin stub's from requires_spec) */
    const uint32_t *requires;                   /* CPL_CL indices */
    const uint32_t *ptypes;                     /* in a driver: CPL_X of each positional parameter of a fn or binder
                                                   of a property; else NULL */
    const uint32_t *psites;                     /* fn: CPL_S of each positional parameter (the span of a boundary
                                                   refinement_violation, 5.5 step 3); else NULL */
    uint32_t nusing;                            /* fn: its using parameters */
    const uint32_t *ucaps;                      /* fn: DefId.0 of each using parameter's capability; else NULL */
} cplrt_def;
typedef struct cplrt_site {                     /* CPL_S: every span a fault can report */
    uint32_t file;                              /* FileId.0, an index into cplrt_program.files */
    uint32_t start, end;                        /* byte offsets */
    uint32_t line, col, end_line, end_col;      /* SourceMap::location: the S49 span */
} cplrt_site;
#define CPLRT_CK_REQUIRES 1u
#define CPLRT_CK_ENSURES 2u
#define CPLRT_CK_INVARIANT 3u
#define CPLRT_CK_DECREASES 4u
#define CPLRT_CK_WHERE 5u                       /* a refinement predicate */
#define CPLRT_CK_RANGE 6u                       /* a property's for range (S71) */
typedef struct cplrt_clause {                   /* CPL_CL */
    const char *text;                           /* Clause::text (T56): `clause` */
    uint32_t kind;                              /* CPLRT_CK_* */
    uint32_t site;                              /* CPL_S of the clause */
    uint32_t owner;                             /* DefId.0 of the owning function, refinement, property, capability */
    uint32_t op;                                /* an operation clause: the operation index; else CPLRT_NONE */
    const char *owner_id;                       /* the `symbol` an S34 claim names */
    const char *owner_qualified;                /* its `qualified_name` */
    uint32_t bsite;                             /* a loop clause: CPL_A of the loop; else CPLRT_NONE */
} cplrt_clause;
typedef struct cplrt_binding {                  /* one visible local */
    const char *name;
    const char *type;                           /* display_ty_in with the enclosing generics */
    const char *kind;                           /* BindingKind::as_str */
    uint32_t is_mutable;                        /* 1 for a var */
    uint32_t capability;                        /* a using parameter: DefId.0 of its capability, else CPLRT_NONE */
} cplrt_binding;
typedef struct cplrt_bsite {                    /* CPL_A: the locals visible at an assert, a loop, or a hole */
    const char *text;                           /* an assert: Stmt::Assert::text; else NULL */
    uint32_t site;                              /* an assert: the asserted expression; else CPLRT_NONE */
    uint32_t n;
    const cplrt_binding *bindings;              /* innermost first: the order of a `visible` argument */
    uint32_t nobject;                           /* a loop: the actual.bindings object, one key per name */
    const uint32_t *object;                     /* indices into bindings (the innermost per name), in name order */
} cplrt_bsite;
typedef struct cplrt_hole {                     /* CPL_H, by HoleId */
    const char *name;                           /* without the ? */
    const char *expected_type;                  /* display_ty_in of the hole's type */
    uint32_t site;
    uint32_t bsite;
} cplrt_hole;
typedef struct cplrt_refinement {               /* CPL_R */
    uint32_t def;                               /* DefId.0 */
    const char *type_name;                      /* constraint_type_name */
    const char *declared_name;                  /* DeclName::display: the expected_type of a boundary violation */
    const char *predicate;                      /* the predicate's Clause::text */
    uint32_t clause;                            /* CPL_CL of the predicate */
    uint32_t base;                              /* CPL_X of the root base strip(R) (types::strip) */
    uint32_t nchain;                            /* check_order(def), root first, as CPL_R indices */
    const uint32_t *chain;
} cplrt_refinement;
#define CPLRT_TY_INT 1u
#define CPLRT_TY_BOOL 2u
#define CPLRT_TY_TEXT 3u
#define CPLRT_TY_BYTES 4u
#define CPLRT_TY_LIST 5u
#define CPLRT_TY_STRUCT 6u
#define CPLRT_TY_ENUM 7u
#define CPLRT_TY_WRAPPER 8u
#define CPLRT_TY_REFINED 9u
#define CPLRT_TY_CAPABILITY 10u
#define CPLRT_TY_PARAM 11u                      /* a type parameter of a generic declaration: matches any value */
typedef struct cplrt_type {                     /* CPL_X: the index is the package's cpl_type id */
    const char *text;                           /* the qualified type text (5.3): cpl_type_name, interop_* payloads */
    const char *display;                        /* Program::display_ty: the <expected_type> of stdlib S45 and semantics S80 */
    uint32_t form;                              /* CPLRT_TY_* */
    uint32_t def;                               /* struct, enum, wrapper, capability: DefId.0; refined: CPL_R */
    uint32_t elem;                              /* List: element; wrapper: inner; refined: the root base strip(R) */
    uint32_t nmembers;                          /* struct: fields; enum: variants */
    const uint32_t *members;                    /* struct: CPL_X per field; enum: nmembers + 1 payload offsets */
    const uint32_t *payloads;                   /* enum: the payload types of every variant, concatenated */
} cplrt_type;
typedef struct cplrt_std {                      /* DefId.0 of the std declarations the runtime constructs */
    uint32_t unit, option, result, constraint_error, foreign_error, foreign_error_kind, foreign_detail;
} cplrt_std;
#define CPLRT_AF_HANDLE 1u                      /* export argument forms (5.4) */
#define CPLRT_AF_I64 2u
#define CPLRT_AF_I32 3u
#define CPLRT_AF_U64 4u
#define CPLRT_AF_U32 5u
#define CPLRT_AF_BOOL 6u
#define CPLRT_AF_TEXT 7u
#define CPLRT_AF_BYTES 8u
#define CPLRT_AF_UNIT 9u                        /* no C parameter, no output */
typedef struct cplrt_export {
    const char *symbol;
    uint32_t function;                          /* DefId.0 */
    uint32_t nparams;
    const uint32_t *forms;                      /* per positional parameter: CPLRT_AF_* */
    const uint32_t *types;                      /* per positional parameter: CPL_X of its declared type */
    uint32_t nusing;
    const uint32_t *caps;                       /* per using parameter: DefId.0 of its capability */
    uint32_t split;                             /* 1: a Result return split into ok_out (CPL_OK), err_out (CPL_ERR) */
    uint32_t ok_form, err_form;                 /* the result's form (ok_form alone when not split): CPLRT_AF_* */
} cplrt_export;
typedef struct cplrt_grant {                    /* a backend-c binding (6.1) */
    const char *binding;                        /* the manifest table name */
    uint32_t capability;                        /* DefId.0 */
    const char *label;                          /* the adapter detail of a ForeignError: the binding name */
    cpl_status (*init)(cpl_runtime *rt, void **state, cpl_error **error);
    void (*fini)(void *state);
    const cplrt_capimpl *impl;                  /* the generated binding layer over the adapter's functions */
    uint32_t declarative;                       /* 1: a generated wrapper (6.3), whose init fails other than with
                                                   CPL_E_NOMEM only when [state] create returned NULL */
} cplrt_grant;

/* ---- the entry points of <prefix>_prog.c (CPLPKG(code)) --------------------------------------------------- */
typedef int (*cplrt_fn_entry)(cplrt_cx *cx, const cplrt_w *args, const cplrt_capref *const *using, cplrt_w *out);
typedef int (*cplrt_pred_entry)(cplrt_cx *cx, uint32_t site, cplrt_w self, int *holds);
typedef int (*cplrt_test_entry)(cplrt_cx *cx, const cplrt_w *binders);
typedef int (*cplrt_bound_entry)(cplrt_cx *cx, cplrt_w *out);
typedef struct cplrt_bound { uint32_t property, binder, hi; cplrt_bound_entry entry; } cplrt_bound;
typedef struct cplrt_code {
    uint32_t ndefs;
    const cplrt_fn_entry *fns;                  /* by DefId.0: exports' trampolines; with CPL_WITH_TESTS, those of
                                                   every fn with a block body and every callable builtin stub */
    const cplrt_pred_entry *preds;              /* by DefId.0: cpl_p<n> of every refinement */
    const cplrt_test_entry *tests;              /* by DefId.0: cpl_t<n> (CPL_WITH_TESTS only; else NULL) */
    uint32_t nbounds;
    const cplrt_bound *bounds;                  /* cpl_b<n>_<binder>_lo and _hi (CPL_WITH_TESTS only) */
} cplrt_code;
struct cplrt_program {                          /* CPLPKG(program), in <prefix>_tables.c */
    cpl_library_info info;                      /* what <prefix>_library_info returns (4.2) */
    const cplrt_code *code;
    uint32_t ndefs;       const cplrt_def *defs;
    uint32_t nfiles;      const char *const *files;   /* project-relative paths, '/' separators */
    uint32_t nsites;      const cplrt_site *sites;
    uint32_t nclauses;    const cplrt_clause *clauses;
    uint32_t nbsites;     const cplrt_bsite *bsites;
    uint32_t nholes;      const cplrt_hole *holes;
    uint32_t nrefs;       const cplrt_refinement *refinements;
    uint32_t ntypes;      const cplrt_type *types;    /* [0] unused; [1..11] the well-known types of cpl.h */
    uint32_t nexports;    const cplrt_export *exports;
    uint32_t ngrants;     const cplrt_grant *grants;
    cplrt_std std;
    uint32_t max_call_depth;                    /* [run] max_call_depth at build time: the executable's default (8) */
};
extern const cplrt_program CPLPKG(program);
extern const cplrt_code CPLPKG(code);

/* ---- coverage (1.4): only drivers emitted with EmitConfig::coverage, compiled with -DCPLRT_COVERAGE=1, count -- */
#if defined(CPLRT_COVERAGE) && CPLRT_COVERAGE
extern _Atomic uint64_t CPLPKG(cov)[];                    /* <prefix>_tables.c: one counter per lowered HIR node */
extern const char *const CPLPKG(cov_kinds)[];             /* its HIR variant: "ExprKind::Call", "Stmt::While", ... */
extern const uint32_t CPLPKG(ncov);
#define CPLRT_COV(k) ((void)atomic_fetch_add_explicit(&CPLPKG(cov)[k], 1u, memory_order_relaxed))
#else
#define CPLRT_COV(k) ((void)0)
#endif

/* ---- helpers called by emitted code. Every int-returning helper returns CPLRT_OK or CPLRT_FAULT; every
   `cplrt_w *out` has CPLRT_SET semantics; word arguments are borrowed. ------------------------------------------ */
#define cplrt_enter_fn CPLRT(enter_fn)
#define cplrt_enter_act CPLRT(enter_act)
#define cplrt_leave CPLRT(leave)
#define cplrt_tick CPLRT(tick)
/* depth (S72), stack guard, time, cancellation, then the frame push; refusal leaves no frame (3.4) */
int cplrt_enter_fn(cplrt_cx *cx, cplrt_frame *fr, uint32_t def, uint32_t site, const cplrt_w *params, uint32_t n);
int cplrt_enter_act(cplrt_cx *cx, cplrt_frame *fr, uint32_t def, uint32_t site);  /* the same without the depth */
int cplrt_leave(cplrt_cx *cx, cplrt_frame *fr, int st);    /* pop; on a fault append {def, site} to the backtrace */
int cplrt_tick(cplrt_cx *cx, uint32_t site);               /* before a loop condition: time and cancellation */

#define cplrt_int_add CPLRT(int_add)
#define cplrt_int_sub CPLRT(int_sub)
#define cplrt_int_mul CPLRT(int_mul)
#define cplrt_int_neg CPLRT(int_neg)
#define cplrt_int_divrem CPLRT(int_divrem)
#define cplrt_int_cmp CPLRT(int_cmp)
int cplrt_int_add(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out);
int cplrt_int_sub(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out);
int cplrt_int_mul(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out);
int cplrt_int_neg(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w *out);
/* the only division (T9): quotient toward zero, remainder a - q * b (S37); either output may be NULL;
   a zero divisor raises division_by_zero at site with input {"a", "b"} */
int cplrt_int_divrem(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *q, cplrt_w *r);
static inline int cplrt_int_div(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{ return cplrt_int_divrem(cx, site, a, b, out, NULL); }
static inline int cplrt_int_rem(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{ return cplrt_int_divrem(cx, site, a, b, NULL, out); }
int cplrt_int_cmp(cplrt_w a, cplrt_w b);                   /* -1, 0, 1; never fails */

#define cplrt_equal CPLRT(equal)
#define cplrt_concat CPLRT(concat)
#define cplrt_list_new CPLRT(list_new)
#define cplrt_struct_new CPLRT(struct_new)
#define cplrt_enum_new CPLRT(enum_new)
#define cplrt_wrap CPLRT(wrap)
#define cplrt_result_ok CPLRT(result_ok)
#define cplrt_result_err CPLRT(result_err)
#define cplrt_option_some CPLRT(option_some)
#define cplrt_check_failure CPLRT(check_failure)
int cplrt_equal(cplrt_cx *cx, cplrt_w a, cplrt_w b, int *eq);           /* structural ==, iterative (2.8) */
int cplrt_concat(cplrt_cx *cx, cplrt_w a, cplrt_w b, cplrt_w *out);     /* ++ on List, Text, Bytes */
int cplrt_list_new(cplrt_cx *cx, const cplrt_w *items, uint32_t n, cplrt_w *out);
int cplrt_struct_new(cplrt_cx *cx, uint32_t def, const cplrt_w *fields, uint32_t n, cplrt_w *out);
int cplrt_enum_new(cplrt_cx *cx, uint32_t def, uint32_t variant, const cplrt_w *payload, uint32_t n, cplrt_w *out);
int cplrt_wrap(cplrt_cx *cx, uint32_t def, cplrt_w inner, cplrt_w *out);
int cplrt_result_ok(cplrt_cx *cx, cplrt_w v, cplrt_w *out);
int cplrt_result_err(cplrt_cx *cx, cplrt_w e, cplrt_w *out);
int cplrt_option_some(cplrt_cx *cx, cplrt_w v, cplrt_w *out);
/* check<R>: Err(ConstraintError) for the refinement CPL_R `refinement` whose predicate was false for v */
int cplrt_check_failure(cplrt_cx *cx, uint32_t refinement, cplrt_w v, cplrt_w *out);

#define cplrt_requires_failed CPLRT(requires_failed)
#define cplrt_ensures_failed CPLRT(ensures_failed)
#define cplrt_header_measure CPLRT(header_measure)
#define cplrt_invariant_failed CPLRT(invariant_failed)
#define cplrt_loop_measure CPLRT(loop_measure)
#define cplrt_op_requires_failed CPLRT(op_requires_failed)
#define cplrt_op_ensures_failed CPLRT(op_ensures_failed)
#define cplrt_claim CPLRT(claim)
#define cplrt_claim_range CPLRT(claim_range)
#define cplrt_refine_unsound CPLRT(refine_unsound)
#define cplrt_assert_failed CPLRT(assert_failed)
#define cplrt_hole CPLRT(hole)
#define cplrt_no_match CPLRT(no_match)
#define cplrt_internal CPLRT(internal)
#define cplrt_bi_uncallable CPLRT(bi_uncallable)
/* Each *_failed, hole, and internal helper sets cx->fault (or the memory record) and returns nothing. A `visible`
   array (1.4) holds the words of a binding site's locals, innermost first; the entry of a binding whose
   cplrt_binding.capability is not CPLRT_NONE (a using parameter) is (cplrt_w)(uintptr_t) of its
   const cplrt_capref *, which the runtime reads only for its handle number and never retains or releases. */
void cplrt_requires_failed(cplrt_cx *cx, uint32_t clause);      /* input: the frame; caller: the frame below */
void cplrt_ensures_failed(cplrt_cx *cx, uint32_t clause, cplrt_w result);
int cplrt_header_measure(cplrt_cx *cx, cplrt_frame *fr, uint32_t clause, cplrt_w measure);   /* S35 */
void cplrt_invariant_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *visible, uint32_t n, uint64_t iteration);
/* a loop measure after the condition held: >= 0 and below *previous, else the decreases violation; on success
   *previous holds the measure */
int cplrt_loop_measure(cplrt_cx *cx, uint32_t clause, const cplrt_w *visible, uint32_t n, uint64_t iteration,
                       cplrt_w measure, cplrt_w *previous);
void cplrt_op_requires_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *args, uint32_t n);
void cplrt_op_ensures_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *args, uint32_t n, cplrt_w result);
void cplrt_claim(cplrt_cx *cx, uint32_t clause);           /* S34 and S70: a fault escaped the clause */
void cplrt_claim_range(cplrt_cx *cx, uint32_t property);   /* S71: with cx->range_text */
void cplrt_refine_unsound(cplrt_cx *cx, uint32_t site, uint32_t refinement, cplrt_w v);   /* T39 */
void cplrt_assert_failed(cplrt_cx *cx, uint32_t bsite, const cplrt_w *visible, uint32_t n);
void cplrt_hole(cplrt_cx *cx, uint32_t hole, const cplrt_w *visible, uint32_t n);
/* internal_error no_matching_arm at the match `site`: "no arm of the match matches <value text of scrutinee>" */
void cplrt_no_match(cplrt_cx *cx, uint32_t site, cplrt_w scrutinee);
void cplrt_internal(cplrt_cx *cx, uint32_t site, const char *kind, const char *detail);
/* a call of the stub `stub` whose builtin `id` is not callable (1.4): gen_range's precondition first, then
   builtin_type_called or generator_not_a_value with the evaluator's detail and the frame {stub, site}; always
   CPLRT_FAULT */
int cplrt_bi_uncallable(cplrt_cx *cx, uint32_t site, uint32_t stub, const char *id, const cplrt_w *args, uint32_t n);

#define cplrt_cap_invoke CPLRT(cap_invoke)
/* a capability operation's host call: one reserved handle slot per handle argument (5.7), the binding layer, the
   foreign-failure mapping (6.5), and validation (6.4) */
int cplrt_cap_invoke(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, uint32_t op,
                     const cplrt_w *args, uint32_t n, cplrt_w *out);

/* ---- the 46 callable builtins (2.7): cplrt_bi_<id>(cx, site, stub, arguments..., out) ----------------------- */
#define CPLRT_BI1(id) int CPLRT(bi_##id)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
#define CPLRT_BI2(id) \
    int CPLRT(bi_##id)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
CPLRT_BI1(flatten); CPLRT_BI1(int_to_text); CPLRT_BI1(int_abs); CPLRT_BI2(int_min); CPLRT_BI2(int_max);
CPLRT_BI1(text_len); CPLRT_BI1(text_is_empty); CPLRT_BI1(bytes_len); CPLRT_BI1(bytes_is_empty);
CPLRT_BI1(list_len); CPLRT_BI1(list_is_empty); CPLRT_BI2(list_take); CPLRT_BI2(list_drop); CPLRT_BI1(list_drop_last);
CPLRT_BI2(list_push); CPLRT_BI2(list_get); CPLRT_BI1(list_first); CPLRT_BI1(list_last); CPLRT_BI1(list_reverse);
CPLRT_BI2(list_contains); CPLRT_BI1(option_is_some); CPLRT_BI1(option_is_none); CPLRT_BI2(option_unwrap_or);
CPLRT_BI2(option_ok_or); CPLRT_BI1(result_is_ok); CPLRT_BI1(result_is_err); CPLRT_BI2(result_unwrap_or);
CPLRT_BI1(result_ok); CPLRT_BI1(result_err); CPLRT_BI1(text_trim); CPLRT_BI1(text_trim_start);
CPLRT_BI1(text_trim_end);
CPLRT_BI2(text_starts_with); CPLRT_BI2(text_ends_with); CPLRT_BI2(text_contains); CPLRT_BI2(text_split);
CPLRT_BI2(text_join); CPLRT_BI1(text_to_upper); CPLRT_BI1(text_to_lower); CPLRT_BI1(text_parse_int);
CPLRT_BI1(text_from_int); CPLRT_BI1(text_chars); CPLRT_BI1(bytes_from_list); CPLRT_BI1(bytes_to_list);
CPLRT_BI1(bytes_from_text); CPLRT_BI1(bytes_to_text);
#undef CPLRT_BI1
#undef CPLRT_BI2
#define cplrt_bi(id) CPLRT(bi_##id)                         /* emitted code calls cplrt_bi(list_take)(cx, ...) */

/* ---- helpers called by <prefix>_api.c (4.2, 5.3-5.7, 6.1) ---------------------------------------------------- */
typedef union cplrt_arg {                       /* one C argument of an export, by its form */
    cpl_value value;
    int64_t i64;
    int32_t i32;
    uint64_t u64;
    uint32_t u32;
    struct { const void *ptr; size_t len; } span;               /* CPLRT_AF_TEXT and CPLRT_AF_BYTES */
} cplrt_arg;
typedef union cplrt_out {                       /* one output pointer of an export, by its form */
    cpl_value *value;
    int64_t *i64;
    int32_t *i32;
    uint64_t *u64;
    uint32_t *u32;
} cplrt_out;
#define cplrt_runtime_open CPLRT(runtime_open)
#define cplrt_export_call CPLRT(export_call)
#define cplrt_cap_create CPLRT(cap_create)
#define cplrt_call_borrow CPLRT(call_borrow)
#define cplrt_call_take CPLRT(call_take)
#define cplrt_cap_ops CPLRT(cap_ops)
#define cplrt_grant_call CPLRT(grant_call)
cpl_status cplrt_runtime_open(const cplrt_program *p, const cpl_runtime_options *options, cpl_runtime **out,
                              cpl_error **error);
/* 5.5 steps 1-4 for export `index`; args and caps in parameter order, outs: [ok_out, err_out] or [out] */
cpl_status cplrt_export_call(cpl_runtime *rt, uint32_t index, const cplrt_arg *args, const cpl_cap *caps,
                             const cplrt_out *outs, const cpl_call_options *options, cpl_error **error);
/* <prefix>_<capability>_new (5.7): the typed ops struct (size: its sizeof in the library; op_offsets: where its
   operation pointers are, declaration order) is validated and copied; user is taken only on CPL_OK */
cpl_status cplrt_cap_create(cpl_runtime *rt, uint32_t capability, const cplrt_capimpl *impl, const void *ops,
                            size_t size, const size_t *op_offsets, void *user, cpl_cap *out, cpl_error **error);
const void *cplrt_cap_ops(const cplrt_capref *cap);        /* the runtime's copy of the typed ops struct */
/* a borrowed handle, valid until the callback returns, in a slot that cplrt_cap_invoke reserved before the call
   (5.7), so it cannot fail */
cpl_value cplrt_call_borrow(cpl_call *call, cplrt_w w);
cpl_status cplrt_call_take(cpl_call *call, cpl_status st, cpl_value result, cplrt_w *out);   /* 5.7 result rule */
cpl_status cplrt_grant_call(cpl_runtime *rt, uint32_t grant, cpl_cap *out, cpl_error **error);   /* 6.1 */
#endif
```

### B.2 `cpl_rt_gen.h`, the generated tables

`cpl_rt::generated_header` (toolchain 1.12) produces `cpl_rt_gen.h` from
`cpl-rtspec` (toolchain 1.11, T70): the same text for every package, which
the runtime digest covers (Section 2.2). Only the runtime's units include
it. The declarations below are exact; an entry shown is the value
`cpl-rtspec` holds for edition 0.1, and a `/* ... */` stands for the
entries the generator writes in the order its comment states. Every array
is `static const`, with its count beside it, so the C side reads it
without a second definition.

```c
/* cpl_rt_gen.h: the tables of cpl-rtspec formatted as C (spec/cabi.md 2.9 and Appendix B.2). Generated at emit
   time by cpl_rt::generated_header; never edited or checked in. Included only by the runtime's units. */
#ifndef CPL_RT_GEN_H
#define CPL_RT_GEN_H 1
#include <stddef.h>
#include <stdint.h>

/* ---- message templates: one per entry of cpl_rtspec::messages::ALL, in its order, named CPLRT_MSG_ and the Rust
   constant's name, its text a C string literal holding the {name} placeholders of the Rust text unchanged -------- */
#define CPLRT_MSG_CONTRACT_VIOLATION "{clause_kind} clause failed: {clause}"
#define CPLRT_MSG_DIVISION_BY_ZERO "division by zero"
#define CPLRT_MSG_INTERNAL_ERROR "internal error ({kind}): {detail}"
/* ... */

/* ---- White_Space (cpl_rtspec::whitespace::RANGES): the 25 scalars of stdlib.md S22 as inclusive ranges --------- */
typedef struct cplrt_range { uint32_t lo, hi; } cplrt_range;
#define CPLRT_WHITE_SPACE_N 10u
static const cplrt_range CPLRT_WHITE_SPACE[CPLRT_WHITE_SPACE_N] = {
    { 0x9u, 0xdu }, { 0x20u, 0x20u }, { 0x85u, 0x85u }, { 0xa0u, 0xa0u }, { 0x1680u, 0x1680u },
    { 0x2000u, 0x200au }, { 0x2028u, 0x2029u }, { 0x202fu, 0x202fu }, { 0x205fu, 0x205fu }, { 0x3000u, 0x3000u } };

/* ---- the builtin registry (cpl_rtspec::builtins::BUILTINS, sorted by id) ------------------------------------- */
typedef struct cplrt_requires_spec {            /* one precondition clause, the fallback of requires_spec */
    const char *clause;                         /* its canonical text */
    uint32_t ninputs;
    const char *const *inputs;                  /* the input names, "self" for a member's receiver */
} cplrt_requires_spec;
typedef struct cplrt_builtin_spec {
    const char *id;
    uint32_t callable;                          /* 0 for the 6 type ids and the 7 generator ids */
    uint32_t nrequires;
    const char *fallback_symbol;                /* the stub's qualified name, for a program that does not declare it */
    const cplrt_requires_spec *requires;        /* the precondition clauses in source order (stdlib.md 10.4) */
} cplrt_builtin_spec;
#define CPLRT_BUILTINS_N 59u
static const cplrt_builtin_spec CPLRT_BUILTINS[CPLRT_BUILTINS_N] = {
    { "bool", 0u, 0u, "std.prelude.Bool", NULL },
    /* ... one entry per id, in byte order of the ids */
};

/* ---- limits (cpl_rtspec::limits) --------------------------------------------------------------------------- */
#define CPLRT_LIMIT_DEFAULT_MAX_CALL_DEPTH 10000u     /* semantics S38 */
#define CPLRT_LIMIT_EVAL_STACK_SIZE 1073741824u       /* the evaluation thread: 1 GiB */
#define CPLRT_LIMIT_GUARD_MARGIN 67108864u            /* 64 MiB, or an eighth of a smaller stack */
#define CPLRT_LIMIT_API_MAJOR 0u                      /* C API 0.1 */
#define CPLRT_LIMIT_API_MINOR 1u
#define CPLRT_LIMIT_DRIVER_PROTOCOL 1u                /* cpl-driver protocol 1 */

/* ---- std.foreign (cpl_rtspec::foreign) ---------------------------------------------------------------------- */
#define CPLRT_FOREIGN_KINDS_N 7u                       /* ForeignErrorKind in declaration order: the CPL_FOREIGN_* */
static const char *const CPLRT_FOREIGN_KINDS[CPLRT_FOREIGN_KINDS_N] = {
    "Exception", "InvalidArgument", "InvalidResult", "Transport", "Timeout", "Cancelled", "Unavailable" };
#define CPLRT_FOREIGN_DETAIL_ORDER_N 7u                /* the fixed detail keys of stdlib.md S45, in their order */
static const char *const CPLRT_FOREIGN_DETAIL_ORDER[CPLRT_FOREIGN_DETAIL_ORDER_N] = {
    "adapter", "capability", "operation", "foreign_type", "foreign_code", "pointer", "expected_type" };

/* ---- the interop codes (cpl_rtspec::interop::CODES, in the order of spec/cabi.md 4.9) ----------------------- */
typedef struct cplrt_interop_code { const char *code; int32_t status; } cplrt_interop_code;
#define CPLRT_INTEROP_N 24u
static const cplrt_interop_code CPLRT_INTEROP[CPLRT_INTEROP_N] = {
    { "interop_version_mismatch", 6 }, { "interop_struct_size_unsupported", 6 },   /* the status as in cpl.h */
    /* ... one entry per code */
};
/* the index of each code in CPLRT_INTEROP: CPLRT_INTEROP_ and the code without its interop_ prefix, in upper case */
#define CPLRT_INTEROP_VERSION_MISMATCH 0u
#define CPLRT_INTEROP_STRUCT_SIZE_UNSUPPORTED 1u
/* ... */
#endif
```

## Appendix C: generated headers (normative examples)

### C.1 `reader.h`

What `cpl build --target c` generates for the verbatim `examples/reader`
(default export set; its binding has backend `python`, so it has no
grant), with the digests shown as placeholders. It compiles together with
`cpl.h` as C11 and as C++17.

```c
/* reader.h: generated by cpl 0.1.0 (edition 0.1, C API 0.1) for project "reader"; do not edit.
   Regenerate with `cpl build --target c`; the CPL sources and cpl.toml are the editing authority.
   runtime <64 hex>; program <64 hex>; exports <64 hex>; target x86_64-linux-gnu */
#ifndef READER_H
#define READER_H 1
#include "cpl.h"
#if CPL_API_VERSION_MAJOR != 0 || CPL_API_VERSION_MINOR < 1
#error "reader.h was generated for CPL C API 0.1"
#endif
#ifdef __cplusplus
extern "C" {
#endif
#define READER_ABI 0u

/* ---- package ---- */
const cpl_library_info *reader_library_info(void);
cpl_status reader_runtime_open(const cpl_runtime_options *options, cpl_runtime **out, cpl_error **error);

/* ---- types; ids are valid only with this package's runtimes ---- */
/* List<Int> */
cpl_type reader_type_list_int(void);
/* List<List<Int>> */
cpl_type reader_type_list_list_int(void);
/* Result<reader.images.Dimensions, std.foreign.ForeignError> */
cpl_type reader_type_result_dimensions_foreign_error(void);
/* Result<reader.pages.Page, reader.pages.StoreError> */
cpl_type reader_type_result_page_store_error(void);
/* struct reader.images.Dimensions { width: Positive, height: Positive } */
cpl_type reader_type_dimensions(void);
#define READER_DIMENSIONS_F_WIDTH 0u
#define READER_DIMENSIONS_F_HEIGHT 1u
/* capability reader.images.ImageCodec */
cpl_type reader_type_image_codec(void);
/* struct reader.pages.Page { id: PageId, text: Text } */
cpl_type reader_type_page(void);
#define READER_PAGE_F_ID 0u
#define READER_PAGE_F_TEXT 1u
/* type reader.pages.PageId(Text) */
cpl_type reader_type_page_id(void);
/* capability reader.pages.PageRead */
cpl_type reader_type_page_read(void);
/* type reader.pages.Positive = Int where self > 0 */
cpl_type reader_type_positive(void);
/* enum reader.pages.StoreError { Missing(PageId), Unavailable } */
cpl_type reader_type_store_error(void);
#define READER_STORE_ERROR_V_MISSING 0u
#define READER_STORE_ERROR_V_UNAVAILABLE 1u

/* ---- capabilities ---- */
/* capability reader.images.ImageCodec */
typedef struct reader_image_codec_ops {
    /* sizeof(reader_image_codec_ops) */
    uint32_t struct_size;
    /* padding; never read */
    uint32_t reserved;
    /* fn dimensions(image: Bytes) -> Result<Dimensions, ForeignError>
       image: a borrowed byte span, valid for the call
       on CPL_OK *result: an owned Result<Dimensions, ForeignError>, validated by the runtime before CPL sees it */
    cpl_status (*dimensions)(void *user, cpl_call *call, const uint8_t *image, size_t image_len, cpl_value *result);
    /* optional; called once, after the last reference is released and no call is in flight */
    void (*drop)(void *user);
} reader_image_codec_ops;
CPL_LAYOUT_ASSERT(sizeof(reader_image_codec_ops) == 24u)
cpl_status reader_image_codec_new(cpl_runtime *rt, const reader_image_codec_ops *ops, void *user,
                                  cpl_cap *out, cpl_error **error);
/* capability reader.pages.PageRead */
typedef struct reader_page_read_ops {
    /* sizeof(reader_page_read_ops) */
    uint32_t struct_size;
    /* padding; never read */
    uint32_t reserved;
    /* fn get(id: PageId) -> Result<Page, StoreError>
       id: borrowed for the call
       on CPL_OK *result: an owned Result<Page, StoreError>, validated by the runtime before CPL sees it */
    cpl_status (*get)(void *user, cpl_call *call, cpl_value id, cpl_value *result);
    /* optional; called once, after the last reference is released and no call is in flight */
    void (*drop)(void *user);
} reader_page_read_ops;
CPL_LAYOUT_ASSERT(sizeof(reader_page_read_ops) == 24u)
cpl_status reader_page_read_new(cpl_runtime *rt, const reader_page_read_ops *ops, void *user,
                                cpl_cap *out, cpl_error **error);

/* ---- exports ---- */
/* reader.images.image_width(image: Bytes, using codec: ImageCodec) -> Result<Positive, ForeignError>
   identity reader.images.image_width (provisional); effects { codec.dimensions }
   image is copied in. codec is borrowed.
   CPL_OK: *ok_out is an owned Positive. CPL_ERR: *err_out is an owned ForeignError.
   CPL_FAULT and CPL_E_*: *error, when error is not NULL. */
cpl_status reader_image_width(cpl_runtime *rt, const uint8_t *image, size_t image_len, cpl_cap codec,
                              cpl_value *ok_out, cpl_value *err_out,
                              const cpl_call_options *options, cpl_error **error);
/* reader.pages.load_page(id: PageId, using db: PageRead) -> Result<Page, StoreError>
   identity reader.pages.load_page (provisional); effects { db.get }
   id is borrowed. db is borrowed.
   CPL_OK: *ok_out is an owned Page. CPL_ERR: *err_out is an owned StoreError.
   CPL_FAULT and CPL_E_*: *error, when error is not NULL. */
cpl_status reader_load_page(cpl_runtime *rt, cpl_value id, cpl_cap db,
                            cpl_value *ok_out, cpl_value *err_out,
                            const cpl_call_options *options, cpl_error **error);
/* reader.pages.load_text(id: PageId, using db: PageRead) -> Result<Text, StoreError>
   identity reader.pages.load_text (provisional); effects { db.get }
   id is borrowed. db is borrowed.
   CPL_OK: *ok_out is an owned Text. CPL_ERR: *err_out is an owned StoreError.
   CPL_FAULT and CPL_E_*: *error, when error is not NULL. */
cpl_status reader_load_text(cpl_runtime *rt, cpl_value id, cpl_cap db,
                            cpl_value *ok_out, cpl_value *err_out,
                            const cpl_call_options *options, cpl_error **error);
/* reader.pages.paginate_ints(items: List<Int>, size: Int) -> Result<List<List<Int>>, ConstraintError>
   identity reader.pages.paginate_ints (provisional); effects {}
   items is borrowed. size is borrowed.
   CPL_OK: *ok_out is an owned List<List<Int>>. CPL_ERR: *err_out is an owned ConstraintError.
   CPL_FAULT and CPL_E_*: *error, when error is not NULL. */
cpl_status reader_paginate_ints(cpl_runtime *rt, cpl_value items, cpl_value size,
                                cpl_value *ok_out, cpl_value *err_out,
                                const cpl_call_options *options, cpl_error **error);

/* not exported: reader.pages.paginate is generic (project_export_skipped_generic) */
#ifdef __cplusplus
}
#endif
#endif
```

### C.2 `cboundary_adapter_image_codec.h`

The adapter header of the `cboundary` fixture's binding `image_codec`
(Section 6.2). It compiles, with `cpl_adapter.h` and the fixture's
`cboundary.h`, as C11 and as C++17.

```c
/* cboundary_adapter_image_codec.h: generated by cpl 0.1.0 for binding "image_codec"
   (capability reader.images.ImageCodec, backend c, isolation in_process). Define every function below in
   adapters/image_codec.c. They have hidden visibility and are called only by the runtime. */
#ifndef CBOUNDARY_ADAPTER_IMAGE_CODEC_H
#define CBOUNDARY_ADAPTER_IMAGE_CODEC_H 1
#include "cpl_adapter.h"
#include "cboundary.h"
#ifdef __cplusplus
extern "C" {
#endif
/* once per grant: the adapter's state for this capability (*state and *error are NULL on entry); on any status
   but CPL_OK the grant fails and fini is not called */
CPL_ADAPTER cpl_status cboundary_adapter_image_codec_init(cpl_runtime *rt, void **state, cpl_error **error);
/* once, after the last reference to the granted capability is released and no call is in flight; any thread */
CPL_ADAPTER void cboundary_adapter_image_codec_fini(void *state);
/* fn dimensions(image: Bytes) -> Result<Dimensions, ForeignError>
   image: a borrowed byte span, valid for the call
   on CPL_OK *result: an owned Result<Dimensions, ForeignError>, validated by the runtime before CPL sees it */
CPL_ADAPTER cpl_status cboundary_adapter_image_codec_dimensions(void *state, cpl_call *call,
                                                               const uint8_t *image, size_t image_len,
                                                               cpl_value *result);
#ifdef __cplusplus
}
#endif
#endif
```
