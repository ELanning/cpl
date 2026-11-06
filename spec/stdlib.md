# CPL standard library, edition 0.1 (Milestones 1 and 2)

This document specifies the prelude and the standard library surface that
the CPL 0.1 toolchain ships for Milestones 1 and 2; Milestone 2 changes no
signature, contract, or stub of it. The Milestone 1 runtime is the
*reference evaluator*, a tree-walking interpreter over a typed IR; that is
`semantics.md` S51's deliberate departure from the design's "native
execution" deliverable (design, Implementation order, Milestone 1, and
Architecture, "The first compilation target is native executables"), not a
decision of this document. From Milestone 2 the native runtime of `cabi.md`
Section 2 is a second runtime that these rules bind, and it must produce
what the evaluator produces (`semantics.md` S75); S45-S47 add what the C
boundary needs of this document: how C bindings produce `ForeignError`
values (S45), how the boundary's machine integers relate to the reserved
bounded types (S46), and the one source of the tables and texts that both
engines share (S47). Everything here binds every runtime; what is marked
"reference" describes the evaluator only and does not bind the native
backend. For every operation it gives the signature, the semantics, the
fault conditions, the complexity the implementation must meet, and the
specification (`requires`/`ensures` clauses in CPL contract syntax, plus
equational laws) that the verifier of Milestone 5 will rely on.

It is derived from `design-doc.md`, which remains authoritative for intent,
and from `spec/grammar.md`, whose productions every signature and contract
below must parse under (grammar decisions are cited as `D<n>`). Where the
design is explicit this document follows it. Where it is silent a rule is
chosen and marked with a line beginning `Decision:` that gives the rule, the
rationale, and the rejected alternative; decisions are numbered `S1`, `S2`,
... so that other specifications can cite them. `semantics.md` numbers its
own decisions `S1`, `S2`, ... as well, so a bare `S<n>` in this document
always means a decision of this document, a decision of `semantics.md` is
always written `semantics.md S<n>`, and other specifications cite this
document's decisions as `stdlib.md S<n>` (both `semantics.md` and
`toolchain.md` already do). Where the design is internally inconsistent the
chosen reading is marked `Design conflict:`. Every `Decision:` and `Design
conflict:` line also carries a project-wide stable identifier `[D-nnn]`,
assigned in document order and indexed in `spec/decisions.md`; the local
`S<n>` numbers remain the citation form. The design names a small
operation surface explicitly (Section 13.4 lists it: `trim`, `flatten`,
`check<T>`, `ConstraintError`, the six `List` members, `++` on lists, `?`
on `Result`, `gen.list`/`gen.int`/`gen.positive_int`, `ForeignError`);
everything else in this document is an addition of this document. Within a
family the design names, an operation it does not name is marked
`Extension:` in its stub (`List.get`, `Option.ok_or`,
`std.bytes.from_text`); a family the design leaves entirely to the library
(`std.text` beyond `trim`, the `Int` members, `std.bytes`, `gen.bool`,
`gen.text`, `gen.option`, `gen.range`, `Text` and `Bytes` `++`) is
introduced by a `Decision:` and is not marked member by member. Names held
for later editions are marked `Reserved:`.

Scope boundary: this document owns the names, types, contracts, and runtime
behaviour of everything under `std` and in the prelude. The typing rules of
the language itself (refinements, generics, `?`, `match`) belong to
`spec/semantics.md`; where this document depends on one of those rules the
dependency is marked `Semantics:` and collected in Section 13.2 so the two
documents can be written in parallel and reconciled by the consistency
pass. Command-line flags, receipts, and the JSON shape of diagnostics
belong to `spec/toolchain.md`; this document names only the fields the
standard library must supply.

The acceptance corpus is `examples/reader/` and `examples/holes/`: every
standard-library name those files use is defined here, and Section 12 lists
concrete evaluations that both engines must reproduce (`cpl-eval`, and from
Milestone 2 the native backend, `cabi.md` Section 3).

## 0. Conventions

### 0.1 Signatures

Signatures are written in CPL declaration syntax exactly as they appear in
the stub files of Section 11. Three annotations classify them:

| Annotation | Meaning at the use site |
| - | - |
| (none) | a module function: `trim(text)`, `std.text.trim(text)`, or `flatten(lists)` for prelude functions |
| `@method` | a member called with parentheses on a receiver, which is the first parameter: `fn take<T>(list: List<T>, n: Int)` is used as `xs.take(n)` |
| `@property` | a member read without parentheses: `fn len<T>(list: List<T>) -> Int` is used as `xs.len`; with zero parameters, a *module-level property*: `fn int() -> Gen<Int>` is used as `gen.int` (S40) |

A `@method` or `@property` function with a receiver cannot be called as a
free function (`take(xs, n)` and `len(xs)` are errors, Section 10.3), so
each operation has exactly one spelling, as D43 requires. The receiver type
is the head constructor of the first parameter's type (`List` for
`List<T>`, `Int`, `Text`, `Bytes`, `Option`, `Result`). Operators (`+`,
`++`, `==`, ...) have no stub; their typing is fixed in tables in this
document and they are evaluated by intrinsics (Section 10.6).

Decision: [D-180] S40. A `@property` function with zero parameters is a
*module-level property*, the third classification: it has no receiver, is
not entered in any member table (S37), and is exempt from the receiver-head
rule of Section 10.2; it is an ordinary `pub` item of its module, resolved
as a module-qualified chain by `semantics.md` 2.4 (`gen.int`,
`std.gen.int`), read without parentheses, and `property_called` when called
(`gen.int()`: `int is a property; write gen.int`). The four generator
properties of `std.gen` are the only module-level properties in 0.1, and
no `std.gen` item can be imported (S28), so a module-level property is
always spelled through its module. Rationale: the design writes `gen.int`
and `gen.positive_int` without parentheses, CPL has no module-level values
(D14) and no function values (D32), and a zero-parameter builtin read as a
property is the smallest reading under which the design's example is
well-formed with the D43 property/method distinction the checker already
has (`semantics.md` S54 adopts this). Rejected alternatives: a `gen` object
value (needs a type that is neither data nor a capability); module-level
`let` constants (D14).

### 0.2 Faults versus errors

| Outcome | Meaning | Examples |
| - | - | - |
| ordinary value | the operation is total on its typed inputs | `xs.len`, `trim(t)`, `xs.get(i)` |
| `Err(e)` | a recoverable, expected failure of dynamic data; always a `Result` | `check<Positive>(0)`, `parse_int("x")`, `std.bytes.to_text(b)` |
| fault | an execution fault: a violated `requires` clause of a standard operation, division by zero (S8), a failed `assert`, or resource exhaustion. Faults are not values, cannot be caught, and terminate the current `cpl test`/`cpl run` evaluation with a structured diagnostic (`semantics.md` 6.7 lists the fault codes) | `xs.take(-1)`, `a / 0`, `split(t, "")` |

Every potential fault of a standard operation is visible as a `requires`
clause in its stub, or, for the operators, in the tables of this document;
there are no hidden fault sources (Section 5, S19).
Resource exhaustion (memory) may occur in any operation that allocates and
is outside the functional-correctness claims below, as the design states.

### 0.3 Contracts and laws

Each operation's stub carries `requires` and `ensures` clauses in CPL
syntax. They are typechecked like user contracts (pure, total under the
preceding clauses, `result` names the returned value, D50) and are the
operation's specification: the verifier assumes them when checking a
caller and never re-proves them; how that reliance is recorded is S41
(Section 10.4). Where a property is not expressible as an `ensures` clause
over one call (for example `xs.take(n) ++ xs.drop(n) == xs`), it is stated
in a *laws* table. Law variables (`xs`, `ys`, `n`, `p`, ...) are
universally quantified over well-typed values; `p[x := e]` denotes
substitution in a predicate. Laws are written in CPL expression syntax so
that they can be transcribed into `property` declarations and checked by
sampling in the toolchain's own test suite (Section 10.7); that sampling is
evidence about the builtin implementations, never a proof of a law, and it
discharges nothing (design: "finite sampling does not prove a universal
statement").

Semantics: contracts in this document rely on the following rules of
`semantics.md` (collected in Section 13.2): `and`/`or` evaluate their right
operand only when needed, so `b != 0 and a % b == 0` is total; in `a == b`
and `a != b` the operand whose type is known fixes the expected type of the
other, so `option == None`, `res == Ok(n)`, and `result == []` typecheck
(grammar.md Section 5 (h) states the `[]` case); a value of a refinement
type is usable wherever its base type is expected (so `size: Positive` is a
valid `Int` argument), and the result of an operator or standard operation
is always base-typed (`size - 1` is `Int`, never `Positive`).

### 0.4 Complexity notation

`n` is the length of the receiver or first argument, `m` of the second;
`|v|` is the bit length of an `Int`. Bounds are requirements on any
conforming implementation unless marked "reference", which describes the
Milestone 1 reference evaluator only (`semantics.md` S51). Section 4.1
fixes the representation requirements for `List<T>`.

## 1. Prelude

### 1.1 Names in scope without `use`

The prelude is the module `std.prelude` (Section 11.1). Every `pub`
declaration of that module is in scope in every CPL module, plus the single
module name `gen`.

| Name | Kind | Defined |
| - | - | - |
| `Int` | opaque builtin type | Section 2 |
| `Bool` | opaque builtin type | Section 3.1 |
| `Text` | opaque builtin type | Section 3.3 |
| `Bytes` | opaque builtin type | Section 3.4 |
| `Unit` | enum with the single variant `Unit` | Section 1.2 |
| `List<T>` | opaque builtin type | Section 4 |
| `Option<T>` | `enum Option<T> { Some(T) None }` | Section 5.1 |
| `Result<T, E>` | `enum Result<T, E> { Ok(T) Err(E) }` | Section 5.2 |
| `ConstraintError` | struct | Section 1.5 |
| `Some`, `None`, `Ok`, `Err`, `Unit` | constructors (variants of the prelude enums) | D44 |
| `check<T>(value)` | intrinsic function | Section 1.4 |
| `flatten(lists)` | function | Section 1.6 |
| `gen` | the module `std.gen` | Section 8 |
| `Int8` `Int16` `Int32` `Int64` `UInt8` `UInt16` `UInt32` `UInt64` | reserved type names, unusable in 0.1 (S11) | Section 2.5 |
| `Float32` `Float64` | reserved type names, unusable in 0.1 (S11); 0.1 has no floating-point type, literal, or operator | Section 2.5 |
| `Gen` | reserved type name, unusable in user code (S11, S28) | Section 8.1 |

Members (`xs.len`, `n.abs()`, `opt.unwrap_or(d)`) are not names; they are
reachable through their receiver type wherever that type is (Section 10.3).

Design conflict: [D-181] the design says the prelude supplies "Int, Bool, Text,
Bytes, Unit, List, Option, Result, and their constructors", yet the one
module whose header it shows, `reader.pages`, imports only
`std.text.{trim}` and uses `flatten` (in `paginate`'s contract and body),
`check<Positive>`, and `ConstraintError` with no `use` declaration for
them. The examples are the executable corpus, so the prelude includes those
three names; grammar.md D57 and `semantics.md` 1.7 record the same reading.
The design shows no module header or imports for the test module that
writes `gen.list(gen.int)` (it only places those examples in
`tests.pagination`), so the examples neither show nor rule out an import
for `gen`; binding `gen` in the prelude (S1) is a decision the design's
examples make convenient, not one they force.

Decision: [D-182] S1. The prelude scope is the `pub` declarations of `std.prelude`,
the module `std.gen` under the name `gen`, and (as `semantics.md` 2.1 scope
1 states) the root segment of every module path in the program, including
`std`, so that `std.text.trim("x")` and `std.bytes.from_list([1])` resolve
with no `use` and fully qualified paths always work; `gen` is bound by the
toolchain when it loads `std`, not by a `use` line in the stub, so that no
import cycle exists between `std.prelude` and `std.gen` (which uses prelude
types). User module paths may not begin with `std` or `gen`
(`semantics.md` 1.1: `reserved_module_path`). Rationale: the design writes
`gen.list(gen.int)` in a module for which it shows no imports, so binding
`gen` everywhere lets the example stand as written, and a data-driven
prelude (the stub file is the prelude) keeps `cpl inspect std.prelude.*`
truthful. Rejected alternatives: requiring `use std.gen` in test modules
(would force an import the design's example does not show); re-exporting
through a `use std.gen` line in `std/prelude.cpl` (introduces re-export
semantics the language does not otherwise have and a module cycle).

### 1.2 Unit

`Unit` is the type with one value, declared in the prelude as
`pub enum Unit { Unit }`. The value is written `Unit`; a block with no tail
expression has value `Unit` (grammar.md Section 3.11); a function declared
`-> Unit` returns it implicitly; `Ok(Unit)` is the success value of
`Result<Unit, E>`. `Unit == Unit` is `true`. `Unit` has no members.

Decision: [D-183] S2. The unit value literal is the constructor `Unit`, spelled like
its type; there is no `()` and no lowercase `unit`. This is grammar.md D9
restated for the library: declaring `Unit` as a one-variant enum makes the
value an ordinary constructor for name resolution (D44), for patterns
(`Unit =>`), and for equality, with no special case anywhere in the
typechecker or evaluator. Rationale: `()` would introduce tuple syntax CPL
does not have, and `unit` would be the only lowercase constructor in the
language. Rejected alternatives: `()`; `unit`.

### 1.3 Shadowing and resolution

Decision: [D-184] S3. Prelude names cannot be redefined at module level: a
module-level declaration or a `use` import whose name equals a prelude name
of Section 1.1 (the reserved names of S11 included) is the error
`prelude_name_redefined` of `semantics.md` S6, which owns this rule and
which this document adopts. Only local bindings may shadow a prelude name
(`semantics.md` 2.3: `let flatten = 3` hides the function in that block,
with no diagnostic). `use std.prelude.{flatten}` names the prelude
declaration itself, so it redefines nothing: it is permitted and redundant,
with no diagnostic (`toolchain.md` 3.6 states the same). The prelude's enums
(`Unit`, `Option`, `Result`) take part in constructor resolution (D44) as
if imported by `use` in every module: a user enum with a variant named
`None` makes an unqualified `None` with no expected type ambiguous, which
D44 reports with both candidates; `Option.None` and `std.prelude.Option.None`
are always available. Rationale: a prelude name must mean one thing in
every module an agent reads, a new prelude name arrives only with a new
edition (which a project opts into through `edition` in `cpl.toml`, so
forward compatibility is not at stake), and silent shadowing of a
constructor is exactly the hazard D2 and D44 exist to prevent. Rejected
alternatives: lexical shadowing of the prelude with a warning (the rule of
an earlier draft of this document; `semantics.md` S6 rejects it, and two
documents cannot both own name resolution); letting user variants silently
win (hides the clash).

### 1.4 `check<T>`

```text
check<T>(value: strip(T)) -> Result<T, ConstraintError>    // intrinsic, no stub; strip(T) is the root base (semantics.md 3.3)
```

`check` is a typechecker intrinsic: its parameter type depends on the type
argument, which no stub declaration can express.

| Rule | Statement |
| - | - |
| type argument | `T` must name a refinement type `type T = B where p` (there are no type aliases in 0.1, D64, so nothing is resolved through). Any other `T` is the check error `check_requires_refinement` (`semantics.md` 3.7): `check requires a refinement type; Int has no predicate` / `PageId is a nominal wrapper; construct it with PageId(...)`. |
| argument type | `strip(T)` (`semantics.md` 3.3): the base `B` of the refinement; if `B` is itself a refinement, its base, recursively (the *root base*). A value already typed `T` is accepted (refinements widen to their base). |
| result | `Ok(v)` with `v` the same value as `value`, now typed `T`, when every predicate in the chain (root first) holds with `self` bound to `value`; otherwise `Err(e)` with `e.type_name` the name of the first refinement whose predicate is false, rendered by S44 (the declaration reached from `T` as the call site names it), and `e.message` describing the value and the predicate. |
| evaluation | the predicate is evaluated as an ordinary pure expression; a fault inside it (for example `self / 0`) propagates as that fault, blamed on the refinement declaration's `where` clause (`semantics.md` S34, 6.10: `symbol` the refinement, `facet` `contract`). |
| verifier | in the `Ok(v)` branch the verifier may assume `v == value` and `p[self := v]` for every predicate of the chain; in the `Err` branch it may assume the conjunction is false. |
| complexity | that of the predicate. |

Decision: [D-185] S4. `check<T>` accepts refinement types only; nominal wrappers,
base types, structs, and enums are rejected at check time. Rationale: a
wrapper is constructed by its constructor and the construction cannot
fail, so a `Result`-returning `check<PageId>` would tell a reader that
validation happened when none did; `check<Int>(n)` returning `Ok(n)`
unconditionally would be equally misleading. The design introduces `check`
specifically as the dynamic counterpart of a refinement proof. Rejected
alternative: a total `check` over all types that returns `Ok` for
non-refinements.

Semantics: the compile-time proof that lets the literal `2` be passed where
`Positive` is expected (`paginate([1, 2, 3], 2)`) is a rule of
`semantics.md`; this document only requires that it exist for literals
whose predicate constant-folds to `true`, and that any `Int` expression that
is not so proved be rejected with the hint `use check<Positive>(...)`.

### 1.5 `ConstraintError`

```text
pub struct ConstraintError {
    type_name: Text
    message: Text
}
```

| Field | Content | Stability |
| - | - | - |
| `type_name` | the name of the type whose constraint failed, rendered by S44: `reader.pages.Positive` for `check<Positive>` in `reader.pages`, `std.gen.Positive` for `check<gen.Positive>`, `Int` for `std.text.parse_int`, `Text` for `std.bytes.to_text` | stable; tests may compare it |
| `message` | one line for humans (it holds no LF or CR: value text escapes them, `semantics.md` S40, though U+0085, U+2028, and U+2029 inside a quoted value appear literally): `0 does not satisfy self > 0`, `"12a" is not a decimal integer`, `invalid UTF-8 at byte 3` | the message of `check` is fixed by `semantics.md` 6.10 (`<value text> does not satisfy <predicate text>`) and tests compare it (`semantics.md` 11, `toolchain.md` 12.3); the messages of the standard parsers (`parse_int`, `to_text`) are informative only and may change between toolchain versions |

`ConstraintError` has structural equality like every struct. User code may
construct one (`ConstraintError { type_name: "reader.pages.Size", message:
"too large" }`), which is how a hand-written parser such as the corpus's
`parse_size` can report its own constraint failures with the same type.

Decision: [D-186] S5. `ConstraintError` is a struct with exactly the two fields
above, not an enum. Rationale: it is the error type of both `check` and the
standard parsers, and both need the same two facts (which constraint, what
went wrong); callers almost never branch on it, they propagate it with `?`
or report it; and because 0.1 has no struct patterns (D33) a later edition
can add a field (for example a rendered `value: Text`) without breaking any
`match`. Rejected alternatives: an enum with variants per failure kind
(`Refinement`, `Parse`), which would force every parser to agree on a
closed set; a plain `Text` message, which would lose the type name that
diagnostics and tests need.

Decision: [D-187] S44. `type_name` is rendered by one rule: a prelude type renders
as its bare name (`Int`, `Text`, `Bytes`, `Bool`, `List`, `Option`,
`Result`, `Unit`, `ConstraintError`); every other declaration renders as
its qualified name (`semantics.md` 1.4: `reader.pages.Positive`,
`std.gen.Positive`). For `check<T>` the name is that of the refinement
declaration whose predicate was the first to fail, reached from the type
the call site names through the declared base chain: `check<Positive>(0)`
in `reader.pages` gives `reader.pages.Positive`; `check<gen.Positive>(0)`
gives `std.gen.Positive` (the two are one type under S29, but the call
site named `std.gen.Positive`); for `type Small = Positive where self < 10`
in module `m`, `check<Small>(0)` gives `reader.pages.Positive` (the root
predicate `self > 0` fails first) and `check<Small>(20)` gives `m.Small`.
Rationale: tests compare `type_name`, so it must not depend on whether the
toolchain spells prelude types through their `std.prelude` module (nobody
writes `std.prelude.Int`), and naming the declaration the program wrote at
the call site is what `semantics.md` S55 and 6.10 already require.
Rejected alternatives: `std.prelude.Int` for prelude types; always the
call-site type (would hide which predicate of a chain failed).

### 1.6 `flatten`

```text
@builtin("flatten")
pub fn flatten<T>(lists: List<List<T>>) -> List<T>
    ensures not lists.is_empty() or result == []
    ensures forall part in lists: part.len <= result.len
```

`flatten(lists)` is the concatenation of the elements of `lists` in order;
the full specification is the law set in Section 4.3 (L7). It faults only
on resource exhaustion. Complexity: O(total number of elements).

Decision: [D-188] S6. `flatten` is a prelude function only; there is no method form
`lists.flatten()`. Rationale: the design writes `flatten(result)` in
contracts and `flatten(paginate(...))` in tests, and the verifier's laws are
keyed by one symbol; a second spelling would give the formatter and the
semantic index two names for one operation, which D43 avoids for `len`.
Rejected alternative: both forms.

### 1.7 Equality and ordering

| Operator | Operand types | Result | Meaning |
| - | - | - | - |
| `==` `!=` | `T`, `T` for any *data type* `T` | `Bool` | structural equality, defined below |
| `<` `<=` `>` `>=` | `Int`, `Int` only | `Bool` | numeric order |

Structural equality: `Int` numerically; `Bool` by value; `Text` as equal
sequences of Unicode scalar values (no normalization, Section 3.3); `Bytes`
bytewise; `List<T>` elementwise with equal length; enums (including
`Option`, `Result`, `Unit`) by variant and payload; structs fieldwise;
nominal wrappers by payload (`PageId("a") == PageId("a")`); refinement
values as their base. Operands of different types are a check error (no
`Int == Text`); a refinement value may be compared with its base.

Decision: [D-189] S7. Equality is defined on every data type and on nothing else; a
*data type* is any type that is neither a capability nor `Gen<_>` and
contains neither (the *value type* of `semantics.md` 3.2, S13, S14).
Capability handles have no `==` (`cpl check`: `PageRead has no equality`),
generators have no `==` and no place in the term language (S28), and type
arguments must be data types (`List<PageRead>` and `List<Gen<Int>>` are
check errors), so an unconstrained type parameter `T` always supports `==`
inside a generic body. Ordering comparisons exist only on `Int` in 0.1:
`Text` and `Bool` have no `<` (Section 3.3, S13). Rationale: effect bounds
name handles by
parameter (D23), so a handle inside a collection could never be named in an
`effects` clause; excluding capabilities from type arguments keeps that rule
sound and makes equality total over generics without a constraint
language. Rejected alternatives: an `Eq` constraint on type parameters
(there is no trait system); reference equality on handles.

## 2. Int

### 2.1 Values and literals

`Int` is the arbitrary-precision integer. Literals are decimal `INT` tokens
(D6); `-5` is unary minus applied to `5` (D48). There is no overflow and no
range check; the only limit is memory (resource exhaustion). Reference
representation: `num_bigint::BigInt`.

### 2.2 Operators

| Operator | Types | Result | Faults | Specification |
| - | - | - | - | - |
| `a + b` | `Int, Int` | `Int` | none | mathematical sum |
| `a - b` | `Int, Int` | `Int` | none | mathematical difference |
| `a * b` | `Int, Int` | `Int` | none | mathematical product |
| `a / b` | `Int, Int` | `Int` | `requires b != 0` (fault `division_by_zero`, S8) | truncating division (toward zero): `result * b + a % b == a` and `(a % b).abs() < b.abs()` |
| `a % b` | `Int, Int` | `Int` | `requires b != 0` (fault `division_by_zero`, S8) | remainder of truncating division; takes the sign of the dividend `a` or is `0`: `a % b == 0 or ((a % b > 0) == (a > 0))` |
| `-a` | `Int` | `Int` | none | negation |
| `a == b` `a != b` | `Int, Int` | `Bool` | none | numeric equality |
| `a < b` `a <= b` `a > b` `a >= b` | `Int, Int` | `Bool` | none | numeric order; comparisons do not chain (write `a <= b and b <= c`) |

Worked values (also in Section 12):

| `a` | `b` | `a / b` | `a % b` |
| - | - | - | - |
| 7 | 2 | 3 | 1 |
| -7 | 2 | -3 | -1 |
| 7 | -2 | -3 | 1 |
| -7 | -2 | 3 | -1 |
| 6 | 3 | 2 | 0 |
| 0 | 5 | 0 | 0 |

Decision: [D-190] S8. `/` truncates toward zero and `%` takes the sign of the
dividend (C99, Java, JavaScript, Rust, Go semantics); `a / 0` and `a % 0`
are the fault `division_by_zero` of `semantics.md` 6.3 and 6.7 (category
`runtime`, `symbol` the enclosing declaration, `input: {"a": ..., "b": 0}`),
not a `contract_violation`: the operators are intrinsics with no stub
(Section 10.6), so there is no declaration whose `requires` clause could be
blamed. The row `requires b != 0` in the table above is the verifier's
obligation: under `@prove` every `/` and `%` generates the proof obligation
`b != 0` exactly as a callee's precondition would, and the design's
`divide_exactly` discharges it by its own `requires b != 0`. Rationale: the
design's `divide_exactly` writes `requires b != 0` before `requires a % b
== 0` and explains that sequential checking "makes the remainder expression
safe after the nonzero check", so division by zero must be a fault the
caller is obliged to exclude, not an `Err` or a defined value; a distinct
fault code lets tests and agents recognise it without parsing a clause
text, and `semantics.md` S37 and `toolchain.md` T9 already use it.
Truncation is what C, JavaScript (`BigInt`), and Rust compute natively, and
CPL computes `/` and `%` in CPL semantics regardless of the host's own
division (Python's `//` and `%` floor), so every wrapper marshals integer
results unchanged. Semantics: `semantics.md` states the same division rule
(S37) and makes every `/` and `%` generate the proof obligation `b !=
0` under `@prove` (its 5.1 does so from Milestone 5). Rejected alternatives: floored division (Python; differs
from the C and JavaScript hosts for negative operands); Euclidean division;
`Option`-returning division (would make `a / b` unusable in contracts); a
`contract_violation` blamed on a synthetic symbol `std.prelude.Int.div`
(the rule of an earlier draft of this document; that symbol names no
declaration that `cpl inspect` or a selector could resolve).

Complexity: `+`, `-`, comparisons O(|a| + |b|); `*` O(|a| * |b|) reference;
`/`, `%` O(|a| * |b|) reference.

### 2.3 Methods

```text
@method
@builtin("int_to_text")
pub fn to_text(value: Int) -> Text
    ensures result.len >= 1

@method
@builtin("int_abs")
pub fn abs(value: Int) -> Int
    ensures result >= 0
    ensures result == value or result == -value

@method
@builtin("int_min")
pub fn min(value: Int, other: Int) -> Int
    ensures result == value or result == other
    ensures result <= value and result <= other

@method
@builtin("int_max")
pub fn max(value: Int, other: Int) -> Int
    ensures result == value or result == other
    ensures result >= value and result >= other
```

| Member | Semantics | Faults | Complexity |
| - | - | - | - |
| `n.to_text()` | decimal rendering: optional leading `-`, then digits with no leading zeros (`0` for zero), no `+`, no separators, ASCII only; `parse_int(n.to_text()) == Ok(n)` (law I3) | none | O(\|n\|^2) reference |
| `n.abs()` | absolute value | none | O(\|n\|) |
| `a.min(b)` | the smaller operand (`a` when equal) | none | O(\|a\| + \|b\|) |
| `a.max(b)` | the larger operand (`a` when equal) | none | O(\|a\| + \|b\|) |

Decision: [D-191] S9. `min` and `max` are `Int` methods (`a.min(b)`), not prelude
functions. Rationale: `Int`'s other operations are methods, a postfix chain
reads left to right in long contract expressions (`(list.len - 1).max(0)`),
and two more free names in the prelude would collide with the `min`/`max`
functions user modules commonly write over their own types. Rejected
alternatives: prelude functions `min(a, b)`/`max(a, b)`; `std.int.min`.

Decision: [D-192] S10. `to_text()` is the only `Int` formatting operation and
produces exactly the canonical literal form (what the formatter emits for
the same value, with `-` for negatives). `std.text.from_int(n)` is defined
as equal to `n.to_text()` (Section 6) so that `std.text` contains the
parse/format pair together. Rejected alternative: radix or padding
parameters (deferred to a later `std.int`).

Laws (Section 13 index I1-I5):

| Id | Law |
| - | - |
| I1 | `b == 0 or (a / b) * b + a % b == a` |
| I2 | `b == 0 or (a % b).abs() < b.abs()` |
| I3 | `parse_int(n.to_text()) == Ok(n)` |
| I4 | `a.min(b) == b.min(a)` and `a.max(b) == b.max(a)` and `a.min(b) + a.max(b) == a + b` |
| I5 | `n.abs() == n.max(-n)` |

### 2.4 Parsing from text

Parsing is `std.text.parse_int(text) -> Result<Int, ConstraintError>`
(Section 6, S25); the corpus's `parse_size` glue delegates to it. There is
no `Int.parse` member and no prelude `parse` function: a parser takes `Text`
and belongs to `std.text`.

### 2.5 Reserved bounded integer types

The design states that "bounded machine integers are distinct types with
explicit, checked conversions" (Core syntax), that a C export may select "a
checked conversion to a machine type" (C), and that `Int` maps to a
"managed integer handle or checked machine integer" at the C interface
(Value mappings). It gives those types no names, literals, conversion
operations, or overflow semantics, and they are first needed by the C
boundary of Milestone 2, which meets that need with checked conversions of
its binding layer instead (S46). This document narrows the 0.1 Milestone 1
integer surface to `Int` alone; that is a scope reduction made here, not a
reading of a design conflict.

Decision: [D-193] S11. 0.1 Milestone 1 has exactly one integer type, `Int`, and no
floating-point type, literal, or operator (the lexer rejects `1.5`,
grammar.md 1.5). The prelude reserves the type names `Int8`, `Int16`,
`Int32`, `Int64`, `UInt8`, `UInt16`, `UInt32`, `UInt64`, `Float32`,
`Float64`, and `Gen` (S28): in 0.1 they have no values, literals,
operators, or members; using one in a type position is the check error
`unresolved_type` (`semantics.md` 9.4) with the note `Int64 is reserved for
a later edition` or `Gen is not nameable in edition 0.1`, and declaring a
`type`, `struct`, `enum`, or `capability` with one of
these names is `prelude_name_redefined` (S3) with the note `reserved for a
later edition` (for `Gen`, a note pointing at `std.gen`'s declaration of
it: `Gen` is reserved because it is not nameable, not for a later edition);
a type parameter may still take any of these names, which it then hides
within its declaration (`semantics.md` 2.3). The ten numeric names are not declared in the stub files
(Section 11); they are a fixed list in the std loader of `cpl-sema`
alongside the stubs, and this decision is its normative source. `Gen` is
reserved in the same sense for user code (not declarable, not importable,
not nameable in a type position, S28) but *is* declared, as the
`@builtin("gen")` type that the `std.gen` signatures need (8.5, 11.5); the
reservation is a rule about user modules, not about the std unit. Planned
surface, not implemented and not binding: `Int64.from_int(n) ->
Result<Int64, ConstraintError>` and `x.to_int() -> Int` as the explicit,
checked conversions the design requires, checked `+ - * / %` that fault on
overflow, wrapping variants under explicit names, and `Float64` with the
NaN and signed-zero contract semantics the design says must be specified
before any floating-point API is exposed (Value mappings; Open
implementation decisions). Rationale: the design specifies none of the
literal, conversion, or overflow semantics these types need and first needs
them at the C boundary, so defining them now would fix semantics the
boundary design has not yet constrained, while reserving the spellings
means no user type `Int32` or `Float64` must be renamed when they land;
unusable types that typecheck would let signatures name types the C
generator cannot yet honour. Rejected alternatives: defining the eight
bounded types with conversions in 0.1 (would fix overflow and conversion
rules before the boundary that motivates them exists); reserving nothing
(`semantics.md` Section 10 reserves no names of its own and leaves the
names to this document).

Decision: [D-285] S46. The bounded integer types stay reserved in Milestone
2, and S11 is unchanged: the machine integers of the C boundary are not
language types. A C export may select, per entry point, a checked
conversion of its top-level `Int` positions (refinements of `Int` included)
to `int64`, `int32`, `uint64`, or `uint32` (`[interop.c] exports`,
`toolchain.md` 11.1; `cabi.md` Section 5), and a declarative C binding
states the C scalar type of each argument and result (`int8` to `uint64`,
the C `int`, and `bool`, `cabi.md` Section 6). These conversions belong to
the binding layer, outside CPL: an export's input always fits an `Int`; an
export's result outside its selected type is a status of the C API
(`interop_int_out_of_range`, `cabi.md` Section 4), never a fault and never
a CPL value, and the handle form of the same function still returns it; an
argument that a binding cannot convert to its C type is `ForeignError {
kind: InvalidArgument }` (S45); a C integer result becomes an `Int` and a C
`bool` result a `Bool` (a value other than 0 or 1 is `InvalidResult`, S45),
which `semantics.md` S77 then validates against the operation's declared
type. No CPL source names a machine type, and neither the prelude nor any
`std` module changes. Rationale: the design's "checked conversion to a
machine type" (C, line 469) and its mapping of `Int` to a "managed integer
handle or checked machine integer" (Value mappings, line 495) are needs of
the boundary and are met there, while the language types the design also
announces ("Bounded machine integers are distinct types with explicit,
checked conversions", Core syntax, line 73) need literal, overflow, and
conversion rules that no Milestone 2 consumer constrains, which is S11's
own argument for deferring them. Rejected alternatives: defining `Int64`
and its siblings in Milestone 2 (would fix overflow and literal semantics
for a boundary that does not need them in CPL source); mapping `Int` to
`int64_t` by default at the boundary (silently narrows, where the design
requires an explicit selection).

## 3. Bool, Unit, Text, Bytes

### 3.1 Bool

| Form | Types | Result | Semantics |
| - | - | - | - |
| `true`, `false` | | `Bool` | keyword literals |
| `not a` | `Bool` | `Bool` | negation; binds tighter than comparison (`not a == b` is `(not a) == b`, grammar.md 3.12) |
| `a and b` | `Bool, Bool` | `Bool` | conjunction; `b` is evaluated only when `a` is `true` |
| `a or b` | `Bool, Bool` | `Bool` | disjunction; `b` is evaluated only when `a` is `false` |
| `a == b`, `a != b` | `Bool, Bool` | `Bool` | equality |

`Bool` has no members and no ordering (`true < false` is a check error).
Short-circuit evaluation is what makes sequential contract clauses total:
`b != 0 and a % b == 0` never evaluates `a % 0`. Semantics: `semantics.md`
6.2 specifies `and`/`or` as short-circuiting, and contract clauses are
evaluated by the same rules (its Section 5).

### 3.2 Unit

See Section 1.2. `Unit` has no members.

### 3.3 Text

A `Text` is a finite sequence of Unicode scalar values (U+0000-U+D7FF and
U+E000-U+10FFFF). It is a value: immutable, structurally compared.

| Rule | Statement |
| - | - |
| element | the Unicode scalar value, never a byte, a UTF-16 code unit, or a grapheme cluster |
| surrogates | cannot occur: the lexer rejects `\u{D800}`-`\u{DFFF}` (grammar.md 1.5) and adapters reject unpaired surrogates or apply an explicit replacement policy at the boundary (design, Value mappings); no standard operation can produce one |
| normalization | none, ever: `"\u{e9}" == "e\u{301}"` is `false`; `trim`, `split`, `==` operate on the scalar sequence as given |
| `len` | the number of scalar values: `"héllo".len == 5`, `"😀".len == 1`, `"e\u{301}".len == 2` |
| encoding | an implementation detail (the reference uses UTF-8 `String`); UTF-8 is exposed only through `std.bytes.from_text`/`to_text` (Section 3.4) and the C boundary |
| ordering | none in 0.1 (S13) |
| indexing | none in 0.1; `std.text.chars` yields the scalar values as a `List<Text>` (Section 6) |

Members and operators:

```text
@property
@builtin("text_len")
pub fn len(text: Text) -> Int
    ensures result >= 0

@method
@builtin("text_is_empty")
pub fn is_empty(text: Text) -> Bool
    ensures result == (text.len == 0)
```

| Form | Types | Result | Faults | Semantics | Complexity |
| - | - | - | - | - | - |
| `t.len` | `Text` | `Int` | none | count of scalar values | O(n) reference (UTF-8 scan); O(1) if the implementation caches it |
| `t.is_empty()` | `Text` | `Bool` | none | `t.len == 0` | O(1) |
| `a ++ b` | `Text, Text` | `Text` | none | concatenation; `(a ++ b).len == a.len + b.len` | O(n + m) |
| `a == b`, `a != b` | `Text, Text` | `Bool` | none | equal scalar sequences | O(min(n, m)) |

Decision: [D-194] S12. `++` concatenates `Text` as well as `List<T>` (and `Bytes`,
S14); `+` on `Text` is a check error with the hint `use ++ to concatenate
text`. Rationale: the design gives `++` as the concatenation operator and
no other way to build a `Text` from two; overloading `+` would make the
single arithmetic operator table of Section 2.2 type-dependent. Rejected
alternatives: `+` on `Text` (JavaScript's `1 + "2"` hazard); a
`std.text.concat(a, b)` function as the only form.

Decision: [D-195] S13. `Text` has no ordering comparison, no index operator, no
character type, and no slicing in 0.1. Rationale: scalar-value order is not
what any human expects of text ordering (collation), and sorting does not
exist in 0.1 anyway; indexing by scalar position is O(n) on the reference
representation and invites off-by-one errors that `std.text.chars` plus the
total `List` operations avoid. Rejected alternatives: lexicographic `<` by
scalar value; `t.get(i) -> Option<Text>`.

Laws (T1-T4): `(a ++ b).len == a.len + b.len`; `("" ++ a) == a` and
`(a ++ "") == a`; `(a ++ b) ++ c == a ++ (b ++ c)`; `a.is_empty() == (a == "")`.

### 3.4 Bytes

A `Bytes` is a finite sequence of 8-bit values (integers 0 to 255). There
is no `Bytes` literal (D8); values are built by `std.bytes` or arrive from
adapters.

```text
@property
@builtin("bytes_len")
pub fn len(bytes: Bytes) -> Int
    ensures result >= 0

@method
@builtin("bytes_is_empty")
pub fn is_empty(bytes: Bytes) -> Bool
    ensures result == (bytes.len == 0)
```

| Form | Types | Result | Faults | Semantics | Complexity |
| - | - | - | - | - | - |
| `b.len` | `Bytes` | `Int` | none | number of bytes | O(1) |
| `b.is_empty()` | `Bytes` | `Bool` | none | `b.len == 0` | O(1) |
| `a == b`, `a != b` | `Bytes, Bytes` | `Bool` | none | bytewise equality | O(min(n, m)) |
| `a ++ b` | `Bytes, Bytes` | `Bytes` | none | Extension: concatenation | O(n + m) |

Module `std.bytes` (stub file Section 11.3):

```text
module std.bytes

@builtin("bytes_from_list")
pub fn from_list(values: List<Int>) -> Bytes
    requires forall v in values: v >= 0 and v <= 255
    ensures result.len == values.len
    ensures to_list(result) == values

@builtin("bytes_to_list")
pub fn to_list(bytes: Bytes) -> List<Int>
    ensures result.len == bytes.len
    ensures forall v in result: v >= 0 and v <= 255
    ensures from_list(result) == bytes

/// Extension: UTF-8 encoding of a text.
@builtin("bytes_from_text")
pub fn from_text(text: Text) -> Bytes
    ensures result.len >= text.len
    ensures result.len <= 4 * text.len
    ensures to_text(result) == Ok(text)

/// Extension: strict UTF-8 decoding.
@builtin("bytes_to_text")
pub fn to_text(bytes: Bytes) -> Result<Text, ConstraintError>
```

| Function | Semantics | Faults / errors | Complexity |
| - | - | - | - |
| `from_list(values)` | the byte sequence with the given values | fault (contract violation, clause `forall v in values: v >= 0 and v <= 255`) for any value outside 0..255 | O(n) |
| `to_list(bytes)` | the values as `Int`s, in order | none | O(n) |
| `from_text(text)` | the UTF-8 encoding of the scalar sequence (1-4 bytes per scalar) | none | O(n) |
| `to_text(bytes)` | `Ok(text)` when `bytes` is well-formed UTF-8 (no overlong forms, no encoded surrogates, nothing above U+10FFFF, no truncated sequence), else `Err(ConstraintError { type_name: "Text", message: "invalid UTF-8 at byte <offset>" })`, `<offset>` being the 0-based offset of the first byte of the first ill-formed sequence (`E0 80 80` gives 0) | none (the failure is an `Err`) | O(n) |

Decision: [D-196] S14. `Bytes` is constructed from `List<Int>` by
`std.bytes.from_list`, whose range requirement is a `requires` clause (a
fault when violated), not a `Result`; the `Bytes`-to-`Text` direction is a
`Result` because malformed input is ordinary dynamic data; `++` is defined on
`Bytes`; `from_text`/`to_text` are extensions. Rationale: a byte list built
in source is checkable by the verifier (`forall v in values: v >= 0 and v <=
255` over literals constant-folds), and dynamic byte data arrives from
adapters as `Bytes` already, so a `Result` from `from_list` would only add
unwrapping; UTF-8 conversion is the one encoding every interop target
agrees on and the design's `ImageCodec` example takes `Bytes`, so a program
that reads text out of bytes needs it. Rejected alternatives: `from_list
-> Result<Bytes, ConstraintError>`; a lenient `to_text` with replacement
characters (the design requires an *explicit* replacement policy at
boundaries; it can be added later as `to_text_lossy`).

Laws (B1-B3): `to_list(from_list(vs)) == vs` under the range requirement;
`from_list(to_list(b)) == b`; `to_text(from_text(t)) == Ok(t)`.

## 4. List<T>

`List<T>` is the immutable sequence type. A list literal `[a, b, c]` has
elements of one type; `[]` takes its element type from the expected type
(grammar.md Section 5 (h)). Equality is structural (S7). Nested lists
(`List<List<T>>`) are ordinary.

### 4.1 Representation requirements

Decision: [D-197] S15. A `List<T>` value must support `len`, `is_empty()`, and
`get(i)` in O(1); every other operation in this section must run in time
linear in the sizes of its inputs and output. The reference evaluator
represents a list as a view of a shared, immutable, contiguous buffer
(`Value::List(ListRef)`, `toolchain.md` 1.7): `len` and `get` index it
directly, `take`, `drop`, and `drop_last` are views that copy nothing
(O(1)), and `push` and `++` copy (O(result)) except into a buffer the list
alone holds, where they append in place. Every implementation keeps
`take`, `drop`, and `drop_last` O(1) or within a constant of it (the native
runtime copies a result of at most 8 elements), so that walking a list with
`first()` and `drop(1)` is linear; none may use a linked representation
that makes `len` or `get` linear. Rationale: the design's `paginate` loop
calls `rest.len` and `rest.drop(size)` on every iteration; with views both
are O(1) and the loop is O(n) overall, and `get` being O(1) is what makes
the `Extension` accessors (Section 4.5) usable in loops. Rejected
alternative: a cons-list or finger-tree representation (either makes a
common operation non-constant).

Implementation note: `take(n)`/`drop(n)` receive an arbitrary-precision
`n`; clamp by comparing `n` with `list.len` *before* converting to a
machine index, so that `xs.take(10_000_000_000_000_000_000)` is `xs`, never
an overflow.

### 4.2 The design's operation set

```text
@property
@builtin("list_len")
pub fn len<T>(list: List<T>) -> Int
    ensures result >= 0

@method
@builtin("list_is_empty")
pub fn is_empty<T>(list: List<T>) -> Bool
    ensures result == (list.len == 0)

@method
@builtin("list_take")
pub fn take<T>(list: List<T>, n: Int) -> List<T>
    requires n >= 0
    ensures result.len == n.min(list.len)
    ensures result ++ list.drop(n) == list

@method
@builtin("list_drop")
pub fn drop<T>(list: List<T>, n: Int) -> List<T>
    requires n >= 0
    ensures result.len == (list.len - n).max(0)
    ensures list.take(n) ++ result == list

@method
@builtin("list_drop_last")
pub fn drop_last<T>(list: List<T>) -> List<T>
    ensures result.len == (list.len - 1).max(0)
    ensures result == list.take((list.len - 1).max(0))

@method
@builtin("list_push")
pub fn push<T>(list: List<T>, value: T) -> List<T>
    ensures result.len == list.len + 1
    ensures result == list ++ [value]
    ensures result.drop_last() == list
```

| Form | Result | Faults | Semantics | Complexity (reference) |
| - | - | - | - | - |
| `xs.len` | `Int` | none | number of elements; a property, never `xs.len()` (D43) | O(1) |
| `xs.is_empty()` | `Bool` | none | `xs.len == 0`; a method, never `xs.is_empty` (D43) | O(1) |
| `xs.take(n)` | `List<T>` | contract violation `n >= 0` | the first `n` elements, or all of `xs` when `n >= xs.len` (clamping) | O(1) view |
| `xs.drop(n)` | `List<T>` | contract violation `n >= 0` | `xs` without its first `n` elements; `[]` when `n >= xs.len` | O(1) view |
| `xs.drop_last()` | `List<T>` | none | `xs` without its last element; `[]` for `[]` (the design's rule) | O(1) view |
| `xs.push(v)` | `List<T>` | none | a new list with `v` appended; `xs` is unchanged (immutability); the result must be used (`let _ =` to discard, D24) | O(len) copy, O(1) amortized permitted |
| `xs ++ ys` | `List<T>` | none | concatenation | O(n + m) |
| `flatten(xss)` | `List<T>` | none | concatenation of all elements (Section 1.6) | O(total elements) |

Decision: [D-198] S16. A negative argument to `take` or `drop` is a precondition
violation (fault), not clamped to zero; a too-large argument clamps to the
length. This is the design's sentence "accept a nonnegative integer and
clamp at the list length" read literally. Rationale: a negative count is
almost always an arithmetic bug (`size - 1` when `size` was `0`), and the
verifier obligation `n >= 0` is what surfaces it statically; clamping at
the top is what makes the final partial page of `paginate` fall out of
`rest.take(size)` without a branch. Rejected alternative: clamping at both
ends (hides the bug and weakens the postconditions, since `take(-1)` would
have to equal `take(0)`).

### 4.3 Laws for `++`, `take`, `drop`, `drop_last`, `push`, `flatten`

| Id | Law (universally quantified; `n >= 0`, `m >= 0`) |
| - | - |
| L1 | `xs.take(n) ++ xs.drop(n) == xs` |
| L2 | `(xs ++ ys).take(xs.len) == xs` and `(xs ++ ys).drop(xs.len) == ys` |
| L3 | `([] ++ xs) == xs`, `(xs ++ []) == xs`, `(xs ++ ys) ++ zs == xs ++ (ys ++ zs)` |
| L4 | `(xs ++ ys).len == xs.len + ys.len` |
| L5 | `xs.push(y) == xs ++ [y]` |
| L6 | `xs.push(y).drop_last() == xs` and `xs.push(y).len == xs.len + 1` |
| L7 | `flatten([]) == []`, `flatten([xs]) == xs`, `flatten(xss.push(xs)) == flatten(xss) ++ xs`, `flatten(xss ++ yss) == flatten(xss) ++ flatten(yss)` |
| L8 | `(forall x in xs ++ ys: p) == ((forall x in xs: p) and (forall x in ys: p))` |
| L9 | `(forall x in xs.push(y): p) == ((forall x in xs: p) and p[x := y])` |
| L10 | `(forall x in []: p) == true` and `(forall x in [y]: p) == p[x := y]` |
| L11 | `xs.take(n).len <= n`, `xs.take(n).len <= xs.len`, `xs.drop(n).len <= xs.len` |
| L12 | `not (n <= xs.len) or xs.take(n).len == n`; `not (n > 0 and xs.len > n) or xs.drop(n).len < xs.len` (an earlier statement without `n > 0` was false at `n == 0`: `[1].drop(0)` is `[1]`; `paginate`'s `decreases rest.len` relies on this law with `n = size > 0`) |
| L13 | `(xs.len == 0) == (xs == [])` |
| L14 | `(forall x in xs: p)` implies each of `(forall x in xs.take(n): p)`, `(forall x in xs.drop(n): p)`, `(forall x in xs.drop_last(): p)` |
| L15 | `xs.take(n).take(m) == xs.take(n.min(m))` and `xs.drop(n).drop(m) == xs.drop(n + m)` |

Worked check (the obligations of the design's completed `paginate`, each
discharged by the laws above; this is the set the Milestone 5 verifier must
be able to use):

| Obligation | Discharged by |
| - | - |
| invariant `flatten(pages) ++ rest == items` initially | L7 (`flatten([]) == []`), L3 |
| the same invariant after `pages = pages.push(rest.take(size))`, `rest = rest.drop(size)` | L7 (`push` case), L3 (associativity), L1 |
| invariant `forall page in pages: page.len == size` after the push | L9, then `take` ensures `result.len == size.min(rest.len)` with the guard `rest.len > size`, so the length is `size` |
| `decreases rest.len` | `drop` ensures `result.len == (rest.len - size).max(0)`, and `size > 0` from the refinement `Positive`, so the measure strictly decreases and stays `>= 0` (`len` ensures) |
| `ensures flatten(result) == items` | when the tail is pushed: L7 and the invariant; when `rest` is empty: L13 and L3 |
| `ensures forall page in result: page.len > 0 and page.len <= size` | L9 and the invariant for full pages (`size > 0`); for the tail, the loop exit `not (rest.len > size)` and `not rest.is_empty()` |
| `ensures forall page in result.drop_last(): page.len == size` | tail pushed: L6 gives `result.drop_last() == pages`; no tail: L14 |

### 4.4 `forall` over lists

Decision: [D-199] S17. In 0.1 the collection of `forall x in c: p` must have type
`List<T>` (the only collection type); `x` is bound to `T`. Evaluation is
left to right and stops at the first element for which `p` is `false`; the
quantifier is `true` for `[]`. `p` must be total for every element over
which it is evaluated: a fault inside `p` is a fault of the enclosing
`assert`, contract check, or expression. Rationale: the design says
runtime-checkable quantifiers "range over finite collections", and lists are
the only ones; short-circuit evaluation mirrors `and`. Rejected alternative:
evaluating `p` for every element to report all failures (changes totality
requirements for no diagnostic gain, since a counterexample needs only one
element).

### 4.5 Extensions

Decision: [D-200] S18. Five total accessors are added beyond the design's set:
`get(i)`, `first()`, `last()`, `reverse()`, and `contains(value)`. Each is
total (returns `Option<T>` or `Bool`) so none adds a fault source. `map`,
`filter`, `fold`, `any`, `all`, `sort`, and index syntax `xs[i]` are **not**
included: the first five require function values, which 0.1 does not have
(D32), `sort` requires an ordering on `T` (only `Int` has one, S7), and
`xs[i]` would be a partial operation or a second spelling of `get`. The
idiom for iteration is a `while` loop with invariants, as in `paginate`.
Rationale: without these accessors a program cannot read an element of a
list except by `take`/`drop` tricks, and each accessor has a one-line
specification the verifier can use. Rejected alternatives: a partial
`xs.at(i) -> T` with `requires 0 <= i and i < xs.len` (a hidden fault
source in exactly the position where off-by-one bugs live); adding `map`
and `filter` with a special lambda form (contradicts D32).

```text
/// Extension: the element at a zero-based index, or None when out of range.
@method
@builtin("list_get")
pub fn get<T>(list: List<T>, index: Int) -> Option<T>
    ensures result.is_some() == (index >= 0 and index < list.len)

/// Extension: the first element.
@method
@builtin("list_first")
pub fn first<T>(list: List<T>) -> Option<T>
    ensures result == list.get(0)
    ensures result.is_none() == list.is_empty()

/// Extension: the last element.
@method
@builtin("list_last")
pub fn last<T>(list: List<T>) -> Option<T>
    ensures result == list.get(list.len - 1)
    ensures result.is_none() == list.is_empty()

/// Extension: the elements in reverse order.
@method
@builtin("list_reverse")
pub fn reverse<T>(list: List<T>) -> List<T>
    ensures result.len == list.len
    ensures forall x in list: result.contains(x)
    ensures forall x in result: list.contains(x)

/// Extension: whether some element equals value.
@method
@builtin("list_contains")
pub fn contains<T>(list: List<T>, value: T) -> Bool
    ensures result == (not (forall x in list: x != value))
```

| Form | Result | Faults | Semantics | Complexity |
| - | - | - | - | - |
| `xs.get(i)` | `Option<T>` | none | `Some(element i)` for `0 <= i < xs.len`, else `None` (negative `i` included) | O(1) |
| `xs.first()` | `Option<T>` | none | `xs.get(0)` | O(1) |
| `xs.last()` | `Option<T>` | none | `xs.get(xs.len - 1)`; `None` for `[]` (`get(-1)` is `None`) | O(1) |
| `xs.reverse()` | `List<T>` | none | elements in reverse order | O(n) |
| `xs.contains(v)` | `Bool` | none | `true` iff some element `== v` | O(n) element comparisons |

Laws (L16-L19): `(xs.get(i) == Some(v)) == (i >= 0 and i < xs.len and
xs.drop(i).take(1) == [v])`; `xs.reverse().reverse() == xs`;
`[].reverse() == []` and `xs.push(y).reverse() == [y] ++ xs.reverse()`;
`(xs ++ ys).contains(v) == (xs.contains(v) or ys.contains(v))`.

### 4.6 Statement-position use

Semantics: a non-tail expression statement must have type `Unit`
(grammar.md Section 3.11 notes), so `xs.push(1)` on its own line is the
check error `unused_value` of `semantics.md` 3.11 (`the result of push is
unused; lists are immutable`); write `xs = xs.push(1)` on a `var` or
`let _ = xs.push(1)` (D24).

## 5. Option<T> and Result<T, E>

Both are ordinary prelude enums (Section 11.1); their constructors resolve
by D44 and their equality is structural. `match` is the primary way to
consume them and must be exhaustive; the members below exist for the cases
where a `match` would be noise.

### 5.1 Option<T>

```text
pub enum Option<T> {
    Some(T)
    None
}

@method
@builtin("option_is_some")
pub fn is_some<T>(option: Option<T>) -> Bool
    ensures result == (not option.is_none())

@method
@builtin("option_is_none")
pub fn is_none<T>(option: Option<T>) -> Bool
    ensures result == (option == None)

@method
@builtin("option_unwrap_or")
pub fn unwrap_or<T>(option: Option<T>, default: T) -> T
    ensures option.is_none() or option == Some(result)
    ensures option.is_some() or result == default

/// Extension: Some(v) becomes Ok(v); None becomes Err(error).
@method
@builtin("option_ok_or")
pub fn ok_or<T, E>(option: Option<T>, error: E) -> Result<T, E>
    ensures result.is_ok() == option.is_some()
    ensures option.is_some() or result == Err(error)
```

| Form | Result | Faults | Semantics | Complexity |
| - | - | - | - | - |
| `o.is_some()` | `Bool` | none | `true` iff `o` is `Some(_)` | O(1) |
| `o.is_none()` | `Bool` | none | `true` iff `o` is `None` | O(1) |
| `o.unwrap_or(d)` | `T` | none | the payload of `Some`, else `d`; `d` is always evaluated (it is an argument) | O(1) |
| `o.ok_or(e)` | `Result<T, E>` | none | Extension: `Some(v)` gives `Ok(v)`, `None` gives `Err(e)` | O(1) |

### 5.2 Result<T, E>

```text
pub enum Result<T, E> {
    Ok(T)
    Err(E)
}

@method
@builtin("result_is_ok")
pub fn is_ok<T, E>(res: Result<T, E>) -> Bool

@method
@builtin("result_is_err")
pub fn is_err<T, E>(res: Result<T, E>) -> Bool
    ensures result == (not res.is_ok())

@method
@builtin("result_unwrap_or")
pub fn unwrap_or<T, E>(res: Result<T, E>, default: T) -> T
    ensures res.is_err() or res == Ok(result)
    ensures res.is_ok() or result == default

/// Extension: the success value, if any.
@method
@builtin("result_ok")
pub fn ok<T, E>(res: Result<T, E>) -> Option<T>
    ensures result.is_some() == res.is_ok()

/// Extension: the error value, if any.
@method
@builtin("result_err")
pub fn err<T, E>(res: Result<T, E>) -> Option<E>
    ensures result.is_some() == res.is_err()
```

| Form | Result | Faults | Semantics | Complexity |
| - | - | - | - | - |
| `r.is_ok()` | `Bool` | none | `true` iff `r` is `Ok(_)` | O(1) |
| `r.is_err()` | `Bool` | none | `true` iff `r` is `Err(_)` | O(1) |
| `r.unwrap_or(d)` | `T` | none | payload of `Ok`, else `d` | O(1) |
| `r.ok()` | `Option<T>` | none | Extension: `Ok(v)` gives `Some(v)`, `Err(_)` gives `None` | O(1) |
| `r.err()` | `Option<E>` | none | Extension: `Err(e)` gives `Some(e)`, `Ok(_)` gives `None` | O(1) |
| `r?` | `T` | none | postfix propagation (language form, `semantics.md` 3.8): `Ok(v)` yields `v`, `Err(e)` returns `Err(e)` from the enclosing function, whose error type must be `E`. `?` itself performs no conversion; the design's "explicit conversion" for a differing error type is written before the `?` as ordinary code in 0.1 (`let v = match r { Ok(v) => v  Err(e) => { return Err(to_my_error(e)) } }`, or a helper function returning the enclosing error type), because a dedicated conversion form is deferred (`semantics.md` Section 10) | O(1) |

Laws (O1-O4): `Some(v).unwrap_or(d) == v`, `None.unwrap_or(d) == d`;
`Some(v).ok_or(e) == Ok(v)`, `None.ok_or(e) == Err(e)`; `Ok(v).ok() ==
Some(v)`, `Err(e).ok() == None`, `Ok(v).err() == None`, `Err(e).err() ==
Some(e)`; `r.is_ok() == r.ok().is_some()`.

Decision: [D-201] S19. There is no `unwrap()`, `expect(...)`, or any other member
that converts `None`/`Err` into a fault, on `Option` or on `Result`.
Rationale: faults in CPL are reserved for violated contracts, failed
assertions, and exhaustion; a member whose precondition is "the value is
present" would be a fault source hidden inside an expression, used
habitually by agents coming from Rust, and in checked mode it would
surface far from the decision that made the value absent. Every use has a
total spelling that is one line longer: `match`, `unwrap_or`, `ok_or(e)?`,
or `?` itself for `Result`. Rejected alternative: `unwrap()` with `requires
option.is_some()` (consistent with the contract system, but every call
site would carry a proof obligation that `check`-style code never needs).

Decision: [D-202] S20. `ok_or`, `ok`, and `err` are the only extensions on these
types; there is no `map`, `and_then`, `or_else`, `filter`, or `flatten`
(function values, D32), no `unwrap_or_default` (no default values), and no
`?` on `Option` (the design defines `?` for `Result` only; `o.ok_or(e)?` is
the bridge). Rationale: `ok_or` is what turns a `List` accessor's `None`
into the enclosing function's error type so that `?` applies; `ok`/`err`
let tests assert on half of a `Result` (`assert parse_int("x").is_err()`)
without naming the other half's type. Rejected alternative: a richer
combinator set (every one needs a closure).

## 6. std.text

Module `std.text` (stub file Section 11.2). All operations are module
functions taking the text as the first positional argument: `trim(t)` after
`use std.text.{trim}`, or `std.text.trim(t)` qualified.

Decision: [D-203] S21. `std.text` operations are free functions, not `Text`
members; the only `Text` members are `len` and `is_empty()`. Rationale: the
design imports `trim` with `use std.text.{trim}` and calls `trim(page.text)`;
a method form would make that import meaningless and give every operation
two spellings. Rejected alternative: `page.text.trim()`.

### 6.1 Signatures

```text
module std.text

@builtin("text_trim")
pub fn trim(text: Text) -> Text
    ensures result.len <= text.len
    ensures trim(result) == result

@builtin("text_trim_start")
pub fn trim_start(text: Text) -> Text
    ensures result.len <= text.len
    ensures ends_with(text, result)

@builtin("text_trim_end")
pub fn trim_end(text: Text) -> Text
    ensures result.len <= text.len
    ensures starts_with(text, result)

@builtin("text_starts_with")
pub fn starts_with(text: Text, prefix: Text) -> Bool
    ensures not result or prefix.len <= text.len
    ensures not prefix.is_empty() or result

@builtin("text_ends_with")
pub fn ends_with(text: Text, suffix: Text) -> Bool
    ensures not result or suffix.len <= text.len
    ensures not suffix.is_empty() or result

@builtin("text_contains")
pub fn contains(text: Text, needle: Text) -> Bool
    ensures not result or needle.len <= text.len
    ensures not needle.is_empty() or result

@builtin("text_split")
pub fn split(text: Text, sep: Text) -> List<Text>
    requires not sep.is_empty()
    ensures result.len >= 1
    ensures join(result, sep) == text
    ensures forall part in result: not contains(part, sep)

@builtin("text_join")
pub fn join(parts: List<Text>, sep: Text) -> Text
    ensures not parts.is_empty() or result == ""
    ensures parts.len != 1 or result == parts.get(0).unwrap_or("")

@builtin("text_to_upper")
pub fn to_upper(text: Text) -> Text
    ensures result.len == text.len
    ensures to_upper(result) == result

@builtin("text_to_lower")
pub fn to_lower(text: Text) -> Text
    ensures result.len == text.len
    ensures to_lower(result) == result

@builtin("text_parse_int")
pub fn parse_int(text: Text) -> Result<Int, ConstraintError>
    ensures not text.is_empty() or result.is_err()

@builtin("text_from_int")
pub fn from_int(value: Int) -> Text
    ensures result == value.to_text()
    ensures parse_int(result) == Ok(value)

/// Extension: the scalar values of a text, each as a one-element text.
@builtin("text_chars")
pub fn chars(text: Text) -> List<Text>
    ensures result.len == text.len
    ensures forall c in result: c.len == 1
    ensures join(result, "") == text
```

### 6.2 Semantics

| Function | Semantics | Faults / errors | Complexity |
| - | - | - | - |
| `trim(t)` | `t` without leading and trailing *whitespace* scalars (S22) | none | O(n) |
| `trim_start(t)`, `trim_end(t)` | leading only / trailing only | none | O(n) |
| `starts_with(t, p)` | `true` iff the first `p.len` scalars of `t` equal `p`; always `true` for `p == ""` | none | O(m) |
| `ends_with(t, s)` | `true` iff the last `s.len` scalars of `t` equal `s` | none | O(m) |
| `contains(t, n)` | `true` iff `n` occurs as a contiguous scalar subsequence of `t`; `true` for `n == ""` | none | O(n * m) worst case; O(n + m) permitted |
| `split(t, sep)` | the pieces between non-overlapping occurrences of `sep`, found leftmost first; `split("", sep) == [""]`; adjacent separators yield `""` pieces; a leading or trailing separator yields a leading or trailing `""` (S24) | contract violation `not sep.is_empty()` | O(n * m) worst case |
| `join(parts, sep)` | the elements of `parts` concatenated with `sep` between consecutive elements; `join([], sep) == ""` | none | O(total) |
| `to_upper(t)`, `to_lower(t)` | ASCII letters `a`-`z` / `A`-`Z` mapped to the other case; every other scalar unchanged (S23) | none | O(n) |
| `parse_int(t)` | `Ok(n)` when `t` matches the grammar of S25, else `Err(ConstraintError { type_name: "Int", message: ... })` | none | O(n) scan, O(n^2) conversion reference |
| `from_int(n)` | `n.to_text()` (Section 2.3) | none | as `to_text` |
| `chars(t)` | Extension: one `Text` per scalar value, in order | none | O(n) |

Decision: [D-204] S22. *Whitespace* for `trim`, `trim_start`, and `trim_end` is the
fixed set of 25 scalar values that carry the Unicode `White_Space` property:
U+0009-U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000-U+200A, U+2028,
U+2029, U+202F, U+205F, U+3000. Nothing else is whitespace (in particular
not U+200B zero width space and not U+FEFF). The set is written out so it
does not drift with the Unicode tables of a Rust release. Rationale: the
corpus's `parse_size` glue (not the design, which shows no body for
`parse_size`) trims its input before parsing, and user-entered text in
practice contains U+00A0 from web forms and documents, so an ASCII-only
`trim` would leave such input unparsable with no visible reason; the
`White_Space` set has been stable since Unicode 6.3. Rejected alternatives:
ASCII whitespace only (leaves `"\u{a0}5"` unparsable with no visible
reason); Rust's `char::is_whitespace` by reference (version-dependent).

Decision: [D-205] S23. `to_upper` and `to_lower` are ASCII-only in 0.1. Rationale:
full Unicode case mapping changes length (`ß` to `SS`), is locale-sensitive
(dotless `i`), and depends on the Unicode version compiled into the
toolchain, so two toolchain releases could disagree on `==` results that a
test or proof depends on; the ASCII mapping is total, length-preserving
(`ensures result.len == text.len`), idempotent, and exactly what
identifiers, keys, and protocol tokens need. Full case mapping is reserved
for a later `std.unicode`. Rejected alternative: Unicode default case
mapping.

Both engines take the `White_Space` set of S22 from one table and map case
by S23 without their host language's Unicode functions (S47).

Decision: [D-206] S24. `split` requires a non-empty separator and returns
leftmost-first, non-overlapping pieces so that `join(split(t, sep), sep) ==
t` holds for every `t`. Worked values: `split("a,b,,c", ",") == ["a", "b",
"", "c"]`, `split("a,", ",") == ["a", ""]`, `split(",a", ",") == ["", "a"]`,
`split("", ",") == [""]`, `split("aaa", "aa") == ["", "a"]`. Rationale: an
empty separator has no agreed meaning (Python raises, Rust yields every
scalar with empty ends), and the round-trip law is the property callers
rely on. Rejected alternatives: `split("", sep) == []` (breaks the law);
splitting into scalars for `sep == ""` (that is `chars`).

Decision: [D-207] S25. `parse_int` accepts exactly `("+" | "-")? ASCII_DIGIT+`: an
optional sign, then one or more of `0`-`9`, with no surrounding whitespace,
no `_` separators, no other digit scripts, and no length limit; leading
zeros are accepted (`"007"` is `7`) and `"-0"` is `0`. Anything else is
`Err` with `type_name == "Int"`. Rationale: this is the intersection of
what the C, Python, and JavaScript hosts parse by default, so values that
round-trip through adapters parse identically; source-literal rules (no
leading zeros, `_` separators, D6) are readability rules for code, not for
data, and callers that want whitespace tolerance compose `trim`, as the
corpus's `parse_size` does. Rejected alternatives: accepting `_` (not
produced by any host); rejecting `+` (rejects valid output of common
formatters); accepting Unicode decimal digits (would make `len`-based
reasoning about digit strings unsound).

Decision: [D-208] S26. `chars` is added as the only way to look inside a `Text`.
Rationale: with no character type and no indexing (S13), a program that
must examine text (a validator, a tokenizer) needs some total
decomposition; `chars` plus `List` accessors and `join(cs, "")` give one
with a one-line specification. Rejected alternative: a `Char` type (a new
prelude type with its own literal syntax, deferred).

Laws (X1-X7): `trim(trim(t)) == trim(t)` and `trim(t) ==
trim_start(trim_end(t))`; `starts_with(a ++ b, a)` and `ends_with(a ++ b,
b)` and `contains(a ++ b ++ c, b)`; `join(split(t, sep), sep) == t` when
`not sep.is_empty()`; `split(t, sep).len >= 1`; `to_lower(to_upper(t)) ==
to_lower(t)`; `parse_int(from_int(n)) == Ok(n)`; `chars(a ++ b) == chars(a)
++ chars(b)`.

## 7. std.foreign

Module `std.foreign` (stub file Section 11.4) contains only type
declarations; nothing in it is a builtin. It exists so that
`examples/reader/src/reader/images.cpl` typechecks: `use
std.foreign.{ForeignError}`, `Result<Dimensions, ForeignError>` as a
capability operation's return type, and `?` propagating `ForeignError`
through `image_width`. The block below repeats the declarations of Section
11.4 (the normative text) so that the types can be read here.

```text
module std.foreign

/// Classification of a failure at a foreign boundary.
pub enum ForeignErrorKind {
    /// The foreign code raised an exception or returned an error status.
    Exception
    /// An argument could not be converted to the foreign representation.
    InvalidArgument
    /// A foreign return value could not be converted to, or failed
    /// validation against, the declared CPL type.
    InvalidResult
    /// The worker or transport failed: crash, disconnect, protocol error.
    Transport
    /// The call exceeded the host-imposed time limit.
    Timeout
    /// The call was cancelled by the host before it completed.
    Cancelled
    /// No adapter is bound to the capability, or host policy refused it.
    Unavailable
}

/// One structured diagnostic detail; keys are adapter-defined.
pub struct ForeignDetail {
    key: Text
    value: Text
}

/// A foreign exception or boundary failure mapped to a typed CPL error.
pub struct ForeignError {
    kind: ForeignErrorKind
    message: Text
    details: List<ForeignDetail>
}
```

| Field | Content |
| - | - |
| `kind` | the classification above; the only field a matcher should branch on. `InvalidArgument` covers, for example, an `Int` outside the machine range the adapter accepts; `InvalidResult` covers, for example, a non-positive width returned for a `Positive` field; S45 fixes which kind a C binding produces in which situation |
| `message` | one line for humans, taken from the foreign exception or produced by the adapter or the runtime (S45 fixes how a C binding forms it); not stable between toolchain versions, so a program should not branch on it |
| `details` | key/value pairs in the order of S45, which from Milestone 2 fixes the keys that a C binding's failures carry; for the Python adapters S48 fixes them from Milestone 3, and for the Node adapters they remain suggestions until Milestone 4 fixes them: `adapter`, `capability`, `operation`, `foreign_type` (the C function's name, or the exception class or error enum name), `foreign_code` (numeric status, in decimal), `retry_safe` (`"true"` only when the adapter declares the operation idempotent, since the runtime never retries on its own) |

Decision: [D-209] S27. `ForeignError` is a struct of a closed classification enum, a
message, and a list of key/value details. Rationale: the design asks for
"structured classification and diagnostic details"; a closed enum lets
callers `match` exhaustively on the kind (and distinguishes a worker crash,
`Transport`, from an ordinary foreign exception, as the design requires),
while the open key/value list carries adapter-specific facts without a map
type, which 0.1 lacks. Separating `InvalidArgument` from `InvalidResult`
tells a reader which side of the boundary is at fault. Rejected
alternatives: `ForeignError` as a bare enum with payloads per variant
(every variant would repeat the message and details); a `Text`-only error
(not structured).

Milestone note: in Milestone 1 no adapter exists and no code path produces
a `ForeignError` at runtime; user code may construct one. A function with a
`using` parameter typechecks but cannot be run by `cpl run` or reached from
`cpl test` in Milestone 1 (S42, Section 9). From Milestone 2 the C bindings
produce `ForeignError` values by S45, and `cpl run` runs a function whose
`using` parameters are bound (`semantics.md` S78); tests still obtain no
handle (Section 9).

Decision: [D-284] S45. A C binding produces a `ForeignError` in exactly the
situations below, and the runtime builds the value the same way in both
engines (`semantics.md` S75). A *C binding* is a backend-`c` binding,
whether a reviewed wrapper or a declarative binding, or a capability that a
C host implements with its own callbacks (`cabi.md` Sections 5-6).

| Situation | `kind` | Produced by |
| - | - | - |
| the C function returned an error status, or a NULL that means failure | the kind that the declarative binding's `errors` map gives the status (its `default` entry is required), or the kind that a reviewed wrapper or a host callback reports | the binding |
| an argument cannot be converted to its C representation: an `Int` outside the declared machine type (S46), or a text with an interior U+0000 passed as a NUL-terminated string | `InvalidArgument` | the binding, before the C call |
| a C result cannot be converted to its CPL type (a `bool` other than 0 or 1, a text that is not well-formed UTF-8) | `InvalidResult` | the binding, after the C call |
| a returned value fails validation, of its shape or of a refinement, and the operation's return type is `Result<T, ForeignError>` | `InvalidResult` | the runtime (`semantics.md` S77) |
| an adapter reports that a resource it needs is unavailable; a declarative binding's state cannot be created (at the grant, which then fails with `interop_bind_failed` naming `Unavailable`, `cabi.md` Section 4) | `Unavailable` | the binding |
| the process that hosts the evaluator's C adapters ends during the call (`cabi.md` Section 7) | `Transport` | the evaluator's C adapter |
| an adapter that polls the call's cancellation request observes one | `Cancelled` | the adapter itself (reviewed wrappers only) |
| a host time limit | `Timeout`: not produced in Milestone 2, since an in-process C call cannot be preempted | |

The `message` is the one the adapter or host callback reports (strict
UTF-8: `cabi.md` Section 4 refuses any other); for a declarative binding's
error status, the text that its `message` function returns for the status,
or `<function> returned <code>` when it declares none or the function
returns NULL or text that is not strict UTF-8; for an `InvalidArgument`
that a declarative binding produces before the call, `argument <argument>
of <operation> does not fit <c_type>` for an `Int` outside its C type and
`argument <argument> of <operation> contains U+0000` for a text passed as a
NUL-terminated string; for an `InvalidResult` that the runtime or a
declarative binding produces (rather than one an adapter reports), `the
result of <operation> does not satisfy <expected_type> at <pointer>:
<clause>` when a refinement failed and `the result of <operation> is not a
value of <expected_type> at <pointer>` when the shape did or the binding
could not convert a C result; and for `Transport`, `the adapter host for
<binding> exited with <status>`. In these templates `<operation>` is the
operation's qualified name (`reader.images.ImageCodec.dimensions`);
`<argument>` the parameter's name (`max_side`), followed by `.<field>` for
each field of a struct argument on the way to the value that does not fit
(`size.width`); `<c_type>` that value's binding type (`int32`, `cabi.md`
Section 6); `<expected_type>` the type expected at the failing position,
rendered as `semantics.md` 3.3 renders types (`Program::display_ty`, with
declared names: `Result<Dimensions, ForeignError>`), which for a refinement
failure is the refinement named at the position, as for an argument's
`expected_type` (`semantics.md` S39, S74); `<pointer>` the RFC 6901
position in the result (`/$payload/0/width`), with ` at <pointer>` left out
when the failing position is the whole result (the pointer `""`);
`<clause>` the canonical text of the false predicate; `<binding>` the
binding name of `semantics.md` 6.9; `<function>` the C function's name;
`<code>` the status in decimal; and `<status>` `status <n>` when the
adapter host exited and `signal <n>` when a signal ended it, `<n>` in
decimal. The `details` are, in this fixed order: `adapter` (the binding
name of `semantics.md` 6.9), `capability` (the capability's qualified name,
`reader.images.ImageCodec`), `operation` (the operation's qualified name);
then, for a failure that a declarative binding produces (an error status,
or a conversion before or after the call), `foreign_type` (the C function's
name) and, for an error status, `foreign_code` (the status in decimal);
then the adapter's own entries, in the order it added them, which the
runtime neither removes nor reorders; then, for an `InvalidResult` that the
runtime or a declarative binding produced, `pointer` (`""` for the whole
result) and `expected_type` as in the message. A `ForeignError` is built
with all of these entries or not at all: an allocation failure while a
failure is reported or its details are recorded is the fault of
`semantics.md` S76, never a `ForeignError` that lacks an entry (the
reporting functions of `cabi.md` Section 4 return `CPL_E_NOMEM`, which the
binding returns in turn, `cabi.md` Section 5). `retry_safe` is a key that
only an adapter may add, for an operation it declares idempotent: the
runtime never retries an operation. When the operation's return type cannot
carry a `ForeignError`, a failure that the binding reports is no value at
all but a fault (`semantics.md` S80: `binding_error_unrepresentable`, or
`binding_host_failed` when the adapter host ended). Rationale: the design
maps "exceptions and foreign error codes to typed CPL errors" (Foreign
boundary semantics, line 481) and asks for "structured classification and
diagnostic details" (line 489); a closed table of situations makes the kind
a program matches on the same for every C binding and in both engines, a
fixed message and key order make the whole value byte-identical between the
engines (`semantics.md` S75), and the runtime's own keys come first so that
an agent sees which binding and operation failed without parsing the
message, and an adapter can add facts but never hide those. Rejected
alternatives: adapter-defined keys only (the Milestone 1 "suggested keys":
two engines, or two adapters of one library, would report one failure with
different values); a typed detail struct in `std.foreign` (S27 chose an
open list, which 0.1's lack of maps requires); runtime retries of an
operation the adapter marks `retry_safe` (the design allows a retry only
where an adapter defines it, line 489, and an adapter that defines one can
perform it itself).

Semantics: when a result is replaced by an `InvalidResult` value and when a
failure is a fault instead are `semantics.md` S77 and S80 (Section 13.2).

Decision: [D-342] S48. From Milestone 3 a *Python binding* (a
backend-`python` binding, or a Python object that a Python host passes as a
capability, `spec/python.md` Section 5) and a *worker* (the process that
hosts a Python adapter or a backend-`c` adapter with `isolation =
"worker"`, `spec/python.md` Section 6) produce a `ForeignError` in exactly
these situations, with S45's runtime keys (`adapter`, `capability`,
`operation`) first and its rules for the fault that replaces a failure
without a foreign-error return type:

| Situation | `kind` | `message` | Details after the runtime's keys |
| - | - | - | - |
| the adapter's operation raised an `Exception` | the exception's attribute `cpl_foreign_kind` when it is the name of a variant of `ForeignErrorKind`, else `Exception` | `str()` of the exception, or the qualified name of its class when that is empty | `foreign_type` (the class's `module.qualname`, the prefix `builtins.` left out; `ForeignFailure` for the types module's own class, whose module is named differently in process and in a worker), then the pairs of the exception's attribute `cpl_foreign_details`, in order (pairs that are not two `str` are left out) |
| the operation returned a value that is not of the operation's declared return type | `InvalidResult` | `the result of <operation> is not a value of <expected_type> at <pointer>` (S45's shape form) | `pointer`, `expected_type` (S45) |
| the worker ends during the call | `Transport` | `the adapter host for <binding> exited with <status>` (S45) | |
| the worker does not answer within the binding's `timeout_ms` | `Timeout` | `the adapter host for <binding> did not answer within <n> ms` (`<n>` in decimal) | |
| the call's cancellation is requested while it waits for a worker | `Cancelled` | `the call of <operation> was cancelled` | |
| a call of a grant whose worker ended by one of the three rows above | `Transport` | the message of the row that ended it | |

A worker that hosts a backend-`c` adapter reports the C adapter's own
failures by S45 and adds the worker rows above. The keys of S27's table
are therefore fixed for Python bindings too: `foreign_type` is the
exception's class, `foreign_code` is never added by the runtime, and
`retry_safe` stays a key that only an adapter adds. Rationale: a Python
exception is the design's "foreign exception" (line 487), its class is the
"structured classification" a Python programmer knows, and the attributes
let an adapter that knows better choose `Unavailable` or `Timeout` without
importing anything; the worker rows give each failure of a separate
process the kind that S27 reserved for it (`Transport` for "crash,
disconnect", `Timeout`, `Cancelled`), so a program matches on the kind
and never on the message. Rejected alternatives: mapping chosen Python
exception classes to kinds (`TimeoutError` to `Timeout`, `ConnectionError`
to `Unavailable`), which would guess at what an adapter meant; restarting
the worker on the next call (a hidden retry of initialization, line 489).

## 8. gen: property generators

`std.gen` (stub file Section 11.5) is reachable as `gen` in every module
(S1). A generator is a value of the opaque type `Gen<T>`; the `for` clause
of a `property` draws values from it.

### 8.1 Typing

| Expression | Type | Form |
| - | - | - |
| `gen.int` | `Gen<Int>` | property (no parentheses) |
| `gen.positive_int` | `Gen<std.gen.Positive>` where `pub type Positive = Int where self > 0` | property |
| `gen.bool` | `Gen<Bool>` | property |
| `gen.text` | `Gen<Text>` | property |
| `gen.list(g)` | `Gen<List<T>>` for `g: Gen<T>` | function |
| `gen.option(g)` | `Gen<Option<T>>` for `g: Gen<T>` | function |
| `gen.range(lo, hi)` | `Gen<Int>` for `lo: Int`, `hi: Int`; `requires lo <= hi` | function |

A `for name in e` clause requires `e: Gen<T>` and binds `name: T` in the
property body and in no other place.

Decision: [D-210] S28. `Gen<T>` is not nameable in user code: `Gen` is reserved in
the prelude (Section 1.1, S11), and no item of `std.gen` can be imported:
`use std.gen.{Gen}` is the check error `generator_import` (`semantics.md`
1.6; `Gen is not nameable in edition 0.1`) and `use std.gen.{int}` or `use
std.gen.{list}` is the same error (`std.gen is in scope as gen; write
gen.int`), so every generator has the
one spelling `gen.<name>` (or the equivalent `std.gen.<name>`, which the
module branch of `semantics.md` 2.4 resolves the same way). An expression
of type `Gen<T>` may appear only as the range of a `for` clause or as an
argument of a `std.gen` function; anywhere else (`let g = gen.int`, a
parameter, a field, a return type) is the check error
`generator_outside_for` of `semantics.md` S14 (`generator values may only
appear in a property's for clause`). Rationale: generators are the only
values in 0.1 that are neither data nor capabilities; keeping them out of
the term language means equality, the semantic index, the C boundary, and
the value renderer never see one, and one spelling per generator keeps
receipts, diagnostics, and the formatter from ever seeing a bare `int`.
Rejected alternatives: first-class `Gen<T>` with user combinators (needs
function values, D32); importable generator properties (a bare `int` in
expression position would need its own name-resolution rule in
`semantics.md` 2.2 for four names).

Design conflict: [D-211] the design's property `for size in gen.positive_int` then
calls `paginate(items, size)`, whose parameter type is `reader.pages.Positive`
(`Int where self > 0`), with no `check` and no annotation. A standard
library cannot name a user type, and the design forbids implicit
conversions, so either the binding has a type the user's `Positive`
accepts, or the design's example does not typecheck. Reading chosen: a
refinement type denotes the set of base values satisfying its predicate, so
two refinement declarations with the same base and the same predicate are
the same type, and `std.gen` declares `pub type Positive = Int where self >
0`, giving `gen.positive_int` the type `Gen<Positive>` that `paginate`
accepts. This matches the design's own contrast ("PageId is a nominal
wrapper ... Positive is a refinement of Int"; "a literal such as 2 can
satisfy Positive through a compile-time proof"): membership in a refinement
is proved, not constructed.

Decision: [D-212] S29. Refinement types are structural: `type A = B where p` and
`type C = B' where q`, declared anywhere in the program or in `std`, denote
the same type when `B` and `B'` are the same type and `p` and `q` are the
same *resolved predicate*: their abstract syntax trees are identical
(grammar.md Appendix B node shapes; grouping parentheses produce no node,
so `self > 0` and `(self > 0)` are the same predicate), literals are equal
by value, the bound name is `self` in both (no renaming is needed), and
every name in the two trees resolves to the same declaration by qualified
name (`semantics.md` 1.4) or to `self`. Identity is therefore decided on
resolved syntax, not on text: two declarations `Text where ok(self)` in
modules whose `ok` functions differ are two types, and no formatter rule
takes part. A refinement is never identical to its base. Nominal wrappers
remain nominal. Consequences: `std.gen.Positive` is `reader.pages.Positive`;
`check<Positive>` reports the `type_name` of the declaration named at the
call site (S44); a user who writes `type Positive = Int where 0 < self`
gets a distinct type in 0.1 and the check error `cannot_prove_refinement`
(`semantics.md` S65) at `paginate(items, size)` with the hint `declare the
predicate as self > 0 or use check<Positive>(size)` (Milestone 5 may relax
identity to proven equivalence). Semantics: `semantics.md` S55 states the same rule over the
resolved predicate, and `toolchain.md` 3.3 compares `RefinementClassDef`
members by it. Rejected
alternatives: nominal refinements plus a fact-based coercion from the
generator's postcondition (needs an entailment checker in Milestone 1 and a
new annotation to state generator postconditions); a type ascription in
the `for` clause (`for size: Positive in ...`, a grammar change the
design's example does not use).

Decision: [D-213] S30. The `for` clauses of one property are independent: a range
expression is a contract expression (`semantics.md` 5.1: pure and total, so
it may call pure functions, as in `gen.range(0, limit())`), evaluated once
per property before case 0, and it may not mention a name bound by any
`for` clause of the property (`semantics.md` S43: the binders are visible
only in the body, so such a name is `unresolved_name`). Rationale:
independent clauses make every case a tuple of separately shrinkable
values and keep the receipt a plain record of draws; dependent generation
needs a choice-sequence shrinker. Rejected alternative: dependent clauses
(deferred).

### 8.2 Runs, seeds, sizes

| Parameter | Default | Meaning |
| - | - | - |
| `cases` | 100 | number of generated cases per property (`cpl test --cases N`), at least 1 (`spec/toolchain.md` T57) |
| `max_size` | 100 | cap on the size parameter |
| `seed` | fresh from OS entropy per `cpl test` run | 64-bit run seed; `cpl test --seed S` replays; recorded in the test receipt as a decimal integer |
| `max_shrinks` | 1000 | maximum body executions (shrink steps, 8.4) spent shrinking one failure (`cpl test --max-shrinks N`, `[test] max_shrinks`) |

Case `i` (0-based) runs at size `s = i.min(max_size)`, so early cases are
small and the first failing case is usually already simple.

Random stream: every property has one `ChaCha20` stream for the whole run,
`ChaCha20Rng::from_seed(property_seed)` of `rand_chacha` (stream 0, block
0; `from_seed` takes the 32 bytes as the key directly, so no seeding helper
of `rand` takes part), where `property_seed` is the 32-byte digest
`blake3(seed_le(8) ++ identity_utf8)`: the 64-bit run seed as 8
little-endian bytes followed by the UTF-8 bytes of the property's identity
(`semantics.md` S42: the `@id` value when the property carries one,
otherwise its qualified name exactly as the test result reports it in
`qualified_name`, `tests.pagination."pagination preserves input"`, with no
`kind~` prefix; `toolchain.md` `SymbolPath::display`, so a name containing
`"` or `\` is hashed in its canonically escaped spelling). The digest is
recorded in the test result as `property_seed`, 64 lowercase hex digits
(`semantics.md` S44, S46). Cases draw from that stream in case order, and
within a case the `for` clauses draw in clause order; `--cases N` therefore
changes no case that a smaller `N` would also have run. Shrinking uses no
randomness. Range expressions (`gen.range(1, 10)`) are evaluated once per
property before case 0, `lo` before `hi`, and a fault there fails the
property with that fault and zero cases: a fault raised while the bounds
evaluate is the property's, reported per `semantics.md` S71 (`symbol` the
property, `clause` the range's text), while `lo > hi` is the generator's own
precondition and keeps the shape of Section 12's fault table (`symbol`
`std.gen.range`, `clause` `lo <= hi`, `caller` the property). This setup
runs without the per-case time budget (`toolchain.md` 7.2).

Decision: [D-214] S31. The parameters, seed derivation, draw procedure (8.3), and
distributions here are part of the specification: the same toolchain
version, run seed, and property produce the same cases on every platform.
The stream is keyed by the property's stable identity, as the design asks
of test identity ("Tests have stable identities"; "identity survives a
rename"): a property that carries an `@id` keeps its case sequence under
the same run seed across renames and moves, while one without an `@id` is
keyed by its qualified name, the identity it has (`semantics.md` S42), so
renaming or moving it changes its sequence until `cpl id assign` gives it
an `@id`. A recorded run seed replays a property together with the
identity the test result recorded (`semantics.md` S46 records `symbol`,
`qualified_name`, and the hex `property_seed`; `toolchain.md` 8.2 the
same), so replay is unambiguous. Across toolchain versions only replay of
a recorded seed with the same toolchain is promised (the receipt records
the version). This derivation is `semantics.md` S44's, adopted here and
recorded by `toolchain.md` 1.7.2 and T45; the per-case FNV-1a keys of an
earlier draft of this document are withdrawn.
Rationale: a counterexample must be reproducible by another agent from the
receipt alone, and keying on the identity makes a recorded seed survive
the renames the design says identity survives; one stream per property
keeps a property's cases independent of every other test, so adding or
reordering tests never changes them; the full 32-byte digest as the key
avoids any dependence on a seeding helper's expansion. Rejected
alternatives: leaving the distribution to the implementation (would make
`--seed` meaningless across machines); keying on the qualified name only
(the rule of an earlier revision of this document; a rename would silently
change every case of a property that has a stable identity); one stream
for the whole run (any new test would perturb every property).

### 8.3 Distributions

`u()` is the next `u32` word of the stream (`rng.next_u32()`). `bits(b)`
for `b >= 0` is an `Int` of `b` uniformly random bits: draw `ceil(b / 32)`
words `u_0, u_1, ...` (none when `b == 0`), assemble them little-endian
(`x = u_0 + u_1 * 2^32 + ...`, the first word least significant), and keep
the low `b` bits (`x mod 2^b`). `below(k)` for `k >= 1` is a uniform `Int`
in `[0, k)`: with `b` the bit length of `k - 1` (`0` for `k == 1`, so
`below(1)` is `0` and draws nothing), repeat `x = bits(b)` until `x < k`;
each attempt succeeds with probability above one half, so the expected
number of attempts is below two. No draw is reduced modulo anything, so
there is no modulo bias, and a draw of more than 32 bits is defined
(`gen.range(0, 5_000_000_000)` draws 33 bits as two words). `sign()` is one
`u()`: odd makes the value negative, even leaves it positive. `s` is the
size. Within a row the draws happen in the order written.

| Generator | Draw at size `s` |
| - | - |
| `gen.int` | bucket `u() % 8`: `0` gives `0`; `1` gives `1` then `sign()`; `2` when `s >= 8` gives a *large* value: bit length `L = 64 + below(8 * s + 1)`, magnitude `2^(L - 1) + bits(L - 1)` (the top bit set, `L - 1` random lower bits), then `sign()`; `2` when `s < 8`, and `3`-`7` always, give `below(2 * s + 1) - s` (uniform in `[-s, s]`) |
| `gen.positive_int` | `1 + v.abs()` where `v` is a `gen.int` draw |
| `gen.bool` | `u() % 2 == 1` |
| `gen.text` | length `below(s + 1)`; each scalar by bucket `u() % 4`: `0`,`1` give `a`-`z` (`below(26)`); `2` gives printable ASCII U+0020-U+007E (`below(95)`); `3` gives one of the fixed list `é` U+00E9, `ß` U+00DF, `İ` U+0130, `Ω` U+03A9, `中` U+4E2D, `😀` U+1F600, U+200B, U+0301, U+FFFD, U+00A0 (`below(10)`) |
| `gen.list(g)` | length `below(s + 1)`; each element drawn from `g` at the same size `s`, in order |
| `gen.option(g)` | `None` when `u() % 4 == 0`, else `Some` of a draw from `g` |
| `gen.range(lo, hi)` | `lo + below(hi - lo + 1)`; the size is ignored (the range is already bounded) |

The alphabet of `gen.text` deliberately includes a quote, a backslash,
space, a combining mark, a zero-width space, and a non-BMP scalar so that
escaping, trimming, `len`, and UTF-8 boundaries are exercised.

### 8.4 Shrinking

A case fails when its body raises any fault (`assertion_failed`,
`contract_violation`, `division_by_zero`, ...). On failure the runner
shrinks (`semantics.md` S45): it walks the clauses in order; for the
current clause it tries the candidates of the current value in the order
given below, re-running the body with only that value replaced; the first
candidate whose run raises a fault *with the same fault code* as the
original failure replaces the value, and the walk continues with the same
clause from the accepted value; when no candidate of a clause is accepted
the walk moves to the next clause, and after the last clause it restarts at
the first; a candidate that passes, or that fails with a different code, is
discarded. Every rerun of the body is one *shrink step*. Shrinking stops
when a full pass over all clauses accepts no candidate, or when
`max_shrinks` steps have been spent while another rerun was wanted (then
`shrink_exhausted` is `true`; it is `false` when the last allowed step also
ended the search, and with `max_shrinks` 0 it is `true` exactly when the
failing value has a candidate). Candidates are produced lazily and are not
deduplicated: each rule of the table below that yields a value different
from `v` contributes it, even when an earlier rule yielded the same value
(`1` has the candidates `0`, `0`, `0`, and both single removals of `[0, 0]`
give `[0]`), and each rerun of a duplicate counts as a step. The receipt records the original case, the
shrunk case and the fault it raised, the case index, the run seed, the
number of shrink steps, and whether the budget was exhausted
(`semantics.md` S46). The *first half* and *second half* of a value `v` of
length `len` are `v.take(len / 2)` and `v.drop(len / 2)` (over `chars(v)`
for a text, joined back with `""`), so for an odd length the second half is
the longer one.

| Generator | Candidates for value `v`, in order (each must differ from `v`) |
| - | - |
| `gen.int` | `0`; `v / 2` (truncating); `v - 1` if `v > 0`, `v + 1` if `v < 0` (so `0` has no candidates: every rule yields `0` itself, which is excluded) |
| `gen.positive_int` | `1`; `1 + (v - 1) / 2`; `v - 1` (only candidates `>= 1`; `1` has none) |
| `gen.bool` | `true` shrinks to `false`; `false` has none |
| `gen.text` | `""`; the first half; the second half; `v` without its last scalar; then `v` with the first scalar that is not `a` replaced by `a` |
| `gen.list(g)` | `[]`; the first half; the second half; `v` without element `i` for `i` ascending over every index; then `v` with element `i` replaced by each candidate of `g` for that element, `i` ascending over every index |
| `gen.option(g)` | `Some(x)` shrinks to `None`, then to `Some(x')` for each candidate `x'` of `g`; `None` has none |
| `gen.range(lo, hi)` | with origin `o = 0.max(lo).min(hi)`: `o`; `o + (v - o) / 2`; one step from `v` toward `o` (every candidate lies in `[lo, hi]`) |

Decision: [D-215] S32. Shrinking is greedy, deterministic, first-failing-candidate,
with the candidate orders above, bounded by `max_shrinks` body reruns, and
accepts a candidate only when it fails with the same fault code as the
original failure (`semantics.md` S45, which this document adopts; the "any
failure counts" rule of an earlier draft of this document is withdrawn).
Rationale: the order "smallest first, then halving, then one step" reaches
a local minimum in O(log v) accepted steps for integers and O(n) for
lists, and it is simple enough that two implementers produce identical
shrunk cases from the same original; requiring the same code keeps the
reported counterexample a witness of the failure that was observed (an
`assertion_failed` at `n = 5` must not shrink into a `contract_violation`
at `n = -1`), and since the codes exist no classification is needed beyond
comparing them. Rejected alternatives: accepting any failure (the
QuickCheck convention; wanders to unrelated failures); random shrinking;
integrated (Hypothesis-style) shrinking on the choice sequence (better
minima, far more machinery).

### 8.5 Stubs

```text
module std.gen

/// The type of generators; not nameable outside std (S28).
@builtin("gen")
pub type Gen<T>

/// Structurally the same type as any user refinement Int where self > 0 (S29).
pub type Positive = Int where self > 0

@property
@builtin("gen_int")
pub fn int() -> Gen<Int>

@property
@builtin("gen_positive_int")
pub fn positive_int() -> Gen<Positive>

@property
@builtin("gen_bool")
pub fn bool() -> Gen<Bool>

@property
@builtin("gen_text")
pub fn text() -> Gen<Text>

@builtin("gen_list")
pub fn list<T>(element: Gen<T>) -> Gen<List<T>>

@builtin("gen_option")
pub fn option<T>(element: Gen<T>) -> Gen<Option<T>>

@builtin("gen_range")
pub fn range(lo: Int, hi: Int) -> Gen<Int>
    requires lo <= hi
```

Generators have no `ensures` clauses: the guarantees (`positive_int` draws
are `>= 1`, `range` draws lie in `[lo, hi]`, list lengths are `<= s`) are
properties of the distributions in 8.3, which the `for` binding's type
(`Positive`) and the runner enforce. A generated `Positive` is constructed
by the runner after checking the predicate, exactly as `check<Positive>`
would.

## 9. Capabilities in std: reserved for later milestones

No capability is implemented in 0.1 Milestone 1: the design places time,
randomness, filesystem, networking, and model calls behind capabilities
("Code has no ambient environment access"), and S42 below defers every way
of obtaining a handle. The module paths and operation shapes below are
reserved so that names chosen now survive; nothing in this table is
binding except the paths.

Decision: [D-216] S42. In Milestone 1 no capability handle can be obtained: `std`
declares no capability, the toolchain binds no adapter (`[bindings.*]` in
`cpl.toml` is parsed and inert), and no fixture or recording mechanism
exists yet, so a function with a `using` parameter typechecks and
effect-checks fully but cannot be run by `cpl run` (`semantics.md` S39,
`cannot_run_effectful`) or reached from a test (`semantics.md` S30, 6.9).
The design ties handles to no milestone; this is a scope decision made by
this document and `semantics.md` 6.9 together. Consequence for the design's
Milestone 1 acceptance criterion "examples execute": every pure declaration
of the corpus runs (`paginate`, `paginate_ints`, `positive_size`,
`divide_exactly`, `clean`, `label`, the two tests, and the property), while
`load_page`, `load_text`, and `image_width` are checked but not executed
until a handle source exists. So that the design's recording adapters ("A
recording adapter may replay captured inputs for debugging"), test fixtures
("Test receipts additionally include seeds, fixtures, and environment
inputs"), and declared environment assumptions ("service-dependent tests
must declare fixtures or environment assumptions") are deferred rather than
dropped, two places are reserved for them: the test receipt's `fixtures`
list (`semantics.md` S59 and `toolchain.md` 8.2, empty in Milestone 1) is
where a run's capability bindings are recorded, and a later edition binds
handles for tests through
`[bindings.*]` entries whose `backend` is a fixture or recording adapter;
the first handles arrive with the C boundary and the adapters of Milestones
2-4. Rationale: a handle that could be manufactured in CPL source would
contradict the design ("code cannot manufacture one from a string or a
record"), and a fixture mechanism designed before any adapter exists would
be designed blind. Rejected alternatives: shipping the capability
declarations below without any handle source (signatures that typecheck
but can never run); an ad hoc Milestone 1 fixture syntax.

Milestone 2 note: the first handles arrive with the C boundary, as S42
foresaw. `cpl run` binds the `using` parameters of a function from the
manifest's backend-`c` bindings (`semantics.md` S78), a C host passes the
handles it implements or grants (`cabi.md` Section 5), and every such
binding is validated (`semantics.md` S77). Nothing else in S42 changes:
`std` declares no capability, tests and properties obtain no handle
(`semantics.md` S30), the test receipt's `fixtures` stays `[]`, and the
fixture and recording backends stay reserved. In the verbatim corpus
`load_page` and `load_text` (no binding exists for `PageRead`) and
`image_width` (its one binding has backend `python`, which arrives in
Milestone 3) still typecheck without running.

| Module | Planned contents (sketch) |
| - | - |
| `std.time` | `capability Clock { fn now() -> Instant  fn monotonic() -> Duration }`; value types `Instant`, `Duration` with `Int` nanosecond accessors |
| `std.random` | `capability Random { fn int(lo: Int, hi: Int) -> Int requires lo <= hi ensures lo <= result and result <= hi  fn bytes(count: Int) -> Bytes requires count >= 0 ensures result.len == count }` (distinct from `gen`, which is deterministic test tooling) |
| `std.fs` | `type Path(Text)`, `enum FsError`, `capability FileRead { fn read(path: Path) -> Result<Bytes, FsError> }`, `capability FileWrite { fn write(path: Path, data: Bytes) -> Result<Unit, FsError> }` |
| `std.net` | `type Url(Text)`, `enum NetError`, `capability HttpClient { fn get(url: Url) -> Result<Response, NetError> ... }` |
| `std.env` | `capability Environment { fn get(name: Text) -> Option<Text> }`; there is no ambient environment access |
| `std.model` | `capability Model { fn complete(request: ModelRequest) -> Result<ModelResponse, ModelError> }`; responses are external data to be parsed and validated |
| `std.task` | capability-backed structured task groups (design: "Integration semantics"); runtime task syntax is deferred |
| `std.int` | bounded integer conversions and wrapping arithmetic (Section 2.5) |
| `std.list` | algorithms that need function values or orderings (`sort`, `map`) once the language has them |
| `std.unicode` | full case mapping, normalization, grapheme segmentation |

Decision: [D-217] S33. Every module path beginning with `std.` is reserved (S1),
and a `use` of a reserved-but-unimplemented module is the check error
`unresolved_module` (`semantics.md` 1.6) with a note naming the milestone:
`std.time is reserved for a later edition (capabilities arrive with the C
boundary)`. In 0.1 the implemented `std` modules are
exactly `std.prelude`, `std.text`, `std.bytes`, `std.foreign`, and
`std.gen`. Rationale: a precise diagnostic is more useful to an agent than
`unknown module`, and reserving the whole prefix means no user module can
later collide. Rejected alternative: shipping the capability declarations
without any way to obtain a handle (signatures that typecheck but can never
run).

## 10. Implementation notes for cpl-sema and cpl-eval

### 10.1 Files and loading

Decision: [D-218] S34. The standard library ships as CPL stub files embedded in the
toolchain binary at build time (`toolchain.md` 3.6: `cpl-project/build.rs`,
toolchain T52, generates `STD_FILES` from the directory) under the logical directory
`std/`: `std/prelude.cpl`, `std/text.cpl`, `std/bytes.cpl`,
`std/foreign.cpl`, `std/gen.cpl` (full text in Section 11). The sources
live in the repository at `/std/` so that `cpl inspect
std.prelude.List.take` and the semantic index can quote them, and so that
the verifier can hash them as inputs of a receipt. `cpl-sema` parses them
with the ordinary `cpl-syntax` parser before any user module (they receive
the lowest `FileId`s, `toolchain.md` T15) and resolves the five modules,
the *std unit*, together with the project; declaration and file order are
irrelevant (`semantics.md` S1), so the order in which the files are read
(this list, or `toolchain.md` 3.6's sorted name order) does not matter; a
user project cannot override or extend them (no `std/` lookup in the
project; `toolchain.md` 3.6's `CPL_STD_DIR` override serves the development
of the library itself). Rationale: the std version is then exactly the toolchain
version that `cpl.lock` records, and a checkout needs nothing beyond the
toolchain to typecheck. Rejected alternative: a `std/` directory discovered
on disk at run time (a second versioned artifact to lock).

### 10.2 Stub syntax

Decision: [D-219] S35. A standard-library operation is an ordinary `fn` declaration
with the annotation `@builtin("<id>")` and *no body*; a builtin type is an
ordinary `type` declaration with `@builtin("<id>")` and *no definition*.
Two further annotations, `@method` and `@property`, classify members
(Section 0.1). This needs two extensions to grammar.md, which its Sections
3.9 (`fn_tail`) and 3.5 (`opaque_decl`) carry:

```ebnf
// G1: grammar.md 3.9 fn_tail gains a bodiless alternative
fn_tail   = NEWLINE? block
          | ( NEWLINE fn_clause )+ NEWLINE block
          | ( NEWLINE fn_clause )*                   // bodiless: a builtin stub (S35); cpl check requires @builtin

// G2: grammar.md 3.5 type_decl gains an opaque alternative
type_decl = "pub"? "type" UPPER_IDENT ( wrapper_decl | refinement_decl )
          | "pub"? "type" UPPER_IDENT generic_params?     // opaque: a builtin type (S35); cpl check requires @builtin
```

Parsing: after a `fn` header or clause the parser already peeks past one
`NEWLINE` for a continuation keyword or `{` (grammar.md 2.4 rule 3); with
G1, when neither follows, the declaration is complete with `body: None`
instead of raising the missing-body syntax error of that rule. For G2, after `type
UPPER_IDENT` the parser reads `<`, `(`, or `=` if present and otherwise
ends the declaration. User-visible consequence, in project files as well as
in `std`: a `fn` without a body or a `type` without a definition is no
longer a syntax error that stops the file (D59);
the file parses, `cpl check` reports the check error of the first row below
at that declaration, and the rest of the file is checked normally. Both
forms are validated by `cpl check`, in the spirit of D17 (open grammar,
closed vocabulary):

| Check rule | Code (`semantics.md` 9.4, `toolchain.md` Appendix B) and message |
| - | - |
| a bodiless `fn` without `@builtin`, or an opaque `type` without `@builtin` | `missing_body`: `function without a body requires @builtin` / `type without a definition requires @builtin` |
| `@builtin`, `@method`, or `@property` in a module outside the std unit | `unknown_annotation`: `builtin forms are permitted only in the standard library` |
| `@builtin` on a `fn` that has a body, or on a `type` that has a definition | `annotation_misplaced`: `a builtin declaration has no body` |
| an `@builtin` id unknown to `cpl-eval`'s registry (Section 10.6) | `std_builtin_missing`: `unknown builtin "..."` (a toolchain consistency failure: a check error of category `internal` at the annotation, with `name`, the definition marked `HasErrors`; `cpl-sema` checks the registry's id list without depending on `cpl-eval`: a copy of it in Milestone 1, `toolchain.md` 3.6, and from Milestone 2 the one list of `cpl-rtspec` that both crates read, S47, `toolchain.md` T70) |
| `@method` on a `fn` with zero parameters; `@property` on a `fn` with more than one parameter; both on one `fn` | `annotation_misplaced`: `@method requires a receiver parameter` / `@property takes at most a receiver` / `choose @method or @property` |
| a `@method`/`@property` with a first parameter whose type is not a prelude type head (`Int`, `Bool`, `Text`, `Bytes`, `List`, `Option`, `Result`); a zero-parameter `@property` is exempt (S40) | `annotation_misplaced`: `members may be declared only on prelude types` |

Grammar.md's annotation table (Section 3.3) carries the three rows:
`@builtin` (one `TEXT`, on `fn` or `type`, facet `body`), `@method` (none,
on `fn`, facet `signature`), `@property` (none, on `fn`, facet
`signature`); its D71 admits any keyword token as an annotation name, so
`@property`, whose name is the keyword `property`, lexes and parses as
written here and in Section 11.
Rationale for bodiless rather than hole-bodied: capability operations are
already bodiless signatures in the grammar (`op_decl`), so the parser has
the shape; a `{ ?builtin }` body would make every std operation a hole
that `no_holes` and release builds must special-case, and would misreport
"unresolved" in `cpl inspect`; and a builtin *type* (`pub type Int`) has no
hole-bodied spelling at all, so G2 is needed under either reading. The stub
text of Section 11 is the single normative form, and `toolchain.md` 3.6
loads exactly it (bodiless `@builtin` stubs, `@method`/`@property`, registry
keyed by the `@builtin` id). Rejected alternatives: hole-bodied stubs and a
`@member("Type", "kind")` annotation (an earlier spelling of the same
declarations); a separate stub file format (a second parser for the same
declarations).

Decision: [D-220] S36. In 0.1 every `fn` in the std unit is a `@builtin` stub; the
std unit contains no CPL function bodies. Types in std (`Unit`, `Option`,
`Result`, `ConstraintError`, `ForeignError`, `Positive`) are ordinary
declarations. Rationale: the Milestone 1 acceptance evidence is that the
examples execute; if `from_int` were written in CPL as `value.to_text()`,
the library's correctness would depend on the interpreter under test. The
format permits bodied functions so later editions can add derived helpers.
Rejected alternative: CPL bodies for the trivial helpers.

### 10.3 Member tables and name resolution

Decision: [D-221] S37. `@method` and `@property` stubs with a receiver are not free
names: they are entered into the *member table* of `semantics.md` 3.9,
keyed by (receiver type head, member name), where the heads are `Int`,
`Bool`, `Text`, `Bytes`, `List`, `Option`, and `Result` (the member rows of
this document extend `semantics.md` 3.9 under its S22), and the
duplicate-declaration rule of a module exempts them, so `std/prelude.cpl`
declares `len` three times (for `List`, `Text`, `Bytes`) and `unwrap_or`
twice (`Option`, `Result`). Their qualified name for diagnostics,
selectors, receipts, and `cpl inspect` is `std.prelude.<Receiver>.<name>`
(`std.prelude.List.take`, `std.prelude.Int.min`; never `std.list.take`,
since `std.list` is a reserved, unimplemented module, S33), which parses as
a `selector_path` (grammar.md 4.4). Calling one as a free function is a
resolution error (`take is a method of List; write list.take(n)`). A
zero-parameter `@property` is not a member (S40). Rationale: one spelling
per operation (D43), and a receiver-keyed table is what the typechecker
needs for `page.len` anyway. Rejected alternative: UFCS (both spellings).

Decision: [D-222] S38. The resolution of a postfix `e.name` (grammar.md 3.12 rule
1) is owned by `semantics.md` 2.4 (chain resolution: local bindings before
module roots, module items, enum variants through a type, capability
operations on a `using` parameter) and 3.8-3.9 (struct fields, then the
member table of `strip(T_e)`, the type head after widening a refinement to
its base, so `size.to_text()` with `size: Positive` uses `Int`'s table);
this document does not restate that procedure and supplies only the member
rows (S37). What this document fixes: a member found as `@property` must
not be called (`property_called`: `len is a property; write xs.len`) and a
`@method` must be called (`method_not_called`: `is_empty is a method; write
xs.is_empty()`), exactly as D43 specifies; a member call's receiver may be
any expression, not only a name chain (`[1, 2, 3].take(2)`, `(-5).abs()`,
`parse_size(text)?.is_ok()`, Section 12), which grammar.md 3.12 rule 4 and
`semantics.md` S11 admit; and structs,
enums, wrappers, and capabilities have no member table beyond their fields,
variants, and operations. Rejected alternative: allowing user structs to
declare methods (a language feature the design does not have;
`semantics.md` S22).

Decision: [D-223] S43. A standard-library declaration carries no `@id`; its stable
identity *is* its qualified name (`std.prelude.flatten`,
`std.prelude.List.take`, `std.text.trim`, `std.gen.Positive`), which is
the provisional identity of `semantics.md` S2 (the toolchain flags it
`provisional: true` like every unannotated declaration) but is fixed for
the edition: `std` is never renamed within an edition, and every receipt
pins its content through the std digest (`toolchain.md` 3.6 and 8). The
`symbol` of a diagnostic that blames a std declaration therefore carries
that qualified name, and `qualified_name` carries the same string
(`semantics.md` S49, 6.10). Rationale: generated `@id`s exist so that
identity survives renames and moves (design, Functions and contracts); std
declarations do not move, their names are the interface agents read, and
the std digest already gives each toolchain version one content identity,
so opaque ids would add some sixty strings nobody can read for no
stability gain. `cpl id assign` never edits std files (`toolchain.md`
3.6). Rejected alternatives: generated `@id`s in the stubs (an earlier
revision of `toolchain.md` 3.6); a distinct spelling for provisional
identities such as a `kind~` prefix (an earlier revision of `toolchain.md`
4.3; `semantics.md` S2 spells them as qualified names).

### 10.4 Contracts of builtins at run time

Decision: [D-224] S39. The Rust implementation of a builtin checks its own
`requires` clauses first and raises a fault on violation; its `ensures`
clauses are *trusted* and never evaluated at run time, in every contract
mode. This exempts std postconditions from the design's rule that in
`checked` mode "executable preconditions and postconditions run at their
boundaries" (Functions and contracts): the exemption is deliberate, and it
is sound because the builtin implementation *is* the trusted runtime to
which the design says correctness is relative ("Correctness remains
relative to the language semantics, verifier, and trusted runtime", An
implementation with loop invariants), so there is nothing more trusted to
check a builtin's result against. User-function contracts are checked by
the runtime as `semantics.md` Section 5 specifies (`semantics.md` S58
adopts this decision for builtins). Rationale: evaluating `result ++
list.drop(n) == list` after every `take` would make the library quadratic
in checked mode, and the builtin's code is the trust root anyway. Rejected
alternative: evaluating builtin postconditions under a debug flag
(acceptable as a toolchain self-test, Section 10.7, not as a language
mode).

The check of a builtin's `requires` also runs in every contract mode
(`check_contracts: false` included): the precondition is part of what the
operation means (`take` has no result for a negative count), and the
interpreter, which never evaluates a stub's clauses, cannot check it
instead (`toolchain.md` 1.7.1). Its violation has the fault shape of
`semantics.md` 6.10, `caller` being the innermost active declaration (the
refinement declaration for a call inside a `where` predicate), and the
builtin is a frame of the fault's backtrace.

Three stubs state idempotence by naming themselves: `trim` carries `ensures
trim(result) == result`, `to_upper` and `to_lower` the corresponding
clause (6.1, 11.2). `semantics.md` S61 exempts `@builtin` declarations
from `recursive_contract`: such a clause is never evaluated (S39), so the
re-entry that rule guards against cannot occur, and the verifier reads it
as an axiom about the builtin (S41). The stub text of Section 11 is
therefore checked as written.

Decision: [D-225] S41. The stubs' contracts and the laws tables of this document
are the specifications the Milestone 5 verifier assumes about std callees
(design: "These operations have library specifications available to the
verifier"; "The verifier must use the specifications of take, drop, push,
and flatten"). They are axioms of the trusted runtime, not assumptions of
the kind that make a proof conditional: a proof that uses them is `proved`,
not `conditional`, and the reliance is recorded in the receipt as a
*dependency* rather than as a named assumption. The receipt lists every std
declaration whose specification the proof used, with its facet versions,
among `dependencies` (the design's "external specifications" in the
dependency graph), and pins the std text through the std digest in `inputs`
(`toolchain.md` 8, 8.1), so a change to a std specification invalidates the
proof exactly as a changed callee contract would. The design's named
assumptions (`unchecked_callee`, `foreign_adapter`, `hole`, `admitted`,
`unproved_dependency`) concern CPL implementations and external adapters;
a builtin is neither. The property transcription of Section 10.7 is
toolchain self-test evidence about the builtin implementations and
discharges nothing: tests never become proofs. Rationale: the design's
Milestone 5 acceptance evidence is that "proved, failed, and unknown
remain distinct", and `paginate`, the design's own `@prove` example, uses
`take`, `drop`, `push`, and `flatten`; if every std specification were a
conditional assumption, no proof in the language could ever be `proved`
and the strict integration requirement would either reject every candidate
or admit everything, which empties both notions, whereas the design names
the trusted runtime as the base of every proof. Rejected alternative: a
named assumption of a new kind (`std_builtin`) on every proof that uses a
std specification, admitted by default project policy (visible, but it
turns every receipt's `status` into `conditional` and repeats the same
entries in every receipt; a later edition may add it as an opt-in receipt
detail without changing any status).

A precondition fault carries the design's structured shape, as
`semantics.md` S58 fixes it: `code` `contract_violation`, `symbol` the
stub's stable identity, which is its qualified name (S43:
`std.prelude.List.take`), `qualified_name` the same string, `clause_kind`
`requires`, `facet` `contract`, `attributed_to` `caller`, `caller` the
enclosing declaration, `clause` the clause text (`n >= 0`), `input` the
receiver under the key `self` and the other arguments by parameter name,
rendered by the toolchain's value renderer, and `status` `failed`. Division
by zero is the distinct fault `division_by_zero` (S8). The `assert`
statement's fault and resource exhaustion are specified in `semantics.md`
6.7 and `toolchain.md` 1.7.3.

Both engines follow this subsection. The native runtime's builtins
(`cabi.md` Section 2) check their `requires` clauses first, in every
contract mode, with the clause texts, `input` names, and fault shape above,
and never evaluate an `ensures` clause; the precondition facts come from
the one source of S47.

### 10.5 Value representation (reference interpreter)

This table describes the reference evaluator of Milestone 1 (`semantics.md`
S51) and binds nothing else: the Rust `Value` type is `toolchain.md` 1.7's,
and the native backend of the design's Architecture section must satisfy
S15's representation requirements and the observable semantics of this
document, not these Rust types. The representation of the native backend of
Milestone 2 is `cabi.md` Section 2, which meets S15 as this one does.

| CPL type | Rust representation | Notes |
| - | - | - |
| `Int` | `num_bigint::BigInt` | `len`, indices, counts convert from `BigInt` only after clamping |
| `Bool` | `bool` | |
| `Text` | `Rc<String>` (UTF-8) | Unicode scalar counts are cached by weak buffer identity and carried forward through append assignments |
| `Bytes` | `Rc<Vec<u8>>` | Unique append assignments retain capacity; snapshots copy on write |
| `List<T>` | `Value::List(Rc<Vec<Value>>)` | S15; unique appends retain capacity and snapshots copy on write; views are permitted, not required |
| `Unit`, `Option`, `Result`, user enums | `Value::Enum { def, variant, payload: Rc<[Value]> }` | the prelude enums need no special variant |
| structs, wrappers | `Value::Struct { def, fields }`, `Value::Wrapper { def, inner }` | |
| refinements | the base value; the type is static only | `check` returns the same value |
| `Gen<T>` | no `Value`: a `for` range is lowered to the HIR node `ExprKind::Gen(GenExpr)` (`toolchain.md` 3.3, 3.5) that the property runner interprets | never a runtime value (S28) |

### 10.6 Builtin registry

`cpl-eval` exposes `cpl_eval::builtins::lookup(id: &str) -> Option<BuiltinFn>`
with `type BuiltinFn = fn(&mut Interpreter, &[Value]) -> Result<Value, Fault>`
(`toolchain.md` 3.6); the arguments arrive in declaration order (receiver
first). The complete id set for 0.1 (a stub naming any other id is the
check error `std_builtin_missing`, 10.2):

| Group | Ids |
| - | - |
| types | `int` `bool` `text` `bytes` `list` `gen` |
| prelude functions | `flatten` |
| `Int` members | `int_to_text` `int_abs` `int_min` `int_max` |
| `Text` members | `text_len` `text_is_empty` |
| `Bytes` members | `bytes_len` `bytes_is_empty` |
| `List` members | `list_len` `list_is_empty` `list_take` `list_drop` `list_drop_last` `list_push` `list_get` `list_first` `list_last` `list_reverse` `list_contains` |
| `Option` members | `option_is_some` `option_is_none` `option_unwrap_or` `option_ok_or` |
| `Result` members | `result_is_ok` `result_is_err` `result_unwrap_or` `result_ok` `result_err` |
| `std.text` | `text_trim` `text_trim_start` `text_trim_end` `text_starts_with` `text_ends_with` `text_contains` `text_split` `text_join` `text_to_upper` `text_to_lower` `text_parse_int` `text_from_int` `text_chars` |
| `std.bytes` | `bytes_from_list` `bytes_to_list` `bytes_from_text` `bytes_to_text` |
| `std.gen` | `gen_int` `gen_positive_int` `gen_bool` `gen_text` `gen_list` `gen_option` `gen_range` |

The six type ids and the seven `gen_*` ids are registered so that the
check knows them but are not callable: a builtin type is never called, and
a generator is lowered to `ExprKind::Gen` and interpreted by the property
runner (`toolchain.md` T18), so calling one is `internal_error` (`kind`
`builtin_type_called` or `generator_not_a_value`); `gen_range`'s
precondition `lo <= hi` is checked first, by the runner at setup (8.2).

Intrinsics are not in the registry; the interpreter implements them
directly from the typed IR: the `Int` operators (`int_add` `int_sub`
`int_mul` `int_div` `int_rem` `int_neg` and the four orderings), structural
`eq`/`ne`, `concat` for `List`/`Text`/`Bytes` `++`, `not`, short-circuit
`and`/`or`, `forall`, and `check<T>` (Section 1.4). `int_div` and `int_rem`
raise the fault `division_by_zero` of S8, blamed on the enclosing
declaration, through the single `div_rem` function that `toolchain.md` T9
names (from Milestone 2 the one of `cpl-rtspec`, `toolchain.md` T70, which
`cpl-sema`'s constant evaluator calls too and the native runtime's one
division mirrors, `cabi.md` Section 2).

The property runner (`gen_*` consumers) lives in `cpl-eval` and needs
`rand_chacha` (already a dependency) and `blake3` for the property seed
(8.2; `toolchain.md` 1.3 already lists it for derived test seeds, and its
1.3 rationale for `ChaCha20Rng::from_seed`, "a fully specified stream
independent of `rand`'s internal seeding helpers", is exactly why 8.2 uses
`from_seed`). No other external crate is required by this specification
(`cpl-rtspec`, S47, is a crate of the workspace).

Decision: [D-286] S47. Every table and text of this document that a runtime
reproduces has one source, the workspace crate `cpl-rtspec` (`toolchain.md`
T70), from which both engines take it: the `White_Space` ranges of S22; the
builtin registry of this section (its ids, and which of them are callable)
with the `requires` clause texts and `input` names of 10.4 that serve a
program without stubs (every program that a project check produces has the
stubs of Section 11, and both engines then take those facts from the stub
through one function, `cabi.md` Section 3); and the message templates of
every fault and of every value that the runtime builds (the
`ConstraintError` messages of 1.5 and `semantics.md` 6.10, of `parse_int`,
and of `std.bytes.to_text`; the `ForeignError` messages of S45). The
evaluator renders the templates through `cpl-rtspec`, and the native
runtime through C tables generated from it (`cabi.md` Section 2), so the
two cannot drift. Neither engine uses its host language's Unicode
classification or case mapping (Rust's `char::is_whitespace` or
`to_uppercase`, C's `<wctype.h>` or a locale): 0.1 needs none, since S22
enumerates its set, S23 maps ASCII only, and `len`, `chars`, and the
searches work on scalar values and UTF-8 bytes; and a later `std.unicode`
(Section 9) will take one set of tables, generated by `cpl-rtspec` from a
pinned version of the Unicode Character Database, for both engines.
Rationale: two hand-kept copies of one table drift silently, and a scalar
added to one engine's whitespace set or a clause text edited in one place
would make the engines disagree on values and diagnostics that
`semantics.md` S75 requires to be byte-identical; generating the native
tables from the source the evaluator reads removes that drift by
construction, and pinning the Unicode data keeps a toolchain's results
independent of the Rust or C library it was built with, which is the
argument of S22 and S23 applied to both engines. Rejected alternatives: a
copy in the C runtime checked by tests only (drift is found after the fact,
and only where a test looks); each engine's own Unicode library
(version-dependent, as S22 already rejects for Rust).

### 10.7 Conformance

`cpl-eval` must reproduce every row of Section 12, and from Milestone 2 so
must the native backend, byte for byte against the evaluator's outcome
(`semantics.md` S75; suite D2 of `toolchain.md` 12.6). The laws tables (I,
T, B, L, O, X) are to be transcribed into `property` declarations in a
toolchain-internal test module (`std_tests.*`, not shipped) and run with
`gen` at the default parameters; a law that fails is a defect in the
builtin, the law, or the runner, in that order of likelihood. This sampling
is self-test evidence about the implementations; it does not prove a law
and discharges no verifier assumption (S41).

## 11. The stub files

These five files are the normative text of the 0.1 standard library. They
parse under grammar.md plus G1 and G2 (Section 10.2). Ordinary `//`
comments are used for notes that do not document a following declaration,
because a `///` comment must precede a declaration (grammar.md 1.2).

### 11.1 `std/prelude.cpl`

```text
/// The CPL prelude: every pub declaration here is in scope in every module
/// without a use declaration, and the module std.gen is in scope as gen (S1).
module std.prelude

// ---- builtin types ------------------------------------------------------

/// Arbitrary-precision integer.
@builtin("int")
pub type Int

/// Boolean; the literals are true and false.
@builtin("bool")
pub type Bool

/// A sequence of Unicode scalar values.
@builtin("text")
pub type Text

/// A sequence of 8-bit values; built by std.bytes or by adapters.
@builtin("bytes")
pub type Bytes

/// An immutable sequence.
@builtin("list")
pub type List<T>

// ---- prelude data types -------------------------------------------------

/// The type with one value, also written Unit.
pub enum Unit {
    Unit
}

/// A possibly missing value; there is no null.
pub enum Option<T> {
    Some(T)
    None
}

/// Recoverable failure.
pub enum Result<T, E> {
    Ok(T)
    Err(E)
}

/// The error of check<T> and of the standard parsers.
pub struct ConstraintError {
    /// Name of the type whose constraint failed (stdlib.md S44); stable.
    type_name: Text
    /// One human-readable line; not stable across toolchain versions.
    message: Text
}

// check<T>(value) -> Result<T, ConstraintError> is a typechecker intrinsic
// (stdlib.md Section 1.4); its parameter type is the root base of T, which no
// declaration can express, so it has no stub.

// ---- prelude functions --------------------------------------------------

/// Concatenation of a list of lists, in order.
@builtin("flatten")
pub fn flatten<T>(lists: List<List<T>>) -> List<T>
    ensures not lists.is_empty() or result == []
    ensures forall part in lists: part.len <= result.len

// ---- Int members --------------------------------------------------------

/// Decimal rendering with a leading - for negatives.
@method
@builtin("int_to_text")
pub fn to_text(value: Int) -> Text
    ensures result.len >= 1

/// Absolute value.
@method
@builtin("int_abs")
pub fn abs(value: Int) -> Int
    ensures result >= 0
    ensures result == value or result == -value

/// The smaller operand.
@method
@builtin("int_min")
pub fn min(value: Int, other: Int) -> Int
    ensures result == value or result == other
    ensures result <= value and result <= other

/// The larger operand.
@method
@builtin("int_max")
pub fn max(value: Int, other: Int) -> Int
    ensures result == value or result == other
    ensures result >= value and result >= other

// ---- Text members -------------------------------------------------------

/// Number of Unicode scalar values.
@property
@builtin("text_len")
pub fn len(text: Text) -> Int
    ensures result >= 0

@method
@builtin("text_is_empty")
pub fn is_empty(text: Text) -> Bool
    ensures result == (text.len == 0)

// ---- Bytes members ------------------------------------------------------

/// Number of bytes.
@property
@builtin("bytes_len")
pub fn len(bytes: Bytes) -> Int
    ensures result >= 0

@method
@builtin("bytes_is_empty")
pub fn is_empty(bytes: Bytes) -> Bool
    ensures result == (bytes.len == 0)

// ---- List members -------------------------------------------------------

/// Number of elements.
@property
@builtin("list_len")
pub fn len<T>(list: List<T>) -> Int
    ensures result >= 0

@method
@builtin("list_is_empty")
pub fn is_empty<T>(list: List<T>) -> Bool
    ensures result == (list.len == 0)

/// The first n elements; clamps at the length.
@method
@builtin("list_take")
pub fn take<T>(list: List<T>, n: Int) -> List<T>
    requires n >= 0
    ensures result.len == n.min(list.len)
    ensures result ++ list.drop(n) == list

/// All but the first n elements; clamps at the length.
@method
@builtin("list_drop")
pub fn drop<T>(list: List<T>, n: Int) -> List<T>
    requires n >= 0
    ensures result.len == (list.len - n).max(0)
    ensures list.take(n) ++ result == list

/// All but the last element; [] for [].
@method
@builtin("list_drop_last")
pub fn drop_last<T>(list: List<T>) -> List<T>
    ensures result.len == (list.len - 1).max(0)
    ensures result == list.take((list.len - 1).max(0))

/// A new list with value appended; the receiver is unchanged.
@method
@builtin("list_push")
pub fn push<T>(list: List<T>, value: T) -> List<T>
    ensures result.len == list.len + 1
    ensures result == list ++ [value]
    ensures result.drop_last() == list

/// Extension: the element at a zero-based index, or None when out of range.
@method
@builtin("list_get")
pub fn get<T>(list: List<T>, index: Int) -> Option<T>
    ensures result.is_some() == (index >= 0 and index < list.len)

/// Extension: the first element.
@method
@builtin("list_first")
pub fn first<T>(list: List<T>) -> Option<T>
    ensures result == list.get(0)
    ensures result.is_none() == list.is_empty()

/// Extension: the last element.
@method
@builtin("list_last")
pub fn last<T>(list: List<T>) -> Option<T>
    ensures result == list.get(list.len - 1)
    ensures result.is_none() == list.is_empty()

/// Extension: the elements in reverse order.
@method
@builtin("list_reverse")
pub fn reverse<T>(list: List<T>) -> List<T>
    ensures result.len == list.len
    ensures forall x in list: result.contains(x)
    ensures forall x in result: list.contains(x)

/// Extension: whether some element equals value.
@method
@builtin("list_contains")
pub fn contains<T>(list: List<T>, value: T) -> Bool
    ensures result == (not (forall x in list: x != value))

// ---- Option members -----------------------------------------------------

@method
@builtin("option_is_some")
pub fn is_some<T>(option: Option<T>) -> Bool
    ensures result == (not option.is_none())

@method
@builtin("option_is_none")
pub fn is_none<T>(option: Option<T>) -> Bool
    ensures result == (option == None)

/// The payload of Some, or default.
@method
@builtin("option_unwrap_or")
pub fn unwrap_or<T>(option: Option<T>, default: T) -> T
    ensures option.is_none() or option == Some(result)
    ensures option.is_some() or result == default

/// Extension: Some(v) becomes Ok(v); None becomes Err(error).
@method
@builtin("option_ok_or")
pub fn ok_or<T, E>(option: Option<T>, error: E) -> Result<T, E>
    ensures result.is_ok() == option.is_some()
    ensures option.is_some() or result == Err(error)

// ---- Result members -----------------------------------------------------

@method
@builtin("result_is_ok")
pub fn is_ok<T, E>(res: Result<T, E>) -> Bool

@method
@builtin("result_is_err")
pub fn is_err<T, E>(res: Result<T, E>) -> Bool
    ensures result == (not res.is_ok())

/// The payload of Ok, or default.
@method
@builtin("result_unwrap_or")
pub fn unwrap_or<T, E>(res: Result<T, E>, default: T) -> T
    ensures res.is_err() or res == Ok(result)
    ensures res.is_ok() or result == default

/// Extension: the success value, if any.
@method
@builtin("result_ok")
pub fn ok<T, E>(res: Result<T, E>) -> Option<T>
    ensures result.is_some() == res.is_ok()

/// Extension: the error value, if any.
@method
@builtin("result_err")
pub fn err<T, E>(res: Result<T, E>) -> Option<E>
    ensures result.is_some() == res.is_err()
```

### 11.2 `std/text.cpl`

```text
/// Text operations. All are module functions: use std.text.{trim}, then trim(t).
module std.text

/// t without leading and trailing White_Space scalars (stdlib.md S22).
@builtin("text_trim")
pub fn trim(text: Text) -> Text
    ensures result.len <= text.len
    ensures trim(result) == result

@builtin("text_trim_start")
pub fn trim_start(text: Text) -> Text
    ensures result.len <= text.len
    ensures ends_with(text, result)

@builtin("text_trim_end")
pub fn trim_end(text: Text) -> Text
    ensures result.len <= text.len
    ensures starts_with(text, result)

@builtin("text_starts_with")
pub fn starts_with(text: Text, prefix: Text) -> Bool
    ensures not result or prefix.len <= text.len
    ensures not prefix.is_empty() or result

@builtin("text_ends_with")
pub fn ends_with(text: Text, suffix: Text) -> Bool
    ensures not result or suffix.len <= text.len
    ensures not suffix.is_empty() or result

@builtin("text_contains")
pub fn contains(text: Text, needle: Text) -> Bool
    ensures not result or needle.len <= text.len
    ensures not needle.is_empty() or result

/// Pieces between leftmost-first, non-overlapping occurrences of sep.
@builtin("text_split")
pub fn split(text: Text, sep: Text) -> List<Text>
    requires not sep.is_empty()
    ensures result.len >= 1
    ensures join(result, sep) == text
    ensures forall part in result: not contains(part, sep)

/// parts concatenated with sep between consecutive elements.
@builtin("text_join")
pub fn join(parts: List<Text>, sep: Text) -> Text
    ensures not parts.is_empty() or result == ""
    ensures parts.len != 1 or result == parts.get(0).unwrap_or("")

/// ASCII letters to upper case; other scalars unchanged (S23).
@builtin("text_to_upper")
pub fn to_upper(text: Text) -> Text
    ensures result.len == text.len
    ensures to_upper(result) == result

/// ASCII letters to lower case; other scalars unchanged (S23).
@builtin("text_to_lower")
pub fn to_lower(text: Text) -> Text
    ensures result.len == text.len
    ensures to_lower(result) == result

/// Decimal integer: optional + or -, then ASCII digits; nothing else (S25).
@builtin("text_parse_int")
pub fn parse_int(text: Text) -> Result<Int, ConstraintError>
    ensures not text.is_empty() or result.is_err()

/// The canonical decimal rendering; equals value.to_text().
@builtin("text_from_int")
pub fn from_int(value: Int) -> Text
    ensures result == value.to_text()
    ensures parse_int(result) == Ok(value)

/// Extension: the scalar values of text, each as a one-element text.
@builtin("text_chars")
pub fn chars(text: Text) -> List<Text>
    ensures result.len == text.len
    ensures forall c in result: c.len == 1
    ensures join(result, "") == text
```

### 11.3 `std/bytes.cpl`

```text
/// Byte sequences. There is no Bytes literal; build values here.
module std.bytes

/// The bytes with the given values, each in 0..255.
@builtin("bytes_from_list")
pub fn from_list(values: List<Int>) -> Bytes
    requires forall v in values: v >= 0 and v <= 255
    ensures result.len == values.len
    ensures to_list(result) == values

/// The byte values as Ints.
@builtin("bytes_to_list")
pub fn to_list(bytes: Bytes) -> List<Int>
    ensures result.len == bytes.len
    ensures forall v in result: v >= 0 and v <= 255
    ensures from_list(result) == bytes

/// Extension: UTF-8 encoding of a text.
@builtin("bytes_from_text")
pub fn from_text(text: Text) -> Bytes
    ensures result.len >= text.len
    ensures result.len <= 4 * text.len
    ensures to_text(result) == Ok(text)

/// Extension: strict UTF-8 decoding; Err carries type_name "Text".
@builtin("bytes_to_text")
pub fn to_text(bytes: Bytes) -> Result<Text, ConstraintError>
```

### 11.4 `std/foreign.cpl`

```text
/// Typed errors for the foreign boundary (design: Foreign boundary semantics).
module std.foreign

/// Classification of a failure at a foreign boundary.
pub enum ForeignErrorKind {
    /// The foreign code raised an exception or returned an error status.
    Exception
    /// An argument could not be converted to the foreign representation.
    InvalidArgument
    /// A foreign return value could not be converted to, or failed
    /// validation against, the declared CPL type.
    InvalidResult
    /// The worker or transport failed: crash, disconnect, protocol error.
    Transport
    /// The call exceeded the host-imposed time limit.
    Timeout
    /// The call was cancelled by the host before it completed.
    Cancelled
    /// No adapter is bound to the capability, or host policy refused it.
    Unavailable
}

/// One structured diagnostic detail; keys are adapter-defined.
pub struct ForeignDetail {
    key: Text
    value: Text
}

/// A foreign exception or boundary failure mapped to a typed CPL error.
pub struct ForeignError {
    kind: ForeignErrorKind
    message: Text
    details: List<ForeignDetail>
}
```

### 11.5 `std/gen.cpl`

```text
/// Property generators; in scope as gen in every module (S1).
module std.gen

/// The type of generators; not nameable outside std (S28).
@builtin("gen")
pub type Gen<T>

/// Structurally the same type as any user refinement Int where self > 0 (S29).
pub type Positive = Int where self > 0

/// Integers, scaled by the case size; see stdlib.md 8.3.
@property
@builtin("gen_int")
pub fn int() -> Gen<Int>

/// Integers >= 1.
@property
@builtin("gen_positive_int")
pub fn positive_int() -> Gen<Positive>

@property
@builtin("gen_bool")
pub fn bool() -> Gen<Bool>

/// Texts of length <= size over a fixed alphabet (8.3).
@property
@builtin("gen_text")
pub fn text() -> Gen<Text>

/// Lists of length <= size with elements from element.
@builtin("gen_list")
pub fn list<T>(element: Gen<T>) -> Gen<List<T>>

/// None one time in four, else Some of a draw from element.
@builtin("gen_option")
pub fn option<T>(element: Gen<T>) -> Gen<Option<T>>

/// Uniform integers in lo..hi inclusive; the size is ignored.
@builtin("gen_range")
pub fn range(lo: Int, hi: Int) -> Gen<Int>
    requires lo <= hi
```

## 12. Acceptance evaluations

Every row is an `assert` that `cpl-eval` must pass, and from Milestone 2
the native backend too (suite D2 of `toolchain.md` 12.6). `std.text` and
`std.bytes` functions are shown qualified; where `[]`, `None`, or
`Err(...)` needs a type (`[].drop_last()`, `None.unwrap_or(0)`,
`Err("x").ok()`), the value is first bound with an annotated `let`
(`let o: Option<Int> = None`).

| Expression | Value |
| - | - |
| `7 / 2`, `-7 / 2`, `7 / -2`, `-7 / -2` | `3`, `-3`, `-3`, `3` |
| `7 % 2`, `-7 % 2`, `7 % -2`, `-7 % -2` | `1`, `-1`, `1`, `-1` |
| `12345678901234567890 * 10` | `123456789012345678900` |
| `(-5).abs()`, `3.min(5)`, `(-3).max(0)`, `4.min(4)` | `5`, `3`, `0`, `4` |
| `(-5).to_text()`, `0.to_text()`, `1_000.to_text()` | `"-5"`, `"0"`, `"1000"` |
| `[1, 2, 3].take(2)`, `[1, 2, 3].take(5)`, `[1, 2, 3].take(0)` | `[1, 2]`, `[1, 2, 3]`, `[]` |
| `[1, 2, 3].drop(2)`, `[1, 2, 3].drop(5)`, `[1, 2, 3].drop(0)` | `[3]`, `[]`, `[1, 2, 3]` |
| `[1, 2, 3].drop_last()`, `[].drop_last()` | `[1, 2]`, `[]` |
| `[1].push(2)`, `[1] ++ [2, 3]`, `[] ++ []` | `[1, 2]`, `[1, 2, 3]`, `[]` |
| `flatten([[1, 2], [], [3]])`, `flatten([])` | `[1, 2, 3]`, `[]` |
| `[1, 2, 3].len`, `[].len`, `[].is_empty()`, `[1].is_empty()` | `3`, `0`, `true`, `false` |
| `[1, 2, 3].get(1)`, `[1, 2, 3].get(3)`, `[1, 2, 3].get(-1)` | `Some(2)`, `None`, `None` |
| `[1, 2, 3].first()`, `[1, 2, 3].last()`, `[].last()` | `Some(1)`, `Some(3)`, `None` |
| `[1, 2, 3].reverse()`, `[1, 2, 3].contains(2)`, `[1, 2, 3].contains(4)` | `[3, 2, 1]`, `true`, `false` |
| `[[1], [2]] == [[1], [2]]`, `[1, 2] == [2, 1]` | `true`, `false` |
| `forall x in [1, 2, 3]: x > 0`, `forall x in []: false` | `true`, `true` |
| `"héllo".len`, `"😀".len`, `"e\u{301}".len`, `"".len` | `5`, `1`, `2`, `0` |
| `"".is_empty()`, `"a" ++ "b"`, `"\u{e9}" == "e\u{301}"` | `true`, `"ab"`, `false` |
| `std.text.trim("  5 ")`, `std.text.trim("\u{a0}x\u{a0}")`, `std.text.trim("\u{200b}x")` | `"5"`, `"x"`, `"\u{200b}x"` |
| `std.text.trim_start("  a ")`, `std.text.trim_end("  a ")` | `"a "`, `"  a"` |
| `std.text.starts_with("abc", "ab")`, `std.text.starts_with("abc", "")`, `std.text.starts_with("a", "ab")` | `true`, `true`, `false` |
| `std.text.ends_with("abc", "bc")`, `std.text.contains("abc", "b")`, `std.text.contains("abc", "")` | `true`, `true`, `true` |
| `std.text.split("a,b,,c", ",")`, `std.text.split("", ",")`, `std.text.split("a,", ",")`, `std.text.split("aaa", "aa")` | `["a", "b", "", "c"]`, `[""]`, `["a", ""]`, `["", "a"]` |
| `std.text.join(["a", "b"], ", ")`, `std.text.join([], ",")`, `std.text.join(["x"], ",")` | `"a, b"`, `""`, `"x"` |
| `std.text.to_upper("abc\u{e9}")`, `std.text.to_lower("ABC")` | `"ABC\u{e9}"`, `"abc"` |
| `std.text.parse_int("42")`, `std.text.parse_int("-0")`, `std.text.parse_int("+7")`, `std.text.parse_int("007")` | `Ok(42)`, `Ok(0)`, `Ok(7)`, `Ok(7)` |
| `std.text.parse_int("99999999999999999999")` | `Ok(99999999999999999999)` |
| `std.text.parse_int(" 42").is_err()`, `std.text.parse_int("").is_err()`, `std.text.parse_int("4_2").is_err()`, `std.text.parse_int("\u{ff11}").is_err()`, `std.text.parse_int("-").is_err()` | `true` for each |
| `std.text.from_int(-12)`, `std.text.chars("h\u{e9}y")` | `"-12"`, `["h", "\u{e9}", "y"]` |
| `std.bytes.from_list([104, 105]).len`, `std.bytes.to_list(std.bytes.from_list([1, 2]))` | `2`, `[1, 2]` |
| `std.bytes.from_text("\u{e9}").len`, `std.bytes.to_text(std.bytes.from_text("\u{e9}"))`, `std.bytes.to_text(std.bytes.from_list([255])).is_err()` | `2`, `Ok("\u{e9}")`, `true` |
| `Some(1).unwrap_or(0)`, `None.unwrap_or(0)`, `Some(1).is_some()`, `None.is_none()` | `1`, `0`, `true`, `true` |
| `Some(2).ok_or("e")`, `None.ok_or("e")` | `Ok(2)`, `Err("e")` |
| `Ok(1).unwrap_or(0)`, `Err("x").unwrap_or(0)`, `Ok(1).is_ok()`, `Err("x").is_err()` | `1`, `0`, `true`, `true` |
| `Ok(1).ok()`, `Err("x").err()`, `Ok(1).err()` | `Some(1)`, `Some("x")`, `None` |
| `check<Positive>(3)`, `check<Positive>(0).is_err()` | `Ok(3)`, `true` |
| in `reader.pages`: `match check<Positive>(0) { Err(e) => e.type_name  Ok(_) => "" }`; the same with `check<gen.Positive>(0)`; with `check<Small>(0)` and `check<Small>(20)` for `type Small = Positive where self < 10` declared in a module `m` | `"reader.pages.Positive"`, `"std.gen.Positive"`, `"reader.pages.Positive"`, `"m.Small"` (S44) |
| `match std.text.parse_int("x") { Err(e) => e.type_name  Ok(_) => "" }` | `"Int"` (S44) |
| `Unit == Unit`, `PageId("a") == PageId("a")` | `true`, `true` |

Faults (each is a `contract_violation` with `clause_kind: "requires"` and
the clause shown, except the division row, which is the fault
`division_by_zero` with `symbol` the enclosing declaration and `input:
{"a": 1, "b": 0}`, S8):

| Expression | Symbol | Clause |
| - | - | - |
| `[1].take(-1)`, `[1].drop(-1)` | `std.prelude.List.take` / `.drop` | `n >= 0` |
| `1 / 0`, `1 % 0` | the enclosing declaration (code `division_by_zero`) | none; `b != 0` is the verifier obligation only |
| `std.text.split("a", "")` | `std.text.split` | `not sep.is_empty()` |
| `std.bytes.from_list([256])`, `std.bytes.from_list([-1])` | `std.bytes.from_list` | `forall v in values: v >= 0 and v <= 255` |
| `for x in gen.range(5, 1)` (at property setup) | `std.gen.range` | `lo <= hi` |

Check errors (each must be rejected by `cpl check`): `xs.len()`,
`xs.is_empty`, `take(xs, 2)`, `"a" + "b"`, `"a" < "b"`, `true < false`,
`xs.unwrap()`, `let g = gen.int`, `gen.int()`, `use std.gen.{Gen}`,
`use std.gen.{int}`, `check<PageId>("a")`, `check<Int>(1)`,
`let x: Int64 = 1`, `pub struct Int64 { lo: Int }`, `pub type Gen(Text)`,
`use std.time.{Clock}`, `List<PageRead>`, `List<Gen<Int>>`,
`gen.int == gen.int`, `fn flatten(xs: List<Int>) -> Int { xs.len }` at
module level, `xs.push(1)` as a non-tail statement, `opt?` for
`opt: Option<T>`, and in a project file `fn f() -> Int` with no body
(`missing_body`, `function without a body requires @builtin`: a check
error, not a syntax error).

## 13. Indices

### 13.1 Decisions

| Id | Rule |
| - | - |
| S1 | prelude scope = `pub` declarations of `std.prelude`, `gen` for `std.gen`, and every module-path root including `std`; `std` and `gen` are reserved module paths |
| S2 | the unit value is the constructor `Unit` of `enum Unit { Unit }`; no `()`, no `unit` |
| S3 | prelude names cannot be redefined at module level (`semantics.md` S6); only locals shadow; `use std.prelude.{x}` is redundant and permitted; prelude enums take part in D44 resolution |
| S4 | `check<T>` is an intrinsic; `T` must be a refinement; argument type is the root base |
| S5 | `ConstraintError` is a struct `{ type_name: Text, message: Text }` |
| S6 | `flatten` is a prelude function only; no method form |
| S7 | `==` on every data type (value type: no capability, no `Gen`) and nothing else; neither is a type argument; `<` only on `Int` |
| S8 | `/` truncates toward zero, `%` has the dividend's sign, division by zero is the fault `division_by_zero`; `b != 0` is a verifier obligation |
| S9 | `min`/`max` are `Int` methods |
| S10 | `to_text()` renders the canonical literal; `from_int` equals it |
| S11 | one integer type `Int`, no floating point; `Int8`..`UInt64`, `Float32`, `Float64`, `Gen` are reserved, unusable names |
| S12 | `++` concatenates `Text` (and `Bytes`); `+` on `Text` is an error |
| S13 | `Text` has no ordering, indexing, character type, or slicing |
| S14 | `Bytes` from `std.bytes.from_list` with a `requires` range; `to_text` is a `Result`; `from_text`/`to_text` are extensions |
| S15 | `List` representation: O(1) `len`/`get`/`is_empty`; linear everything else; shared views in the reference |
| S16 | negative `take`/`drop` arguments fault; large ones clamp |
| S17 | `forall` ranges over `List<T>` only, left to right, short-circuit |
| S18 | extensions `get`, `first`, `last`, `reverse`, `contains`; no `map`/`filter`/`fold`/`sort`/indexing |
| S19 | no `unwrap()`/`expect()` on `Option` or `Result` |
| S20 | extensions `ok_or`, `ok`, `err`; no other combinators; no `?` on `Option` |
| S21 | `std.text` operations are module functions, not members |
| S22 | `trim` whitespace is the enumerated Unicode `White_Space` set (25 scalars) |
| S23 | `to_upper`/`to_lower` are ASCII-only |
| S24 | `split` requires a non-empty separator; leftmost-first non-overlapping; round-trips with `join` |
| S25 | `parse_int` accepts `("+" \| "-")? ASCII_DIGIT+` only; leading zeros allowed |
| S26 | `chars` extension is the only way to decompose a `Text` |
| S27 | `ForeignError` is a struct of `ForeignErrorKind` (7 variants), `message`, `details: List<ForeignDetail>` |
| S28 | `Gen<T>` is not nameable and no `std.gen` item is importable; generator expressions only in `for` ranges and `std.gen` arguments |
| S29 | refinement types are structural (same base, identical resolved predicate); `std.gen.Positive` is the user's `Positive` |
| S30 | `for` clauses are independent; ranges are contract expressions evaluated once |
| S31 | run parameters (100 cases, size `min(i, 100)`); one ChaCha20 stream per property keyed by the 32-byte blake3 digest of the run seed and the property's identity (`semantics.md` S44); reproducible per toolchain version |
| S32 | greedy deterministic shrinking with fixed candidate orders; same fault code; bounded by `max_shrinks` |
| S33 | all `std.*` paths reserved; unimplemented ones give a milestone-naming error; 0.1 implements exactly five std modules |
| S34 | std stub files are embedded in the toolchain and loaded before user modules |
| S35 | stubs are bodiless `@builtin` functions and definition-less `@builtin` types; grammar deltas G1 and G2 (a bodiless `fn` becomes a check error, not a syntax error); `@method`/`@property`; std-only; Section 11 is the single normative stub text |
| S36 | every std `fn` in 0.1 is a builtin; no CPL bodies in std |
| S37 | member stubs are keyed by (receiver head, name) for the seven prelude heads, named `std.prelude.<Receiver>.<name>`, never callable as free functions |
| S38 | `e.name` resolution belongs to `semantics.md` 2.4 and 3.9; this document fixes property/method use and that any expression may be a receiver |
| S39 | builtin preconditions are checked by the implementation; postconditions are trusted and never evaluated (a deliberate exemption from checked mode) |
| S40 | a zero-parameter `@property` is a module-level property (`gen.int`): no receiver, no member table, read without parentheses |
| S41 | std specifications are trusted-runtime axioms for the verifier, recorded as receipt dependencies plus the std digest, not as conditional assumptions; law sampling proves nothing |
| S42 | no capability handle can be obtained in Milestone 1; `using` functions typecheck but do not run; fixtures reserved in the test receipt; from Milestone 2 C bindings supply handles to `cpl run` (`semantics.md` S78), and tests stay pure |
| S43 | std declarations carry no `@id`; their stable identity is their qualified name |
| S44 | `ConstraintError.type_name` rendering: prelude types bare, others qualified; the first failing refinement reached from the call-site type |
| S45 | how a C binding produces each `ForeignErrorKind`; the message templates; the fixed detail keys and their order (`adapter`, `capability`, `operation`, the declarative pair, the adapter's own, the substitution pair) |
| S46 | the bounded integer types stay reserved in Milestone 2; the C boundary's machine integers are per-export, binding-level checked conversions, not language types |
| S47 | one source (`cpl-rtspec`) for the `White_Space` set, the builtin registry with its precondition texts and inputs, and the message templates; no host-language Unicode classification or case mapping in either engine; a later `std.unicode` from a pinned UCD |

Design conflicts recorded: Section 1.1 (prelude contents versus the
`reader.pages` imports), Section 8.1 (`gen.positive_int` versus the
user-declared `Positive`). Section 2.5 (bounded integers) is a scope
decision of this document (S11, S46), not a design conflict.

### 13.2 Shared rules and where they live

The rules below are stated by `semantics.md` and relied on by this
document; the consistency pass of 2026-10-28 confirmed that the two
documents agree on each row it found; the last three rows were added with
Milestone 2 (2026-10-29), for the next pass to confirm.

| Topic | Shared rule | `semantics.md` | Cited in |
| - | - | - | - |
| division | truncation toward zero, `%` with the dividend's sign, the fault `division_by_zero`, and `b != 0` as a proof obligation under `@prove` from Milestone 5 | 6.3, 6.7, 5.1, S37 | S8 |
| refinement widening | a refinement value is accepted where its base is expected; operator results are base-typed | 3.4 | 0.3 |
| refinement identity | structural (S29): same base, identical resolved predicate (names resolved to the same declaration), so two `Text where ok(self)` with different `ok` functions are distinct types | S55 | S29 |
| type aliases | none (D64); `check<T>` resolves through no alias | 3.3 | 1.4 |
| literal refinement proof | a constant expression satisfies a refinement whose predicates evaluate to `true`; other expressions are rejected with the `check` hint | 3.5, S16 | 1.4 |
| `==` typing | the operand with a known type fixes the expected type of the other (`None`, `Some(x)`, `Ok(x)`, `Err(x)`, `[]`) | 3.6, S17 | 0.3 |
| data types | capabilities and generators are not type arguments; equality exists on all value types | S13, S14, S17 | S7 |
| `and`/`or` | short-circuit in code and in contracts | 6.2 | 3.1 |
| `forall` | over `List<T>`, left to right, short-circuit, vacuous truth | 3.8, 6.2 | S17 |
| statements | non-tail expression statements are `Unit` (`unused_value`); `let _ =` discards | 3.11 | 4.6 |
| `?` | on `Result` only; the error type must be identical to the enclosing function's; the design's explicit-conversion form is deferred and a conversion is written before the `?` | 3.8, Section 10 | 5.2 |
| prelude resolution | S1 and S3: the prelude scope, `prelude_name_redefined` (the reserved names of S11 included), `use std.prelude.{x}` permitted and redundant, `use std.gen.{...}` is `generator_import` | 1.6, 1.7, 2.1, S6, S54 | 1.1, 1.3, S28 |
| callees | a callee is a name chain or a postfix expression ending in `.member_name`, so a method call's receiver may be any expression (`[1, 2, 3].take(2)`, `(-5).abs()`) | S11 | S38 |
| seeds and shrinking | one ChaCha20 stream per property keyed by the 32-byte blake3 digest of the run seed and the identity (`from_seed`); greedy same-fault-code shrinking that continues with the accepted clause; distributions and candidates are 8.3 and 8.4 | S44, S45 | S31, S32 |
| generator spelling | `gen.int()` is `property_called`; a `Gen<T>` value outside a `for` range or `std.gen` argument is `generator_outside_for` | 3.8, S14 | S28, S40 |
| generic inference | type arguments inferred from call arguments (`flatten(result)`, `gen.list(gen.int)`) | 3.7, S19 | 8.1 |
| builtin contracts | builtins check their `requires`, trust their `ensures`; the fault shape of 10.4 | S58, 6.10 | S39 |
| builtin self-reference | a clause of a `@builtin` declaration may name the declaration itself (`ensures trim(result) == result`) and is not `recursive_contract` | S61 | 6.1, 10.4 |
| distinct refinements | two refinements of one root base with different resolved predicates: `cannot_prove_refinement` with the predicate hint, never `type_mismatch` | S65 | S29 |
| native equivalence | the native backend produces the evaluator's values and diagnostics byte for byte, except the divergences it lists | S75 | S47 |
| foreign results | a validated binding's result is checked for shape, then for its refinements in S74 order, before the operation's `ensures`; a failure is `InvalidResult` for a `Result<T, ForeignError>` return type and otherwise a fault, as is a reported failure that the return type cannot carry | S77, S80 | S45, S46 |
| handles for `cpl run` | `cpl run` binds `using` parameters from backend-`c` bindings; tests bind nothing | S78, S30 | S42 (Milestone 2 note), 7 |

`grammar.md` carries G1 and G2 (its 3.9 and 3.5), the three annotation rows
(3.3), the callee wording of 3.12 rule 4, and the corpus note that
`std/*.cpl` parses (Section 1 introduction). `toolchain.md` carries the
same stub model in its 3.6 (bodiless `@builtin` stubs, `@method`/`@property`,
registry keyed by `@builtin` id, receiver heads `Int`, `Bool`, `Text`,
`Bytes`, `List`, `Option`, `Result`, no `@id` in std, S43), T19's lowering
to `std.prelude.<Receiver>.<name>` (S37), the seed derivation and shrinking
rule of S31/S32 in its 1.7.2 and T45, the fault JSON shape of Section 10.4
in its 1.7.3, the `cpl check` rules of 10.2 and 12 through `semantics.md`
1.8, and std declarations among a verification receipt's `dependencies`
(S41) in its 8.1.

### 13.3 Laws

| Group | Ids | Section |
| - | - | - |
| Int | I1-I5 | 2.3 |
| Text | T1-T4 | 3.3 |
| Bytes | B1-B3 | 3.4 |
| List | L1-L19 | 4.3, 4.5 |
| Option/Result | O1-O4 | 5.2 |
| std.text | X1-X7 | 6.2 |

### 13.4 Operations the design names

Everything in this document outside this table is an addition of this
document (Section 0 conventions).

| Design text | Operations | Section |
| - | - | - |
| Core syntax: "The prelude supplies Int, Bool, Text, Bytes, Unit, List, Option, Result, and their constructors" | the eight prelude types and `Some`, `None`, `Ok`, `Err`, `Unit` | 1.1 |
| Core syntax: `use std.text.{trim}`, `trim(page.text)` | `std.text.trim` | 6 |
| Core syntax: `check<Positive>(n)` returns `Result<Positive, ConstraintError>` | `check<T>`, `ConstraintError` | 1.4, 1.5 |
| Expressions and errors: "A postfix ? unwraps Ok or returns Err" | `r?` on `Result` | 5.2 |
| Expressions and errors: "List<T> supports len, is_empty(), take(n), drop(n), drop_last(), and push(value) ... ++ concatenates lists, and flatten concatenates a list of lists" | `List.len`, `is_empty()`, `take(n)`, `drop(n)`, `drop_last()`, `push(value)`, `++` on lists, `flatten` | 4.2, 1.6 |
| Tests and properties: `gen.list(gen.int)`, `gen.positive_int` | `gen.list`, `gen.int`, `gen.positive_int` | 8 |
| Adapter example: `use std.foreign.{ForeignError}`; Foreign boundary semantics: "a `ForeignError` with structured classification and diagnostic details" | `std.foreign.ForeignError` | 7 |
