# CPL semantics, edition 0.1 (Milestones 1 and 2)

This document defines the static semantics (name resolution, typing, effect
checking, contract well-formedness) and the dynamic semantics (evaluation,
checked contracts, faults, tests, holes, build modes, structured
diagnostics) of CPL 0.1 for Milestones 1 and 2. The rules bind every CPL
runtime. The Milestone 1 runtime is the *reference evaluator*, a
tree-walking interpreter over a typed IR in the Rust workspace at the
repository root (`crates/cpl-sema` owns Sections 1-5 and 8-9,
`crates/cpl-eval` owns Sections 6-7, `crates/cpl-diag` owns the envelope of
Section 9). Milestone 2 adds the *native backend*: C that
`crates/cpl-emit-c` emits from the checked typed IR, linked with the C
runtime of `crates/cpl-rt` (`spec/cabi.md` Sections 1-2), which realizes
the dynamic rules of Sections 5-8 a second time and must produce what the
evaluator produces (S75); the evaluator stays the oracle. Milestone 5
verification is only sketched where its vocabulary must already be fixed.

Decision: [D-071] S51. Milestone 1 executes CPL through the reference evaluator
rather than through the native backend that the design's milestone table
names ("native execution", design line 644). This is a deliberate deviation
from the design's implementation order, not a reading of it. Every dynamic
rule of Sections 5-8 is therefore stated as a requirement on "the runtime"
that the reference evaluator and the later native backend must both
satisfy; observable behaviour (values, faults, diagnostics, `cpl run` and
`cpl test` results) may not differ between them (S75 makes this exact and
lists the divergences it sanctions), and the evaluator remains the test
oracle for the backend. Rationale: the design leaves the backend open ("C
emission or direct LLVM lowering", lines 38 and 659), and a backend can
only be validated against semantics that are already fixed; an evaluator
with no code generation fixes them for the whole acceptance corpus first,
and the design's "examples execute" criterion is met by `cpl run` and `cpl
test` over it. Rejected alternative: writing the native backend first
(every semantic question in this document would then be decided by emitted
code, with no independent oracle).

Decision: [D-277] S75. The native backend produces what the reference
evaluator produces; this amends S51, whose "may not differ" it makes exact.
Take a checked program, a function, test, or property of it, arguments, an
evaluation configuration (the call-depth limit of S38, a time budget, and
whether contracts are checked), and capability bindings that behave
identically in both engines (each operation call receives the same answer
from its binding in both when the two engines make the same calls in the
same order). The native backend must then produce exactly one of: (a) the
same value, whose value JSON (S40) at the declared type is byte-identical
in its compact serialization and whose value text is identical; or (b) the
same fault, whose diagnostic (S49) is byte-identical in its compact JSON
serialization (`Diagnostic::to_json`, `spec/toolchain.md` 1.4): `code`,
`category`, `severity`, `message`, `span`, `symbol`, `qualified_name`,
`facet`, `notes`, `backtrace` with every frame, and every code-specific
field of 9.3-9.4 (`input`, `actual`, `bindings` with their values,
`caller`, `iteration`, `clause`, `clause_kind`, `attributed_to`, `status`,
`hole`, `expected_type`, `resource`, `limit`, `depth`, `kind`, `detail`,
`parameter`, `pointer`, and the fields of S77 and S80). Under `cpl test` it
produces the same `test_result` and `contract_counterexample` diagnostics
(S46) with the same receipts (S59), byte for byte, except `duration_ms`,
the `engine` keys of `spec/toolchain.md` T73, and the `engine` and `native`
entries that a run on the native engine adds to a receipt's
`check_configuration` (`spec/toolchain.md` 8.2). What the C runtime hands a
C host (a fault's diagnostic, value JSON, value text; `spec/cabi.md`
Section 4) is byte-identical to the evaluator's rendering of the same
outcome, and a native executable (`spec/cabi.md` Section 8), which runs one
function with `cpl run`'s semantics (S39, S78), reports the value or fault
that `cpl run` reports. A cancellation that a C host requests
(`spec/cabi.md` Section 4) is neither a value nor a fault, and no `cpl run`
or `cpl test` can request one, so it takes no part in this comparison.
Exactly these divergences are sanctioned, and each is tested for its
documented shape (`spec/toolchain.md` 12):

| Divergence | What must still agree | Why it is sanctioned |
| - | - | - |
| the host-stack guard, whose fault is the `resource_exhaustion` that carries `depth` (9.4): whether, in which top-level evaluation, and where the guard trips in either engine, and so, for the `cpl run`, the C export call, or the test or property (any of its cases and shrink reruns) in which either engine trips it, everything the two engines report for it but what the next column lists | when both report a guard fault as the outcome (the fault of the run or the call, or the `fault` of the `test_result`): its `code`, `category`, `severity`, `resource: "stack"`, `limit`, and the presence of `depth`; under `cpl test`, the `test_result`'s `symbol`, `qualified_name`, `seed`, `property_seed`, and `receipt` in every case; nothing else | the guard measures the host stack, of which an evaluator activation uses several host frames and an emitted one about one, and S38 requires only that its exhaustion fault, never crash: where the guard trips, and whether it trips before the evaluation ends, is the engine's, and an engine that does not trip it has evaluated past the point where the other stopped; the toolchain's engines evaluate on stacks of the evaluator's reserved size (`spec/cabi.md` Section 3), so under the default limit the depth counter, on which both agree, decides in both, and this row arises only for a `max_call_depth` deeper than the evaluator's stack holds, or on a C host's own thread |
| a time budget (`resource: "time"`): whether, in which top-level evaluation, and where the deadline is first observed in either engine, and so, for the C export call, or the test or property (any of its cases and shrink reruns) in which either engine observes it, everything the two engines report for it but what the next column lists | when both report a time fault as the outcome: its `code`, `category`, `severity`, `message`, `resource: "time"`, and `limit`; under `cpl test`, the `test_result`'s `symbol`, `qualified_name`, `seed`, `property_seed`, and `receipt` in every case; nothing else | the deadline is wall-clock time, which the two engines spend at different rates (emitted code runs many times faster) although they read the clock at the same points (`spec/cabi.md` Section 3), so an engine that does not observe the deadline has evaluated past the point where the other stopped |
| memory exhaustion: the native runtime raises `resource_exhaustion` with `resource: "memory"` (S76); the evaluator aborts its process, except where its adapter host reports the failure, which it raises as the same code at the operation call (S76) | nothing is compared; the native fault is tested on its own (`spec/toolchain.md` 12) | safe Rust cannot observe allocation failure, so the evaluator has no outcome of its own to compare, and the fault that its adapter host reports takes its span and frames from the evaluator, not from the native runtime's preallocated record (S76; the evaluator's own code never raises `Resource::Memory`, which reaches it only in a fault record rebuilt from a driver, `spec/toolchain.md` 1.7), while a native library must not abort its host |
| a crash or memory error inside foreign code | nothing | foreign code executing in process is part of the trusted boundary and outside CPL semantics (design line 487): natively it ends the host process, while in the evaluator's adapter host it becomes `ForeignError { kind: Transport }` or the `internal_error` of S80 (`spec/stdlib.md` S45) |
| `duration_ms`; the `engine` keys of `spec/toolchain.md` T73; the `engine` and `native` entries that a native run adds to a receipt's `check_configuration` (`spec/toolchain.md` 8.2) | nothing | timing, the name of the engine, and the digests of the native runtime and its compiler, not outcomes |

Any other difference is a defect of the native backend, unless the
evaluator contradicts this document, which then decides (S51); how the two
engines are compared and how this table is applied to their outputs (the
S75 normalization) are `spec/toolchain.md` 12.6 (T78), over the C
renderings and counters that the native side supplies (`spec/cabi.md` 3.9),
and how a difference is reported is `spec/toolchain.md` T73. Rationale: the
design asks the native backend to "preserve CPL semantics" (line 659) and
puts every host binding on "the same native CPL core" (line 38), and a
failure response is structured data that agents act on (line 227), so the
two engines must agree on every field, not on the code alone; each
sanctioned row is a behaviour that no CPL rule fixes or that the evaluator
cannot have (the host stack's use, wall-clock time, allocation failure in
safe Rust, a crash of foreign code), listed so that a test can tell it from
a defect. Rejected alternatives: comparing codes, or parsed JSON, only
(misses `input`, `actual`, `bindings`, backtraces, and the C renderer's
escaping and key order, which is where two implementations drift); equality
by construction through one Rust core that both engines share
(`spec/toolchain.md` T68: a bug of the shared code would be invisible to
the oracle); no sanctioned divergence (the evaluator would have to observe
allocation failure, which safe Rust cannot, or the native runtime would
have to abort on it).

It is derived from `design-doc.md`, which remains authoritative for intent,
and from `spec/grammar.md`, whose token classes (`LOWER_IDENT`,
`UPPER_IDENT`, `INT`, `TEXT`, `HOLE`), nonterminal names (`fn_decl`,
`block`, `match_arm`, ...) and decisions (`D1`-`D73`) are used without
redefinition. Library signatures (`std.text`, `std.foreign`, the `gen`
generators, `List` method specifications) are defined in `spec/stdlib.md`;
the command line, configuration keys and exit codes are defined in
`spec/toolchain.md`; the native backend, the C API, and the rules of C
imports and exports (Milestone 2) are defined in `spec/cabi.md`. Where this
document needs a value from one of them it names the key and the default
and defers the rest.

Where the design is silent a rule is chosen and marked with a line beginning
`Decision:` giving the rule, the rationale, and the rejected alternative.
Decisions are numbered `S1`, `S2`, ... so other documents can cite them
(grammar decisions are `D`-numbered). Where the design is internally
inconsistent the chosen reading is marked `Design conflict:`. Every
`Decision:` and `Design conflict:` line also carries a project-wide stable
identifier `[D-nnn]`, assigned in document order and indexed in
`spec/decisions.md`; the local `S<n>` numbers remain the citation form.

Conventions: "error" means a diagnostic of severity `error` that makes
`cpl check` fail; "warning" and "info" never do. "Fault" means an execution
fault (Section 6.7). Diagnostic codes are written in backticks and are all
listed in Section 9. `Γ` is the typing environment; `Γ ⊢ e : T` reads "under
Γ, expression `e` has type `T`"; `Γ ⊢ e ⇐ T` reads "`e` checks against
expected type `T`" (Section 3.6). `erase(T)`, `strip(T)`, `base(T)` are
defined in Section 3.3.

## 1. Program model

### 1.1 Projects and source roots

| Item | Rule |
| - | - |
| Project root | The directory containing `cpl.toml`. All paths in diagnostics are relative to it, with `/` separators. |
| Edition | `[project] edition = "0.1"` is required; any other value is the error `unsupported_edition` (resolve). |
| Source roots | `[build] sources = ["src"]` (default `["src"]`). Every file ending in `.cpl` under a source root, recursively, is a compilation unit. Files elsewhere are ignored. |
| Work files | `work/*.work` and `work/*.patch` files are parsed for syntax by `cpl check` (S53) and are otherwise read only by `cpl work`; they take no part in resolution, typing, `cpl run`, or `cpl test`. |
| Standard library | Module paths whose first segment is `std` denote the toolchain-supplied standard library (`spec/stdlib.md`). A project may not declare a module whose path begins with `std` or `gen` (`reserved_module_path`, spanning the header's module path; `gen` is the prelude's alias of `std.gen`, S54). Project loading reports it, and such a file is not collected: its declarations take no part in the program, and it is not also `module_file_misplaced` (one diagnostic per mistake). |

A *program* is the set of all compilation units of a project plus the
standard library. `cpl check` checks the whole program; there is no
per-file checking because a module may span files.

Decision: [D-072] S52. a compilation unit with a lexical or syntax error yields
no syntax tree and exactly one parse diagnostic, the earliest error (D59,
D70), and the remaining units are still parsed. If any compilation
unit has a parse diagnostic, `cpl check` reports the parse diagnostics of
every unit and performs no resolution and no typing (the check fails);
`cpl test` and `cpl run` abort in the same way. No `unresolved_*` or
`type_*` diagnostic is ever emitted for a program that did not parse.
Rationale: a module may span files, so a unit that failed to parse cannot
be treated as empty without producing `unresolved_name` cascades for every
reference into it, and D59's one-diagnostic-per-file rule is chosen
precisely to avoid implementation-specific cascades. Rejected alternative:
skipping the failed unit and suppressing resolution errors for names that
might originate in it (a heuristic every implementation would apply
differently).

Decision: [D-073] S53. `cpl check` parses every `work/*.work` and `work/*.patch`
file under grammar Section 4 and reports its lexical and syntax
diagnostics (the parse codes of 9.4, including `unknown_requirement` and
`duplicate_work_item`), so that a malformed work file fails `cpl check`;
it does not resolve selectors, check refs, or execute anything in them,
which is `cpl work` (`spec/toolchain.md`, Milestone 6). Rationale: the
grammar names the corpus work files as part of what "must parse", and a
work file that no Milestone 1 command reads would rot unnoticed; selector
resolution needs the coordinator's scope rules, which are deferred.
Rejected alternative: ignoring `work/` in `cpl check` entirely.

### 1.2 Modules

Every compilation unit begins with `module a.b.c` (grammar 3.1). The
header is authoritative: the file's module is the header path regardless of
its location. All compilation units with the same header form one module;
its declarations are the union of theirs.

| Rule | Diagnostic |
| - | - |
| The conventional location of `module a.b.c` is `<sources>/a/b/c.cpl` under a source root (1.1) or, for further files of the same module, `<sources>/a/b/c/<name>.cpl` (`spec/toolchain.md` 3.7). A file elsewhere is accepted with a warning. | `module_file_misplaced` (warning) |
| Two files of one module may not both declare the same top-level name (Section 1.4). | `duplicate_declaration` |
| Module paths are not constrained by the project name: `reader.pages`, `tests.pagination`, and `holes.pages` may coexist in one project. | none |
| A module path is a namespace, not a container: `reader` need not exist as a module for `reader.pages` to exist. A `use reader.pages` refers to the module; a `use reader.{pages}` refers to the submodule `pages` of the namespace `reader` (Section 1.6). | none |
| Module doc comments (grammar 1.2) from several files of one module are concatenated in file-path order, separated by a blank line. | none |

Decision: [D-074] S1. declaration order and file order are semantically irrelevant.
A declaration may refer to any declaration of its own module (any file) and
to any public declaration it imports, before or after its own position, and
modules may import each other cyclically. Rationale: there are no
module-level values (D14) and every declaration is typed from its own
signature, so no initialization order exists and nothing depends on textual
order; forbidding cycles would only force artificial module splits.
Rejected alternative: Rust/OCaml-style acyclic module graphs.

### 1.3 Declaration kinds

| Kind | Grammar | Introduces | Namespace |
| - | - | - | - |
| nominal wrapper | `type_decl` with `wrapper_decl` | a type and a constructor of the same name | item |
| refinement | `type_decl` with `refinement_decl` | a type | item |
| struct | `struct_decl` | a type (possibly generic) and a struct-literal form | item |
| enum | `enum_decl` | a type (possibly generic) whose variants are constructors scoped to the type | item |
| capability | `capability_decl` | a capability type whose operations are scoped to the type | item |
| function | `fn_decl` | a callable | item |
| test | `test_decl` | an executable example | test |
| property | `property_decl` | an executable example with generated inputs | test |

Each declaration has the facets `identity`, `name`, `signature`,
`contract`, `effects`, `body`, `docs`, and `verification_policy`; the
selector `.definition` covers all of them (grammar 4.4). Facet contents per
kind:

| Facet | fn | capability | capability op | struct / enum / type | test / property |
| - | - | - | - | - | - |
| `identity` | `@id` or provisional identity (S2) | `@id` or provisional | its own provisional identity, the qualified name `<module>.<Cap>.<op>` (1.4); an operation never carries an `@id` | `@id` or provisional | `@id` or provisional |
| `name` | the `LOWER_IDENT` | the `UPPER_IDENT` | operation name | the `UPPER_IDENT` | the `TEXT` name |
| `signature` | generic params, params, `using` params, return type, `pub` | `pub` and the operations' names, parameter types, and return types (the handle's interface; `spec/toolchain.md` 5.1) | params, return type | fields / variants with payload types / wrapper payload, refinement base, generic params, `pub` | `for` clauses (property) |
| `contract` | `requires`, `ensures`, `decreases` | every operation's `requires`/`ensures` clauses | `requires`, `ensures` | refinement `where` predicate | none |
| `effects` | `effects` clause | none: a capability declares no effect bound (the design's `effects` facet is the bound of line 295's "effect bound widened"); each operation is the effect its callers name | none (the op is the effect) | none | none |
| `body` | the `block` | none | none | none | the `block` |
| `docs` | `///` comments | `///` comments (operations included) | `///` comments | `///` comments (members included) | `///` comments |
| `verification_policy` | `@prove` present or not | none | none | none | none |

Decision: [D-075] S2. every top-level declaration has an identity: the argument of
its `@id` annotation if present, otherwise the *provisional identity*, which
is its qualified name (Section 1.4). Identities must be unique across the
program: two `@id` arguments may not be equal, and an `@id` argument may
not equal the qualified name of any declaration, so a provisional identity
never collides with an annotated one (`duplicate_identity`: one diagnostic
at the later declaration, spanning its name token, with `symbol` = the
duplicated `@id` value and a note at the earlier declaration's name token,
S63; the later declaration keeps its provisional identity in the program);
an `@id` argument must be non-empty and match the
accepted syntax `spec/toolchain.md` 4.1 fixes
(`invalid_annotation_argument`). When tooling assigns persistent identities
it inserts `@id` lines (`spec/toolchain.md` fixes the syntax of generated
values); the semantics never depend on whether an identity is provisional.
The `symbol` field of every diagnostic (9.2) carries the identity and is
always accompanied by `qualified_name`; for a capability operation both are
its qualified name (1.3). Rationale: the design says `@id` is generated by tooling, so
unannotated declarations must still be addressable by every diagnostic.
Rejected alternative: refusing to check a declaration without `@id`.

### 1.4 Names and uniqueness within a module

Decision: [D-076] S3. a module has two namespaces: the *item namespace* holds every
`type`, `struct`, `enum`, `capability`, and `fn` declaration and, for each
file, every name that file imports (Section 1.6; an imported name lives
only in the importing file's scope 2, S5, and is never an item of the
module: there is no re-export, so another module cannot import it from
this one); the *test namespace* holds `test` and
`property` declarations keyed by their `TEXT` name. Within a namespace a
name may be declared at most once across all files of the module
(`duplicate_declaration` for items, `duplicate_test_name` for tests, both
pointing at both locations). Rationale: the grammar already separates type
names (`UPPER_IDENT`) from function names (`LOWER_IDENT`), so a single item
namespace loses nothing and keeps lookup one map per module; tests have
`TEXT` names and cannot collide with items. Rejected alternative: separate
type and value namespaces as in ML.

Enum variants and capability operations are *members* of their declaring
type, not module-level names; they are reached through the type
(`StoreError.Missing`) or by the resolution rules of Section 2.5. A variant
name must be unique within its enum (`duplicate_variant`), an operation
within its capability (`duplicate_operation`), a field within its struct
(`duplicate_field`), a parameter within its function (`duplicate_parameter`,
counting `using` parameters), and a type parameter within its declaration
(`duplicate_type_parameter`).

The *qualified name* of a declaration is `<module path>.<name>` for items
(`reader.pages.paginate`), `<module path>.<Type>.<Member>` for variants and
operations (`reader.pages.StoreError.Missing`, `reader.pages.PageRead.get`),
and `<module path>."<name>"` for tests and properties
(`tests.pagination."pagination handles empty input"`).

### 1.5 Visibility

A declaration is *public* when prefixed with `pub`, otherwise *private*.
Private declarations are visible in every file of their own module and
nowhere else; a `use` of a private item, or a qualified path to one from
another module, is the error `private_item`. Struct fields, enum variants,
and capability operations have no visibility of their own: they are
reachable wherever their type is reachable (design: "accessible through
their exported type").

A type expression is *exportable* when every `type_path` in it names a
prelude type (Section 1.7), a type parameter of the enclosing declaration,
or a public declaration (a refinement must itself be public; its base is
checked by the rule for its own declaration below). The public-signature
rule is:

| Public declaration | Must be exportable | Diagnostic |
| - | - | - |
| `pub fn` | every parameter type, every `using` parameter's capability type, the return type | `private_type_in_public_signature` |
| `pub capability` | every operation's parameter and return types | `private_type_in_public_signature` |
| `pub struct` | every field type | `private_type_in_public_signature` |
| `pub enum` | every variant payload type | `private_type_in_public_signature` |
| `pub type W(P)` | `P` | `private_type_in_public_signature` |
| `pub type R = B where p` | `B` | `private_type_in_public_signature` |

The diagnostic carries `symbol` (the public declaration), `facet:
"signature"`, and `type` (the offending private type's qualified name).
Private declarations may mention anything visible to them.

Decision: [D-077] S4. the `contract` facet of a public declaration (its `requires`,
`ensures`, `decreases`, and a public refinement's `where` predicate) may
call only public functions and prelude functions; a call to a private
function is the error `private_in_public_contract` (`symbol`, `facet:
"contract"`, `clause`, `callee`). Rationale: the design makes contracts the
boundary another module relies on and `cpl inspect` shows them to agents
in other modules; a contract that depends on a private helper cannot be
understood without reading a body, which defeats "minimal hidden context".
Rejected alternative: allowing private helpers in public contracts (as
Dafny does with non-opaque functions).

### 1.6 Imports

`use` declarations (grammar 3.2) bring names into the *file scope*.

| Form | Meaning |
| - | - |
| `use a.b.{x, Y}` | each `use_item` is looked up in module `a.b`: a public item (type, struct, enum, capability, function) is imported under its own name; a `LOWER_IDENT` that is not an item but is a prefix of some module path `a.b.c...` imports the *submodule alias* `c`, through which `c.item` and `c.sub.item` chains resolve (Section 2.4). |
| `use a.b.c` (D15) | the same lookup for the single item or submodule `c` of `a.b`. |
| `use std.text.{trim}` | the standard library is imported exactly like project modules. |

| Rule | Diagnostic |
| - | - |
| The module `a.b` must exist (some compilation unit or `std` module has that header, or it is a proper prefix of one for the submodule form). A path that is only a prefix admits only submodule items: `use ns.{Thing}`, or `use ns.{f}` where `ns.f` is no module and no prefix of one, is `unresolved_module` with `module` = the prefix (`ns`), spanning the item. | `unresolved_module` |
| The item must exist in that module. | `unresolved_name` (with `module`) |
| The item must be public. | `private_item` |
| The same name may not be imported twice in one file, even from the same module. | `duplicate_import` |
| An imported name may not equal the name of a declaration of the importing module (any file) or a prelude name (1.7, the reserved names of `spec/stdlib.md` S11 included); importing a prelude declaration from `std.prelude` itself (`use std.prelude.{flatten}`) names the same declaration, redefines nothing, and is permitted and redundant, with no diagnostic for the redundancy (`spec/stdlib.md` S3, `spec/toolchain.md` 3.6), though one the file never uses is `unused_import` like any import. Likewise `use std.gen` and `use std.{gen}` import the submodule alias `gen`, which names the module the prelude alias names: permitted and redundant, not `prelude_name_redefined`; `use gen.{int}` is `unresolved_module`, since `gen` is not a module path in a `use`. | `import_conflicts_with_declaration`, `prelude_name_redefined` |
| No item of `std.gen` may be imported: generators have the one spelling `gen.<name>` (or `std.gen.<name>`), and `Gen` is not nameable (`spec/stdlib.md` S28). | `generator_import` |
| Enum variants, capability operations, tests, and properties cannot be imported (the grammar admits only `LOWER_IDENT` path segments, so `use m.E.V` does not parse; `use m.{V}` finds no item). | `unresolved_name` |
| An import that is never used in the file is a warning. An import is *used* when some name of the file resolves through it (a type, a function, the root of a chain), when a constructor name's lexical candidates of S10 step 2 include something it imports, whichever step then decides the constructor, and when S10 step 3 reports a type it imports as `not_a_constructor`. Because some constructors are decided only once the expected type is known, the warning is reported after body typing. | `unused_import` (warning) |
| Glob imports and renaming do not exist (D16). | parse error |

Decision: [D-078] S5. imports are scoped to the file that contains them, not to the
module. Rationale: a file is the unit an agent reads; a name used in one
file must be explained by that file's own header, and the hole report's
"visible bindings" would otherwise depend on sibling files. Rejected
alternative: module-wide imports (any file of the module sees every file's
`use` lines).

Decision: [D-079] S6. a module-level declaration or import whose name equals a
prelude name (Section 1.7) is the error `prelude_name_redefined`. Local
bindings may shadow anything (Section 2.3 gives the order). Rationale: a
reader seeing `check<Positive>(n)` or `Option<T>` anywhere must be able to
assume the prelude meaning without checking the module's declarations;
rejecting the redefinition costs nothing because the prelude names are
few. Rejected alternative: lexical shadowing of the prelude with a warning.

A rejected redefinition is not entered into the module's item namespace:
the name keeps its prelude meaning in every file, and the rejected
declaration is marked `HasErrors`, so nothing that refers to it cascades
(S62). The note of `prelude_name_redefined` points at the prelude's own
declaration of the name (its `std.prelude` stub, or `std.gen`'s for
`Gen`), says what `check`, `gen`, and the prelude constructors are, or, for
the ten reserved numeric names of `spec/stdlib.md` S11, says `reserved for
a later edition`.

### 1.7 The prelude

The prelude is in scope in every file without `use`. Its names cannot be
redefined at module level (S6).

| Name | Kind | Definition |
| - | - | - |
| `Int` | type | arbitrary-precision integer (Section 6.3) |
| `Bool` | type | `true`, `false` |
| `Text` | type | immutable sequence of Unicode scalar values (Section 6.4) |
| `Bytes` | type | immutable sequence of 8-bit bytes |
| `Unit` | type and constructor | the single value `Unit` (D9) |
| `List<T>` | generic type | immutable sequence; members in Section 3.9 |
| `Option<T>` | generic enum | variants `Some(T)`, `None` |
| `Result<T, E>` | generic enum | variants `Ok(T)`, `Err(E)` |
| `Some`, `None`, `Ok`, `Err` | constructors | always resolvable unqualified (D44) |
| `ConstraintError` | type | the error of `check`; the struct `{ type_name: Text, message: Text }` of `spec/stdlib.md` 1.5 (a value type with `==`) |
| `check<R>(value)` | intrinsic generic function (`spec/stdlib.md` 1.4: it has no stub) | `R` must be a refinement type; `value : strip(R)` (the root base); returns `Result<R, ConstraintError>` (Sections 3.7, 6.10) |
| `flatten(lists)` | generic function | `flatten<T>(lists: List<List<T>>) -> List<T>`; concatenation in order |
| `gen` | module alias | the module `std.gen` under the name `gen` (S54); its generator properties `gen.int`, `gen.positive_int`, `gen.bool`, `gen.text` and functions `gen.list(g)`, `gen.option(g)`, `gen.range(lo, hi)` are typed in 3.8 and defined in `spec/stdlib.md` Section 8 |
| `Gen<T>` | type | the type of a generator; hermetic (S14); users cannot write it in a type position |
| `Int8`, `Int16`, `Int32`, `Int64`, `UInt8`, `UInt16`, `UInt32`, `UInt64`, `Float32`, `Float64` | reserved type names (`spec/stdlib.md` S11) | no values, literals, operators, or members in 0.1; in a type position they are `unresolved_type` with the note `reserved for a later edition`; they count as prelude names for S6 |

`std.text.trim`, `std.text.parse_int`, and `std.foreign.ForeignError` are
library names, not prelude names: they require `use`. The members of the
prelude types (`xs.len`, `n.abs()`, `opt.unwrap_or(d)`) are not names; they
are reached through their receiver type (3.9).

Design conflict: [D-080] design line 71 enumerates the prelude as "Int, Bool,
Text, Bytes, Unit, List, Option, Result, and their constructors", yet the
design's own examples use `ConstraintError` and `check<Positive>` (lines
88-90), `flatten` (line 108), and `gen` (line 276) in modules whose only
import is `std.text.{trim}` or that import nothing from `std`. The
examples are the executable corpus, so the prelude is extended with
exactly `ConstraintError`, `check`, `flatten`, `gen`, and `Gen<T>` to make
the design's own examples compile; grammar D57 and `spec/stdlib.md` 1.1
record the same reading.

Decision: [D-081] S54. `gen` is a universe-scope alias of the module `std.gen`
(adopting `spec/stdlib.md` S1): a chain beginning with `gen` resolves by
the module branch of Section 2.4 exactly as `std.gen.int` would, `gen` is
a reserved module-path root (1.1), and a local binding named `gen` hides
the alias for value chains like any root module name (2.4). The
standard library marks `int`, `positive_int`, `bool`, and `text` as
*generator properties*: zero-parameter functions read without parentheses
(`spec/stdlib.md` 0.1 and 8.1), the only module-level properties in 0.1.
Rationale: the design writes `gen.list(gen.int)` with no import, and
binding a module alias in the prelude reuses the chain resolution and the
property/method distinction (D43) the checker already has. Rejected
alternatives: a `gen` object value (needs a value type that is neither
data nor a capability); requiring `use std.gen` in test modules
(contradicts the design's example).

### 1.8 Well-formedness of declarations (checked before typing bodies)

| Declaration | Rule | Diagnostic |
| - | - | - |
| any | annotations: `@id(TEXT)` on any top-level declaration, `@prove` on `fn` only; each at most once; nothing else (D17); an annotation written with an empty argument list (`@prove()`, `@method()`, `@property()`) counts as one without arguments. The standard library's stub files additionally use the annotations `spec/stdlib.md` defines for builtins, members, and generator properties (`@builtin`, `@property`, `@method`); in project files these are `unknown_annotation`, and inside the standard library their misuse (`spec/stdlib.md` 10.2: `@builtin` on a declaration that has a body or definition, `@method` without a receiver, `@property` with more than one parameter, both on one `fn`, a receiver that is not a prelude type head) is `annotation_misplaced` | `unknown_annotation`, `annotation_misplaced`, `duplicate_annotation`, `invalid_annotation_argument` |
| `fn` without a body, `type` without a definition (grammar 3.9 and 3.5, the bodiless and definition-less forms) | permitted only in the standard library's stub files, and there only with `@builtin` (`spec/stdlib.md` 10.2); anywhere else the declaration is a check error, not a syntax error, and the rest of the file is checked normally | `missing_body` |
| `type W(P)` | `P` is a value type (Section 3.2); `W` is not generic; `W` is a node of the S7 graph | `not_a_value_type`, `infinite_type` |
| `type R = B where p` | `B` is a value type (it may itself be a refinement); the chain of bases must be acyclic (`type A = B where p` whose base chain reaches `A`; every declaration of the cycle is reported, S66); `p : Bool` with `self : B` in scope; `p` is a contract expression (Section 5.1) | `not_a_value_type`, `type_mismatch`, `cyclic_type_definition`, contract diagnostics |
| `struct`, `enum` | field / payload types are value types; every type parameter is used by at least one field or payload; no cycle in the containment graph of S7 | `not_a_value_type`, `unused_type_parameter`, `infinite_type` |
| `capability` | operation parameter and return types are value types (an operation cannot take or return a capability) | `capability_in_value_position` (S13, S27; `not_a_value_type` for any other non-value type) |
| `fn` | positional parameter types and the return type are value types; `using` parameter types are capability types; every type parameter is used (otherwise a warning); `decreases` only meaningful when the function is recursive (otherwise a warning) | `not_a_value_type`, `capability_in_value_position`, `using_param_not_capability`, `unused_type_parameter` (warning on `fn`), `unused_decreases` (warning) |
| `test`, `property` | name non-empty; unique in the module's test namespace | `empty_test_name`, `duplicate_test_name` |

Wherever this table names `not_a_value_type` and the offending type is a
capability, the diagnostic is the more specific `capability_in_value_position`
(S13, S27), in every position: positional parameter, field, payload,
wrapper payload, refinement base, return type, type argument, `let`/`var`
annotation, and the parameter and return types of a capability operation. Since `Gen<T>`
cannot be written in a type position (S14: `unresolved_type`), no 0.1
program triggers `not_a_value_type`; the code is kept for the non-value
types of later editions.

Decision: [D-233] S66. `cyclic_type_definition` is reported on every refinement
declaration of the cycle (`type Even = Odd where self % 2 == 0` and `type
Odd = Even where self % 2 == 1` each carry the error), with `notes` listing
the cycle, exactly as S7 reports `infinite_type`; the declarations on a
cycle have no root base, so `strip` and the scope of `self` are undefined
for them and every reference to them is error-typed (S62). Rationale: the
two cycle diagnostics should behave alike, and an agent asked to fix one
declaration cannot know the cycle exists without the second report.
Rejected alternative: reporting the cycle once, at the declaration
encountered first (depends on traversal order).

`cyclic_type_definition` also covers a refinement whose base mentions the
refinement itself anywhere (`type A = List<A> where p`), since its S55
class would have no fixed point; a refinement whose base chain reaches a
cycle without lying on it is error-typed without a diagnostic of its own
(S62).

Decision: [D-082] S7. recursive data may pass only through `List<_>`, `Option<_>`,
or `Result<_, _>`. Precisely: the *containment graph* has one node per
struct, enum, and wrapper declaration of the program and an edge from
declaration `A` to declaration `B` for every occurrence of `B` in a field,
payload, or wrapper payload type of `A` that is not nested under a `List`,
`Option`, or `Result` constructor, with the type arguments of generic
declarations substituted before the edges are read (`struct Box<T> { v: T }`
with `struct Node { next: Box<Node> }` gives the edge `Node → Node`). Any
cycle in this graph, including a self-loop and mutual recursion
(`struct A { b: B }` with `struct B { a: A }`), is the error `infinite_type`,
reported on every declaration in the cycle with `notes` listing the cycle;
`struct Node { children: List<Node> }` and `enum Tree { Leaf, Node(Option<Tree>) }`
are legal, `struct Node { next: Node }` and `enum E { A(E) }` are not. A
field or payload of a refinement type contributes the edges of its root
base (`strip`), and the substitution of generic arguments stops at depth
16, so that reading the edges of a polymorphically recursive declaration
terminates.
Rationale: a type on such a cycle has no finite values, so no literal,
constructor application, or `check` could ever produce one, and every
declaration of it is dead; the three prelude types are the places where a
value can be absent or empty, which is what breaks the cycle. Rejected
alternative: permitting the declarations and leaving them uninhabited.

## 2. Name resolution

### 2.1 Scopes

Scopes nest as follows, innermost last. Lookup proceeds from the innermost
scope outward and stops at the first scope that binds the name.

| # | Scope | Binds | Opened by |
| - | - | - | - |
| 1 | universe | prelude names (1.7) including the module alias `gen` (S54); the first segment of every module path in the program and `std` (so fully qualified paths resolve, D44, wherever no binding or function of the root's name hides it, 2.4: a module-level `fn std()`, an import alias named `std`, or a project module whose first segment is a prelude function's name, such as `module flatten.x`, leaves such paths unreachable in that scope) | always |
| 2 | module | all item-namespace declarations of the module (every file) and the current file's imports (S5), which must not collide (1.6) | the compilation unit |
| 3 | type parameters | the `generic_params` of the declaration being checked | `fn`, `struct`, `enum` |
| 4 | parameters | positional and `using` parameters of a `fn` or capability operation | `fn_decl`, `op_decl` |
| 5 | `self` | the refinement's underlying value, of the base type | the `where` predicate only |
| 6 | `result` | the returned value, of the declared return type | each `ensures` clause only |
| 7 | property binders | the `for` binders, each visible in the body and in no `for` range (S43) | `property_decl` |
| 8 | block | `let` and `var` bindings, visible from the statement after their own to the end of the block | each `block` |
| 9 | match arm | bindings of the arm's pattern, visible in that arm's body | each `match_arm` |
| 10 | `forall` binder | `x` in `forall x in c: p`, visible in `p` only (not in `c`) | `forall_expr` |

Scopes 8-10 are *local scopes*. Parameters (scope 4) count as local
bindings for S8, so a `let` that reuses a parameter name is
`shadowed_binding`; a parameter named `result` in a declaration with
`ensures` clauses is `result_name_reserved` (S9), not `shadowed_binding`.

Decision: [D-234] S67. `unused_binding` (warning) is reported for a `let`,
`var`, or pattern binding (scopes 8-9) that is never read; an assignment to
a `var` is a write, not a read, so a `var` that is only assigned warns;
names beginning with `_` are exempt; parameters (scope 4), property `for`
binders (scope 7), and `forall` binders (scope 10) are never diagnosed.
Rationale: a write-only `var` is dead state of exactly the kind the warning
exists for, whereas a parameter belongs to a signature that callers depend
on and a `for` binder's generator may be exercised for its draws alone, so
warning on them would push authors toward `_`-prefixed names that then
appear in `cpl inspect` output and hole reports. Rejected alternative:
warning on every unread name (noise in public signatures).

A binding that a later binding shadows, which is the error
`shadowed_binding` (S8), is not also reported `unused_binding`: which of
the two bindings is the unread one is only decided once the shadowing is
fixed (S62).

### 2.2 Which names each position accepts

| Position | Resolves against |
| - | - |
| `name` (`LOWER_IDENT`) in an expression | scopes 10 → 1: local bindings, `result`/`self` where applicable, parameters, then module items (functions, submodule aliases), then prelude functions (`check`, `flatten`), the module alias `gen` (S54), and root module names; a name that resolves to a module, submodule alias, or `gen` and is not the root of a longer chain is `not_a_value` |
| `name` (`UPPER_IDENT`) in an expression or `ctor_path` in a pattern | constructors: Section 2.5 |
| `type_path` in a type position | type parameters (scope 3), then module items that are types (scopes 2-1), with module-qualified prefixes resolved per Section 2.4 |
| `LOWER_IDENT` in `assign_stmt` | a `var` binding in scopes 8 of the enclosing function or test (Section 3.11); anything else is `assign_to_immutable` or `unresolved_name` |
| `effect_ref` `p.op` | `p` is a `using` parameter of the enclosing `fn` (scope 4 only; `effect_param_not_capability` otherwise); `op` is an operation of its capability (`unknown_effect_operation`) |
| `using_arg` expression | a bare `LOWER_IDENT` naming a `using` parameter of the enclosing `fn` (S28) |
| `member_name` after `.` | a member of the left operand's type (field, property, method, operation), or an item of a module when the left operand is a module path (2.4), or a variant when the left operand is an enum type path |
| `use_item` | Section 1.6 |

### 2.3 Shadowing

Decision: [D-083] S8. within one `fn`, `test`, or `property`, a local binding
(`let`, `var`, pattern binding, `forall` binder, property `for` binder) may
not reuse a name that is already bound by a parameter or by a local binding
of the same or an enclosing scope; the error is `shadowed_binding`, pointing
at both bindings. Sibling scopes (two match arms, the two branches of an
`if`, two sequential `while` bodies) may reuse names. Rationale: one name
has one meaning for the whole lexical extent of a function, which keeps the
hole report's "visible bindings" a flat list and prevents the
`let size = check<Positive>(size)?` style in which a name changes type
halfway through a body; the design's own export wrapper writes
`checked_size`. Rejected alternative: Rust-style rebinding.

Local bindings may shadow module-level and universe names (`let trim = 3`
hides `std.text.trim` in that block). No diagnostic is issued; a call whose
callee resolves to such a local is `not_callable` with a note naming the
hidden item. Type parameters likewise: a type parameter may be named like a
prelude or reserved type (`fn f<Int64>(x: Int64) -> Int64`, `fn g<Gen>(x:
Gen)`), and within its declaration it hides that type, scope 3 being
searched before scope 1; the reservations of `spec/stdlib.md` S11 and S28
bind module-level declarations, imports, and type positions that do not
resolve to a type parameter.

Decision: [D-084] S9. the contextual names `result` and `self` (D50) are resolved as
follows: `result` is bound only inside `ensures` clauses and there shadows
every other binding; using `result` in a `requires`, `decreases`,
`invariant`, body, or test is `result_outside_ensures` unless a parameter or
local of that name is in scope (the design's `label(result: ...)`); a
parameter or local named `result` in a declaration that has at least one
`ensures` clause is `result_name_reserved`. `self` is bound only inside a
`where` predicate; elsewhere it resolves like any `LOWER_IDENT` and binding
it, as a parameter, `let`, `var`, pattern binding, or `for`/`forall` binder,
is the warning `self_bound` at the binding; an unbound `self` outside a `where`
predicate is `self_outside_where`. Rationale: these are exactly the
contextual meanings the design describes, stated so that the two names
never silently refer to something else inside a contract. Rejected
alternative: reserving both as keywords (would reject a design example).

### 2.4 Qualified chains

The parser produces `Member(Member(Name a, b), c)` for `a.b.c` (grammar
3.12 rule 1). Resolution folds the chain from the left:

```text
resolve_chain(segments):
    // 1. the root
    if segments[0] is a LOWER_IDENT that resolves (scopes 10 → 1) to a binding (local, parameter,
            `for`/`forall` binder, `result`, `self`) or to a function:
        root = that binding or function; rest = segments[1..]   // value chain: members
    else if segments[0] is a submodule alias (1.6), the prelude alias `gen` (S54),
            or a root module name (scope 1):
        take the longest prefix p = segments[0..k] that is a module path or a
        module-path prefix in the program (`gen` stands for `std.gen`); the next
        segment must be an item of module p (public unless p is the current
        module) -> `unresolved_name` if missing, `private_item` if private; if p
        is only a prefix (no module has exactly that path) and no item follows
        -> `unresolved_module`; a chain that ends inside p -> `not_a_value`
        root = that item; rest = segments[k+1..]
    else if segments[0] is an UPPER_IDENT that resolves (scopes 3 → 1) to a type:
        root = that type; rest = segments[1..]           // `StoreError.Missing`, `PageId`, `Option.None`
    else: `unresolved_name`
    // 2. remaining segments
    for seg in rest:
        if root is an enum type and seg is UPPER_IDENT: root = variant seg (`unresolved_name` if none)
        else if root is a value or an expression: root = Member(root, seg)   // typed in Section 3.9
        else: `unresolved_name` ("`seg` is not a member of module/type/function ...")
    // 3. the result
    if root is a variant, a wrapper type, or a struct type followed by `{`: a constructor reference (3.8)
    if root is any other type (a type parameter, a refinement, a capability, `Int`, `Text`, ...,
        an enum or struct name used bare): `not_a_constructor` (2.5 step 3)
    if root is a function: a callee (3.8); in any other position `not_a_value` (S11)
```

A local binding named like a root module (`let reader = 1`) hides the
module for value chains; type positions and `use` paths are unaffected
because they never consult local scopes. A chain that ends inside a module
path is `not_a_value`, except in callee position, where it is
`not_callable` (3.8). When a chain passes through a module-path prefix and
its next lowercase segment is no module and no prefix, the diagnostic is
`unresolved_module` spanning that segment, with `module` = the prefix
extended by it (`ns.deep.f` for `deep.f()` where `deep` is the alias of the
prefix `ns.deep`). Where a module `x` declares an item `y` and a module
`x.y` also exists, `use x.{y}` imports the item (1.6: a submodule alias is
only a name that is not an item), while the chain `x.y` resolves to the
module (the longest prefix), so `x.y()` is `not_callable` and the item has
no qualified spelling; 0.1 has no diagnostic for this collision.

### 2.5 Constructor resolution

Decision: [D-085] S10. an unqualified `UPPER_IDENT` used as a constructor (a `name`
in expression position, a `ctor_path` of one segment in a pattern) resolves
in this order, refining D44:

1. *Expected-type directed.* If an expected type `E` is known (Section 3.6)
   and `strip(E)` is an enum (including `Option` and `Result`) with a
   variant of that name, or a nominal wrapper of that name, or `Unit` and
   the name is `Unit`, the constructor is that one.
2. *Lexical.* Otherwise the candidates are: every variant of that name in
   every enum declared in the current module or imported into the file;
   every nominal wrapper of that name declared or imported; the prelude
   constructors `Some`, `None`, `Ok`, `Err`, `Unit`. Exactly one candidate
   resolves; none is `unresolved_name`; two or more is
   `ambiguous_constructor` listing the candidates' qualified names and
   suggesting the qualified form.
3. A struct name in expression position not followed by `{` (the D41
   situation) and a type that is not a constructor (`Int`, a refinement, a
   capability, a type parameter, a bare enum name) are `not_a_constructor`.

Qualified constructors `E.V`, `m.E.V`, `m.W` resolve by Section 2.4 and
skip steps 1-2. Rationale: step 1 makes the design's `Err(Missing(_))`
resolve without imports; step 2 keeps `Missing(id)` usable where no type is
expected yet; wrappers must be constructors because the design constructs
`PageId("page-1")`. Rejected alternative: requiring qualification when no
expected type is known.

Patterns are always checked against a known scrutinee type, so step 1
always applies to them; a qualified pattern constructor that belongs to a
different type than the scrutinee is `pattern_type_mismatch`.

### 2.6 Capability parameter names

A `using` parameter is an ordinary binding in scope 4 whose type is a
capability. It may appear only as: the receiver of an operation call
(`db.get(id)`), a `using` argument (`using db`), and the left part of an
`effect_ref`. Every other use (binding it, storing it, comparing it,
returning it, passing it positionally) is an error of Section 4.3. The
operation set of the capability is the member table of the parameter's
type; `db.put` on a `PageRead` parameter is `unknown_member`.

## 3. Type system

### 3.1 Type forms

| Form | Written as | Introduced by | Value type (3.2) | Notes |
| - | - | - | - | - |
| `Int` | `Int` | prelude | yes | arbitrary precision (6.3) |
| `Bool` | `Bool` | prelude | yes | |
| `Text` | `Text` | prelude | yes | Unicode scalar values (6.4) |
| `Bytes` | `Bytes` | prelude | yes | no literal (D8) |
| `Unit` | `Unit` | prelude | yes | one value, the constructor `Unit` |
| list | `List<T>` | prelude | yes | immutable sequence; invariant in `T` (S15) |
| option | `Option<T>` | prelude enum | yes | `Some(T)`, `None` |
| result | `Result<T, E>` | prelude enum | yes | `Ok(T)`, `Err(E)` |
| library value types | `ConstraintError`, `std.foreign.ForeignError`, ... | `spec/stdlib.md` | yes | behave as structs or enums |
| nominal wrapper | `W` for `type W(P)` | `type_decl` | yes | distinct from `P`; constructor `W(p)`; no implicit conversion either way |
| refinement | `R` for `type R = B where p` | `type_decl` | yes | subtype of `B` at the top level only (3.4); identity is structural (S55); there are no transparent type aliases (D64) |
| struct | `S` or `S<T1, ...>` | `struct_decl` | yes | nominal; fields are properties |
| enum | `E` or `E<T1, ...>` | `enum_decl` | yes | nominal; variants are constructors |
| capability | `C` | `capability_decl` | no | parameter-only handle type (Section 4) |
| type parameter | `T` | `generic_params` | yes | opaque inside its declaration; every instantiation is a value type (S13) |
| generator | `Gen<T>` | prelude | no | hermetic: only in `for` ranges and `gen` arguments (S14) |
| function | none | none | no | not a type in 0.1 (S11) |

Decision: [D-086] S11. functions, methods, constructors, and capability operations
are not values and have no type: a callee is always either a name chain
resolved to one of them or a postfix expression ending in `.member_name`
whose member is a method or operation of the receiver's type (`xs.take(2)`,
`[1, 2, 3].take(2)`, `(-5).abs()`, `parse_size(text)?.is_ok()`; grammar
3.12 rule 4, D32, `spec/stdlib.md` S38), and a name that resolves to a
function in any other position is `not_a_value`. Rationale: effect bounds
are checked per call graph edge, which requires every callee to be known
statically. Rejected alternative: first-class function values with effect
rows (a later edition).

Decision: [D-087] S12. user-declared generic structs and enums (D19) are supported
in 0.1 with the same instantiation machinery as `List`, `Option`, and
`Result`: a `type_path` to a generic type must supply exactly as many type
arguments as parameters (`type_argument_count_mismatch`), and a struct
literal of a generic struct infers its arguments from the expected type and
the field initializers (3.7). Rationale: the checker needs substitution for
the prelude types anyway; "not supported" would be an arbitrary cut.
Rejected alternative: rejecting `generic_params` on user types.

### 3.2 Value types and type arguments

A type is a *value type* when it is not a capability type, not `Gen<_>`,
and (for `List<T>`, `Option<T>`, `Result<T, E>`, and generic structs and
enums) all of its type arguments are value types. Every value type supports
`==` (3.6), can be stored in fields, payloads, lists, and bindings, and can
be returned.

Decision: [D-088] S13. every type argument, field type, variant payload type,
wrapper payload type, refinement base type, positional parameter type,
return type, and `let`/`var` type must be a value type; the diagnostic is
`not_a_value_type` (with `type`), except that a capability in any of these
positions (positional parameter, field, payload, return type, `let`/`var`
annotation, ..., 1.8) is the more specific
`capability_in_value_position` (Section 4.3). Inside the standard library
`Gen<T>` is exempt from this rule, because the normative `std.gen` stubs
take and return it (`spec/stdlib.md` S28, 11.5). Consequently a type parameter
`T` is only ever instantiated with a value type and `==` is available on
every `T`, which the design's `flatten(result) == items` on `List<T>`
requires. Rationale: one uniform rule gives equality on generics without
bounds. Rejected alternative: type-class style `Eq` bounds.

Decision: [D-089] S14. `Gen<T>` is hermetic: a value of type `Gen<T>` may appear
only as the range expression of a property `for` clause and as an argument
to a `gen` combinator (`gen.list(gen.int)`); `Gen` cannot be written in a
type position (`unresolved_type`), and a `let`/`var`, field, argument to a
non-`gen` callee, or any other use of a `Gen<T>` expression is
`generator_outside_for`. Rationale: 0.1's generator surface is the closed
set in `spec/stdlib.md` and user-defined generators are deferred; keeping
generators out of the value world avoids defining equality, storage, and
shrinking for first-class generators. Rejected alternative: `Gen<T>` as an
ordinary opaque type.

### 3.3 Refinement chains, erasure, and type identity

There are no transparent type aliases in 0.1 (D64), so a type expression
denotes exactly the declaration its `type_path` names. Diagnostics render
a type in CPL type syntax with every named type by its declared name, not
as the source spelled it (`pa.a.Thing` renders as `Thing` however it was
qualified), a refinement by the declaration named at the use site (S55), a
type parameter by its name, and the error type as `?` (S69); this is
`Program::display_ty_in` (`spec/toolchain.md` 1.6), and the checker's
diagnostics, the hole report (8.3), and the faults of 6.7 all use it. When
the two types of one diagnostic (`expected` and `actual`) would render
alike but are not identical, both render with qualified names (S65).

| Function | Definition |
| - | - |
| `base(R)` | the declared base type of refinement `R`; undefined for non-refinements |
| `strip(T)` | `T` if `T` is not a refinement, else `strip(base(T))`: the first non-refinement in the chain (the *root base*) |
| `chain(R)` | the list of refinements from `R` down to but excluding `strip(R)`: `[R, base(R), ...]` |
| `erase(T)` | `strip(T)` applied at every position: `erase(List<Positive>) = List<Int>`, `erase(Result<Positive, E>) = Result<Int, erase(E)>`, `erase(S<Positive>) = S<Int>`; struct field types are not entered (structs are nominal) |
| `T ≡ U` | *type identity*: structural equality in which nominal types (wrappers, structs, enums, capabilities, the prelude's named types) are identical by qualified name plus identical arguments, type parameters are identical by binder, and refinements are identical by S55: never to their base (`Positive ≢ Int`) |

Decision: [D-090] S55. refinement identity is structural (adopting
`spec/stdlib.md` S29): two refinement types `type A = B where p` and
`type C = B' where q`, declared anywhere in the program or the standard
library, are identical (`A ≡ C`) exactly when `B ≡ B'` and `p` and `q` are
the same *resolved predicate*: their syntax trees are identical once
grouping parentheses are removed, literals are equal by value, the bound
name is `self` in both (so no renaming is needed), and every name in them
resolves to the same declaration by qualified name or to `self` (so two
`Text where ok(self)` predicates whose `ok` functions differ are two types,
and no formatter rule takes part); a refinement is never identical to its
base.
Consequences: `std.gen.Positive` (`Int where self > 0`) is
`reader.pages.Positive`, which is what lets the design's property pass
`size` from `gen.positive_int` to `paginate`; `type Positive = Int where
0 < self` is a distinct type and `paginate(items, size)` with such a `size`
is `cannot_prove_refinement` (S65) with the note "declare the predicate as
`self > 0` or use `check<Positive>(size)`". Where a rule or a diagnostic names a
refinement (`chain(E)` in 3.5, `refinement_false`, `expected_type`, the
`type_name` of a `ConstraintError`, 6.10), it uses the declaration named at
the use site, so two identical refinements render under whichever name the
program wrote there. Rationale: the design describes a refinement as "a
refinement of Int" whose membership is proved or checked, never
constructed, and contrasts it with the nominal wrapper `PageId`; a
standard library cannot name a user's type, so a nominal reading would
leave `for size in gen.positive_int` with no type `paginate` accepts.
Rejected alternatives: nominal refinements plus a special typing rule for
`gen.positive_int` (a one-off coercion with no general principle); a type
ascription in the `for` clause (a grammar change the design's example does
not use).

The resolved-predicate comparison works on keys built from the resolved
trees: a `forall` binder compares by its position, a type argument by the
declaration it names, a name by the declaration it resolves to, and an
unqualified constructor by its unique lexical candidate (S10 step 2), or by
its spelling when it has several; in that rare last case two predicates
that resolve alike can still fall into two classes, which costs a
`cannot_prove_refinement` (S65), never a wrong identity.

Refinements are erased at runtime (6.1): a `Positive` value is an `Int`
value. They exist only in the static semantics.

### 3.4 Assignability

`A ≤ E` ("a value of type `A` is acceptable where `E` is expected") is
decided for an *expression* `e` of inferred type `A`, because one of the
rules inspects `e` itself. With inference variables (3.7) each `≡` test
below is a unification that may bind variables; a failed unification does
not bind.

| # | Condition | Result |
| - | - | - |
| 1 | `A ≡ E` | accepted |
| 2 | `E` is not a refinement and `strip(A) ≡ E` | accepted: refinement to base, top level only |
| 2' | `E` is a refinement and some element of `chain(A)` is `≡ E` | accepted: widening along the chain (`Big ≤ Positive` for `type Big = Positive where self > 100`); no proof is needed because every `Big` value satisfied `Positive`'s predicate when it was established |
| 3 | `E` is a refinement and `strip(A) ≡ strip(E)` (and 2' does not apply) | the compile-time proof rule (3.5) decides: accepted, `refinement_false`, `constant_evaluation_fault`, or `cannot_prove_refinement` |
| 4 | otherwise | `type_mismatch` with `expected` = `E`, `actual` = `A`; when `erase(A) ≡ erase(E)` a note says that refinements convert only at the top level and suggests rebuilding the value |

Decision: [D-091] S15. subtyping between a refinement and its base applies only at
the top level of a type; type constructors are invariant
(`List<Positive>` is not usable as `List<Int>` or vice versa). Rationale:
inference stays unification-based with no variance annotations, and the
design's examples only ever convert at the top level (`checked_size`).
Rejected alternative: covariance of `List`, `Option`, `Result` in refinements
(sound for a runtime that erases refinements, but it complicates
unification and the Milestone 5 encoding).

Consequences: `Positive ≤ Int` holds; `Int ≤ Positive` holds only for a
provable constant; `Big ≤ Positive` holds when `Positive` is in
`chain(Big)` (rule 2'); `PageId ≤ Text` and `Text ≤ PageId` never hold
(nominal wrapper); `Result<Positive, X> ≤ Result<Int, X>` does not hold.

### 3.5 The compile-time proof rule for refinements

Decision: [D-092] S16. an expression `e` with `strip(A) ≡ strip(E)` is accepted at
refinement type `E` only when `e` is a *constant expression* and every
predicate in `chain(E)` evaluates to `true` on its value. The grammar of
constant expressions is:

```ebnf
const_expr = INT | TEXT | "true" | "false" | "Unit"
           | "-" const_expr | "not" const_expr
           | const_expr bin_op const_expr                 // bin_op: + - * / % ++ == != < <= > >= and or
           | "(" const_expr ")"
           | "[" commas(const_expr) "]"
           | ctor ( "(" commas1(const_expr) ")" )?        // enum variant, wrapper, Some/None/Ok/Err
           | type_path "{" commas(field_init_const) "}"   // struct literal without spread
ctor             = ctor_path                              // grammar 3.13: `Missing`, `StoreError.Missing`, `PageId`
field_init_const = LOWER_IDENT ":" const_expr
```

Names, calls, member accesses, `if`, `match`, `forall`, holes, and `?` are
not constant expressions; in particular a `let`-bound name is not, even
when its initializer was a literal. The rule proceeds:

1. Evaluate `e` with the dynamic semantics of Section 6 (same arithmetic,
   same faults). A fault is the error `constant_evaluation_fault` carrying
   the fault's code (`division_by_zero` for `1 / 0`).
2. For each refinement in `chain(E)` from `strip(E)` upward, evaluate its
   `where` predicate with `self` bound to the value, using the
   *compile-time evaluable subset*: literals, constructor applications,
   `self`, unary and binary operators, `.len`, `.is_empty()`, `forall`
   (with its binder), `if`, `match` (with its pattern-bound names), the
   `let`-bound names of contract sub-blocks (S32) and their initializers,
   and parentheses. A predicate that uses any other form (a call to a
   function, `check`, a member outside the subset) makes the proof fail with
   `cannot_prove_refinement` and the note "predicate of `R` calls `f`; use
   `check<R>(...)`"; a fault during predicate evaluation is
   `constant_evaluation_fault`.
3. All predicates `true`: `e` has type `E`. Some predicate `false`:
   `refinement_false` (`expected` = `E`, `actual` = the value, `clause` =
   the predicate text).
4. `e` not a constant expression: `cannot_prove_refinement` (`expected` =
   `E`, `actual` = `A`) with the note "use `check<E>(...)`".

Rationale: the design says "a literal such as 2 can satisfy Positive through
a compile-time proof" and "failure to prove a refinement never becomes an
implicit unchecked conversion"; evaluation of a closed expression is the
only proof procedure available before the Milestone 5 verifier, and
restricting predicates to an operator subset at compile time means the
checker never runs user function bodies (`crates/cpl-sema` needs the value
arithmetic of Section 6.3, not a full evaluator). Rejected
alternatives: constant propagation through `let` (makes a type depend on a
distant initializer), and running arbitrary pure functions at compile time.

The proof is recorded in the typed IR as a static fact; no runtime check is
emitted for it (the reference evaluator re-evaluates the recorded proofs in
checked mode as a guard against an unsound checker, `spec/toolchain.md`
T39: a false predicate there is `internal_error`, never
`refinement_violation`, and the re-check is observable only through the
budgets it spends). This is the only "statically established check eligible for
elimination" that Milestone 1 performs; all contracts run (Section 5).

Decision: [D-232] S65. when `A` is itself a refinement with `strip(A) ≡ strip(E)`
but `A ≢ E` (two refinements of one root base whose resolved predicates
differ, such as a user `type Positive = Int where 0 < self` against
`std.gen.Positive`, `Int where self > 0`, S55), rule 3 of 3.4 applies
unchanged: `e` is almost never a constant expression, so the diagnostic is
`cannot_prove_refinement` (`expected` = `E`, `actual` = `A`, both rendered
with their qualified names when the bare names coincide) with a note that
quotes both predicates and suggests declaring the expected predicate
verbatim or using `check` ("declare the predicate as `self > 0` or use
`check<Positive>(size)`"). It is never `type_mismatch`: rule 4 is reached
only when the root bases differ. Rationale: 9.4 binds
`cannot_prove_refinement` to this step and no rule of 3.4 distinguishes a
refined `A` from its base once `strip` agrees; the diagnostic is also true
(membership of a `std.gen.Positive` value in the user's type cannot be
proved in Milestone 1), and if Milestone 5 relaxes identity to proven
equivalence (`spec/stdlib.md` S29) this is the step where that proof will
land. Rejected alternative: `type_mismatch` with the same note (an earlier
wording of S55's consequences and of `spec/stdlib.md` S29; it would need a
fifth assignability rule).

### 3.6 Equality and ordering

Decision: [D-093] S17. `==` and `!=` are defined on every value type and require
`erase(L) ≡ erase(R)` for the operand types `L` and `R`; the result is
`Bool`. The comparison is structural (6.5): nominal wrappers compare by
payload, structs field by field, enums by variant then payload, lists
element-wise, `Text` by scalar sequence. Comparing a capability is
`capability_equality`, one per handle operand, spanning that operand (S27:
`db != other` with two handles carries two, `db == 1` one); any other
operand-type disagreement is `type_mismatch`. Rationale: refinements are erased at runtime, so
`List<Positive> == List<Int>` is meaningful and useful in contracts
(`flatten(pages) ++ rest == items`); nominal wrappers of different names
never compare because `erase` keeps their names. Rejected alternative:
requiring `L ≡ R` exactly.

Decision: [D-094] S18. the ordering operators `<`, `<=`, `>`, `>=` are defined only
when both operands have `strip` type `Int`; anything else is
`comparison_unsupported`. Rationale: ordering `Text` requires a collation
decision the design does not make; `Bool` and lists have no natural order.
Rejected alternative: lexicographic `Text` order by scalar value.

Operand typing for `==` and `!=` (`xs == []`, `x == None`, `xs == ?y`): the
left operand is inferred first. If its type `L` is fully resolved, the
right operand is typed in check mode against `erase(L)` with the final
assignability test of 3.7 replaced by `erase(R) ≡ erase(L)`: the expected
type flows into a literal-like form or a hole on the right (S19), while a
refined operand is neither proved nor rejected by `==`. If `L` contains
unresolved inference variables, the right operand is inferred and the two
erased types are unified (3.7). Only when both remain unresolved is it
`cannot_infer_type`.

### 3.7 Generics and inference

Every declaration is typed independently from its own signature: there is
no inference across top-level declarations, parameter and return types are
always written, and the `effects` bound is always written (S26). Inside a
body, inference is *local and bidirectional*:

| Mode | Notation | Meaning |
| - | - | - |
| infer | `Γ ⊢ e : T` | compute `T` from `e`; `T` may contain inference variables `?α` |
| check | `Γ ⊢ e ⇐ E` | `E` is the expected type; literal-like forms use `E` to type their parts; afterwards `A ≤ E` is required for the inferred `A` |

Inference variables are created by instantiating a generic function, a
generic constructor, a generic struct literal, or an empty list literal.
They are scoped to the *statement* (or clause, or `for` range) being
checked: a variable still unbound when the statement ends is
`cannot_infer_type` at the expression that created it, with the note "add a
type annotation"; variables unified with one another and still unbound are
one diagnostic, at the expression that created the oldest of them (`[] ==
[]` reports at the left `[]`, and `if c { None } else { None }` reports
once). Unification is structural type identity (3.3): a
variable binds to any type (including a refinement); a refinement is a
leaf; a bound variable is replaced by its binding.

Decision: [D-229] S62. an expression for which a diagnostic has been reported
has the *error type*: it unifies with, is assignable to, and accepts every
type, has every member, and may be called with any arguments, so that no
second diagnostic is reported whose only cause is an operand, callee,
receiver, scrutinee, or initializer that already failed (`let n: Text = 7`
is one `type_mismatch`; a binding initialized by an erroneous expression is
error-typed and never `cannot_infer_type`). Consequences: after
`property_called` or `method_not_called` the expression has the member's
own type, so `for x in gen.int()` is `property_called` alone and not also
`for_range_not_generator`; a `use` item that is itself diagnosed
(`unresolved_module`, `unresolved_name`, `private_item`,
`duplicate_import`, `generator_import`,
`import_conflicts_with_declaration`) binds its name to an erroneous item
that is neither `unused_import` nor reported again where it is used; a call
whose `using` argument was rejected (`capability_as_value`,
`using_argument_not_capability`, `using_arity_mismatch`) is not also
checked against the enclosing bound (4.4, 4.5), because the substitution σ
of S28 does not exist for it. Checks that are conditions on a *position*
rather than on a type are not suppressed by an error elsewhere in the same
expression: every `Gen<T>` expression outside a `for` range or `std.gen`
argument is `generator_outside_for` (so `gen.int == gen.int` carries two,
one per operand), and every misplaced handle is its S27 diagnostic.
Rationale: the design wants every diagnostic to be actionable; a cascade is
a diagnostic whose fix is another diagnostic's fix, and the negative corpus
of `spec/toolchain.md` 12.2 compares exact multisets, so the suppression
rule must be stated. Rejected alternatives: reporting every failed `≤`
test (cascades); suppressing every diagnostic inside an expression that
already has one (hides independent mistakes).

Further consequences of S62, as the checker applies them. An operator
reports at most one operand mismatch and an operator with a failed operand
is error-typed (`"a" + "b"` in a function returning `Text` is one
`type_mismatch`). The error type unifies with and binds the other side, so
beside an erroneous operand of `==`, `!=`, or `++` the other operand's
variables are bound (`[] == ?right` with an unresolvable right side is no
`cannot_infer_type`). A call, constructor application, or method call with
the wrong number of explicit type arguments (`type_argument_count_mismatch`)
is error-typed once its arguments are typed, and so is a generic struct
literal with `missing_field`. A position inside a form that already failed
accepts any type, as the arguments of an erroneous callee do: the arguments
of a callee that is unresolved, `not_callable`, `unknown_member`,
`property_called`, or a malformed `check`, arguments beyond the arity, the
entries of a broken struct literal, the right-hand side of an assignment to
a failed target, and the operand of a misplaced `return`; so an empty list
there is not `cannot_infer_type`, and a hole there is reported as `hole`
(S69), not `hole_type_unknown`. The `using` arguments of a failed call
still follow the shape rule of S28. A diagnosed `use` item named in
constructor position or in a pattern is silent too. The rule about a call
whose `using` argument was rejected applies to calls of functions, whose
bound needs σ: an operation call `db.get(key, using db)` in a pure
function is `using_arity_mismatch` and still `undeclared_effect`, since the
effect `db.get` is performed whatever the argument. The error type renders
as `?` wherever a type is rendered (S69).

Decision: [D-095] S19. expected types flow *into* literal-like forms before their
parts are typed, and *out of* calls after their arguments are typed:

| Form | Order |
| - | - |
| list literal, struct literal, constructor application (`Some(e)`, `Ok(e)`, `W(e)`, `E.V(e)`) | if an expected type `E` exists, unify the form's result type with `strip(E)` first (binding the form's inference variables), then check each element, field, or payload against the resulting part types; finally `A ≤ E` (so a refinement `E` is proved on the whole constant form: `[1] ⇐ NonEmpty`) |
| `if`, `match`, block | if an expected type `E` exists, propagate `E` unchanged into every branch, arm body, and tail (3.8, 3.10): a refinement `E` is proved or rejected per branch, and no `≤` test is applied to the form as a whole (an `if` or a block is never a constant expression, so a whole-form test would reject `fn one() -> Positive { 1 }`) |
| function call, method call, `check`, `flatten`, capability operation call | instantiate the callee with fresh variables (or the explicit `generic_args`); check each argument against its parameter type in order, left to right; then the substituted result type `A` satisfies `A ≤ E` if an expected type exists (unification during `≤` may bind remaining variables) |

Rationale: literal forms are where refinement proofs happen
(`Dimensions { width: 3, height: 4 }`, `Some(2)` at `Option<Positive>`), so
they need the expected type first; calls take their types from their
arguments (`first(xs)` at `Int` with `xs: List<Positive>` must instantiate
`T := Positive`, not `Int`), so they need the arguments first. Rejected
alternatives: arguments-first everywhere (rejects `Some(2) : Option<Positive>`
by invariance), expected-first everywhere (rejects `first(xs)` above), and
backtracking between the two.

Decision: [D-096] S20. when no expected type exists, the *first* element of a list
literal, the first non-diverging branch of an `if`/`else` or `match`, sets
the form's type; every later element or branch is checked against it with
`≤`. Hence `[size, 3]` is `List<Positive>` and `[3, size]` is `List<Int>`;
`if c { 1 } else { size }` is `Int`. Rationale: a deterministic rule that
needs no join computation and reads in source order; the refinement-to-base
direction keeps mixed lists usable. Rejected alternative: computing the
least upper bound by erasing refinements when elements differ.

Instantiation of a generic function `f<T1..Tn>`:

1. If `generic_args` are written, their count must equal `n`
   (`type_argument_count_mismatch`) and each must be a value type (S13);
   otherwise fresh `?T1..?Tn`. Explicit arguments on a non-generic callee
   are also `type_argument_count_mismatch`.
2. Positional arguments: count must equal the parameter count
   (`arity_mismatch`, giving `expected` and `actual` counts); each
   `a_j ⇐ P_j[subst]`.
3. `using` arguments: Section 4.4.
4. Result type `R[subst]`, then `≤ E` if an expected type exists.

`check<R>(value)` additionally requires an explicit type argument
(`check_requires_type_argument`) naming a refinement type
(`check_requires_refinement`); its parameter type is `strip(R)`, the root
base (`spec/stdlib.md` S4), so a value of the root base or of any
refinement in `chain(R)` is accepted by rules 1-2 of 3.4; its result is
`Result<R, ConstraintError>` and its evaluation is 6.10. `flatten` is an
ordinary generic prelude function.

Every `let`/`var` without an annotation receives the inferred type of its
initializer, refinements included (`var rest = items` is `List<T>`;
`let s = size` is `Positive`). A `let`/`var` with an annotation checks the
initializer against it and the binding has the annotated type.

### 3.8 Typing rules for expressions

Notation: `T_e` is the inferred type of `e`; `e ⇐ T` checks `e` against
`T` (3.7); `S = strip(T)`; `subst` is the instantiation of the callee's or
type's parameters. "Diverges" is defined in 3.10. Unless a row says
otherwise the form is typed in infer mode and then `T_e ≤ E` is applied
when an expected type `E` exists; a premise written `e ⇐ T` puts `e` in
check mode with expected type `T` (S56).

| Form (grammar) | Premises | Type | Diagnostics |
| - | - | - | - |
| `INT` | none | `Int` | a refinement `E` is proved by 3.5 |
| `TEXT` | none | `Text` | |
| `true`, `false` | none | `Bool` | |
| `Unit` (constructor) | none | `Unit` | |
| `name` (`LOWER_IDENT`) → local, parameter, `for`/`forall` binder, `result`, `self` | resolved per 2.2 | the binding's type | `unresolved_name`; a `using` parameter here is `capability_as_value`; a function is `not_a_value` |
| `name` (`UPPER_IDENT`), nullary constructor | resolves (2.5) to variant `V` of `E<..>` with no payload, `None`, or `Unit` | `E<?..>` (fresh variables for generic `E`, or from the expected type) | `unresolved_name`, `ambiguous_constructor`, `not_a_constructor`; a variant or wrapper that has a payload: `arity_mismatch` |
| constructor application `C(e1, ..., en)` | `C` resolves to a variant with `n` payload types `P_i`, to `Some`/`Ok`/`Err` (`n = 1`), or to a wrapper `W(P)` (`n = 1`); expected-first (S19); each `e_i ⇐ P_i[subst]` | `E[subst]` or `W` | `arity_mismatch`, plus the diagnostics of each `e_i` |
| struct literal `S { ..b, f1: e1, ... }` | `S` resolves to a struct (generic: fresh variables, expected-first); every `f_i` is a field of `S` (`unknown_field`) listed once (`duplicate_field`); without `..b` every field of `S` is listed (`missing_field`, listing the missing names); with `..b`, `b ⇐ S[subst]`; each `e_i ⇐ type(f_i)[subst]` | `S[subst]` | as listed; `S` private in another module: `private_item`; a path that names nothing: `unresolved_type`; one that names something other than a struct: `not_a_constructor` (below) |
| list literal `[e1, ..., en]` | expected-first: `E` known with `strip(E) ≡ List<T>` → each `e_i ⇐ T`; else `n ≥ 1` → `T = T_{e1}`, each later `e_i ⇐ T` (S20); else `T = ?α` | `List<T>` | `cannot_infer_type` if `?α` survives the statement |
| `( e )` | | `T_e` | |
| `not e` | `e ⇐ Bool` | `Bool` | `type_mismatch` |
| `- e` | `e ⇐ Int` | `Int` | `type_mismatch` (expected `Int`) |
| `e1 + e2`, `e1 - e2`, `e1 * e2`, `e1 / e2`, `e1 % e2` | `e1 ⇐ Int`, `e2 ⇐ Int` | `Int` | `type_mismatch` |
| `e1 ++ e2` | `strip(T_e1)` is `List<T>`, `Text`, or `Bytes`; `e2 ⇐ strip(T_e1)` | `strip(T_e1)` | `type_mismatch` (S21) |
| `e1 < e2`, `<=`, `>`, `>=` | `e1 ⇐ Int`, `e2 ⇐ Int` | `Bool` | `comparison_unsupported` (S18) when an operand's `strip` type is not `Int` |
| `e1 == e2`, `e1 != e2` | both value types; `e1` inferred, then `e2` per 3.6 (check mode against `erase(T_e1)` with the test `erase(T_e2) ≡ erase(T_e1)`, or unification) | `Bool` | `capability_equality`, `type_mismatch` |
| `e1 and e2`, `e1 or e2` | `e1 ⇐ Bool`, `e2 ⇐ Bool` | `Bool` | `type_mismatch` |
| `forall x in c: p` | `strip(T_c) ≡ List<T>`; `x : T` in `p` (scope 10); `p ⇐ Bool` | `Bool` | `forall_range_not_list`, `shadowed_binding` |
| member access `e.m` (no call) | `S = strip(T_e)`; `m` is a field of struct `S` or a property of `S` (3.9) | the field or property type `[subst]` | `unknown_member`; `m` is a method or operation: `method_not_called` |
| method call `e.m(a1, ..., an)` | `S = strip(T_e)`; `m` is a method of `S` with parameters `P_i`; `a_i ⇐ P_i[subst]`; arity exact; explicit type arguments (`xs.first<Int>()`) are accepted when their count equals the stub's generic parameters (`type_argument_count_mismatch` otherwise), and the receiver is then checked as the stub's first argument against the instantiated receiver parameter (3.7 step 2) | the method's result type `[subst]` | `unknown_member`, `arity_mismatch`, `type_mismatch` (at the receiver); `m` is a property: `property_called`; `m` is a field: `not_callable` |
| function call `f(a1, ..., an, using c1, ...)`, `m.f(...)` | `f` resolves (2.4) to a function; instantiation per 3.7 steps 1-4; `using` arguments per 4.4 | result type `[subst]` | `unresolved_name`, `not_callable` (callee is a local, field, or module), `capability_as_value` (callee is a `using` parameter: `db(x)`), `arity_mismatch`, `type_argument_count_mismatch`, `using_arity_mismatch`, effect diagnostics (4.2) |
| explicit `generic_args` on a constructor, `Some<Int>(e)`, `E.V<T1, ...>(e)` | the arguments instantiate the enum's or wrapper's type parameters exactly as 3.7 step 1 instantiates a generic function's (count must equal the type's parameter count; a wrapper has none) | `E<T1, ...>` | `type_argument_count_mismatch` |
| capability operation call `c.op(a1, ..., an)` | `c` is a `using` parameter of capability `C`, possibly in grouping parentheses (`(db).get(k)` is the operation call on `db`); `op` is an operation of `C`; `a_i ⇐ P_i`; the effect `c.op` is within the enclosing function's bound (4.2) | the operation's return type | `unknown_member`, `arity_mismatch`, `undeclared_effect`, `effect_in_contract` |
| `check<R>(e)` | explicit `R` naming a refinement; `e ⇐ strip(R)` (3.7) | `Result<R, ConstraintError>` | `check_requires_type_argument`, `check_requires_refinement` |
| `e?` | `strip(T_e) ≡ Result<T, E1>` and the enclosing function's return type `strip`s to `Result<U, E2>` with `E1 ≡ E2` (design line 94: `?` unwraps `Ok` or returns `Err`; there is no `?` on `Option`, which is handled by `match`) | `T` | `propagate_not_result` (operand is not a `Result`), `propagate_return_mismatch` (the function does not return a `Result`), `propagate_error_mismatch` (`E1 ≢ E2`), `propagate_outside_function` (in a test or property), `propagate_in_contract`; `cannot_prove_refinement` when the declared return type is itself a refinement (S73) |
| hole `?name` | an expected type `E` exists and is fully resolved when the statement ends | `E` | `hole_type_unknown` (no expected type, or one containing unresolved variables: S56 lists the positions); `hole_in_contract`; recorded for the hole report (Section 8) unless it is one of these two errors (S64) |
| generator property `gen.int`, `gen.positive_int`, `gen.bool`, `gen.text` (a module-qualified chain, 2.4, naming a zero-parameter `std.gen` function marked `@property`, S54) | read without a call | the declared `Gen<T>` (`Gen<Int>`, `Gen<Positive>` with `Positive` as in S55, `Gen<Bool>`, `Gen<Text>`) | `property_called` if called; `generator_outside_for` outside a `for` range or a `gen` argument (S14) |
| generator function call `gen.list(g)`, `gen.option(g)`, `gen.range(lo, hi)` | an ordinary function call of a `std.gen` function (`spec/stdlib.md` 8.1); `Gen<T>` arguments are permitted here by S14 | `Gen<List<T>>`, `Gen<Option<T>>`, `Gen<Int>` | as a function call; `generator_outside_for` |
| module, submodule alias, or `gen` in value position (`let m = reader`) | resolves (2.4) to a module path with no item | none | `not_a_value` |
| struct literal whose `type_path` is not a struct (`PageId { .. }`, `Positive { .. }`) | | none | `not_a_constructor` |
| `if c { A }` | `c ⇐ Bool`; `A ⇐ Unit` | `Unit` | `type_mismatch` with the note "an `if` without `else` has type `Unit`" when `E` is not `Unit`; one diagnostic in all: a failing tail is reported at the tail (with that note when `E` is not `Unit`), otherwise the mismatch with `E` at the `if`, and either way the `if` is error-typed |
| `if c { A } else { B }`, `else if` chains | `c ⇐ Bool`; with `E`: `A ⇐ E`, `B ⇐ E` (S19: `E` itself, not `strip(E)`); without: the first non-diverging branch infers `T`, the others `⇐ T` (S20) | `E`, or `T`, or diverging | `type_mismatch` |
| `match s { arms }` | `T_s` fully resolved (`cannot_infer_type`); each pattern `⇐ T_s` (3.12, which strips for constructor matching but binds the unstripped type); arm bodies as the branches of `if` (block or expression); exhaustive (3.13); a `match` with no arms types only when `Σ(T_s) = ∅` and then diverges (S24) | `E`, or `T`, or diverging | `non_exhaustive_match`, `unreachable_arm` (warning), pattern diagnostics |

Decision: [D-097] S56. the *check-mode positions* of an expression are: every
operand whose type a row above fixes (`not`, unary `-`, the arithmetic and
ordering operands, `and`/`or`, the right operand of `++`, the condition of
`if`/`while`, the operand of `assert`), every argument position of a call
or constructor, every element, field, or payload of a literal form with an
expected type (S19), the branches and tails of `if`, `match`, and blocks
with an expected type, the right operand of `==`/`!=` when the left is
resolved (3.6), the initializer of an annotated `let`/`var`, the
right-hand side of an assignment, the operand of `return`, and the tail of
a function body. A hole in any of these positions takes that type. The
*infer-only positions*, where a hole is `hole_type_unknown` and an empty
list gets no expected type (it is `cannot_infer_type` only when nothing
binds its element type by the end of the statement, 3.7, so `[] == xs`,
`[].push(1)`, and `[] ++ xs` are accepted), are: the initializer of an unannotated
`let`/`var`, the scrutinee of `match`, the range of `forall`, the left
operand of `++`, the left operand of `==`/`!=`, the receiver of a member
access or method call, the first element of a list literal and the first
non-diverging branch of an `if`/`match` without an expected type (S20), and
a non-tail expression statement's operand positions not listed above.
Rationale: the design says a hole takes its expected type and the hole
report must be deterministic across implementations, so the set of
positions that supply one must be closed; fixed-type operands supply the
obvious type, while a scrutinee or an unannotated binding has nothing to
supply. Rejected alternative: inferring a hole's type from later uses of
the binding it initializes (needs a second pass over the block).

Decision: [D-098] S21. `++` is defined on `List<T>`, `Text`, and `Bytes`, requiring
both operands to have the same `strip` type; it is the only concatenation
operator and `+` is numeric only. Rationale: the design gives `++` for
lists and no spelling for text concatenation; one concatenation operator
for all sequences is what a reader expects from the precedence table's
"addition/subtraction/concatenation" level. Rejected alternative: `+` on
`Text`.

Decision: [D-251] S73. `e?` in a function whose declared return type `R` is
itself a refinement (with `strip(R)` a `Result`, so the premise of the `e?`
row holds) is the error `cannot_prove_refinement` at the `?` expression,
with `expected` = `R`, `actual` = `strip(R)`, and a note that the
propagated `Err` would leave the function at `R` without a proof and that
a `match` on the operand returning `check<R>(...)` of the value is the
checked spelling. The propagated `Err(x)` is an exit value (5.2 step 5),
an implicit `return Err(x)`, which 3.4 rule 3 accepts at `R` only through
the proof rule of 3.5, and a propagated value is never a constant
expression (3.5 step 4). The success path is unaffected: the tail or
`return` that produces the function's value is checked against `R` as
always. Rationale: the design says "failure to prove a refinement never
becomes an implicit unchecked conversion" (line 75), and accepting `?`
there let `Err("x")` leave `fn passes(r: Result<Int, Text>) -> OkOnly` at
a refinement whose predicate admits only `Ok` values, unseen by the
evaluator's re-check of proofs, since no `Refine` node exists for it
(`spec/toolchain.md` T39). Rejected alternatives: accepting `?` whenever
`strip(R)` is a `Result` (an earlier reading of the `e?` row, unsound); a
runtime check of `R`'s predicate at the exit (`refinement_violation` is a
boundary fault, never raised by CPL-to-CPL code, 6.7);
`propagate_return_mismatch` (the return type does strip to a `Result`;
what fails is the proof, which `cannot_prove_refinement` names).

### 3.9 Member tables

Member lookup uses `strip(T_e)` (a refinement of a struct exposes the
struct's fields). A member is a *property* (used without parentheses) or a
*method* (used with a call); using one as the other is `property_called`
or `method_not_called` (D43).

| Type | Properties | Methods (signature; contract from the design) |
| - | - | - |
| `List<T>` | `len : Int` | `is_empty() -> Bool`; `take(n: Int) -> List<T>` requires `n >= 0`, clamps at `len`; `drop(n: Int) -> List<T>` requires `n >= 0`, clamps; `drop_last() -> List<T>` (empty for empty input); `push(value: T) -> List<T>` (returns a new list) |
| `Text` | `len : Int` (number of Unicode scalar values) | `is_empty() -> Bool` |
| `Bytes` | `len : Int` | `is_empty() -> Bool` |
| struct `S<..>` | each field, with its declared type `[subst]` | none |
| capability `C` | none | each operation, callable only on a `using` parameter (Section 4) |
| `Int` | none | the `@method` stubs of `spec/stdlib.md` 2.3: `to_text()`, `abs()`, `min(other)`, `max(other)` |
| `Option<T>`, `Result<T, E>` | none | the `@method` stubs of `spec/stdlib.md` 5: `is_some()`, `is_none()`, `unwrap_or(d)`, `ok_or(e)`; `is_ok()`, `is_err()`, `unwrap_or(d)`, `ok()`, `err()` |
| `Bool`, `Unit`, wrapper, enum, type parameter | none | none |

The design-fixed members are listed above; `List<T>` additionally has the
`@method` extensions of `spec/stdlib.md` 4.5 (`get(i)`, `first()`,
`last()`, `reverse()`, `contains(v)`). Every member is a `@method` or
`@property` stub of `std/prelude.cpl` (`spec/stdlib.md` S37), named
`std.prelude.<Receiver>.<member>` in diagnostics.

Decision: [D-099] S22. the member table of every type is closed as above;
`spec/stdlib.md` may add properties and methods to the prelude types
(`Text` searching, `Option` and `Result` helpers) but may not change the
property/method spelling the design fixes (`len`, `is_empty()`, `take(n)`,
`drop(n)`, `drop_last()`, `push(value)`), and user declarations cannot add
members to any type. All other operations on library types are functions
in `std` modules (`std.text.trim(text)`). Rationale: the design fixes these
spellings and a closed table is what makes `unknown_member` decidable
without a trait system. Rejected alternative: user-defined methods.

Decision: [D-100] S23. a nominal wrapper exposes no member; its payload is reached
only by the irrefutable one-arm match `match id { PageId(text) => ... }`.
Rationale: unwrapping is then always visible in source and the pattern
grammar already has the form (`PageId(t)` in grammar 3.13); `let`
destructuring does not exist (D25). Rejected alternative: a built-in
`.inner` property or `.0`.

### 3.10 Blocks and divergence

A `block` is typed in a fresh scope 8:

```text
type_block(block, E?):            // E is the expected type, if any
    for each statement s except a tail expression, in order:
        if a previous statement diverged: warn `unreachable_statement` (once per block)
        type_statement(s)                     // 3.11
    if block has a tail expression t:
        if a previous statement diverged and the block has no warning yet:
            warn `unreachable_statement` at t     // S24: a tail is an expr_stmt too
        t ⇐ E if E exists, else infer t       -> the block's type
    else if the block diverges:               -> the block's type is E (or "diverging" without E)
    else:                                     -> Unit: the missing tail stands for the constant expression `Unit`,
                                                 checked against E if E exists (3.4: `type_mismatch` with note
                                                 "missing tail expression" by rule 4; for a refinement E of `Unit`,
                                                 the proof rule of 3.5, so `where false` is `refinement_false`)
```

An arm whose body is an expression rather than a block never diverges,
even an `if` or `match` all of whose branches diverge (S24 reads only block
arms).

Decision: [D-101] S24. a block *diverges* when its last statement is a
`return_stmt`, or is an `expr_stmt` whose expression is an `if` with `else`
all of whose branches diverge (a branch is a block, which diverges as
defined here, or in an `else if` chain a further `if`, which diverges when
all of its branches do), or a `match` all of whose arms are
blocks that diverge, or a `match` with no arms whose scrutinee type has an
empty constructor set (`Σ(T) = ∅`, 3.13: an enum with no variants, of which
no value exists). A diverging block or expression satisfies any expected
type; a statement after a diverging statement in the same block is the
warning `unreachable_statement`. Nothing else diverges (`while` never
does; `assert false` does not). Rationale: without it a body ending in
`return x` inside both branches of an `if` would need a dummy tail
expression; keeping the definition syntactic needs no flow analysis.
Rejected alternative: a `Never` type.

### 3.11 Typing rules for statements

| Statement | Premises | Diagnostics |
| - | - | - |
| `let x = e` | `T_e` fully resolved; `T_e` is a value type; `x` not already bound (S8); binds `x : T_e` immutably | `cannot_infer_type`, `capability_as_value`, `generator_outside_for`, `shadowed_binding` |
| `let x: T = e` | `T` well-formed value type; `e ⇐ T`; binds `x : T` | `type_mismatch`, refinement diagnostics (3.5) |
| `let _ = e`, `let _: T = e` | as above without binding; the value is discarded (D24) | |
| `var x = e`, `var x: T = e` | as `let`; binds `x` mutably | |
| `x = e` | `x` resolves to a `var` binding of the enclosing `fn`, `test`, or `property` body (any enclosing block); `e ⇐ type(x)` | `assign_to_immutable` (`x` is a `let`, parameter, or binder), `unresolved_name`, `type_mismatch` |
| `return e` | inside a `fn` body; `e ⇐ declared return type` | `return_outside_function`, `type_mismatch` |
| `return` | inside a `fn` body whose return type `strip`s to `Unit`; the missing operand stands for the constant expression `Unit`, checked against the declared return type (a refinement of `Unit` is proved by 3.5, as for a missing tail, 3.10) | `return_outside_function`, `return_value_missing`, refinement diagnostics (3.5) |
| `assert e` | `e ⇐ Bool` | `type_mismatch` |
| `while c` clauses `{ B }` | `c ⇐ Bool`; each `invariant p`: `p ⇐ Bool` and `p` is a contract expression (5.1); `decreases m`: `strip(T_m) ≡ Int` and `m` is a contract expression; `B ⇐ Unit` | `type_mismatch`, contract diagnostics |
| `expr_stmt` (not the tail) | `e ⇐ Unit` (check mode, so a bare hole statement `?log_items` has type `Unit`), or the expression diverges; a failure of the `≤ Unit` test on the statement's own expression is reported as `unused_value` with `type` = `T_e` instead of `type_mismatch`; when `e` is an `if`, `match`, or block, `Unit` flows into its branches (S19) and a non-`Unit` branch tail is `type_mismatch` there (S68) | `unused_value` (with the note "use `let _ = ...` to discard"), `type_mismatch` |
| `expr_stmt` (tail) | `e ⇐ E` of the block, or inferred | per 3.8 |

Assignment to a `var` declared in an enclosing block of the same body is
permitted (the design's `pages = pages.push(...)` inside `while` and `if`).
There are no closures, so no other function can observe a `var`.

Decision: [D-235] S68. `unused_value` replaces `type_mismatch` only for the
statement's own expression (a call, a literal, an operator form, a name):
`xs.push(1)` as a statement is `unused_value`. When the statement's
expression is an `if`, a `match`, or a block, the expected type `Unit`
flows into every branch and tail (S19) and no whole-form test exists, so
each branch tail whose type is not `Unit` is `type_mismatch` (3.10) at that
tail: `if c { xs.push(1) } else { xs.push(2) }` as a statement carries two,
and `if c { xs.push(1) }` carries one whether or not it is a statement (the
`A ⇐ Unit` premise of the `else`-less `if`, 3.8). Rationale: the branch
tail is where the value is produced and where the fix (`let _ = ...` inside
the branch, or dropping the value) goes, and the `else`-less form already
reports there, so one code for both forms keeps the rule uniform. Rejected
alternative: `unused_value` at every branch tail (the `else`-less form
would then differ between statement and value position).

A parenthesized `if` or `match` statement, `(if c { 1 } else { 2 })`,
counts as the `if` or `match` itself: `Unit` flows into its branches, and a
non-`Unit` branch tail is `type_mismatch` there.

### 3.12 Pattern typing

`p ⇐ T` where `T` is the scrutinee type (or a payload type), `S = strip(T)`:

| Pattern | Premises | Binds |
| - | - | - |
| `_` | none | nothing |
| `x` (`LOWER_IDENT`) | `x` not bound in an enclosing scope (S8); once per pattern (`duplicate_pattern_binding`) | `x : T` (the unstripped type: a `Positive` payload binds a `Positive`) |
| `INT`, `- INT` | `S ≡ Int` | nothing |
| `TEXT` | `S ≡ Text` | nothing |
| `true`, `false` | `S ≡ Bool` | nothing |
| `C` (nullary) | `C` resolves against `S` (2.5 step 1): a payload-less variant of enum `S`, `None`, or `Unit` | nothing |
| `C(p1, ..., pn)` | `C` resolves against `S` to a variant with `n` payload types `P_i` (`pattern_arity_mismatch` otherwise), to `Some`/`Ok`/`Err` with `n = 1`, or to the wrapper `S` itself with `n = 1`; each `p_i ⇐ P_i[subst]` | the union of the sub-patterns' bindings |

Any other combination is `pattern_type_mismatch` (`expected` = `T`,
`actual` = the pattern's constructor type or literal kind). A qualified
`ctor_path` whose type is not `S` is `pattern_type_mismatch`. Struct
scrutinees admit only `_` and bindings (D33).

### 3.13 Match exhaustiveness

Decision: [D-102] S25. for exhaustiveness and reachability every scrutinee type
`T` has a *constructor set* `Σ(T)`, computed on `strip(T)`: `Bool` is
treated as an enum with constructors `true` and `false`, `Unit` as an enum
with the single constructor `Unit`, a wrapper `W(P)` as an enum with the
single constructor `W` (arity 1), `Option`/`Result`/user enums have their
variants (an enum with no variants has `Σ = ∅`), and a struct `S` has the
single constructor `S` whose arity is its field count (no pattern names it,
D33, so a struct scrutinee is matched only by `_` or a binding). `Int`,
`Text`, `Bytes`, `List<_>`, and a type parameter have an infinite
constructor set (`finite(T)` is false), so `Int` and `Text` literal
patterns never make a match exhaustive and such a match needs a `_` or
binding arm. `Gen<_>` and capabilities cannot be scrutinees (S14, S27).
Rationale: `true`/`false` enumerate their type completely and every
mainstream language with exhaustive matching accepts a two-arm `Bool`
match, while no finite set of integer or text literals can; giving every
value type a constructor set makes the algorithm below total. Rejected
alternative: treating all literal patterns alike (forces `_` on `Bool`
matches).

The checker runs the usefulness algorithm over pattern matrices
(Maranget): rows are the arms' patterns in order, each a vector of length
one initially.

```text
Σ(T), finite(T): as in S25
useful(P, q):                                   // is there a value matched by q and by no row of P?
    if q is the empty vector: return P has no rows
    let T be the type of column 1
    if q1 is a constructor c(r1..rk) (literals are constructors with k = 0):
        return useful(specialize(c, P), r1..rk ++ q[2..])
    // q1 is a wildcard or binding
    let H = constructors appearing at the head of some row of P
    if finite(T) and H = Σ(T):
        return exists c in Σ(T): useful(specialize(c, P), _^arity(c) ++ q[2..])
    else:
        return useful(default(P), q[2..])
specialize(c, P): rows whose head is c(r1..rk) become r1..rk ++ rest; rows whose head is a
                  wildcard/binding become _^k ++ rest; other rows are dropped
default(P):       rows whose head is a wildcard/binding, with the head removed
```

| Check | Rule | Diagnostic |
| - | - | - |
| exhaustiveness | `useful(rows of all arms, [_])` must be false | `non_exhaustive_match` with `missing`: witness patterns built during the search (the missing constructor applied to `_` for each payload, e.g. `Err(Unavailable)`; `_` for an infinite type), listed in the declaration order of the missing constructors, depth-first; `type` (the scrutinee type) |
| reachability | arm `i` is reachable iff `useful(rows 1..i-1, row i)` | `unreachable_arm` (warning) on the arm |

A single-arm match on a wrapper or on `Unit` is exhaustive. A `match` with
no arms is exhaustive only when `Σ(T) = ∅` (an enum with no variants), and
then diverges (S24); on every other type it is `non_exhaustive_match` with
`missing` = the constructors of `Σ(T)` (or `["_"]` for an infinite set).
Witnesses follow the pseudo-code: when some constructors of a column are
missing the witness is each missing constructor (`Some(1)` alone reports
`None`); when a column's specialized matrix has no rows, the witness lists
every constructor of that column's type, as an arm-less match does (`enum S
{ Both(Bool, Bool) }` with the one arm `Both(true, true)` reports
`Both(false, true)` and `Both(false, false)`); a struct's constructor, which
no pattern can name, renders as `_`. A pattern that already has a
diagnostic takes no part in reachability, and a `match` with such a pattern
is not checked for exhaustiveness (S62).
Guards do not exist, so the analysis is exact. At runtime a match with no
matching arm cannot occur after a successful check; if it does, the fault
is `internal_error` (6.7).

### 3.14 Bodies

| Body | Checked as |
| - | - |
| `fn` body | `type_block(body, declared return type)`; `return`, `?`, and `result` refer to this function |
| `test` body | `type_block(body, Unit)`; no `return`, no `?` |
| `property` body | `type_block(body, Unit)` with the `for` binders in scope; no `return`, no `?` |
| contract clauses, `where` predicates, `for` ranges | expressions, typed as `⇐ Bool` (`requires`, `ensures`, `invariant`, `where`), `strip ≡ Int` (`decreases`), or `Gen<T>` for some `T` (`for`, S14; anything else is `for_range_not_generator`) and subject to Section 5.1 |

An `fn` whose body is a hole (`{ ?build_pages }`) typechecks: the hole
takes the return type. Callers of a function containing holes typecheck
normally against its signature; the hole is reported per Section 8.

Outside a `fn` body (in a contract expression, a `test`, or a `property`),
`return` and `?` are not checked against any function: the position is the
error (`return_outside_function`, `propagate_outside_function`, or, in a
contract expression, `propagate_in_contract` and `statement_in_contract`),
and the operand of such a `return` is checked against the error type
(S62).

## 4. Effect system

### 4.1 Capability parameters and effect bounds

A capability type `C` (from `capability_decl`) is a *handle type*: it is
not a value type (3.2). The only way a function obtains a handle is a
`using` parameter (`using db: PageRead`), whose type must be a capability
(`using_param_not_capability`). An operation call `db.get(id)` is a
*tracked effect* named `db.get`.

A function's *effect bound* is the set of `effect_ref`s in its `effects`
clause, each `p.op` where `p` is one of its own `using` parameters
(`effect_param_not_capability`) and `op` an operation of `p`'s capability
(`unknown_effect_operation`).

Decision: [D-103] S26. the effect bound is explicit for every function, public or
private, as a direct consequence of design line 132 ("functions are
externally pure unless they declare effects", stated for every function):
a function with no `effects` clause, or with `effects {}`, is *pure* and
may neither call a capability operation nor call a function whose bound is
non-empty after substitution (4.4). No bound is ever inferred; the design's
"public functions require explicit effect bounds if effectful" (line 634)
restates the rule for the public case and does not open private bounds to
inference. The decision this document adds is that the empty clause
`effects {}` and an absent clause mean the same thing. Rationale: the bound
"is an upper bound checked through the whole call graph" (line 207), and
an inferred bound would make a private function's effects invisible in its
header, which agents read instead of bodies. Rejected alternative:
inferring bounds for private functions.

### 4.2 No storage of handles (aliasing)

Decision: [D-104] S27. a capability handle can be referred to only by the name of
the `using` parameter that received it. It may appear exactly in three
positions: as the receiver of an operation call (`db.get(id)`), as a
`using` argument (`using db`), and as the left part of an `effect_ref`.
Every other occurrence is an error:

| Occurrence | Diagnostic |
| - | - |
| `let`/`var` initializer, assignment right-hand side, positional argument, list element, struct field initializer, constructor payload, `return` value, block tail, `match` scrutinee, `forall` range, `assert` operand | `capability_as_value` |
| operand of `==`/`!=` | `capability_equality`, one per handle operand, spanning it (S17) |
| capability type in a positional parameter, field, payload, wrapper payload, refinement base, return type, type argument, or `let`/`var` annotation | `capability_in_value_position` |
| a `using` parameter on a `test`, `property`, or capability operation | parse error (grammar admits none) |

Rationale: with no aliases and no storage, the set of handles a function
can act through is exactly its `using` parameter list, so the bound check
of 4.4 is a syntactic substitution and is complete; the design asks for the
bound to be "checked through the whole call graph, including aliases and
capability forwarding", and forbidding aliases makes forwarding the only
case. Rejected alternative: capabilities as first-class values stored in
records (`ctx.reader`), which D31's grammar leaves room for and a later
edition may add together with an alias analysis.

### 4.3 `using` arguments and forwarding

Decision: [D-105] S28. at a call `g(a1, ..., an, using c1, ..., using cm)` the
`using` arguments are matched to `g`'s `using` parameters *by position*;
their count must equal `g`'s `using` parameter count
(`using_arity_mismatch`, also when `g` has none and some are passed, or has
some and none are passed); and each `c_j` must be a bare `LOWER_IDENT`
naming a `using` parameter of the *enclosing* function whose capability
type is identical to the `j`-th `using` parameter's type
(`using_argument_not_capability`, `type_mismatch`). Rationale: this is the
only shape S27 leaves, and positional matching mirrors how positional
parameters work; a by-name form would be a second argument syntax the
grammar does not have. Rejected alternative: resolving `using` arguments
by capability type (ambiguous when two parameters share a type).

Every `using` argument that is not a bare name is
`using_argument_not_capability`, whatever it contains: `using (db)`,
`using db(key)`, `using key.nope`, and `using ?h` (the one report for that
hole, S64); the typer emits the code, although its category is `effect`,
because only the syntax tells a bare name from any other expression.

A capability operation has no `using` parameters, so an operation call
never carries `using` arguments.

### 4.4 Effect checking

For a function `f` with bound `B_f` (a set of `p.op` pairs), every call in
`f`'s body, contracts excluded (4.5), is checked:

```text
check_call(f, call):
    if call is `c.op(...)` on using parameter c:
        require (c, op) ∈ B_f                               else `undeclared_effect`
    if call is `g(..., using c1, ..., using cm)`:
        let σ map g's using parameters u1..um to c1..cm       // S28
        for each (u, op) ∈ B_g: require (σ(u), op) ∈ B_f    else `undeclared_effect`
    // method calls on prelude types, constructors, `check`, `flatten` have empty bounds
```

`undeclared_effect` carries `symbol` (= `f`), `qualified_name`, `facet:
"effects"`, `effect` (the offending `c.op` after substitution), and
`callee` (the operation or function called), and spans the call; a call
reports one per offending effect. An entry of `f`'s own `effects` clause
that is itself diagnosed suppresses the effects it may have meant (S62): an
`effect_ref` `p.op` with an unknown `op` covers every operation through
`p`, and one whose `p` is not a `using` parameter covers every operation
named `op`. A pure `f` (empty bound) therefore rejects every operation
call and every call whose substituted bound is non-empty, which realizes the
design's "functions are externally pure unless they declare effects" (line
132) and its "`effects` is an upper bound checked through the whole call
graph" (line 207). Because `B_g` is `g`'s declared bound, not its body,
checking is modular: changing `g`'s body never invalidates `f`'s check;
widening `g`'s bound does.

| Additional rule | Diagnostic |
| - | - |
| the same `effect_ref` listed twice | `duplicate_effect` (warning) |
| a `using` parameter that appears in no operation call and no `using` argument | `unused_capability` (warning, spanning the parameter, `facet: "signature"`); not reported for a function without a body, nor when a call in the body lacks or has a rejected `using` argument for a parameter of the same capability (`using_arity_mismatch`, `using_argument_not_capability`, `type_mismatch`), because fixing that error means passing the handle (S62) |
| an `effect_ref` whose operation is never exercised by the body (bound wider than the body) | none: the bound is an upper bound by design |

Decision: [D-106] S29. an effect listed in the bound but never used is not
diagnosed, while an unused `using` parameter is a warning. Rationale: the
design explicitly makes the bound an upper bound ("describes potential
behavior"), so a wide bound is a legitimate interface promise; an unused
handle, by contrast, grants authority for nothing. Rejected alternative: an
"unused effect" warning (would discourage stable bounds).

### 4.5 Purity of tests, properties, and contracts

Decision: [D-107] S30. `test` and `property` declarations are pure: they have no
`using` parameters (grammar), hence no handles, hence by S27-S28 they can
call neither capability operations nor functions with non-empty bounds
(the call would need a `using` argument no name can supply). Capability
test doubles do not exist in 0.1. Rationale: the design gives no mechanism
for a test to obtain a handle, and the runtime of Milestone 1 cannot bind
one (Section 6.9). Rejected alternative: a `with` form that instantiates a
recording capability for a test (deferred to the adapter milestones).
Milestone 3 amends this decision: a test or property may declare `using`
parameters, which `cpl test` binds to fixtures (S83); one without them
stays pure.

Contract expressions (`requires`, `ensures`, `invariant`, `decreases`,
`where`, `for` ranges) are typed with an *empty* bound regardless of the
enclosing function's bound: an operation call in one is `effect_in_contract`
(one per clause and effect, spanning the clause, category `effect`) and a
call to a function with a non-empty bound would be `undeclared_effect`,
which no program reaches: the `using` argument it needs is rejected by 5.1
as `capability_as_value` (no contract position can pass a handle: a
function's handles resolve in its clauses, but every `using` argument
there is rejected), or
its absence is `using_arity_mismatch`, and either way the call is not also
`undeclared_effect` (S62). Contract expressions are not counted in
`EffectBound::inferred`.

### 4.6 Contracts on capability operations

Decision: [D-108] S31. a capability operation may carry `requires` and `ensures`
clauses (D21). They are typed with the operation's parameters in scope
(`result` in `ensures`), with an empty bound, and under Section 5.1. In
checked mode a `requires` clause is evaluated *by the caller* after the
arguments and before the handle is invoked; a violation is a
`contract_violation` whose `symbol` is the operation
(`reader.pages.PageRead.get`, its qualified name, 1.3), with `facet:
"contract"`, `attributed_to: "caller"`, and `caller` naming the calling
function. An `ensures` clause is evaluated after the handle returns, with
`result` bound to the returned value; a violation is a `contract_violation`
whose `symbol` is the operation, whose `facet` is `"contract"` (the clause
belongs to the operation's contract facet, design line 291), and whose
`attributed_to` is `"binding"`, because the party at fault is the bound
implementation, not CPL code (S57). Rationale: the
design calls adapter contracts "assumptions unless independently verified";
evaluating them at the boundary turns an assumption into a check in
development and gives Milestone 3-4 adapters a defined failure. Rejected
alternative: contracts on operations as documentation only.

Operations have no `decreases` and no `effects` (grammar). In Milestone 1
no CPL program can obtain a handle (6.9), so these semantics are exercised
by the typechecker and, at run time, only through the reference
evaluator's host API (`spec/toolchain.md` 1.7, `Interpreter::bind_capability`);
from Milestone 2 they also run under `cpl run` over C bindings (S78) and
under C hosts (6.9), with the operation's result validated between the host
call and its `ensures` clauses (S77).
A check diagnostic about an operation's clause (5.1)
blames the capability: its `symbol` is the capability's identity and its
`facet` is `contract`, since the operations' clauses are the capability's
contract facet (`spec/toolchain.md` 5.1); the operation's own qualified
name is the `symbol` of the runtime diagnostics of its clauses alone (and,
from Milestone 2, of the `refinement_violation` of its result, S77): its
`contract_violation` (above), and a fault raised while one of its clauses
is evaluated, of which the operation is "the declaration whose clause was
being evaluated" (S34; `pl.caps.Store.get` for `requires 100 / (id - 7) !=
1000` called with `id` 7).

## 5. Contracts

### 5.1 Contract expressions: purity and totality

A *contract expression* is the expression of a `requires`, `ensures`,
`invariant`, or `decreases` clause, a refinement `where` predicate, or a
property `for` range. It must be pure and total under its preceding
assumptions. Statically:

| Rule | Diagnostic |
| - | - |
| no hole | `hole_in_contract` |
| no `?` | `propagate_in_contract` (also outside any function, in a `where` predicate or a `for` range, where it takes precedence over `propagate_outside_function`) |
| no capability operation call, no call to a function with a non-empty bound (4.5) | `effect_in_contract`, `undeclared_effect` |
| no `using` argument (there is no handle to pass); in a contract this is the only report for a `using` argument, whatever its form (`using (db)` is `capability_as_value` alone, not also `using_argument_not_capability` or `type_mismatch`, as `propagate_in_contract` takes precedence over `propagate_outside_function`): exactly one per argument, spanning the whole argument, whose `name` is the argument's text and `type` its type (`?` when erroneous) when the argument is not a handle; a hole inside such an argument is covered by it (no `hole_in_contract` beside it, S64), while a resolution error inside it (`unresolved_name`) and a call-level `using_arity_mismatch` are still reported | `capability_as_value` |
| blocks inside the expression (the branches of an `if`, the block bodies of `match` arms) may contain only `let` statements and a tail expression: no `var`, assignment, `return`, `assert`, `while`, or expression statement; a statement nested inside one already reported is not reported again | `statement_in_contract` (spanning the statement) |
| `requires`, `ensures`, `invariant`, `where` have type `Bool`; `decreases` has `strip` type `Int` | `type_mismatch` |
| a clause of declaration `d` may not call `d` itself, unless `d` is a `@builtin` declaration of the standard library (S61); this covers a loop `invariant` or `decreases` that calls the enclosing function, and self-calls are found from the syntax and name resolution, so a call that could not be typed counts too | `recursive_contract` (one per clause, spanning it) |
| a public declaration's contract facet calls only public and prelude functions (S4: its `requires`, `ensures`, `decreases`, a public refinement's `where`, and a public capability's operation clauses; a `while` invariant is part of the body facet and is not covered) | `private_in_public_contract` (S4; category `resolve`) |
| `result` only in `ensures`; `self` only in `where` | S9 |
| `assert` is a statement and cannot appear | grammar |

Decision: [D-109] S32. nested blocks inside contract expressions admit only `let`
and a tail expression. Rationale: the design requires contracts to be pure
and total; `let` is pure sugar for a sub-expression, while `var`,
assignment, `return`, and loops either mutate or exit, and `assert` would
turn a contract into a second failure channel. Rejected alternative:
expression-only contracts with no `let` (hurts readability of long
`ensures`).

Decision: [D-110] S33. a contract clause may not call the declaration it belongs to
(direct self-reference is `recursive_contract`); indirect recursion through
another function is not detected statically and, if it loops at runtime,
ends in `resource_exhaustion`. Rationale: an `ensures` that calls its own
function would re-enter its own `ensures` forever in checked mode, so the
direct case is always a mistake; detecting the indirect case needs a
call-graph analysis that Milestone 5 will own. Rejected alternative: no
check.

Decision: [D-228] S61. the clauses of a `@builtin` declaration (`spec/stdlib.md`
10.2) are exempt from S33: `ensures trim(result) == result` on
`std.text.trim`, and the corresponding clauses of `to_upper` and
`to_lower`, are legal, and the checker reports no `recursive_contract` for
a declaration carrying `@builtin`. Such a clause is a specification: a
builtin's `ensures` clauses are never evaluated at run time (S58,
`spec/stdlib.md` S39), so the re-entry S33 guards against cannot occur, and
the Milestone 5 verifier reads them as axioms about the builtin
(`spec/stdlib.md` S41), idempotence included. The exemption is keyed on
`@builtin`, which only standard-library files may carry (1.8), so no
project declaration can claim it. Rationale: the normative stub text of
`spec/stdlib.md` Section 11 states idempotence this way, and the
alternative spellings lose information. Rejected alternatives: moving the
three clauses to the laws tables (a law is sampled, not assumed,
`spec/stdlib.md` S41); exempting every bodiless declaration (capability
operations are bodiless and their `ensures` clauses run at the boundary,
S31).

Totality is not checked statically in 0.1: a contract expression may fault
(`a % b` with `b == 0`). The design's rule that preconditions are checked
sequentially makes the remainder safe after `requires b != 0`: clauses are
evaluated in source order and a later clause runs only if every earlier
clause held. From Milestone 5 every `/` and `%` in the body or clauses of a
`@prove` function is a proof obligation `b != 0` on its divisor, exactly as
a callee's precondition is (`spec/stdlib.md` S8; `spec/toolchain.md` 8.1
lists it as the `division` obligation).

Decision: [D-111] S34. a fault raised while evaluating a contract expression is
reported as that fault (`division_by_zero`, `resource_exhaustion`, a nested
`contract_violation` from a called function's own contract), with the
diagnostic's `facet` set to `"contract"`, `clause` set to the clause text,
and `symbol` set to the declaration whose clause was being evaluated; it is
not converted into a `contract_violation` of the enclosing clause. An
`internal_error` is the exception: it names no declaration, facet, or
clause, whatever was being evaluated (6.7, 9.4), because it reports a state
of the runtime rather than of the program.
Rationale: a faulting contract is a specification bug distinct from a
violated one, and the design keeps faults and ordinary outcomes separate.
Rejected alternative: treating a faulting clause as `false`.

Decision: [D-248] S70. when contract expressions nest (a clause calls a
function or evaluates a predicate whose own clause runs in turn), the
innermost clause that a fault escapes is the one S34 reports: the fault
keeps that clause's `symbol`, `facet: "contract"`, and `clause`, and the
outer clauses leave it as it is; a `contract_violation` raised by a called
function's own contract (its clause evaluated to `false`, so no fault
escaped that clause) is claimed by the clause whose evaluation called the
function, as S34's example of a nested `contract_violation` says. S34
rewrites those three fields only: `clause_kind`, `attributed_to`, `caller`,
`input`, `actual`, `span`, and the backtrace still describe the fault as it
was raised, so for `xs.take(-1)` inside a `requires` of `f` the diagnostic
names `f` and its clause while `input` holds `self` and `n` and `span` is
the stub's clause, and the `contract_counterexample` of S46 is built from
the violation as raised. Every fault but `internal_error` (S34) is claimed
this way, including an `assertion_failed` or `unresolved_hole` raised in
the body of a function that a clause calls (a `for` range bound calling
`asserting()`): its `symbol` and `clause` become the clause's (the
property and the range text, S71), and the asserted expression or the
hole's own declaration then remain visible only through the backtrace and
the fault's other fields (`hole`, `bindings`). Rationale: the innermost clause is where the
faulting expression is written (6.10 already names the refinement for a
fault inside a `check<R>` predicate), while a violation is the fault of the
clause that relied on the callee's contract; keeping the other fields lets
an agent still see the inner violation. Rejected alternatives: the
outermost clause always (blames a declaration for a fault inside a
predicate it never wrote); new fields for the enclosing clause (fields 9.4
does not define; possible in a later edition).

Decision: [D-249] S71. a property's `for` range is a contract expression of
the property (above), and S34 applies to every fault raised while its
expressions are evaluated at setup (the bounds of `gen.range`, and every
call they make): the fault is reported with `symbol` = the property,
`facet: "contract"`, and `clause` = the canonical text of the range
expression alone, without `for x in` (`gen.range(1 / 0, 5)`), unless an
inner clause claimed it (S70). The generator's own precondition is not
such a fault: `gen.range`'s `requires lo <= hi`, which the runner checks
after both bounds have been evaluated, keeps the builtin shape of 6.10
(`symbol` `std.gen.range`, `clause` `lo <= hi`, `caller` the property),
as `spec/stdlib.md` 12 requires. Rationale: the range is written by the
property, so a fault in it is the property's specification bug, exactly
as for any other contract expression (S34 also reports loop invariants,
which belong to the body facet, under `contract`), while a violated
generator precondition is the generator's documented contract, which
stdlib 12 names. Rejected alternatives: exempting everything raised in a
bound from S34 (an earlier reading: a helper's division by zero would be
blamed on the helper, not on the property that wrote the range); applying
S34 to the generator's own precondition as well (contradicts stdlib 12).

### 5.2 Checked mode: functions

The default and only contract mode in Milestone 1 is `checked`. Every
contract clause of every declaration runs at its boundary; the only check
eliminated statically is a refinement proved by 3.5. A call `f(a1, ...,
an)` of a function `f` with clauses `requires r1..rk`, `ensures s1..sm`
executes, in this order:

1. The caller evaluates `a1, ..., an` left to right (6.2). `using` arguments
   bind handles; nothing is evaluated for them.
2. Parameters are bound.
3. `r1` is evaluated; if `true`, `r2`; and so on. The first clause that
   evaluates to `false` raises `contract_violation` with `clause_kind:
   "requires"`, `symbol` = `f`, `facet: "contract"`, `attributed_to:
   "caller"`, `clause` = that clause's text, `input` = the argument values
   by parameter name (the positional parameters: a `using` parameter's
   handle is no argument value and is not listed), `caller` = the calling declaration's qualified name
   (omitted when the caller is the command line, S39), `span` = the clause.
   Later clauses are not evaluated. When every `requires` clause held and
   `f` has a `decreases` clause, 5.4 step (a) runs now, under the
   preconditions (the measure is written after them and may rely on them,
   design line 118).
4. The body executes.
5. *Every exit* of the body evaluates the `ensures` clauses: the tail
   expression's value, each `return e`, `return` (value `Unit`), and each
   `?` that propagates an `Err` (its propagated value is the exit value).
   With `result` bound to the exit value, `s1, ..., sm` are evaluated in
   order; the first `false` raises `contract_violation` with `clause_kind:
   "ensures"`, `symbol` = `f`, `facet: "contract"`, `attributed_to:
   "body"`, `clause`, `input` (the original argument values; parameters are
   immutable so they are still available), `actual` = the exit value,
   `span` = the clause.
6. The exit value is returned to the caller.

A `Result`-returning function's `ensures` sees the whole `Result`,
including `Err` values; a postcondition about the success value must match
`Ok` explicitly (design). A fault inside the body propagates without
evaluating `ensures`.

### 5.3 Checked mode: loops

A `while c` loop with `invariant i1..ik` and optional `decreases m`:

```text
evaluate i1..ik in order; the first false raises contract_violation
    (clause_kind: "invariant", iteration: 0)                 // before the first test of c
prev = none
loop:
    if not c: exit the loop                                  // c is evaluated each time around
    if decreases present:
        v = evaluate m
        if v < 0:                 contract_violation (clause_kind: "decreases", actual: {"measure": v})
        if prev != none and not v < prev:
                                  contract_violation (clause_kind: "decreases", actual: {"previous": prev, "measure": v})
        prev = v
    execute the body block
    evaluate i1..ik in order; the first false raises contract_violation
        (clause_kind: "invariant", iteration: n)             // after every iteration body
```

Invariant and decreases violations carry `symbol` = the enclosing
declaration, `facet: "contract"`, `attributed_to: "body"`, `clause` = the
clause text, `iteration` = the number of completed iterations, `input` =
the enclosing function's arguments as in 5.2 (its positional parameters;
`{}` for a function without
parameters; absent for a loop in a `test` or `property` body, which has no
arguments), and `actual` = an object of the fixed
shape `{"bindings": {<name>: <value JSON>, ...}, "measure"?: <Int>,
"previous"?: <Int>}`: `bindings` holds every local binding visible at the
loop, the parameters and `using` parameters included, `measure` and `previous` are present exactly where the pseudo-code
above shows them (an `invariant` violation has `bindings` only). The
measure is evaluated before each iteration body and must be a nonnegative
`Int` that is strictly smaller than the previous iteration's value; it is
not evaluated after the loop exits.

### 5.4 Checked mode: recursive functions with `decreases`

Decision: [D-112] S35. for a function `f` with a header `decreases m`: (a) on every
entry to `f`, at the end of step 3 of 5.2 (after every `requires` clause
has held, so that the measure may rely on the preconditions, as `decreases
a / b` under `requires b != 0` must) and before the body, `m` is evaluated
with the parameters bound; a negative value raises `contract_violation`
(`clause_kind: "decreases"`, `actual: {"measure": v}`);
if the call stack contains an earlier activation of `f` (the same
declaration, through any number of intermediate frames of other functions)
and `v` is not strictly smaller than the measure recorded by the *nearest*
such activation, it raises `contract_violation` (`clause_kind:
"decreases"`, `actual: {"previous": prev, "measure": v}`); (b) the value
is recorded on the activation. Rationale: the design says a recursive
verified function declares a decreasing measure and that it must decrease
on every back edge; comparing with the nearest activation of the same
function is the dynamic analogue and also catches re-entry through
helpers. Rejected alternative: comparing only direct self-calls.

When the nearest activation of `f` has not recorded its measure yet (it is
still evaluating its `requires` clauses, one of which called `f` again),
no comparison is made. A header `decreases` violation also carries
`input`, as a `requires` violation does.

A `decreases` clause on a function that is not recursive (no path in the
program's call graph from `f` back to `f`) is the warning
`unused_decreases` (`facet: "contract"`, spanning the clause). The call
graph includes the calls made from clauses (`spec/toolchain.md` 6.5 records
them as interface references), except a function's direct call of itself
from its own clauses, which is `recursive_contract` (or, for a `@builtin`,
an S61 axiom): that call alone makes no function recursive, so it neither
requires a `decreases` nor uses one. Termination is an obligation of every `@prove` function
(design line 134), and a `decreases` clause is what states it (lines 170
and 172), so each of the following is the warning `missing_decreases` in
Milestone 1 and an `unknown` termination obligation from Milestone 5 (S36):
a `@prove` function that is recursive (lies on a cycle of the call graph,
directly or through other functions) and has no header `decreases`; and
every `while` loop without a `decreases` clause in the body of a `@prove`
function. The warning carries `symbol` and `facet: "contract"`; its `span`
is the function's name token or, for a loop, the whole loop from `while`
(S63). `cpl prove` lists the first condition, for any function, as the
`termination` obligation already in Milestone 1 (`unknown`, reason
`no_measure`, `spec/toolchain.md` 8.1), while a loop without `decreases`
has no obligation of its own (there is no measure to state one) and is
reported only through the note of 5.5.

### 5.5 `@prove`

Decision: [D-113] S36. `@prove` on a function records the verification policy
"every contract and termination obligation of this function must be
discharged by the verifier". (a) The contracts of a `@prove` function are
checked at runtime exactly like any other function's; (b) `cpl prove
<symbol>` and `cpl check` (in text and JSON rendering alike, 9.1) report the
function's verification status as the verifier decides it
(`spec/verification.md`; diagnostic `verification_result`, severity
`info`, one per `@prove` declaration): `proved` when every obligation is
proved, termination is known, and no typed hole is left; `failed` when an
obligation fails, with the inputs that break it; `unknown` otherwise, with
the reason. A function with a `missing_decreases` warning (5.4) is
`unknown` for termination (`no_measure`) whatever its other obligations;
(c) a release check (Section 8.2) rejects every declaration whose
verification policy is unsatisfied: a `@prove` function with status
`unknown` or `failed` is the error `verification_unsatisfied`, with no
project-level switch to weaken it, because the design says an `unknown`
result never weakens the requirement (line 134) and that release builds
require the verification obligations to be closed (line 225). The design's
only admission mechanism, "explicitly admitted by project policy" (line
136), concerns a *caller's* named assumption about an unchecked or foreign
*dependency* of its proof, never a declaration's own `@prove` obligations;
it belongs to Milestone 5 (Section 10), where every admitted assumption is
recorded per dependency in the receipt's `assumptions` (S59) and remains
rejected for CPL implementations under `no_unresolved_assumptions` (D36).
Rationale: tests and runtime checks never silently become a proof; the
`holes` corpus project fails `--release` for two reasons (its hole and its
`@prove` function, `unknown` with `unresolved_hole`), and `reader` for one
(`paginate`, whose list postconditions are outside the verifier's
fragment). Rejected alternatives: treating `@prove` as a no-op; a
project-wide `admit` switch for unknown results (deletes the meaning of
`@prove` for any project that flips it).

Verification statuses are `proved`, `failed`, and `unknown` (9.5;
`conditional` arrives with Milestone 5 and satisfies the policy); a
`checked`-mode run that passes contributes nothing to them. A function
with no proof obligation, or only refinement obligations that 3.5
established, is `proved` (`spec/toolchain.md` 8.1's status table).

`verification_result` carries `symbol`, `qualified_name`, `facet:
"verification_policy"`, `status`, `reason`, and the `receipt` of S59,
spans the function's name token, and has a note when the function's
termination is also unknown ((b) above). `cpl check` emits it for `@prove`
functions only; `cpl prove` reports on any function, `@prove` or not, and
adds the same note whenever 5.4's conditions hold (the function is
recursive without a header `decreases`, or has a `while` loop without
one). The checker emits it without the
`receipt`, which needs the Git revision and the module content hashes; the
command layer attaches it (`spec/toolchain.md` 1.6, 7.2), as it does the
hole report's `dependencies`. `verification_unsatisfied` marks its
definition `HasErrors`, as `hole_in_release` does.

### 5.6 Contract diagnostics summary

| Situation | Code | Key fields |
| - | - | - |
| `requires` false at entry | `contract_violation` | `clause_kind: "requires"`, `symbol` (callee), `facet: "contract"`, `attributed_to: "caller"`, `caller`, `clause`, `input`, `status: "failed"` |
| `ensures` false at an exit | `contract_violation` | `clause_kind: "ensures"`, `symbol`, `facet: "contract"`, `attributed_to: "body"`, `clause`, `input`, `actual`, `status: "failed"` |
| `invariant` false | `contract_violation` | `clause_kind: "invariant"`, `symbol`, `facet: "contract"`, `attributed_to: "body"`, `clause`, `iteration`, `input`, `actual: {bindings}` (5.3) |
| `decreases` negative or not decreasing | `contract_violation` | `clause_kind: "decreases"`, `symbol`, `facet: "contract"`, `attributed_to: "body"`, `clause`, `iteration` (loops), `actual: {bindings?, previous?, measure}` (5.3, S35) |
| operation `requires`/`ensures` false | `contract_violation` | as above with `symbol` = the operation; `attributed_to: "caller"` for `requires`, `"binding"` for `ensures` (S31) |
| a builtin's `requires` false (`xs.take(-1)`) | `contract_violation` | `clause_kind: "requires"`, `symbol` = the builtin's qualified name, `facet: "contract"`, `attributed_to: "caller"`, `caller`, `clause`, `input` (6.10) |
| a test or property observes an `ensures`/`invariant`/`decreases` violation in a called declaration | `contract_counterexample` (7.5) | the design's example shape: `symbol`, `facet: "body"`, `clause`, `input`, `actual`, `status: "failed"` |
| fault while evaluating a clause | the fault's own code | `facet: "contract"`, `clause`, `symbol` (S34) |
| verification status | `verification_result` | `symbol`, `facet: "verification_policy"`, `status: proved \| failed \| unknown`, `reason`, `receipt` (S59) |

Decision: [D-114] S57. every `contract_violation` carries `facet: "contract"`,
because the clause that failed belongs to the declaration's contract facet
(design line 291: "its contract includes requires and ensures clauses"),
and a separate field `attributed_to` names the party at fault: `"caller"`
for a `requires` clause (the caller must prove or check a precondition
before entering the body, design line 130), `"body"` for a function's
`ensures`, `invariant`, or `decreases` clause, and `"binding"` for a
capability operation's `ensures` clause (the host-bound implementation,
S31). `contract_counterexample` keeps the design's `facet: "body"`, which
already encodes the same attribution for the body it indicts. Rationale:
the facet vocabulary is the design's addressing scheme for declarations and
must not be overloaded to carry blame; a dedicated field keeps `facet`
meaningful to selectors and `attributed_to` meaningful to agents deciding
what to fix. Rejected alternative: `facet: "effects"` for operation
`ensures` violations (misuses a documented facet name).

From Milestone 2, S77 extends `attributed_to` to exactly one other
diagnostic: the `refinement_violation` of a capability operation's result,
which carries `attributed_to: "binding"` (9.2).

## 6. Dynamic semantics

### 6.1 Values

| Value | Representation (abstract) | Notes |
| - | - | - |
| `Int` | arbitrary-precision integer | no overflow |
| `Bool` | `true` / `false` | |
| `Text` | finite sequence of Unicode scalar values | no normalization; surrogate code points cannot occur |
| `Bytes` | finite sequence of bytes | |
| `Unit` | the single value `Unit` | |
| `List<T>` | finite sequence of values | persistent: every operation returns a new list and never observes mutation |
| struct | the struct's qualified name plus one value per field | |
| enum (incl. `Option`, `Result`) | the enum's qualified name, the variant name, and the payload values in order | |
| wrapper | the wrapper's qualified name plus the payload value | distinct from the bare payload |
| refinement | *erased*: a `Positive` value is an `Int` value | established statically (3.5) or by `check` |
| capability handle | opaque host object; never constructible in CPL source | none exist in Milestone 1; from Milestone 2 bound from the manifest's bindings or by a C host (6.9) |
| generator | opaque; exists only inside the property runner | S14 |

All values are immutable. A struct literal with spread copies the base's
fields and replaces the listed ones, producing a new value; the base is
unchanged. `push`, `take`, `drop`, `drop_last`, and `++` produce new lists.
A `var` is a rebindable *name*, not a mutable cell: `pages = pages.push(x)`
binds `pages` to a new list; any other binding that referred to the old
list still sees the old list. Because there are no closures and no
reference values, no mutable state is ever shared or escapes a function
(design: "mutable references cannot escape a function").

Implementations may share structure freely (values are immutable), may
intern small integers, and must not expose any identity notion: two values
are indistinguishable iff they are `==` (6.5).

### 6.2 Evaluation order

Evaluation is strict and left to right everywhere:

| Form | Order |
| - | - |
| binary operator `e1 op e2` | `e1`, then `e2`, then the operation; except `and`/`or`: `e1`, and `e2` only if needed (`false and _` is `false` without evaluating `_`; `true or _` is `true`) |
| call `f(a1, ..., an)` | `a1` ... `an`, then the call (5.2) |
| method call `e.m(a1, ..., an)` | `e`, then `a1` ... `an`, then the method |
| struct literal | the spread base first, then field initializers in source order |
| list literal, constructor application | elements / payloads in source order |
| `if c {A} else {B}` | `c`, then exactly one branch |
| `match s { arms }` | `s`, then the arms' patterns are tried in source order; the first matching arm's body is evaluated; no other arm body runs |
| `forall x in c: p` | `c`; then `p` for each element in order, stopping at the first `false` (result `false`); `true` for an empty list |
| `let`, `var`, assignment | the initializer, then the binding |
| `while` | per 5.3 |
| `e?` | `e`; `Ok(v)` continues with `v`; `Err(x)` exits the function with `Err(x)` as the exit value (5.2 step 5 applies) |
| `check<R>(e)` | `e`, then the predicates of `chain(R)` per 6.10 |
| builtin method or function call | receiver, then arguments, then the builtin's `requires` clauses, then the operation (6.10) |
| block | statements in order; the tail expression's value is the block's value, `Unit` if none |
| contract clauses | per Section 5 |

### 6.3 `Int` arithmetic

| Operation | Result |
| - | - |
| `a + b`, `a - b`, `a * b`, unary `-a` | exact mathematical result |
| `a / b` | `b == 0`: fault `division_by_zero`; otherwise the quotient truncated toward zero |
| `a % b` | `b == 0`: fault `division_by_zero`; otherwise `a - (a / b) * b`: the remainder has the sign of the dividend `a` (or is zero) and `\|a % b\| < \|b\|` |
| `<`, `<=`, `>`, `>=` | mathematical order |

Decision: [D-115] S37. division truncates toward zero and the remainder takes the
dividend's sign (`7 / -2 == -3`, `7 % -2 == 1`, `-7 / 2 == -3`, `-7 % 2 ==
-1`); the identity `a == (a / b) * b + a % b` always holds, and `a % b == 0`
holds exactly when `b` divides `a`, as the design's `divide_exactly`
example requires. Rationale: this is the convention of C, JavaScript
`BigInt`, Rust (`num-bigint`'s `/` and `%`), and most languages agents are
trained on, so `cpl-eval` can use the host operation directly; the
design's only arithmetic example is satisfied by every convention.
Rejected alternatives: floor division (Python) and Euclidean division
(Dafny, SMT-LIB), each of which Milestone 5 can map to with an explicit
encoding.

### 6.4 `Text` and `Bytes`

| Operation | Semantics |
| - | - |
| `t.len` | number of Unicode scalar values (not bytes, not grapheme clusters) |
| `t.is_empty()` | `t.len == 0` |
| `t1 ++ t2` | concatenation of scalar sequences |
| `t1 == t2` | equal iff the scalar sequences are identical; no Unicode normalization is applied |
| `TEXT` literal | the decoded sequence (grammar 1.5) |

`Bytes` behaves identically over bytes. Every other `Text` operation is a
`std.text` function (`spec/stdlib.md`).

### 6.5 Equality

`==` is structural on the erased value:

| Type | `a == b` iff |
| - | - |
| `Int`, `Bool`, `Unit` | same value |
| `Text`, `Bytes` | same length and pairwise equal elements |
| `List<T>` | same length and pairwise `==` elements |
| struct | same struct type (guaranteed statically) and all fields pairwise `==` in declaration order |
| enum, `Option`, `Result` | same variant and pairwise `==` payloads |
| wrapper | payloads `==` |

`!=` is the negation. Comparison terminates because values are finite trees.

### 6.6 Faults versus `Result`

A function communicates *recoverable* failure through its `Result` or
`Option` return type; `?` propagates a `Result`'s `Err`; `match` handles
both. A *fault* is an execution failure that no CPL construct can catch:
evaluation of the current `cpl run`, test, or C export call stops, the
fault becomes a structured diagnostic, and the process, test, or call
reports failure (6.8; a C export returns the diagnostic to its host,
`spec/cabi.md` Section 4). Faults never become values; `Result` values
never become faults. `check<R>(v)` returning `Err(ConstraintError)` is an
ordinary value, not a fault.

### 6.7 The complete fault list

| Fault code | Category | Raised when | Fields beyond the envelope (9.2) |
| - | - | - | - |
| `contract_violation` | contract | a `requires`, `ensures`, `invariant`, or `decreases` clause fails (Section 5), including a builtin's `requires` (6.10) | `clause_kind`, `clause`, `symbol`, `facet: "contract"`, `attributed_to` (S57), `input`, `actual`, `caller`, `iteration`, `status: "failed"` |
| `unresolved_hole` | hole | evaluation reaches a `HOLE` expression | `hole` (name), `symbol`, `expected_type`, `bindings` (names, types, and *values* in scope) |
| `division_by_zero` | runtime | `a / 0` or `a % 0` (6.3), including inside a contract (S34) or a compile-time constant (3.5, reported as `constant_evaluation_fault`) | `input: {"a": ..., "b": 0}` |
| `refinement_violation` | runtime | a value enters CPL at a boundary claiming a refinement type it does not satisfy: a `cpl run` argument (S39 (a)) and, from Milestone 2, an argument of a C export (S81) or the result of a capability operation whose binding is validated and whose return type is not a foreign-error return type (S77); never raised by CPL-to-CPL code, where refinements are established statically | `expected_type`, `actual`, `clause` (the predicate), `pointer`; `parameter` for an argument (S39, S81); `attributed_to: "binding"` and `binding` for an operation's result (S77) |
| `assertion_failed` | test | an `assert e` evaluates `e` to `false` (7.1) | `clause` (the expression text), `bindings` (visible local bindings with values), `symbol` |
| `internal_error` | internal | an impossible state: a `match` with no matching arm, a member missing on a value, a type confusion between the typed IR and a value, an operation on a handle that the host did not bind (6.9); from Milestone 2 also a capability binding that broke its contract with the runtime (S80) | `kind` (free-form identifier of the impossible state; S80 fixes the four binding kinds and their `detail` texts), `detail`; for a binding kind also `operation` and `binding`, and for `binding_result_ill_typed` `pointer` (S80) |
| `resource_exhaustion` | internal | the call depth exceeds the configured limit, memory allocation fails (raised by the native runtime, and by the evaluator only for a failure that its adapter host reports, S76), a host-imposed time budget expires, or an opt-in step budget is exhausted | `resource: "stack" \| "memory" \| "time" \| "steps"`, `limit` (for `time`, the budget in whole milliseconds, a fractional budget truncated; for `memory`, the heap budget in bytes, `0` when none was set, S76) |

A fault carries the `span` of the expression being evaluated (the native
memory fault of S76 carries none) and a `backtrace`: the list of active
declarations (qualified names with call spans), innermost first. Every
activation is a frame: CPL functions, test and property bodies, refinement
predicates, and builtin callees (a builtin whose precondition failed
included), each with the span of the call that entered it. A fault raised
before any activation (the `refinement_violation` of a `cpl run` argument
or, from Milestone 2, of a C export's argument, S81) has an empty
backtrace, which the JSON omits (9.2). `internal_error` has no `symbol`,
hence no `facet`, and no `clause` (S34). The backtrace is never shortened
(the one exception is the native memory fault of S76, whose backtrace may
be empty): a runaway recursion carries one frame per activation, up to the
call-depth limit of S38 or the depth at which the evaluation thread's stack
guard stops it (`spec/toolchain.md` 1.7; in the native runtime
`spec/cabi.md` Section 3), and a command that reports the fault twice
(`cpl run`'s `diagnostics` and `fault`, `spec/toolchain.md` 7.2) carries it
twice.

Decision: [D-116] S38. the call-depth limit is `[run] max_call_depth = 10000`
activations by default (configurable in `cpl.toml`; `spec/toolchain.md`
names the CLI override); the runtime, whether the reference evaluator or a
native backend (S51), must enforce it explicitly (an explicit depth
counter, an explicit evaluation stack, or an equivalent guard in generated
code) rather than rely on the host stack, so that exceeding it is always
the fault `resource_exhaustion` with `resource: "stack"` and never a
process crash. Loops have no step limit by default; `spec/toolchain.md` may
define an opt-in step budget for `cpl run` and `cpl test`, whose exhaustion
is `resource_exhaustion` with `resource: "steps"`, and a time budget
applies only where it defines one (tests) or, from Milestone 2, where a C
host requests one for a call (`spec/cabi.md` Section 4). Rationale: the
design classifies resource exhaustion as a fault, and a fault must be
reportable as structured data, which a host stack overflow is not. Rejected
alternative: unbounded recursion with a crash.

Decision: [D-250] S72. the activations S38 counts are calls of CPL
functions: the limit is exceeded when a function call would make the number
of active function calls greater than `max_call_depth`. Test and property
bodies, refinement predicates, and builtin callees are frames of the
backtrace (6.7) but are not counted, so with `max_call_depth = 1` a
function whose body calls `[1].take(1)` runs, and with `max_call_depth = 0`
no function call runs at all while test and property bodies, predicates,
and builtins still do; a refused top-level call (`cpl run`'s, or from
Milestone 2 a C export's, S81) has no enclosing declaration, so its
`resource_exhaustion` names none (no `symbol`). Values are not bounded by
it: a loop can build a value nested arbitrarily deep, and the runtime must
drop, compare, and render values of any depth without exhausting the host
stack (the reference evaluator does all four iteratively,
`spec/toolchain.md` 1.7). Rationale: the limit then means the same for `cpl
run f 9999` and for a test that calls `f(9999)`, and neither a builtin nor
a predicate recurses on its own. Rejected alternative: counting every
backtrace frame (a test calling `f` at the limit would fault while `cpl
run` of the same call succeeds).

Decision: [D-278] S76. Memory exhaustion in the native runtime is a fault.
The native runtime checks every allocation it makes during an evaluation,
those for the arguments and the result of a capability operation's callback
included; when one fails (the allocator returns nothing, or a heap budget
that the host set would be exceeded), or a C callback reports that its own
allocation failed (`spec/cabi.md` Sections 4-5), the evaluation ends in the
fault `resource_exhaustion` with `resource: "memory"` and `limit` the heap
budget in bytes (`0` when none was set), blamed, as the other exhaustions
are, on the innermost active declaration (on none at the top level, S72),
with no `span`. The fault is built without allocating, so its `backtrace`
may be empty, the one exception to 6.7's "never shortened"; every value the
evaluation had partly built is released. Outside an evaluation (the C API's
own value functions) allocation failure is a status of the C API, not a
fault (`spec/cabi.md` Section 4). The reference evaluator cannot observe
its own allocation failure and aborts its process instead, and S75
sanctions that divergence; the one memory fault it raises is one that its
adapter host reports, for an allocation failure in a C adapter or in the
binding layer of the driver that runs it (`spec/cabi.md` Section 7), which
it raises as this code at the operation call, spanning the call as the
faults of S80 do. Rationale: the design makes resource exhaustion an
execution fault ("Contract violations and resource exhaustion are execution
faults rather than ordinary success values", line 96) and keeps allocation
failure outside the functional-correctness claim (line 172), not outside
the fault model; a native library that aborted would end the host process
with it, and a heap budget makes the same path deterministic for tests that
use a runtime from one thread at a time (concurrent calls share the budget,
so which of their allocations fails depends on how they interleave).
Rejected alternatives: aborting in the native runtime as safe Rust does (a
library would end its host on an allocation failure the host could have
survived); a C API status in place of the fault (the evaluation would end
without the structured diagnostic that every other exhaustion of S38 has).

Decision: [D-282] S80. A capability binding that breaks its contract with
the runtime is the fault `internal_error` with one of four fixed kinds and,
beyond `kind` and `detail`, the fields `operation` (the operation's
qualified name, 1.4: `reader.images.ImageCodec.dimensions`) and `binding`
(the binding name of 6.9), plus, for `binding_result_ill_typed`, `pointer`
(the RFC 6901 position in the result of the first part that is not of its
declared type). Its `detail` is the text of its row, and its `message` is
therefore `internal error (<kind>): <detail>`, as for every
`internal_error`:

| `kind` | Raised when | `detail` |
| - | - | - |
| `binding_result_ill_typed` | a result that S77 validates is not a value of the operation's declared return type, and that type is not a foreign-error return type (6.9; with one, the result becomes an `Err(ForeignError)` value, S77) | `the result of <operation> is not a value of <expected_type> at <pointer>` |
| `binding_error_unrepresentable` | the binding reports a foreign failure (`spec/stdlib.md` S45: one that its adapter or host callback reports, or one that its binding layer produces, such as an `Int` argument outside its C type) for an operation whose return type is not a foreign-error return type | `<binding> reported <Kind> for <operation>: <message>` |
| `binding_protocol` | a binding breaks its interface with the runtime: a C implementation of the operation (a C host's callback, or an adapter's operation function) returns a status that the callback contract does not allow, `CPL_OK` without a valid owned result, or `CPL_FOREIGN_ERROR` without having reported a failure (`spec/cabi.md` Section 5); or a scripted host, a host of the toolchain's tests that answers from a list of responses (the native engine's, or the evaluator's over the same list), has no response for the call (`spec/cabi.md` Section 3) | in that order: `<binding> returned status <code> from <operation>`; `<binding> returned CPL_OK from <operation> without a valid result`; `<binding> returned CPL_FOREIGN_ERROR from <operation> without reporting a failure`; `<binding> has no scripted response for <operation>(<arguments>)` |
| `binding_host_failed` | the process that hosts the evaluator's C adapters (`spec/cabi.md` Section 7) ends during the call of an operation whose return type is not a foreign-error return type (with one, the result is `Err(ForeignError { kind: Transport })`, `spec/stdlib.md` S45) | `the adapter host for <binding> exited with <status>` |

In the templates `<operation>` and `<binding>` are the values of
`operation` and `binding`; the first template is the shape form of the
`InvalidResult` message of `spec/stdlib.md` S45 and the last its
`Transport` message, and they take S45's `<expected_type>` (the declared
type at the failing position), `<pointer>` (the value of `pointer`, with
` at <pointer>` left out for the whole result), and `<status>`; `<Kind>` is
the name of the reported `ForeignErrorKind` variant and `<message>` the
reported message as value text (S40); `<code>` is the returned status in
decimal; and `<arguments>` is the value texts (S40) of the call's
arguments, separated by `, `. Both engines render every template from the
one source of `spec/stdlib.md` S47. Every fault of the table spans the
operation call and carries the backtrace of the declarations active there
(6.7); the operation is not an activation and adds no frame. Like every
`internal_error` it blames no declaration (S34): it carries no `symbol`,
`facet`, or `clause`, whatever was being evaluated, and no clause claims
it; the two fields name the operation and the binding instead. Rationale:
each is a state that no CPL code can cause or handle, because the
operation's declared type promises a value and a binding that breaks the
promise has failed the trusted boundary (design line 213), so the fault
reports the running system rather than the program; fixed kinds with
structured fields let an agent find the binding to fix without parsing
`detail`, and fixed `detail` texts make the `message` byte-identical
between the engines (S75), which no Milestone 1 text fixes for kinds the
Milestone 1 evaluator never raised. Rejected alternatives: a new fault code
for binding failures (a code that only a broken binding could raise, for a
condition `internal_error` already covers); `refinement_violation` for a
result of the wrong shape (no predicate failed, so there is no `clause`);
blaming the calling declaration through `symbol` (it did nothing wrong);
`detail` texts left to each engine (S75 compares `message`, which contains
`detail`).

### 6.8 Faults in `cpl run` and `cpl test`

| Command | On a fault |
| - | - |
| `cpl run <function> ...` | evaluation stops; the fault's structured diagnostic (9.2) is reported and the exit status is nonzero (`spec/toolchain.md` 7.4 fixes the exit codes; a fault is told from a compile error by the diagnostics and from a usage error by the exit code) |
| `cpl test [selector]` | the test or property whose evaluation raised the fault *fails*; its `test_result` carries the fault; other tests still run; the exit status is nonzero if any test failed |

Decision: [D-117] S39. `cpl run` evaluates exactly one function of the checked
program, named by its qualified name or `@id` (or another target spelling
of `spec/toolchain.md` T59), public or private, on
argument values supplied from the command line; this document fixes what
the run means and `spec/toolchain.md` fixes the command's argument syntax,
the encoding of the printed result, and the exit codes. Semantics: (a) each
argument is decoded against the corresponding positional parameter type; a
refinement-typed parameter is checked at runtime, and an argument that
fails its predicate is the fault `refinement_violation` (the argument did
not pass through the compile-time rule); (b) a generic function cannot be
run (`cannot_run_generic`, resolve: a type parameter has no argument to
instantiate it from, and the design's monomorphic export wrappers such as
`paginate_ints` exist for this), nor can a function with `using`
parameters (`cannot_run_effectful`, resolve: no handle exists, 6.9;
amended from Milestone 2 by S78, which binds them from the manifest);
(c) the call executes in checked mode exactly as 5.2 describes, with no
calling declaration, so a `requires` violation at the top level omits
`caller`; (d) the result value, including an `Err(...)`, is a successful
run; a fault is reported as its diagnostic. Rationale: 0.1 has no I/O
capability, so a `main` could not print; evaluating a named function on
supplied arguments is the only observable execution the design's "examples
execute" can mean in Milestone 1. Rejected alternatives: a conventional
`main`; instantiating type parameters from the arguments (would make the
run's meaning depend on an inference the source never wrote).

The function may be any function of the checked program, a standard-library
function or member stub included (its receiver is then the first
argument). A generator of `std.gen`, whose result is a `Gen<T>` with no
value outside a property's `for` clause (S14), is not a function in this
sense: `cpl run` refuses it as it refuses a declaration that is no
function (`spec/toolchain.md` T61). The positional arguments are the constant expressions of
`spec/toolchain.md` 7.2: the grammar of 3.5 without its binary operators
(3.5's grammar serves the proof rule, 7.2 owns the argument syntax). An
argument's `refinement_violation` (a) carries `expected_type` (the
declared type at the failing position), `actual` (the value at that
position), `clause` (the first predicate of its chain, root first, that is
false), `parameter`, and `pointer` (the RFC 6901 position inside the
argument), and spans the parameter; a predicate that faults is reported per
S34. These checks run in every contract mode: they are boundary checks,
not contracts.

Decision: [D-252] S74. the refined positions of one argument (S39 (a)) are
checked in an order fixed by the value's structure: sibling positions in
value order (list elements by index, struct fields in declaration order,
payloads by position), and every refined position inside a refinement's
base value before that refinement's own chain, which is checked root first
(6.10); the first predicate that is false (or faults, S34) is the one
reported. Rationale: a contract expression is "pure and total under its
preceding assumptions" (design), and a value of the base type is only
well-formed once its own positions hold: for `type Wide = Rect where
self.w / self.h > 1` over a struct `Rect` whose fields `w` and `h` are
`Positive`, an argument whose `h` is 0 is reported at `/h` as a violation
of `Positive` (`clause` `self > 0`), not as a `division_by_zero` in
`Wide`'s predicate, which no `Rect` built by CPL code can make fault; this
is 6.10's root-first order applied to a value's structure. Rejected
alternative: the outer refinement first (an earlier behaviour of the
evaluator, under which a predicate ran on a value its own field types
exclude).

A refinement whose base is itself a refinement is one position, not two:
the positions inside its value are those of its root base `strip(R)` (3.3),
and its whole chain is then checked at its own position, root first, so
that `expected_type` names `R` whichever of its predicates is false. For
`type Small = Positive where self < 10`, an argument `0` of type `Small` is
reported at its position with `expected_type` `Small` and `clause`
`self > 0`, never as a separate, earlier position of `Positive`. This
paragraph, added in Milestone 2, changes no outcome: it states the reading
of S74 that the Milestone 1 evaluator implements
(`cpl_eval::json::refined_positions`), so that the native boundary checks
of S77 and S81 make the same one.

Decision: [D-280] S78. From Milestone 2 `cpl run` binds `using` parameters;
this amends S39 (b). A function with `using` parameters runs when the
capability of every `using` parameter is the `capability` of exactly one
`[bindings.*]` table whose backend this toolchain can bind
(`spec/toolchain.md` 11.1; in Milestone 2 `backend = "c"`, with `"c"` in
`[interop] targets`, `spec/cabi.md` Section 6); it remains
`cannot_run_effectful` (resolve) when some `using` parameter's capability
has no such binding, with a note that names the capability, or the binding
and the milestone in which its backend arrives (`spec/toolchain.md` 7.2).
The refusals keep their order (`spec/toolchain.md` 1.7:
`cannot_run_effectful` after `cannot_run_generic`, before the argument
count and the decoding of the arguments). Then the handles are bound, one
per `using` parameter, in parameter order and numbered from 0 in that order
(the `$handle` of value JSON, `spec/toolchain.md` 7.3); then the refined
positions of the arguments are checked (S39 (a), S74); then the call runs
with no calling declaration (S39 (c)). Binding a handle grants its binding,
in either engine: the adapter initializes then, whether or not the call
would reach one of its operations. A binding that cannot be granted (its
adapter fails to initialize) is the error `interop_bind_failed`
(`spec/cabi.md` Section 4; exit 1, `spec/toolchain.md` 7.4), and nothing
runs: no argument's refinement is checked and the function is not called.
Every handle bound this way is validated (S77). The command line is the
host whose policy grants the binding (design line 559): in edition 0.1 the
grant gives an `in_process` adapter the privileges of the driver process
that runs it, a child of the `cpl` process with that process's privileges
(`spec/cabi.md` 3.8, 6.7, 7.2), and nothing narrower; a `cpl run` that a
coordinator task starts is confined by the task's execution boundary
(`spec/toolchain.md` 10.8), as every process the task starts is. Tests and
properties stay pure (S30): `cpl test` binds nothing in edition 0.1, and
the receipt's `fixtures` stays `[]` (S59, `spec/stdlib.md` S42). Rationale:
without it no CPL program that the toolchain runs could call C, so "CPL
calls a C library" (design line 645) would rest on C hosts alone and could
never be compared with the evaluator; the manifest already requests the
binding (line 559: "The manifest requests the binding; host policy grants
its actual privileges"), and a `cpl run` that binds it is the host granting
it. Rejected alternatives: keeping `cannot_run_effectful` for every `using`
function in Milestone 2 (no CPL-side evidence that could be compared with
the oracle); binding by capability type without a manifest entry (authority
that the manifest never requested).

Decision: [D-283] S81. A value that enters CPL as an argument of a C export
(`spec/cabi.md` Section 5) is checked as a `cpl run` argument is, before
the first activation: a value that is not of its parameter's type is a
usage error of the C API (`interop_argument_type_mismatch` and the other
argument codes of `spec/cabi.md` Section 4), as an ill-typed `cpl run`
argument is `usage_bad_argument`, and nothing runs; a refined position
whose predicate is false, in the order of S74, is the fault
`refinement_violation` with exactly the fields of an argument's (S39 (a):
`symbol` the function, `facet: "body"`, `span` the parameter,
`expected_type`, `actual`, `clause`, `parameter`, `pointer`, no
`attributed_to`, an empty backtrace), and a predicate that faults is
reported per S34; then the function runs as a top-level evaluation with no
calling declaration (S39 (c)). Rationale: the design's export wrapper
"checks foreign inputs even if an internal CPL caller could establish the
refinement statically" (line 594), and a C host is the same kind of
boundary as the command line, so one rule gives it the diagnostic that `cpl
run` gives, byte for byte (S75); a refinement failure is a fault under `cpl
run` (6.7), so it is one here. Rejected alternatives: a usage status for
the refinement failure (the same input would be a fault from the command
line and a usage error from C); trusting refinements that a CPL caller
would have had to prove (a C host proves nothing).

Decision: [D-340] S82. From Milestone 3 `cpl run` (S78) and `cpl test`'s
fixtures (S83) also grant bindings whose backend is `python`, with
`"python"` in `[interop] targets`: such a binding's adapter runs in a
worker (`spec/python.md` Section 6), whatever its `isolation`, because the
toolchain hosts no foreign code in process; the grant is made where S78
binds, a failed grant is `interop_bind_failed`, and every handle is
validated (S77). A worker is an adapter host in S80's sense: when it exits
during an operation call whose return type is not a foreign-error return
type, the fault is `binding_host_failed` with `the adapter host for
<binding> exited with <status>`, and with one, the call's result is the
`Transport` failure of `spec/stdlib.md` S45 and S48. A worker that does not
answer within its binding's `timeout_ms`, or whose call's cancellation is
requested, is a foreign failure that the binding layer reports (`Timeout`,
`Cancelled`; with no foreign-error return type, `binding_error_unrepresentable`).
S80's `binding_protocol` row gains one situation, with its `detail`
template: a worker answer that is not cpl-worker protocol 1 (`<binding>
sent an answer that is not cpl-worker protocol 1 for <operation>:
<reason>`, `<reason>` one of `not JSON`, `not a response to cap_call`, and
`a value that is not a node list`). Rationale: the design's managed Python worker
(line 436) is the only way for a process without Python to call it, and
treating a worker as S80 treats the evaluator's C adapter host gives one
failure model to every out-of-process adapter, with the kinds that stdlib
S27 already reserves (`Transport`, `Timeout`, `Cancelled`); a garbled
worker answer is the worker breaking its interface, which is what
`binding_protocol` means. Rejected alternatives: new fault kinds for
worker failures (the existing kinds already distinguish a host that ended
from one that broke protocol); restarting a failed worker (a hidden retry,
design line 489).

Decision: [D-118] S40. values are rendered in two canonical forms. *Value text*
(for `message` strings, diagnostics rendered as text, and wherever this
document shows a value inline) is CPL literal syntax: `Int` in decimal,
`Text` as a `TEXT` literal in which `"` and `\` are escaped as `\"` and
`\\`, U+000A, U+000D, and U+0009 as `\n`, `\r`, and `\t`, every other
scalar value in U+0000-U+001F and U+007F as `\u{H}` with the minimal number
of lowercase hex digits (`\u{1}`, `\u{7f}`), and every other scalar value
literally; `Bool` and `Unit` as their constructors, lists as `[a, b]`,
structs as `Page { id: PageId("p"), text: "t" }`, variants as
`Missing(PageId("p"))` / `Unavailable`, wrappers as `PageId("p")`, `Bytes`
as `Bytes("<lowercase hex>")`. *Value JSON* (for the `input`, `actual`,
`expected`, `bindings`, `failing_case`, and `shrunk` fields of
diagnostics) is: `Int` → JSON number (arbitrary precision: consumers must
parse with a precision-preserving reader), `Bool` → boolean, `Text` →
string, `Unit` → `{"$type": "Unit"}`, `Bytes` → `{"$type": "Bytes", "hex":
"..."}`, list → array, struct → `{"$type": "<qualified name>", "<field>":
..., ...}`, enum and `Option`/`Result` → `{"$type": "<qualified name>",
"$variant": "<Variant>", "$payload": [...]}` (`$payload` omitted for
payload-less variants), wrapper → `{"$type": "<qualified name>",
"$payload": [v]}`; the `$type` of a prelude type is its bare prelude name
without type arguments (`"Option"`, `"Result"`, `"Unit"`, `"Bytes"`,
`"ConstraintError"`), that of a user type its qualified name. Rationale:
the design's example renders `items` and `size` as plain JSON arrays and
numbers, which fixes the list and integer encodings; `$`-prefixed keys
cannot collide with field names (fields are `LOWER_IDENT`); explicit tags
keep nested options distinguishable as the design's value-mapping table
demands; fixing the escape set makes two implementations byte-identical.
Rejected alternatives: encoding `Int` as a decimal string (contradicts the
example) and untagged enums.

### 6.9 Capabilities at runtime

No capability handle can be created in Milestone 1: there are no standard
capabilities (Section 10) and no adapters. Functions with `using`
parameters are fully checked (Sections 3-5) but cannot be invoked by `cpl
run` (S39) or reached from tests (S30). The runtime implements operation
calls over an abstract handle interface, so that later milestones supply
handles without changing the evaluation rules; an operation call on a
handle that the host did not bind is `internal_error` (`kind:
"unbound_capability"`).

From Milestone 2 a handle comes from one of three sources, never from a CPL
expression (S27). Each handle has a *binding name*, which the faults of S77
and S80 carry as `binding` and the `ForeignError` details of
`spec/stdlib.md` S45 as `adapter`:

| Source | Bound by | Binding name |
| - | - | - |
| a `[bindings.<name>]` table of the manifest with `backend = "c"` (`spec/toolchain.md` 11.1, `spec/cabi.md` Section 6) | `cpl run` (S78), in either engine, and a C host that calls the package's grant for it (`spec/cabi.md` Section 5) | `<name>`, in either engine |
| a capability that a C host implements with its own callbacks (`spec/cabi.md` Section 5) | the C host | `"host"` |
| a host of the toolchain's own tests: one bound through the reference evaluator's host API (`spec/toolchain.md` 1.7), or a scripted host of the native engine (`spec/cabi.md` Section 3) | the test | the label the test gave it |

In the evaluator a handle's binding name is the label it is bound with
(`Handle::label`; `BindOptions::label`, `spec/toolchain.md` 1.7.4), the one
name the interpreter keeps for a handle, so the evaluator binds the handle
of a `[bindings.<name>]` table with the label `<name>`, and both engines
report that name.

The standard library still declares no capability (`spec/stdlib.md` S42),
and tests and properties still obtain no handle (S30, S78).

The Milestone 1 host API stays trusted: a host bound through
`Interpreter::bind_capability` (`spec/toolchain.md` 1.7, used by the
evaluator's own tests) is not validated. The operation's `requires` and
`ensures` clauses run (S31), but the value the host returns is not checked
against the operation's declared return type, so a host answering `0` for
`fn size() -> Positive` is not caught at the boundary, and one answering a
`Text` for an `Int` meets `internal_error` (`kind: "type_confusion"`)
wherever the value is first used, as in Milestone 1. Every host that a
Milestone 2 path binds is validated instead (S77).

Decision: [D-279] S77. Every value returned by a capability implementation
that a Milestone 2 path binds is validated, in both engines, after the host
returns and before the operation's `ensures` clauses run (S31). The
Milestone 2 paths are a backend-`c` binding (natively in the package, and
in the evaluator through its adapter host, `spec/cabi.md` Section 7), a
capability that a C host creates through the C API, a scripted host of the
native engine, and every evaluator host bound with validation requested
(`Interpreter::bind_capability_with` with `BindOptions { validate_results:
true }`, `spec/toolchain.md` 1.7). Validation is the boundary check of an
argument (S39 (a)) applied to a result, and it runs in every contract mode:

1. *Shape*: the value is a value of the operation's declared return type,
   walked in value order (struct fields in declaration order, list elements
   by index, payloads by position).
2. *Refinements*: every refined position of the value, in the order of S74
   (siblings in value order, the positions inside a refinement's base
   before the refinement, each chain root first), evaluated by the engine
   itself; a predicate that faults is reported per S34, as for an argument.
   For `Dimensions { width: Positive, height: Positive }` inside `Ok` the
   positions are `/$payload/0/width`, then `/$payload/0/height`.
3. *A failure*, at the first position that fails, is decided by the return
   type. A *foreign-error return type* is `Result<T,
   std.foreign.ForeignError>` for some `T`, the design's adapter pattern
   (line 526):
   - with a foreign-error return type, the value is replaced by
     `Err(ForeignError { kind: InvalidResult, message, details })`, whose
     message and details are `spec/stdlib.md` S45's, and the operation's
     `ensures` clauses then see that value;
   - otherwise a failed refinement is the fault `refinement_violation` with
     `symbol` the operation's qualified name (1.3,
     `reader.images.ImageCodec.dimensions`), `facet: "body"` (the body of a
     bodiless operation is its bound implementation; `spec/toolchain.md`
     1.7.3's rule for a fault with a `symbol`), `span` the operation call,
     `expected_type` (the refinement at the failing position, rendered as
     for an argument), `actual` (the value there), `clause` (its first
     false predicate, root first), `pointer` (the RFC 6901 position in the
     result), `attributed_to: "binding"`, and `binding` (the binding name,
     above);
   - otherwise a value of the wrong shape is the fault `internal_error`
     with `kind: "binding_result_ill_typed"` and the fields of S80.

`attributed_to` thereby extends beyond `contract_violation` to exactly this
one fault, with the one value `binding`: the field names the party at fault
(S57), which for a value entering CPL is its producer, while an argument's
`refinement_violation` keeps the shape of S39 and S81, without
`attributed_to`. A foreign failure that the binding reports instead of a
result is not validated: it becomes a `ForeignError` value by
`spec/stdlib.md` S45, or the fault of S80 when the return type is not a
foreign-error return type. Rationale: the design requires foreign results
to be validated before they become values ("validate dynamic inputs and
outputs", line 479; "The adapter validates returned dimensions before
constructing Positive values", line 540), and doing it in the runtime makes
that hold when an adapter forgets; `InvalidResult` is the kind
`spec/stdlib.md` 7 defines for a foreign return value that "failed
validation against the declared CPL type", so where the declared type can
carry it the program receives an ordinary error value it can match, and
where it cannot the failure must be a fault, blamed on the party that
produced the value. The Milestone 1 host API stays trusted, so that the
accepted Milestone 1 tests that bind ill-typed hosts keep their outcome
(`spec/toolchain.md` T77). Rejected alternatives: trusting every binding as
Milestone 1 trusts its test hosts (the design requires validation);
validating the Milestone 1 host API too (it changes the accepted outcome of
those tests); checking refinements only (a value of the wrong shape would
reach CPL code as a type confusion far from its source); `facet:
"signature"` on the result's `refinement_violation` (the facet would carry
blame, which S57 forbids).

Decision: [D-281] S79. An evaluation that a host starts while one of its
operations is executing (a C callback that calls an export of a package,
its own included, `spec/cabi.md` Sections 4-5) is a fresh top-level
evaluation: it has no calling declaration (a top-level `requires` violation
carries no `caller`, S39 (c)), its own call-depth count (S38, S72), its own
clause attribution (S34, S70), and its own time budget and cancellation
from its own call options; its outcome, a value or a fault with its own
diagnostic, is returned to the host and is never merged into the outer
evaluation, which sees only what the operation returns (S77). The two
evaluations share only the host stack, so the guard of S38 ends an
unbounded export, callback, export recursion with `resource_exhaustion`
(`resource: "stack"`), never with a crash. A host of the reference
evaluator has no access to the interpreter that called it (`HostContext`
carries only the program, `spec/toolchain.md` 1.7), so an evaluation it
starts runs on an interpreter of its own and is fresh by construction.
Rationale: the design leaves re-entrancy to be defined ("Define ownership,
event-loop affinity, reentrancy, and cancellation without implying
rollback", line 663); a fresh evaluation is what a host that calls a
library function expects, it needs no state shared between the two
evaluations, and the C API's per-call evaluation contexts give it by
construction (`spec/cabi.md` Section 4); continuing the outer evaluation
would let a host observe or disturb CPL state in the middle of an
operation. Rejected alternatives: refusing re-entry with a busy status (an
adapter could not use the package it serves, which the Python and Node
adapters of Milestones 3-4 need); a nested evaluation that inherits the
outer depth, deadline, attribution, and caller (its outcome would depend on
where the operation happened to be called from).

### 6.10 Prelude functions and methods at runtime

Decision: [D-119] S58. every `@builtin` declaration of the standard library
(`spec/stdlib.md` 10.6: the members of the prelude types, `flatten`, and
the `std.text`, `std.bytes`, and `std.gen` functions) and the intrinsic
`check` are *builtins*: the runtime implements them
directly, checks each builtin's `requires` clauses itself before executing
it, and trusts its `ensures` clauses, which are never evaluated at runtime
(`spec/stdlib.md` S39; they are the specification the Milestone 5 verifier
assumes). A violated builtin `requires` is the fault `contract_violation`
with `clause_kind: "requires"`, `symbol` = the builtin's qualified name
(`std.prelude.List.take`, `spec/stdlib.md` S37), `facet: "contract"`,
`attributed_to: "caller"`, `clause` = the clause text (`n >= 0`), `input`
= the receiver under the key `self` and the arguments by parameter name,
`span` = the stub's clause, and `caller` = the innermost active declaration
(the refinement declaration for a builtin called inside its `where`
predicate). A builtin checks its preconditions in every contract mode: they
are part of its definition (`take` has no result for a negative count).
The members' semantics:

| Operation | Result (`xs` has `len` = `k`) |
| - | - |
| `xs.len` | `k` |
| `xs.is_empty()` | `k == 0` |
| `xs.take(n)` | `requires n >= 0`; the first `min(n, k)` elements |
| `xs.drop(n)` | `requires n >= 0`; the elements after the first `min(n, k)` |
| `xs.drop_last()` | `xs` without its last element; `[]` for `[]` |
| `xs.push(v)` | a new list of `k + 1` elements ending in `v`; `xs` is unchanged |
| `xs ++ ys` | the elements of `xs` followed by those of `ys` |
| `flatten(xss)` | the concatenation of the inner lists in order; `[]` for `[]` |
| `t.len`, `t.is_empty()`, `t1 ++ t2` on `Text`/`Bytes` | per 6.4 |

`check<R>(v)` evaluates the `where` predicates of `chain(R)` from
`strip(R)` upward with `self` bound to `v`, in the compile-time evaluable
subset or not (at runtime every pure expression is admissible). The first
predicate that evaluates to `false` yields `Err(ConstraintError {
type_name: "<qualified name of that refinement's declaration, as named
through R at the call site (S55)>", message: "<value text of v> does not
satisfy <predicate text>" })` and no later predicate runs; when every
predicate is `true` the result is `Ok(v)`, the same value now carrying the
static type `R`. A fault inside a predicate (`check<Nz>(0)` for `type Nz =
Int where 10 / self > 0`) is reported per S34 with `symbol` = the
refinement declaration, `facet: "contract"`, and `clause` = the predicate
text. `spec/stdlib.md` 1.4-1.5 fixes the struct and the `message` wording.
Rationale: the design gives `take` and `drop` a precondition ("accept a
nonnegative integer") and `check` a `Result`, and a builtin's own code is
the trust root, so its postconditions cannot be checked against anything
more trusted; naming the receiver `self` in `input` reuses the refinement
predicate's convention. Rejected alternatives: clamping negative `take`/
`drop` arguments (hides a caller bug the design's own contract names);
evaluating builtin `ensures` in checked mode (quadratic `take`).

## 7. Tests and properties

### 7.1 `assert`

`assert e` evaluates `e` (type `Bool`); `true` continues; `false` raises
the fault `assertion_failed`. It is a statement in any body (functions
included); a failing `assert` inside a function called by a test fails the
test like any fault.

Decision: [D-120] S41. `assert` takes exactly one expression and no message (D27).
The fault carries `clause` (the source text of the expression), `span`
(the asserted expression's: for a parenthesized one, the expression inside
the parentheses, which are no node of the typed IR, while `clause` keeps
them as written), `symbol` (the enclosing declaration, unless a contract
expression called the function the `assert` is in, S70), and `bindings`: every local binding
and parameter visible at the `assert`, with its type and value JSON,
innermost first as in the hole report (8.3). Rationale: a message adds
nothing an agent cannot derive from
the expression text and the bindings, and the design's diagnostics are
structured data, not prose. Rejected alternative: `assert e, "message"`.

### 7.2 Test identity and selection

Decision: [D-121] S42. a test or property is identified by its `@id` if present and
otherwise by the qualified name `<module path>."<name>"` (1.4); the `TEXT`
name must be non-empty (`empty_test_name`) and unique within the module's
test namespace (`duplicate_test_name`). A selector `m.*` (grammar 4.4)
selects every test and property of module `m` and of every module whose
path extends `m` (`spec/toolchain.md` T43); a selector cannot name a
single test (D69: `selector_seg` admits no `TEXT` segment), so a single
test is reached only through the command-line filters `spec/toolchain.md`
defines, which match the identity or the name text; a selector that
matches no test is the configuration error `empty_test_selection` (design:
"an empty selection is a configuration failure"). Rationale: tests "have stable
identities and live in namespaces like ordinary declarations", and the name
string is the only name they have. Rejected alternative: numbering tests
by position.

### 7.3 Running tests

`cpl test` compiles the whole program (any error aborts the run with the
compile diagnostics and no test results) and then executes every selected
test and property, in the order of their qualified names (the byte order of
the qualified name as rendered, `spec/toolchain.md` 1.7.2), each in a fresh
evaluation (no state is shared because no state exists), in checked mode.
A test is *tested* (status `tested`, S50) when its body evaluates to
completion; it *fails* when a fault is raised. A failing test never prevents
another test from running.

Decision: [D-341] S83. From Milestone 3 a test or property may declare
`using` parameters and an `effects` clause (`spec/grammar.md` D74); its
body is then effect-checked by S26-S28 exactly as a function's, its bound
being its `effects` clause (empty when absent). `cpl test` binds each such
parameter to the *fixture* of its capability, the binding that
`[test.fixtures]` names for the capability's qualified path
(`spec/toolchain.md` 11.1, `spec/python.md` Section 8); a parameter whose
capability has no fixture is the error `test_fixture_missing` (category
`test`, at the parameter, `capability`), reported by `cpl check` as well,
since the manifest is part of the program's inputs. Fixtures are granted
once per execution of a test and once per property, for all of its cases
and its shrinking, before the body first runs, in parameter order, and
released after the test or property ends; every handle is validated (S77)
as a `cpl run` handle is (S78). A grant that fails ends the test or
property as *failed*, with the error `test_fixture_failed` (`binding`,
`capability`, and the grant's own diagnostic as a note) in place of a fault,
and the next test still runs. The receipt of the run records each fixture
granted, an assumption of kind `foreign_adapter` per fixture, and the
adapter runtimes' versions (S59; `spec/toolchain.md` 8.2): a test that
calls an adapter is evidence about the adapter's behavior, which the
receipt names, not a proof about CPL alone. This amends S30, which keeps
tests and properties without `using` parameters pure. Rationale: "service-
dependent tests must declare fixtures or environment assumptions" (design
line 600), and test receipts carry "seeds, fixtures, and environment
inputs" (line 261); a fixture is a binding the manifest requests and the
test runner grants, as `cpl run` grants a function's (S78), so no new
authority enters. A grant per test isolates tests from each other's adapter
state; a property's cases share one grant as they share one seed. Rejected
alternatives: one grant for the whole run (state would leak between
tests); a grant per case (a worker start per case); recording adapters
that capture and replay calls (deferred; design line 213 describes one as
debugging aid, not evidence).

### 7.4 Properties

A `property` has `for x_i in g_i` clauses and a body. Each `g_i` is a
contract expression (5.1) of type `Gen<T_i>` (S14) that is evaluated once
when the property starts, in a scope with no binders; the body is evaluated
once per generated case with `x_i : T_i` bound.

Decision: [D-122] S43. property `for` ranges are independent: a range expression
may not mention an earlier binder (scope 7 of 2.1 is visible only in the
body; a binder used in a later range is `unresolved_name`). Rationale:
independent generators shrink coordinate-wise (S45) with no need to
regenerate dependents, and the design's example is independent. Rejected
alternative: dependent generation (`for size in gen.int_range(1,
items.len)`), deferred.

Decision: [D-123] S44. the runner's parameters are: `cases` = `[test] cases = 100`
by default (CLI `--cases`, `spec/toolchain.md`); a 64-bit unsigned *run
seed* chosen randomly per `cpl test` invocation unless `--seed <n>` is
given, and always recorded in the report and the receipt and in every
property's result (a plain test's result has none, S46); a *property seed*, the 32-byte
digest `blake3(run_seed_le_bytes || identity_utf8)` where `identity` is
the property's identity (S42), recorded as 64 lowercase hex digits, so
that adding, renaming (under an `@id`), or reordering tests never changes
another property's cases; a ChaCha20 (`rand_chacha`) stream per property
seeded with the property seed as its 32-byte seed, consumed by the
generators in `for`-clause order for each case; and a
*size* parameter for case `i` (0-based) of `size = min(i, 100)`, which
`gen` combinators use as their magnitude bound (`spec/stdlib.md` 8.3
defines every generator's draw at a size; `gen.list(g)` draws a length from
`[0, size]`, `gen.int` mixes `0`, `±1`, uniform values in `[-size, size]`,
and occasional large values).
Rationale: the design requires recorded, reproducible seeds; per-property
derivation keeps replays stable under unrelated edits; a size ramp finds
small counterexamples first. Rejected alternatives: one shared stream for
all properties; a fixed default seed (hides flaky generators).

### 7.5 Shrinking and failure reports

Decision: [D-124] S45. when case `i` fails (any fault, including `assertion_failed`
and `contract_violation`), the runner shrinks the binder tuple
coordinate-wise: for binder 1 it asks the generator for the ordered list of
*shrink candidates* of the failing value (`spec/stdlib.md`: integers move
toward 0, lists drop elements then shrink elements, `Positive` values move
toward 1, nested generators shrink their payloads), re-runs the body with
that coordinate replaced (other coordinates fixed), accepts the first
candidate that fails *with the same fault code*, and repeats from that
value; when no candidate of binder 1 fails it proceeds to binder 2, and
after the last binder restarts from binder 1 until a full pass accepts
nothing. The process is bounded by `[test] max_shrinks = 1000` body
evaluations; when the bound is reached while another rerun is wanted the
current tuple is reported with `shrink_exhausted: true` (not when the last
allowed rerun also ended the search; with a bound of 0 it is `true` exactly
when the failing value has a candidate), and candidates are not
deduplicated (`spec/stdlib.md` 8.4). The reported counterexample is the final tuple,
and the reported fault is the one it raised. Rationale: greedy
coordinate-wise descent is deterministic, needs no generator inversion, and
reproduces the fault class that was observed instead of wandering to an
unrelated failure. Rejected alternatives: random shrinking; shrinking only
the first binder.

Decision: [D-125] S46. every test and property produces one `test_result`
diagnostic (category test):

| Field | Test | Property |
| - | - | - |
| `symbol`, `qualified_name` | the identity (S42) | the identity |
| `status` | `"tested"` (the body ran to completion) or `"failed"` (S50) | `"tested"` (every case ran to completion) or `"failed"` |
| `severity` | `info` when tested, `error` when failed | same |
| `duration_ms` | wall time | wall time |
| `seed` | absent | the run seed |
| `property_seed` | absent | the derived seed (S44, hex) |
| `cases` | absent | the number of cases executed: the configured count on success (`[test] cases`, default `100`, or `--cases`), the failing index plus one on failure, and `0` when the property failed at setup (a fault while its `for` ranges were evaluated, S71), which also leaves `failing_case` and `shrunk` absent |
| `failing_case` | absent | `{index, input}` where `input` is the original failing binder tuple (value JSON) |
| `shrunk` | absent | the shrunk binder tuple (value JSON) and `shrink_steps`, `shrink_exhausted` |
| `fault` | the fault diagnostic object (6.7) | the fault raised by the shrunk tuple |
| `receipt` | the test receipt (S59) | the test receipt (S59) |

If the fault is a `contract_violation` with `clause_kind` in {`ensures`,
`invariant`, `decreases`} raised by a declaration other than the test
itself, the runner additionally emits `contract_counterexample`:
`symbol` = that declaration, `facet: "body"`, `clause`, `input` = that
declaration's arguments at the violation, `actual`, `status: "failed"`,
`found_by` = the test's identity, and also `clause_kind` (with `iteration`
for a loop clause), spanning the violated clause. It is built from the
violation as raised, before any S34/S70 rewrite of the fault the record
reports; a violation in a function that a property's `for` range calls at
setup (S71) yields one too; and none is emitted for a capability
operation's `ensures`, whose failure is the host binding's (S31). The
`test_result` diagnostic itself has no span. This is the design's example diagnostic: a
concrete executable counterexample against a body, distinct from the
abstract solver models of Milestone 5 (which will carry `facet:
"contract"` and `status: "failed"` from `verification_result`). A
`requires` violation inside a test is not a counterexample against the
callee; it is reported only through the test's `fault`. Rationale: the
design distinguishes "a concrete executable counterexample" from "an
abstract solver model" and from `tested`; attaching the counterexample to
the body that failed is what makes the report actionable. Rejected
alternative: reporting only `test_result`.

Decision: [D-126] S59. every `test_result` and every `verification_result` carries
a `receipt` object that binds the evidence to the inputs it depends on, as
the design requires of verification and test receipts (line 261) and of
every response ("identifies the source revision", line 245):

| Field | Content |
| - | - |
| `source_revision`, `dirty`, `toolchain`, `modules` | exactly the `dependencies` object of S48, where `modules` lists every project module in the static call closure of the tested or verified declaration, its own module included (a test depends on the implementations it executes, design line 293; standard-library modules are covered by `toolchain.std`, as in S48) |
| `check_configuration` | `verification_result`: `{release}`; `test_result`: `{cases, max_shrinks, max_call_depth, seed}` (the values in effect, S38, S44, S45) |
| `assumptions` | the named assumptions the evidence depends on: `[]` in Milestone 1; from Milestone 5 one `{kind, symbol, facet?, description}` per unchecked callee, hole, foreign adapter, or policy-admitted dependency (design line 136) |
| `fixtures`, `environment` | `test_result` only: capability bindings supplied to the run (`[]` in Milestone 1, 6.9, and in Milestone 2, where tests bind nothing, S78) and the environment inputs the run depended on (`{os, arch}` in Milestone 1) |

A `verification_result` whose `assumptions` is `[]` is the Milestone 1
verification receipt; a `test_result` is the test receipt. `spec/toolchain.md`
may embed the same object in its command output and in standalone receipt
documents, and may add fields, but may not omit any of these. Rationale:
evidence that is not bound to its inputs cannot be safely reused or
invalidated (design line 423), and the per-module content hash is the
Milestone 1 stand-in for dependency revisions until `cpl.lock` and the
semantic index exist. Rejected alternative: leaving the binding to a
separate receipt document only (every consumer of a `test_result` would
have to correlate two artifacts to learn what was tested).

## 8. Holes and build modes

### 8.1 Holes

A `HOLE` expression `?name` is an unfinished expression. Statically it
takes the expected type (3.8); it may not appear in a contract expression
(`hole_in_contract`). Several holes may share a name within a declaration
and across declarations; they are distinguished by span. A declaration
containing a hole is otherwise fully checked: its signature is complete,
callers typecheck against it, its contracts are checked, and its effect
bound is checked for the calls that do exist. Dynamically, evaluating a hole
raises the fault `unresolved_hole` (6.7).

Decision: [D-231] S64. every hole is reported exactly once per check. In
development mode the report is the `hole` diagnostic (S47, 8.3). A hole
that is an error, because it has no expected type (`hole_type_unknown`,
S56) or stands in a contract expression (`hole_in_contract`, 5.1), is
reported as that error only: no `hole` diagnostic and no hole-report entry
accompanies it, since the report's `expected_type` would be absent or the
position is one that may never stay unfinished. Under `--release` a
`fn`-body hole's single report is `hole_in_release`, which *replaces* the
`hole` diagnostic for that hole (S47; `CheckResult::holes` of
`spec/toolchain.md` 1.6 still lists it), while a `test`/`property`-body hole
keeps `hole` at severity `warning`. Rationale: two diagnostics at one span
for one unfinished expression are noise to an agent and double-count in
every summary, and the design's hole report is defined by its expected
type. Rejected alternative: emitting `hole` beside every error or release
diagnostic (the hole report would need an `expected_type: null` form).

Decision: [D-247] S69. a hole whose expected type exists but is, or
contains, the error type of S62 (`takes(?h)` with `fn takes(x: Nope)`
after `unresolved_type`, a hole among the arguments of a failed callee) is
still reported exactly once, as S64 requires: as `hole` (or, under
`--release`, `hole_in_release`) with its hole-report entry, the error type
rendering as `?` in its `expected_type`. The error type renders as `?` in
every rendering of a type (diagnostic messages and fields, hole reports,
`Program::display_ty`), the spelling `cannot_infer_type` already uses for
an unresolved variable. Rationale: S64 promises one report per hole, and
dropping such a hole would let `--release` pass a program with a `fn`-body
hole, while `hole_type_unknown` would make one mistake two errors (S62);
`?` marks the unknown part without inventing a type. Rejected alternatives:
no report (an earlier behaviour of the checker, under which release could
pass an unfinished body); `hole_type_unknown` (a second error caused by the
first).

### 8.2 Development and release modes

Decision: [D-127] S47. `cpl check` has two modes. *Development* (the default)
reports every hole as the diagnostic `hole` with severity `info`, so a
program with holes checks successfully, runs until a hole is reached, and
has tests that fail only when they reach one. *Release* (`cpl check
--release`, and the `cpl build`/packaging commands of later milestones)
rejects (a) every hole in the *linked program* with `hole_in_release`
(error, reported in place of `hole`, S64): every `fn` body of every compilation unit,
private declarations included, because without an entry point every
function of a library is linked; `test` and `property` bodies are not part
of the linked program (the design runs them through the test runner, lines
283-285), so a hole in one stays the diagnostic `hole`, raised to severity
`warning` under `--release`, and the test fails when it reaches the hole
(the development behaviour above, 7.3); and (b) every declaration whose
verification policy is unsatisfied with `verification_unsatisfied` (S36).
Nothing else differs between the modes: contracts are checked at runtime in
both. Rationale: the design says "release builds reject holes in the linked
program and require the configured verification obligations to be closed"
and that a hole is "legal during development". Rejected alternatives: a
reachability analysis from exported functions (meaningless for a library in
Milestone 1 and a source of surprising "unused hole" acceptance); counting
test bodies as linked (a hole in a test would block the release of code
that has none).

### 8.3 The hole report

Decision: [D-128] S48. the `hole` diagnostic carries, in addition to the envelope:

| Field | Content |
| - | - |
| `hole` | the hole's name (`build_pages`) |
| `symbol`, `qualified_name`, `facet: "body"` | the enclosing declaration |
| `expected_type` | the hole's type, rendered as 3.3 renders types (`List<List<T>>`): named types by their declared names, type parameters by name, and `?` for an erroneous part (S69) |
| `bindings` | every binding visible at the hole, innermost first: `{name, type, kind}` with `kind` ∈ `parameter`, `using`, `let`, `var`, `pattern`, `forall`, `for`, `result`, `self`; `var` bindings carry `mutable: true` |
| `capabilities` | for each `using` parameter in scope, in the order of `bindings`: `{parameter, capability, operations: [{name, signature, in_bound}]}`, the operations in the capability's declaration order, where `in_bound` says whether `parameter.operation` is in the enclosing function's effect bound (an operation outside the bound can be listed but not called) |
| `requires`, `ensures` | the enclosing function's clauses as canonical text (the `clause` rendering of 9.2), in order (empty arrays for tests) |
| `invariants` | the `invariant` clauses of every enclosing `while`, innermost first |
| `dependencies` | the revisions the hole's context depends on, an object `{source_revision, dirty, toolchain, modules}`: `source_revision` is the commit of the revision (`HEAD`) when the project root is inside a Git work tree with at least one commit, else `"unversioned"`; `dirty` is `true` when there is such a commit and the revision's state is `worktree`, by the check of `spec/toolchain.md` 6.3, which reads the source-set roots only (omitted otherwise, and always when there is no commit); `toolchain` is `{cpl, edition, std}` (the `cpl` version, the edition, and the standard library version); `modules` is `[{module, content_hash}]` for the enclosing declaration's own module and every other project module whose definitions it refers to (standard-library modules are never listed: `toolchain.std` covers them), where `content_hash` is the blake3 digest of that module's canonical formatted text. The same object is the core of every receipt (S59) |

Rationale: these are exactly the items the design lists ("expected type,
visible bindings, available capability operations, relevant contracts, and
the revision of each dependency"); the per-module content hash is the
Milestone 1 stand-in for dependency revisions until `cpl.lock` and the
semantic index exist. Rejected alternative: deferring the report to the
semantic index of Milestone 5.

## 9. Structured diagnostics

Decision: [D-129] S60. this document is the single owner of the diagnostic
contract: which codes exist (9.4), their category and severity, when each
fires, the envelope field names and spellings of 9.2, the code-specific
field names of 8.3, 9.3, and 9.4, the identity format of `symbol` (S2),
the value encodings (S40), and the `status` vocabulary (9.5).
`spec/toolchain.md` owns the command-line syntax, exit codes,
configuration keys (whose names and defaults this document cites where it
needs them), receipt documents, and the per-command JSON object that wraps
these diagnostics; it must emit these codes and field names unchanged and
may add fields but not rename or omit them. `spec/grammar.md` owns the
message texts of lexical and syntax diagnostics; their codes are the parse
table of 9.4, the finer catalogue D59 defers to. Rationale: the design
requires the structured API and the CLI to "return the same underlying
results", which is impossible if two documents spell the same fact
differently; the semantic document is where each diagnostic's meaning is
defined, so it must also fix the name a consumer filters on. Rejected
alternative: defining codes here and field spellings in the toolchain
document (every rule in this document that names a field would then be
provisional).

### 9.1 Severity and outcome

| Severity | Effect on `cpl check` | Effect on `cpl test` / `cpl run` |
| - | - | - |
| `error` | the check fails (nonzero exit) | compile errors abort; faults fail the test or run |
| `warning` | reported; the check succeeds | reported (the check's warnings included) |
| `info` | reported (hole reports, verification statuses, tested tests) | the command's own are reported (`test_result` of a tested test); the check's (`hole`, `verification_result`) are `cpl check`'s and are not repeated (`spec/toolchain.md` T58) |

Every command emits the same diagnostic objects whether rendered as text
or as JSON; the object that wraps them under `--json`, the line-oriented
`--jsonl` form, and the order of the list are `spec/toolchain.md` 7.1's
(S60). The rendered text form is derived from the object and carries no
extra information.

### 9.2 Envelope

Decision: [D-130] S49. every diagnostic is a JSON object with these fields;
optional fields are omitted when absent (never `null`), unknown fields must
be ignored by consumers, and code-specific fields use the names in 9.3-9.4:

| Field | Presence | Content |
| - | - | - |
| `code` | always | one of the codes in 9.4 |
| `category` | always | `parse`, `resolve`, `type`, `effect`, `contract`, `hole`, `test`, `release_policy`, `runtime`, `internal` for the codes of this document; `spec/toolchain.md` Appendix B adds `project`, `index`, `git`, `work`, `patch`, `usage`, and from Milestone 2 `build`, for the codes it owns, and `spec/cabi.md` Section 4 adds `interop` for the codes of the C API (`spec/toolchain.md` T76) |
| `severity` | always | `error`, `warning`, `info` |
| `message` | always | one human-readable sentence; never the only carrier of a fact that a field carries |
| `span` | when a source location is known (S63 fixes the primary span of every check diagnostic) | `{file, line, col, end_line, end_col}`: `file` relative to the project root with `/` separators; `line` 1-based; `col` 1-based, counted in Unicode scalar values; `end_col` exclusive; a point span has `end_line == line` and `end_col == col` |
| `symbol` | when a declaration is involved | the stable identity: the `@id` argument if present, else the qualified name (S2); for a capability operation or a builtin, its qualified name (1.3, 6.10) |
| `qualified_name` | whenever `symbol` is present | the qualified name (1.4), so consumers need not resolve `@id`s |
| `facet` | when a facet is involved, and only together with `symbol` | `identity`, `name`, `signature`, `contract`, `effects`, `body`, `docs`, `verification_policy` |
| `clause` | contract and assertion diagnostics | the *canonical text* of the clause or expression: the formatter's rendering on one line with canonical spacing, source line breaks removed and parentheses kept exactly as written (`spec/toolchain.md` 5.2, canonical text, and 2.4 rule 8), so that `requires b  !=  0` and `requires b != 0` both yield `"b != 0"` and `clause` is a stable key; a clause containing an `if` or `match`, whose canonical blocks are multi-line, has every line break and the indentation after it replaced by one space (`spec/toolchain.md` T56) |
| `clause_kind` | `contract_violation` | `requires`, `ensures`, `invariant`, `decreases` |
| `attributed_to` | `contract_violation`; from Milestone 2 also the `refinement_violation` of a capability operation's result (S77) | `caller`, `body`, `binding` (S57); always `binding` on a `refinement_violation` |
| `input`, `actual`, `expected` | where listed | value JSON (S40) or a rendered type, as the code specifies |
| `status` | where listed | 9.5 |
| `notes` | optional | `[{message, span?}]`: related locations (the other declaration of a duplicate, the shadowed binding, the rule being applied) |
| `backtrace` | faults, omitted when empty (6.7) | `[{qualified_name, span?}]`, innermost first |

The `category` field and its ten-value set are chosen by this document:
the design fixes only `code`, `symbol`, `facet`, `clause`, `input`,
`actual`, and `status` (its example at lines 229-239) and has no category
notion; `runtime` is the category of the two faults that are neither
contract, hole, test, nor internal failures. Rationale: `span`, `message`,
`severity`, and `category` are the minimum a renderer and a filter need;
`qualified_name` is added because an `@id` alone is opaque to a human
reader. Rejected alternative: a nested `details` object (would move the
design's top-level fields).

Decision: [D-230] S63. the primary `span` of every check diagnostic is fixed, so
that two implementations agree line for line (`spec/toolchain.md` 12.2
compares marker lines). (1) A diagnostic about an expression spans that
expression: the operand that failed a check, the call, the constructor,
the literal, the hole (`hole_in_contract` included), the `using` argument,
the `?` expression (`propagate_in_contract`), each handle operand
(`capability_equality`); `unknown_member` and `method_not_called` span the
member name, while `property_called` spans the whole call (`xs.len()`: the
parentheses are the mistake); `type_mismatch` with the note
"missing tail expression" spans the whole block, `{` to `}`, as does every
diagnostic of the proof that a missing tail's `Unit` undergoes against a
refinement of `Unit` (3.10: `refinement_false`,
`constant_evaluation_fault`, `cannot_prove_refinement`), while those of a
bare `return` span the `return` statement. (2) A
diagnostic about a clause (`requires`, `ensures`, `invariant`, `decreases`,
`effects`, `where`, `for`) spans the clause from its keyword:
`unused_decreases`, `effect_in_contract`, `recursive_contract`,
`private_in_public_contract`, `result_outside_ensures` (outside a clause:
the name), `type_mismatch` on a clause; `duplicate_effect` spans the
repeated reference and `statement_in_contract` the offending statement.
(3) A diagnostic about a
declaration as a whole spans the declaration's name token (the `TEXT` name
of a test or property; the module path for `module_file_misplaced` and
`reserved_module_path`): `duplicate_declaration`, `duplicate_identity`,
`duplicate_test_name`, `missing_body`, `prelude_name_redefined`,
`infinite_type`, `cyclic_type_definition`, `missing_decreases` on a
function, `verification_result`, `verification_unsatisfied`,
`cannot_run_generic`, `cannot_run_effectful`; a diagnostic about a part of
a header spans that part: the type parameter (`unused_type_parameter`), the
parameter (`using_param_not_capability`, `unused_capability`, `self_bound`
on a parameter), the offending type (`capability_in_value_position`,
`not_a_value_type`, `private_type_in_public_signature`, `unresolved_type`),
the field, variant, or operation; an annotation diagnostic
(`unknown_annotation`, `duplicate_annotation`,
`invalid_annotation_argument`, `annotation_misplaced`,
`std_builtin_missing`, and the toolchain's `id_kind_mismatch`) spans the
annotation. (4) A two-location diagnostic is *one* diagnostic whose primary
span is the *later* occurrence in (file path, item or statement order) and
whose `notes` carry the earlier one, at the same kind of span
(`duplicate_identity`: the later declaration's name token, with the note
at the earlier one's) (`duplicate_declaration`,
`duplicate_identity`, `duplicate_import`, `duplicate_test_name`,
`duplicate_field`, `duplicate_variant`, `duplicate_operation`,
`duplicate_parameter`, `duplicate_type_parameter`,
`duplicate_pattern_binding`); where one occurrence is the subject the
primary span is that one: `import_conflicts_with_declaration` spans the
import, `shadowed_binding` the shadowing binding, `assign_to_immutable` the
whole assignment, `prelude_name_redefined` the redefinition. (5)
`missing_decreases` on a loop spans the loop (from `while`),
`unreachable_statement` the first unreachable statement, `unreachable_arm`
the arm, `non_exhaustive_match` the `match` expression. (6) Span-less are
only the diagnostics with no source location: the `project_*` diagnostics
of `spec/toolchain.md` Appendix B and `unsupported_edition` (the manifest
and lock are not source files; the corpus harness treats a span in them as
none) and `empty_test_selection`. Rationale: an agent acts on the span and
a human reads the `-->` line, so a diagnostic with a known location must
carry it (a span-less `reserved_module_path` could not even name the
file); the later occurrence of a duplicate is the one an author just wrote,
and one diagnostic with a note keeps counts equal to mistakes. Rejected
alternatives: span-less declaration-level diagnostics (an earlier reading
of 5.4 and of the `verification_result` example in `spec/toolchain.md`
7.2); two diagnostics for a duplicate (an earlier wording of
`spec/toolchain.md` 3.7).

From Milestone 2 the `build_*` codes of `spec/toolchain.md` Appendix B and
the `interop_*` codes of `spec/cabi.md` Section 4 are span-less as well
(`spec/toolchain.md` T76): they describe a build or a call of the C API,
not a source location. This extends item (6) of S63 and changes nothing
else in it.

### 9.3 Fields by code: conventions

| Field name | Meaning wherever it appears |
| - | - |
| `name` | the identifier the diagnostic is about |
| `module` | a module path |
| `type` | a rendered type |
| `callee`, `caller` | qualified names |
| `effect` | an effect reference `param.op` after substitution |
| `candidates` | list of qualified names |
| `missing` | list of names or witness patterns |
| `hole`, `expected_type`, `bindings`, `capabilities`, `requires`, `ensures`, `invariants`, `dependencies` | per 8.3 (`bindings` innermost first, in every diagnostic that carries it: 8.3, S41, `unresolved_hole`) |
| `actual` on `invariant`/`decreases` violations | the object `{bindings, measure?, previous?}` of 5.3 |
| `attributed_to` | `caller`, `body`, or `binding` (S57); on the `refinement_violation` of an operation's result always `binding` (S77) |
| `iteration`, `seed`, `property_seed`, `cases`, `failing_case`, `shrunk`, `shrink_steps`, `shrink_exhausted`, `fault`, `found_by`, `duration_ms` | per Sections 5 and 7 |
| `receipt` | the object of S59: `source_revision`, `dirty`, `toolchain`, `modules`, `check_configuration`, `assumptions`, and for tests `fixtures`, `environment` |
| `kind`, `detail`, `resource`, `limit` | per 6.7 (`limit` for `memory`: S76; the binding kinds of `internal_error`: S80) |
| `parameter`, `pointer` | the name of a parameter, and an RFC 6901 position inside an argument (S39, S81) or inside an operation's result (S77, S80) |
| `operation`, `binding` | the qualified name of a capability operation (1.4) and the binding name of 6.9 (S77, S80) |
| `reason` | `verification_result`: the verifier's reason for `unknown` (`spec/verification.md`), null otherwise |

### 9.4 The complete code list (Milestones 1 and 2)

Severity is `error` unless marked (W) warning or (I) info. "F" marks a
runtime fault. Fields listed are in addition to the envelope.

Milestone 2 adds no code to this list: what it adds to this document is new
producers and new fields of existing codes (`refinement_violation`, S77 and
S81; `internal_error`, S80; `resource_exhaustion` with `resource:
"memory"`, S76; `cannot_run_effectful`, S78). The codes it adds belong to
the documents that own them: the `build_*` codes, the new `project_*`
codes, `engine_divergence`, and the command's own `internal_error` kinds to
`spec/toolchain.md` Appendix B, and the `interop_*` codes to `spec/cabi.md`
Section 4.

**parse** (lexer and parser; each message begins with the text
`spec/grammar.md` quotes for it, grammar D72. D59 names two diagnostic
*classes*, lexical and syntax, and defers "a finer code catalogue" to the
diagnostics specification: this table is that catalogue, every quoted grammar message maps to exactly one row, and a
failed file carries exactly one parse diagnostic, the earliest, D70, S52)

| Code | Raised by | Fields |
| - | - | - |
| `invalid_utf8` | grammar 1.1; raised when the file's bytes are read, before lexing (D70); the file counts as failed | |
| `invalid_line_terminator` | lone CR | |
| `non_ascii_outside_text` | grammar 1.1 | `found` |
| `invalid_character` | `;`, `!`, `&`, `\|`, other unexpected characters (grammar 1.6 messages); `control character in text literal` (D62) | `found` |
| `unterminated_text_literal` | grammar 1.5 | |
| `invalid_escape` | grammar 1.5: an unknown character after `\`, and a `\u{}` that is a surrogate, out of range, or has more than six hex digits | `found` |
| `invalid_int_literal` | separators or leading zeros (grammar 1.5) | |
| `float_literal_unsupported` | grammar 1.5 | |
| `literal_adjacent_identifier` | `1e5`, `2x` | |
| `prefixed_literal_reserved` | D8 | |
| `doc_comment_position` | grammar 1.2 (both messages), including a doc comment in an admitted position that is not followed by what it documents (before `}`, at the end of the file, before `use`, `base`, `expect`, or a header's `{`) | |
| `reserved_word` | D4, in every position (also after `@`, D71, and after `require`, D36) | `name` |
| `hole_name_case` | D10: `hole names are snake_case` | `name` |
| `hole_name_keyword` | grammar 1.7: `a hole name cannot be a keyword` (its own row: not `hole_name_case`, whose message is `hole names are snake_case`) | `name` |
| `syntax_error` | every other grammar violation: the generic `expected <description>, found <token>` (D59, D72); a header without its mandatory body (grammar 2.4 rule 3: `` expected `{` ``, `` expected `{` or a clause ``, `` expected a `for` clause or `{` ``, `` expected `expect` on its own line ``, `` expected `{` on its own line ``); `<elements> are separated by newlines` (D55: `match arms are separated by newlines`, `fields are separated by newlines`, ...); `expected a member name`; `` expected `->` ``; `expected expression, found end of line`; `expected end of line after the declaration` (also `type A<T> = ...` and `type A<T>(T)`, grammar 3.5); `Variant()`; a chained comparison; `forall` as an operand (D45); a postfix operator on a hole; `a struct literal names a type; variants have positional payloads`; `a type declaration is a wrapper, type A(T), or a refinement, type A = T where ...` (D64); `add names a namespace; write <path>.*` (grammar 4.2); `` expected `change` (a .work file declares at least one change) `` (grammar 4.1); `require` with no requirement after it (D36); a trailing comma in a generic list, or a line break in one where newlines are significant (grammar 2.2); the implementation limits of grammar D73 (`nesting is too deep (more than 64 levels)`, `an expression has too many operators (more than 1000); split it into bindings`, with the implementation's limits in place of 64 and 1000) | `expected`, `found` on every message of the generic `expected ..., found ...` shape (D72); none on the others |
| `duplicate_clause` | second `effects` or `decreases` on one header | `name` |
| `use_after_declaration` | D13 (`.cpl`: a `use` after a declaration; `.work`: a `use` after a `change`, grammar 4.1) | |
| `positional_after_using` | grammar 3.9 (parameters) and 3.12 (arguments) | |
| `misplaced_else` | D12: a line beginning with `else` in any significant-newline context of any file form (grammar 2.4 rule 4), except where the grammar requires one particular token (the `module` header at the start of a `.cpl` file, the declaration after an annotation line), where it is the generic `syntax_error` for that token (grammar 2.4 rule 4) | |
| `unknown_requirement` | D36: any token after `require` other than the end of the line, `}`, the end of the file (those are `syntax_error`), or a reserved-future word (`reserved_word`) | `found` |
| `duplicate_work_item` | grammar 4.1-4.2 (`duplicate base`, `duplicate write set`, duplicate task name) | `name` |

**resolve**

| Code | Raised by | Fields |
| - | - | - |
| `unsupported_edition` | 1.1 | `found` |
| `reserved_module_path` | 1.1 | `module` |
| `module_file_misplaced` (W) | 1.2 | `module`, `expected` (conventional path) |
| `duplicate_declaration` | 1.4 | `name`, `notes` (other location) |
| `duplicate_identity` | S2 | `symbol` (the duplicated `@id` value), `qualified_name` (the later declaration's), `facet: "identity"`, `notes` |
| `duplicate_import`, `import_conflicts_with_declaration`, `prelude_name_redefined` | 1.6, S6 | `name`, `notes` |
| `generator_import` | 1.6 (`spec/stdlib.md` S28) | `name` |
| `missing_body` | 1.8 (a bodiless `fn` or definition-less `type` outside the standard library) | `name` |
| `unresolved_module` | 1.6, 2.4 | `module` |
| `unresolved_name` | 2.2-2.5 | `name`, `module` (when a qualified lookup failed) |
| `unresolved_type` | type positions, `Gen` written by a user (S14), a reserved type name of `spec/stdlib.md` S11 (note `reserved for a later edition`) | `name` |
| `private_item` | 1.5 | `symbol`, `qualified_name` |
| `private_type_in_public_signature` | 1.5 | `symbol`, `facet: "signature"`, `type` |
| `private_in_public_contract` | S4 | `symbol`, `facet: "contract"`, `clause`, `callee` |
| `unused_import` (W), `unused_binding` (W) | 1.6 (never for a `use` item that is itself diagnosed, S62); a `let`/`var`/pattern binding never read (`_`-prefixed names are exempt; an assignment is not a read; parameters and `for`/`forall` binders are never diagnosed, S67) | `name` |
| `shadowed_binding` | S8 | `name`, `notes` |
| `duplicate_pattern_binding` | 3.12 | `name` |
| `ambiguous_constructor` | S10 | `name`, `candidates` (qualified names, sorted) |
| `not_a_constructor`, `not_a_value` | S10 step 3, S11 | `name` |
| `unknown_annotation`, `annotation_misplaced`, `duplicate_annotation`, `invalid_annotation_argument` | 1.8 | `name` |
| `result_outside_ensures`, `result_name_reserved`, `self_outside_where`, `self_bound` (W) | S9 | `name` |
| `assign_to_immutable` | 3.11 | `name`, `notes` (the binding) |
| `return_outside_function`, `propagate_outside_function` | 3.8, 3.11 | |
| `duplicate_field`, `unknown_field`, `missing_field` | 1.4, 3.8 | `name` / `missing`, `type` |
| `duplicate_variant`, `duplicate_operation`, `duplicate_parameter`, `duplicate_type_parameter` | 1.4 | `name` |
| `unused_type_parameter` | 1.8 (error on `struct`/`enum`, W on `fn`) | `name` |
| `cyclic_type_definition`, `infinite_type` | 1.8, S7, S66 (on every declaration of the cycle) | `type`, `notes` (the cycle) |
| `cannot_run_effectful`, `cannot_run_generic` | S39, S78 | `symbol` |

**type**

| Code | Raised by | Fields |
| - | - | - |
| `type_mismatch` | 3.4 rule 4 and every `⇐` failure | `expected`, `actual` (rendered types; on an operand of `++`, `expected` reads `List<T>, Text, or Bytes`), `notes` |
| `cannot_infer_type` | 3.7 | `type` (with variables rendered `?`) |
| `cannot_prove_refinement` | 3.5 step 4, S65 (two distinct refinements of one root base included), S73 (`?` in a function whose return type is a refinement) | `expected`, `actual`, `notes` (suggests `check`; S65: quotes both predicates) |
| `refinement_false` | 3.5 step 3 | `expected`, `actual` (value JSON), `clause` |
| `constant_evaluation_fault` | 3.5 steps 1-2 | `fault` (the fault code), `clause` (in step 1, which evaluates no clause, the expression's canonical text) |
| `check_requires_type_argument`, `check_requires_refinement` | 3.7 | `type` (for `check_requires_type_argument`, which has no type argument, the argument's inferred type) |
| `type_argument_count_mismatch` | S12, 3.7 | `expected`, `actual` (counts) |
| `not_a_value_type` | S13; not reachable in 0.1 (a capability is `capability_in_value_position`, 1.8, and `Gen` cannot be written) | `type` |
| `arity_mismatch`, `using_arity_mismatch` | 3.7, S28 | `callee` (`"check"` for the intrinsic, which has no stub), `expected`, `actual` (counts) |
| `not_callable` | 3.8 | `name`, `notes` |
| `unknown_member`, `property_called`, `method_not_called` | 3.9, D43 | `name`, `type` |
| `unused_value` | 3.11 | `type` |
| `non_exhaustive_match` | 3.13 | `missing` (witness patterns), `type` (scrutinee) |
| `unreachable_arm` (W) | 3.13 | |
| `pattern_type_mismatch`, `pattern_arity_mismatch` | 3.12 | `expected`, `actual` (for a constructor pattern, the constructor's type name without arguments: `Option`) |
| `propagate_not_result`, `propagate_return_mismatch`, `propagate_error_mismatch` | 3.8 | `expected` (`Result<T, E>` on the first two), `actual` |
| `return_value_missing` | 3.11 | `expected` |
| `comparison_unsupported` | S18 | `actual` |
| `forall_range_not_list`, `for_range_not_generator`, `generator_outside_for` | 3.8, 3.14, S14 | `actual` |
| `unreachable_statement` (W) | S24 | |

**effect**

| Code | Raised by | Fields |
| - | - | - |
| `using_param_not_capability`, `effect_param_not_capability`, `unknown_effect_operation` | 4.1 | `name`, `type` |
| `duplicate_effect` (W), `unused_capability` (W) | 4.4, S29 | `effect` / `name`; `unused_capability` also `symbol`, `qualified_name`, `facet: "signature"` |
| `undeclared_effect` | 4.4 | `symbol`, `qualified_name`, `facet: "effects"`, `effect`, `callee` |
| `effect_in_contract` | 4.5 | `symbol`, `facet: "contract"`, `clause`, `effect` |
| `capability_as_value`, `capability_equality`, `capability_in_value_position`, `using_argument_not_capability` | S27, S28 | `name`, `type`; the first two also `symbol`, `qualified_name`, and `facet` (`body` or `contract`, where the occurrence is) |

**contract**

| Code | Raised by | Fields |
| - | - | - |
| `hole_in_contract`, `propagate_in_contract`, `statement_in_contract` | 5.1 | `symbol`, `facet: "contract"`, `clause`; `hole_in_contract` also `hole` |
| `recursive_contract` | S33 (never for a `@builtin` declaration, S61) | `symbol`, `clause` |
| `unused_decreases` (W), `missing_decreases` (W) | 5.4 | `symbol`, `facet: "contract"`; `span` per S63 (the clause; the function's name, or the loop) |
| `contract_violation` (F) | Section 5, 6.10 | `clause_kind`, `clause`, `symbol`, `facet: "contract"`, `attributed_to`, `input`, `actual`, `caller`, `iteration`, `status: "failed"`, `backtrace` |
| `contract_counterexample` | S46 | `symbol`, `facet: "body"`, `clause`, `clause_kind`, `iteration` (loops), `input`, `actual`, `status: "failed"`, `found_by` |
| `verification_result` (I) | S36 | `symbol`, `qualified_name`, `facet: "verification_policy"`, `status`, `reason`, `receipt` (S59, attached by the command layer, 5.5); a note when termination is also unknown |

**hole**

| Code | Raised by | Fields |
| - | - | - |
| `hole` (I; W for a hole in a `test`/`property` body under `--release`) | S47 | per 8.3 |
| `hole_type_unknown` | 3.8 (no `hole` diagnostic beside it, S64) | `hole`, `symbol` |
| `unresolved_hole` (F) | 6.7 | `hole`, `symbol`, `expected_type`, `bindings` (with values), `backtrace` |

**test**

| Code | Raised by | Fields |
| - | - | - |
| `empty_test_name`, `duplicate_test_name` | S42 | `name`, `notes` |
| `empty_test_selection` | S42 | `selector` |
| `test_result` (I when `tested`, error when `failed`) | S46 | per S46, including `receipt` (S59) |
| `assertion_failed` (F) | S41 | `clause`, `symbol`, `bindings`, `backtrace` |
| `test_fixture_missing` (Milestone 3) | S83 | `capability`, `symbol` |
| `test_fixture_failed` (Milestone 3) | S83 | `binding`, `capability`, `symbol`, `notes` |

**release_policy**

| Code | Raised by | Fields |
| - | - | - |
| `hole_in_release` | S47 | `hole`, `symbol`, `expected_type` |
| `verification_unsatisfied` | S36 | `symbol`, `facet: "verification_policy"`, `status` |

**runtime**

| Code | Raised by | Fields |
| - | - | - |
| `division_by_zero` (F) | 6.3 | `input`, `backtrace` |
| `refinement_violation` (F) | 6.7, S39, S77, S81 | `expected_type`, `actual`, `clause`, `pointer`, `backtrace`; `parameter` for an argument (S39, S81); `attributed_to` (`binding`) and `binding` for an operation's result (S77) |

**internal**

| Code | Raised by | Fields |
| - | - | - |
| `internal_error` (F) | 6.7, S80 | `kind`, `detail`, `backtrace`; `operation` and `binding` for the binding kinds, and `pointer` for `binding_result_ill_typed` (S80) |
| `resource_exhaustion` (F) | S38, S76 | `resource`, `limit` (milliseconds for `time`, bytes for `memory`, 6.7), `backtrace` (possibly empty for `memory`, S76); `depth` when the evaluation thread's stack guard (rather than the depth counter) ended the run (`spec/toolchain.md` 1.7; in the native runtime `spec/cabi.md` Section 3) |

### 9.5 The `status` vocabulary

Decision: [D-131] S50. `status` takes the design's four values, `proved`,
`failed`, `unknown`, and `tested` (design line 241), plus, from Milestone
5, `conditional`, the design's "conditional proof" (line 136), split over
two vocabularies that share only `failed`, and appears only on the codes
listed:

| Vocabulary | Values | Codes |
| - | - | - |
| verification | `proved`, `failed`, `unknown`; from Milestone 5 also `conditional` (every obligation proved and at least one named assumption in the receipt, `spec/toolchain.md` 8.1; never emitted in Milestone 1) | `verification_result` (all of them); `contract_violation` and `contract_counterexample` (always `failed`: a concrete violation is a failed obligation, as in the design's example); `verification_unsatisfied` (`unknown` or `failed`; `conditional` satisfies the policy, S36) |
| test | `tested`, `failed` | `test_result` |

`tested` means the test or every property case ran to completion; it never
appears on a verification code, and `proved`/`unknown` never appear on a
test code, so test evidence can never be read as a proof. `failed` is
shared because the design's own example reports a test-found concrete
counterexample as `status: "failed"`; `code` is the discriminator
(`test_result` versus the contract codes). Milestone 5 fills the receipt's
`assumptions` (S59) without changing these values. Rationale: the design
names `tested` and requires it to remain distinct from the three
verification statuses; renaming it would drift from a documented
vocabulary. Rejected alternatives: `passed` in place of `tested` (drift); a
distinct test-failure value (would make the design's `failed` example
unreportable from a test).

## 10. Deferred from 0.1 Milestone 1

| Design item | Status (in Milestone 1, unless a row names a later milestone) |
| - | - |
| floating-point types, `NaN`/signed-zero contract semantics | no type, no literal (`float_literal_unsupported`) |
| bounded machine integers with checked conversions | no types; the names `Int8`..`UInt64` and `Float32`/`Float64` are reserved by `spec/stdlib.md` S11 and listed in 1.7 (a later edition adds the types to `std`); the checked machine-integer conversions of Milestone 2's C boundary belong to the binding layer, not the language (`spec/stdlib.md` S46) |
| `Bytes` literals and operations beyond `len`, `is_empty()`, `++`, `==` | `std` functions in a later edition |
| user-defined error conversions for `?`, `?` on `Option` | none: `?` is defined on `Result` only and `E` must be identical (3.8) |
| standard capabilities: time, randomness, filesystem, networking, model calls | none in edition 0.1 (`spec/stdlib.md` S42); no handle can be created in Milestone 1 (6.9); the handles of project capabilities come from C bindings and C hosts from Milestone 2 (6.9, S78), and from Python and Node adapters in Milestones 3 and 4 |
| adapters, foreign calls, `ForeignError` construction, `[bindings]` in `cpl.toml` | Milestone 1: parsed and validated (`spec/toolchain.md` 11.1), bound by nothing; C adapters arrive in Milestone 2 (`backend = "c"`: 6.9, S77, S78, `spec/stdlib.md` S45, `spec/cabi.md` Section 6), Python and Node adapters in Milestones 3 and 4 |
| runtime task groups, cancellation, `async`/`await` | reserved words only (D4); the cancellation that a C host may request from Milestone 2 is a request of the host, not a language construct, and no CPL code observes it (S75, `spec/cabi.md` Section 4) |
| verification beyond the decided fragment: proof reuse, named assumptions and their admission by project policy (per dependency, never for a declaration's own `@prove` obligations) | the verifier of `spec/verification.md` decides linear integer arithmetic over the HIR and reports `unknown` with a reason outside it (S36); `assumptions: []` in every receipt (S59) |
| static elimination of checked contracts | none beyond the refinement proof rule (3.5) |
| ghost assertions, `lemma`, mathematical quantifiers beyond `forall` over lists | reserved words only |
| capability handles as values, aliasing analysis | forbidden (S27) |
| opaque handles to foreign objects held by CPL code (design lines 467, 480, 504) | none in edition 0.1: a foreign object lives in its adapter, as the one `[state]` of a grant or created and destroyed inside one call, and CPL code reaches it only through the capability handle of its grant (`spec/cabi.md` 6.3, C39); a CPL type for foreign handles is left to a later edition |
| closures, function values, user-defined methods, traits/bounds | none (S11, S22) |
| dependent property generators, user-defined generators, `Gen<T>` as a value | none (S14, S43) |
| capability test doubles / recording adapters | none (S30), in Milestone 2 too (S78) |
| semantic index, `cpl inspect`, semantic diff, `cpl work check`, patch validation | specified in `spec/toolchain.md` (Sections 5, 6, 9, 10) over the syntactic facet model and delivered in Milestone 1; `.work` execution and integration (`cpl work run`, `cpl work integrate`) are Milestone 6 |
| `cpl.lock`, dependency resolution, external packages | `cpl.lock` is written on request and checked under `--release` (`spec/toolchain.md` 11.2); no external packages; dependency revisions are content hashes (S48) |
| `Text` ordering, normalization, grapheme operations | none (S18) |
| destructuring `let`, struct/list/or-patterns, guards | none (D25, D33) |

## 11. Acceptance corpus expectations

| Project | `cpl check` | `cpl check --release` | `cpl test` |
| - | - | - | - |
| `examples/reader` | succeeds: no errors; `verification_result` (I, `unknown`) for `paginate`; `module_file_misplaced` nowhere (files are at conventional paths); the literal `2` in `paginate([1, 2, 3], 2)` and `paginate(items, 2)` is proved `Positive` by 3.5; `size` from `gen.positive_int` is `reader.pages.Positive` by S55; the `work/` files parse (S53) | fails with one `verification_unsatisfied` for `reader.pages.paginate` (`@prove`, `unknown`) | both tests are `tested`; the property passes 100 cases; three `test_result` records with `status: "tested"`, each with a `receipt` whose `modules` include `reader.pages` and `tests.pagination` |
| `examples/holes` | succeeds with two `hole` reports: `build_pages` (`expected_type: List<List<T>>`, bindings `items: List<T>`, `size: Positive`, `ensures` × 3) and `summary` (`expected_type: Text`, bindings `page: Page`); plus `verification_result` `unknown` for `paginate` | fails with two `hole_in_release` and one `verification_unsatisfied` | no tests |

`cpl run` (S39; argument syntax per `spec/toolchain.md`): running
`holes.pages.summarize` on any `Page` value raises `unresolved_hole`
(`hole: "summary"`), while `holes.pages.paginate` cannot be run at all
(`cannot_run_generic`); running `reader.pages.divide_exactly` on `7` and
`0` raises `contract_violation` (`clause_kind: "requires"`, `attributed_to:
"caller"`, no `caller`, `clause: "b != 0"`, `input: {"a": 7, "b": 0}`);
running `reader.pages.paginate_ints` on `[1, 2, 3]` and `2` returns the
value `Ok([[1, 2], [3]])`, and on `[1, 2, 3]` and `0` returns
`Err(ConstraintError { type_name: "reader.pages.Positive", message: "0 does
not satisfy self > 0" })` (6.10), a value and not a fault.

Milestone 2 changes none of these results. `reader.pages.load_page` and
`reader.pages.load_text` remain `cannot_run_effectful`, since no
`[bindings.*]` table binds `reader.pages.PageRead`, and so does
`reader.images.image_width`, whose only binding has backend `python`, which
arrives in Milestone 3 (S78).

## Appendix A: decision index

| Id | Rule |
| - | - |
| S1 | declaration and file order irrelevant; cyclic module imports allowed |
| S2 | identity = `@id` or the qualified name as provisional identity |
| S3 | one item namespace plus one test namespace per module |
| S4 | public contracts may call only public and prelude functions |
| S5 | imports are file-scoped |
| S6 | prelude names cannot be redefined at module level |
| S7 | `infinite_type` = a cycle in the containment graph not broken by `List`, `Option`, `Result` |
| S8 | no shadowing of parameters or enclosing locals within a body |
| S9 | `result` only in `ensures`, `self` only in `where` |
| S10 | constructor resolution: expected type, then lexical candidates, qualified always |
| S11 | functions are not values; callees are name chains or member calls on any receiver expression |
| S12 | user generic structs and enums are supported |
| S13 | type arguments, fields, payloads, bindings must be value types; `==` on every `T` |
| S14 | `Gen<T>` is hermetic |
| S15 | refinement subtyping only at the top level; constructors invariant |
| S16 | compile-time refinement proof by constant evaluation |
| S17 | `==` requires `erase`-identical operand types |
| S18 | ordering comparisons on `Int` only |
| S19 | expected type flows into literal-like forms first, out of calls last |
| S20 | first element / branch sets the type when nothing is expected |
| S21 | `++` on `List`, `Text`, `Bytes` |
| S22 | member tables are closed |
| S23 | wrappers have no payload member; unwrap by match |
| S24 | syntactic divergence; `unreachable_statement` warning |
| S25 | constructor sets for every scrutinee type; `Bool` exhaustive by `true`/`false`; `Int`/`Text` literals never exhaustive |
| S26 | effect bounds explicit for all functions; absent or empty means pure |
| S27 | capability handles cannot be stored or aliased |
| S28 | `using` arguments positional, bare parameter names |
| S29 | unused effects not diagnosed; unused capabilities warned |
| S30 | tests and properties are pure |
| S31 | capability operation contracts: `requires` by caller, `ensures` attributed to the binding (`attributed_to`, S57) |
| S32 | contract sub-blocks admit only `let` and a tail |
| S33 | direct self-call in a contract is an error |
| S34 | a faulting contract clause reports its own fault |
| S35 | function `decreases` evaluated after the preconditions and compared with the nearest enclosing activation |
| S36 | `@prove`: checked at runtime, status from the verifier (`spec/verification.md`), rejected by release unless `proved`; dependency assumptions are Milestone 5 |
| S37 | truncating division; remainder has the dividend's sign |
| S38 | explicit call-depth limit `[run] max_call_depth = 10000` |
| S39 | `cpl run` evaluates one non-generic, handle-free function on supplied arguments (handles from bindings from Milestone 2, S78); syntax and encoding are toolchain.md's |
| S40 | canonical value text and value JSON encodings |
| S41 | `assert` has no message; carries clause and bindings |
| S42 | test identity `module."name"`; single tests are selected only by command-line filters (D69); empty selection is a configuration error |
| S43 | property `for` ranges are independent |
| S44 | 100 cases, random recorded run seed, 32-byte blake3-derived property seed, ChaCha20 stream, size ramp |
| S45 | greedy coordinate-wise shrinking, same fault code, bounded |
| S46 | `test_result` fields (`tested`/`failed`, `receipt`); `contract_counterexample` for body-attributed violations |
| S47 | development reports holes as `info`; release rejects holes in the linked program (tests excluded) and unsatisfied policies |
| S48 | hole report fields and the `dependencies` object |
| S49 | diagnostic envelope, `category` set, `symbol`/`qualified_name` |
| S50 | `status` values `proved`/`failed`/`unknown` (plus `conditional` from Milestone 5) and `tested`/`failed`; `code` discriminates `failed` |
| S51 | Milestone 1 runs on the reference evaluator; rules bind every runtime; the native backend must equal it (S75) |
| S52 | a parse failure in any unit stops resolution and typing |
| S53 | `cpl check` parses `work/` files for syntax only |
| S54 | `gen` is the prelude's alias of module `std.gen`; generator properties |
| S55 | refinement identity is structural (same base, same resolved predicate) |
| S56 | check-mode positions (holes take their type) versus infer-only positions |
| S57 | `contract_violation` has `facet: "contract"` and `attributed_to` (also a capability result's `refinement_violation`, S77) |
| S58 | builtins check their `requires`, trust their `ensures`; `check<R>` evaluation |
| S59 | `receipt` on every `test_result` and `verification_result` |
| S60 | this document owns the diagnostic contract; toolchain.md owns CLI, config, receipts |
| S61 | `@builtin` declarations are exempt from `recursive_contract`; their self-referencing `ensures` are axioms |
| S62 | error type: no cascades from a diagnosed operand; position checks (generators, handles) still fire |
| S63 | primary spans: expression, clause, declaration name or header part, later duplicate with a note; span-less only without a source location |
| S64 | one report per hole: an error hole has no `hole` beside it; `hole_in_release` replaces `hole` |
| S65 | two distinct refinements of one root base: `cannot_prove_refinement`, never `type_mismatch` |
| S66 | `cyclic_type_definition` on every declaration of the cycle |
| S67 | `unused_binding` for unread `let`/`var`/pattern bindings; assignment is not a read; parameters and binders exempt |
| S68 | `unused_value` for the statement's own expression; a non-`Unit` branch tail of an `if`/`match` statement is `type_mismatch` |
| S69 | a hole whose expected type is erroneous is still reported once, as `hole` or `hole_in_release`, its `expected_type` rendering the error type as `?` (as every rendering of a type does) |
| S70 | nested contract expressions: the innermost clause a fault escapes reports it; a callee's violation is claimed by the calling clause; S34 rewrites only `symbol`, `facet`, `clause` |
| S71 | a fault while a property's `for` range evaluates is reported per S34 with the range's text; `gen.range`'s own `lo <= hi` keeps the builtin shape |
| S72 | the call-depth limit counts CPL function activations only; values may nest arbitrarily and are handled without host recursion |
| S73 | `e?` in a function whose declared return type is a refinement is `cannot_prove_refinement`: a propagated `Err` would leave at the refinement unproved |
| S74 | an argument's refined positions are checked siblings in value order, the positions inside a refinement's base before the refinement, each chain root first; a refinement over a refinement is one position |
| S75 | native execution equals the evaluator: the same value JSON and value text, byte-identical fault diagnostics and test records; sanctioned divergences only for whether and where the stack guard trips or a time budget's deadline is observed (and what follows from it), memory exhaustion, crashes in foreign code, `duration_ms`, the engine keys, and a native receipt's digests |
| S76 | native memory exhaustion is `resource_exhaustion` with `resource: "memory"`, `limit` the heap budget in bytes or 0, no span, a possibly empty backtrace; the evaluator aborts, except for a failure its adapter host reports |
| S77 | every Milestone 2 binding's results are validated (shape, then the refinements in S74 order, before `ensures`): `InvalidResult` for a foreign-error return type, else `refinement_violation` (`attributed_to: "binding"`) or `internal_error`; the Milestone 1 host API stays trusted |
| S78 | `cpl run` binds `using` parameters from the one usable `[bindings.*]` table per capability, in parameter order, before the argument checks, granting each then in either engine; the command line is the granting host; tests bind nothing |
| S79 | an evaluation that a host starts during an operation is a fresh top-level evaluation; the outer one is never continued |
| S80 | `internal_error` kinds `binding_result_ill_typed`, `binding_error_unrepresentable`, `binding_protocol`, `binding_host_failed`, with `operation`, `binding`, `pointer`, and a `detail` of fixed templates; each spans the operation call |
| S81 | C export arguments are checked as `cpl run` arguments: an ill-typed one is a C API usage error, a false refinement the fault `refinement_violation` |
| S82 | `cpl run` and fixtures grant Python bindings in workers; a worker is an adapter host for S80; a malformed worker answer is `binding_protocol` |
| S83 | tests and properties may declare `using` parameters bound to fixtures (`[test.fixtures]`), granted per test and per property; `test_fixture_missing`, `test_fixture_failed` |
