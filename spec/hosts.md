# CPL source-level hosts: Rust, Go, and the JVM (Java and Kotlin), edition 0.1

This document specifies `cpl build --target rust|go|java`: CPL compiled to the
source of a host language, so that the host calls CPL the way Kotlin calls
Java. The generated package is ordinary source of the host language. A CPL
function is a host function, a CPL struct or enum is a host type, a
capability is a host trait or interface the host implements, and a call
from the host into CPL is a direct call in the host's own calling
convention: no foreign-function boundary, no marshalling, no handle table,
and the host compiler inlines across the boundary. Kotlin uses the Java
package through its Java interop, as it uses any Java library.

It adds to `spec/toolchain.md` (the `build` command and `[interop] targets`)
and comes after `python.md` in the precedence order of `spec/README.md`. The
implementation is the crate `cpl-emit-host`. Decisions are numbered `H1`..`H13`.

## 0. Scope

| Term | Meaning |
| - | - |
| *target* | `rust`, `go`, or `java` (`kotlin` and `jvm` name `java`) |
| *package* | what `cpl build --target <t>` writes: a Cargo crate, a Go module, or Java sources (Section 2) |
| *runtime* | the target's runtime, written into the package as source: values, faults, the calling context, the builtins, value text, value JSON |
| *internal form* | the function a CPL function compiles to, which CPL code calls: it takes the calling context last and checks the contracts |
| *host form* | the function a host calls for a `pub fn`: it makes a calling context, checks its arguments at the boundary, and reports a fault as the host's error |
| *driver* | an optional JSON driver in the package, one request per line, which the differential tests run (Section 6) |

**H1.** The host targets compile the checked program in checked mode: every
`requires`, `ensures`, loop `invariant`, `decreases` measure, struct
invariant, and refinement checked by the evaluator is checked by the
generated code, with the same fault code, clause kind, and blamed symbol.
The reference evaluator stays the oracle (semantics 9): a disagreement is a
bug of the backend.

## 1. Building

`cpl build --target rust|go|java [--release] [--out DIR] [--json]` checks
the project (any error: exit 1, nothing is written), requires the target in
`[interop] targets` (`kotlin` enables `java`; `build_target_not_enabled`,
exit 1), and writes the package under `DIR`, by default
`.cpl/build/<target>`. Nothing is compiled: the host's own toolchain
(`cargo`, `go`, `javac`, `kotlinc`) builds the package as part of the host's
build. `--artifact`, `--sanitize`, and `--lock` do not apply (`bad_argument`).

**H2.** A file whose content is unchanged keeps its time stamp, so the host's
incremental build skips it. A file an earlier build generated that the
program no longer needs is removed: the package directory keeps the list in
`.cpl-generated`. Nothing else in the directory is touched.

`[interop] targets` accepts `"rust"`, `"go"`, `"java"`, and `"kotlin"` beside
the existing backends. `[bindings.*] backend` does not: a host target calls
CPL, and capabilities reach CPL as the host's own trait or interface
implementations (Section 3.4), so no binding is needed.

The build report (`--json`) is the `build` envelope with `target` the host
target, `profile` `"source"`, one `source` artifact per file with its
blake3 digest, and one export per `pub fn`.

## 2. Packages

| Target | Package | The program's modules | Runtime |
| - | - | - | - |
| `rust` | a Cargo crate named after the project, with an empty `[workspace]` table so that it builds standalone | nested `pub mod`s (`shop.pricing` is `pricing::shop::pricing`) | `src/cpl.rs`, the module `cpl` |
| `go` | a Go module named after the project, the program in its root package | one package; one file per CPL module (`shop.pricing.cpl.go`); a name two modules share is qualified by its module path | the package `<module>/cpl` |
| `java` | sources under `src/`, the base package named after the project | one Java package per CPL module (`pricing.shop.pricing`); its functions are static methods of one final class named after the module's last segment (`Pricing`) | the package `<base>.cpl`; Java 17 or later |

**H3.** Only the program's own declarations and the standard library's
refinements the program reaches are generated: its `check<R>` calls, the
refinements in its declared types, and their refinement chains.

## 3. The mapping

### 3.1 Values

| CPL | Rust | Go | Java / Kotlin |
| - | - | - | - |
| `Int` | `cpl::Int` | `cpl.Int` | `cpl.Int` (Kotlin operators: `+ - * < >`, unary `-`) |
| `Bool` | `bool` | `bool` | `boolean` (`Boolean` as a type argument) |
| `Text` | `cpl::Text` (shared `str`) | `string` | `String` |
| `Bytes` | `cpl::Bytes` | `cpl.Bytes` | `cpl.Bytes` |
| `List<T>` | `cpl::List<T>` | `cpl.List[T]` | `cpl.List<T>` (`Iterable<T>`) |
| `Option<T>`, `Result<T, E>` | `Option<T>`, `Result<T, E>` | `cpl.Option[T]`, `cpl.Result[T, E]` | sealed `cpl.Option<T>` (`Some`, `None`), `cpl.Result<T, E>` (`Ok`, `Err`) |
| `Unit` | `()` | `cpl.Unit` | `cpl.Unit.UNIT` |
| `ConstraintError`, `ForeignError` | runtime structs | runtime structs | runtime records |
| a refinement `R` of `T` | `T` | `T` | `T` |

**H4.** `Int` is arbitrary precision and holds a machine integer while the
value fits: arithmetic is a machine operation with an overflow check, and
only a result that does not fit allocates. Division and remainder truncate
(semantics 6.3).

**H5.** Values are immutable. A list is a view of a shared buffer:
`take`, `drop`, and `drop_last` are views and copy nothing, so walking a
list with `first()` and `drop(1)` is linear. Appending to a list that ends
where its buffer's claimed elements end happens in place (in Go and Java
the claim is an atomic high-water mark, in Rust the buffer's only
reference), so a loop that builds a list with `xs = xs.push(v)` or
`xs = xs ++ ys` is linear.

`Text` holds Unicode scalar values. A Go string may hold any bytes and a
Java string unpaired surrogates, so Go and Java check every text that
enters CPL: a host form refuses an argument holding a text that is not
Unicode, at any depth (in a list, a field, a payload), as a
`refinement_violation`, and what a capability returns is checked the same
way (H13), including the texts of a `ForeignError` it returns. Rust's
`str` is Unicode already and is not checked.

### 3.2 Declarations

| CPL | Rust | Go | Java / Kotlin |
| - | - | - | - |
| `struct P { x: Int }` | `pub struct P { pub x: Int }` deriving `Clone`, `PartialEq`, `Eq`, `Hash`, `Debug` | `type P struct { X cpl.Int }` | `record P(Int x)` |
| `enum E { A, B(Int) }` | `pub enum E { A, B(Int) }` | `type E interface`, with `E_A{}` and `E_B{F0}` | `sealed interface E` with `record A()` and `record B(Int f0)` |
| `type W(T)` | `pub struct W(pub T)` | `type W struct { V T }` | `record W(T value)` |
| generics | type parameters bounded by `cpl::Value` | type parameters `any` | type parameters |

**H6.** A field, payload, or wrapped value whose type contains its own
declaration by value is held by reference where the host needs one:
`cpl::Boxed` in Rust (`enum Tree { Node(cpl::Boxed<Tree>) }`), a shared
reference like `Arc` whose drop does not recurse (a value nested a million
levels deep, built by a CPL loop, is dropped on any stack: past 128 levels
the rest is queued; `cpl::Boxed::opt` holds an `Option`'s `None` with no
allocation, as the generated code does for a held field or payload of an
`Option` type, so a tree's leaves cost nothing), a pointer in Go for a
struct (a Go enum is an interface, a reference already). Java needs none.

Every generated type has structural equality, CPL value text
(semantics S40: `toString` in Java, `ValueText` in Rust, `cpl.ValueText` in
Go), and value JSON (toolchain 7.3).

### 3.3 Functions

A function `f` compiles to an internal form (`f__x` in Rust, `x_f` in Go
and Java) that takes the calling context last. A `pub fn` also has a host
form:

| Target | Host form | A fault is |
| - | - | - |
| `rust` | `pub fn f(args) -> Result<T, cpl::Fault>` | the `Err` |
| `go` | `func F(args) (T, error)` | the `error`, a `*cpl.Fault` |
| `java` | `public static T f(args)` (camelCase) | a thrown `cpl.Fault`, an unchecked exception without a stack trace |

**H7.** Faults are the host's cheapest escape: a `Result` with `?` in Rust,
a panic recovered only by the host form in Go, and an exception in Java.
The path without a fault pays nothing for them.

The host form checks the refinements and struct invariants reachable in its
arguments (semantics S39, S81) and refuses an argument outside its type as
`refinement_violation`, as the evaluator's boundary does. A generic `pub fn`
is called with the host's own type arguments: a parameter whose type holds
a type parameter is not checked (its check would need the type argument),
and every other parameter is, as in any host form. The internal forms are
not the host's to call: `pub(crate)` in Rust and unexported in Go; Java has
no visibility across packages short of a module system, so its internal
forms are public by necessity, named `x_` and `b_` and documented as
internal. CPL has no null: a Java `null` or a Go `nil` (an enum, a
capability, a recursive field) anywhere in an argument is outside its type,
and the call ends in the fault `refinement_violation` ("an argument holds
null") when the generated code reads it, never in a `NullPointerException`
or a panic that escapes the host form. Lists cross by copy: a Go `ListOf`
or `ListLit` and a Java `List.of` or `List.lit` copy the host's slice or
array, and Go's `Items` returns a copy, so a host cannot change a list
after a CPL function checked it.

### 3.4 Capabilities

A capability is a trait (Rust: `&dyn Cap` parameters) or an interface (Go,
Java) that the host implements; an operation returning `Unit` returns
nothing in the host. A host form takes the handles as its last parameters,
and the operation's `requires` and `ensures` are checked around the call
(semantics S31). In Java and Kotlin, a capability with one operation is a
functional interface: a lambda implements it.

**H13.** The host's implementation is a binding in the sense of semantics
S77: what an operation returns is validated against the refinements and
struct invariants reachable in its return type (and, in Go and Java, its
texts, Section 3.1), after the host returns and before the `ensures` run. A result that fails is, for an operation returning
`Result<T, ForeignError>`, the value `Err(ForeignError { kind:
InvalidResult, ... })` with the details `operation` and `expected_type`;
otherwise a `refinement_violation` fault blamed on the operation's identity.
(The message names the whole return type, not the failing position and
predicate that the evaluator's adapter bindings report.) A return type
without a refinement or invariant (or text, in Go and Java) costs nothing.

## 4. Faults

A fault carries the diagnostic `code` (`contract_violation`,
`assertion_failed`, `division_by_zero`, `unresolved_hole`,
`resource_exhaustion`, `refinement_violation`), the `clause_kind` of a
contract violation, the failing `clause` text, the blamed `symbol` (a stable
identity), and the message of the evaluator.

**H8.** The calling context counts activations against the call-depth limit
(semantics S38, 10000 by default) and keeps the measures of the active
recursive functions: an activation's measure exists from its entry, after
its `requires`, until its exit, after its `ensures` (S35), so that a call
from a function's own contracts is measured as the evaluator measures it.

**H9.** S34 without catching: the calling context keeps a stack of the
contract expressions being evaluated (clauses, measures, refinement and
invariant predicates). The generated code pushes the owner before it
evaluates the expression and pops it after; a clause's own failure is
raised after the pop. A fault ends the call, so when it reaches the host
form the innermost expression still on the stack is the one it escaped,
and the host form reports the fault as that expression's owner's. An
expression that cannot fault (no call, division, predicate, or builtin with
a `requires`; `Int` arithmetic never faults) gets no push and pop: S34 is
observable only through a fault.

**H10.** Rust also guards the native stack: an activation that finds the
stack nearly exhausted faults with `resource_exhaustion`. Java's host form
reports a `StackOverflowError` as `resource_exhaustion`; Go's stacks grow.

## 5. Performance

**H11.** A call from the host into CPL is a call of the host language: the
cost of the boundary is making the calling context (a small structure whose
stacks live in place, so a call that nests few clauses allocates nothing)
and checking the arguments of refined types, nothing else. CPL code calls
its internal forms directly, and the host compiler may inline them.
Contract checks cost what the expressions in them cost. In Rust, a
variable or field that an operator, a comparison, or a reading builtin
(`len`, `first`, `get`, `take`, `drop`, `contains`, ...) uses is borrowed,
not cloned. A variable is moved, not cloned, where it is read for the last
time (a liveness analysis of the body: the statement reads it once, or
reads fields of it that do not overlap), a match arm after which the
scrutinee is dead moves its bindings out of it, and a parameter that an
`ensures` reads after the body is borrowed by the body, not copied for it.

**H14.** The Rust, Go, and Java hosts give a function of the scalar subset (the
analysis `cpl_hir::scalar` shares with the native engine, cabi.md 1.3a: no
generics or capabilities; parameters, result, and locals of `Int`, `Bool`,
or plain records of them; arithmetic, comparisons, `if`, `match` on
literals and bindings, `while`, `assert`, contracts, a `decreases` on a
function that is its own recursive group, and calls of such functions;
and `Text` (records with `Text` fields included), enums whose payloads are
`Int` and `Bool`, lists of `Int`, `Bool`, records, and lists of `Int` and
`Bool`, and options of a non-list element: everything of cabi.md 1.3a but
its words (`cpl_hir::scalar::Caps::HOSTS`), a list of lists' element read
by `get`, `first`, or `last` and then `unwrap_or`) a *speculative form*
on machine integers: `f__q`
in Rust (an `Option` of an `i64`, a `bool`, a `cpl::Text`, the generic
form's own list, or a struct or enum of the module `__q`, a record that
holds texts being `Clone` and cloned where it is copied), `q_f` in Go (the
value and an `ok`; texts are strings, options `cpl.QOpt`), `q_f` in Java
(a `long`, `boolean`, `String`, list, or record `Support.QR<n>` or
`Support.QE<n>`, an option being a pair of locals, bailing by throwing `cpl.Q.BAIL` or
the `ArithmeticException` of `Math.addExact` and its kin, caught by the
internal form with a `StackOverflowError`). Every operation that could do
anything but compute the same value bails: an `i64` overflow (checked
arithmetic, `MIN / -1` included), a zero divisor, a contract, assertion, or
measure that does not hold, a literal past `i64`, the depth limit, and in
Rust the native stack's guard (checked every 16 levels). The internal form
tries the speculative one first when every argument fits (an `Int` that is
not a bigint, a record whose `Int` fields are not) and, on a bail, runs
itself from the start: the function is pure, so it computes what it always
did and raises what it always raised. The measure handed to a speculative
call is that of the nearest generic activation of the same function, as
`MeasureSet` would compare (none while it is evaluating its `requires`),
and within the speculative calls the caller's own. After 16 bails in one
host call, the speculative forms are no longer tried in it. A list local
that never leaves its function (`cpl_hir::scalar::flat_locals`: assigned
only `[]` and its own `push`, read only by `len`, `is_empty`, `get`,
`first`, and `last`) is a plain array of its speculative elements in each
host: a Rust `Vec`, a Go slice, a Java array with a count (grown by
doubling; a bail past 2^30 elements). A list local *built* that way and
then used as a list (`cpl_hir::scalar::built_locals`: declared at the top
of the body, flat by those rules in every top-level statement before some
statement `k`, the body's value counting as the last, and never written
from `k` on) is held the same way until `k`, and made the generic form's
list once, just before `k` (a loop that builds a list and returns it pays
no count or claim per push). Rust holds such a list of `Int` as a `Vec` of
`cpl::Int`, which becomes the list without a copy; Go and Java hold
`int64`s and `long`s (no pointer per element) and convert them once.

A text variable that its function appends to (`t = t ++ a ++ b`, in the
generic forms and the speculative ones) is appended in place in every host: Rust moves the `String` and pushes onto
it, Go appends to a `cpl.TextBuf` whose bytes the string shares, and Java
appends to a `cpl.TextBuf` builder whose `String` is made when the
variable is read; so a loop of appends is linear, not quadratic.

## 6. Differential testing

The JSON driver (`Options::driver`) answers one request per line,
`{"function": "<qualified name>", "args": [<value JSON>...]}`, with
`{"value": ...}`, `{"fault": {"code", "clause_kind"?, "symbol"}}`, or
`{"outside": ...}` for an argument outside the function's domain.

**H12.** Each backend is tested against the evaluator on random programs of
`cpl_eval::testing::programs` and on the differential corpora with swept
inputs of every type (`cargo test -p cpl-emit-host --test hosts`, and the
`host_diff` example for longer runs): values must be equal as value JSON,
and faults must agree on code, clause kind, and blamed symbol. Resource
exhaustion is not compared. A toolchain that is not installed skips its
part.

The example `examples/hosts` is one CPL module called from a Rust, a Go, a
Java, and a Kotlin program that print the same lines.
