# Automatic verification, 2026-11-04

This amendment adds the verifier that `spec/toolchain.md` 8.1 and
semantics S36 anticipated: `cpl-prove` decides the proof obligations of a
function, and `cpl prove`, `cpl check`, receipts, and release checks report
its results in place of Milestone 1's `unknown` / `verifier_unavailable`.
It also decides behavioral equivalence of two versions of a function, which
change contracts use for `preserve_behavior` (`spec/evolution.md`).

## Guarantees

The verifier is **sound**: it reports `proved` for an obligation only when
the obligation holds on every input that satisfies the function's
preconditions and argument types, and `failed` only together with inputs on
which it fails. It **decides its fragment**: an obligation that involves
only the constructs listed under *Exact fragment* is `proved` or `failed`,
except when a resource limit is reached (`limit`) or, for conditions whose
variables are eliminated with coefficients other than ±1 (multiplication
or division by constants), when integer tightening of the rational
elimination does not settle integer feasibility (`integer_reasoning`; the
elimination is exact whenever every eliminated variable has unit
coefficients, the usual case for program conditions). Everything else is
`unknown` with a reason; `unknown` never weakens `@prove` (S36).

General verification is undecidable, so no verifier proves every true
obligation. Outside the fragment the verifier still proves what follows from
contracts and invariants, and reports why it stopped otherwise.

## Method

The function is executed symbolically over all inputs: parameters are fresh
symbolic values constrained by their types (refinement predicates, struct
invariants) and the function's `requires` clauses. Every branch (`if`,
`match`, short-circuit `and`/`or`, faults) forks a path with its condition.
At each obligation site the verifier records, per path, the path condition
with the obligation negated, and decides it with a procedure for
quantifier-free linear integer arithmetic: negation normal form, case
splitting with pruning, exact elimination of unit equalities, a gcd test,
and Fourier–Motzkin elimination with integer tightening. An unsatisfiable
condition on every path proves the obligation. A satisfying model is a
counterexample when the path abstracted nothing (below); it is rebuilt by
back-substitution and checked against the condition, so a reported model
always satisfies it.

### Exact fragment

* `Int` and `Bool` values with `+`, `-`, unary `-`, comparisons, `==`,
  `!=`, `not`, `and`, `or`; multiplication and division (`/`, `%`,
  truncating) by constants.
* `if`, `match` (constructor, literal, wildcard, binding, and wrapper
  patterns, nested), `let`, `var` assignment, `return`, `assert`,
  `concurrent` blocks (their result is that of running the tasks in order).
* Structs, enums (an opaque enum value is resolved per path by a variant
  tag; equality of enum values compares tags and payloads), wrappers,
  `Option`, `Result`, `check<R>(x)`, `?`, and lists of known length
  (`[a, b]`, `++` of such lists, `len`, `is_empty`, `push`, and `forall` over
  them).
* Refinement types and struct invariants: assumed for parameters, decided
  at struct literals and `check<R>`.
* Calls to functions without effects that are not on the call stack: their
  bodies are executed in place at their generic instance, up to five calls
  deep; `abs`, `min`, and `max` are exact.
* Text literals and their lengths.

### Abstractions

Each abstraction over-approximates, so a proof stays sound; a path that used
one is *inexact*, and a model of an inexact path is a *candidate*, reported
as `unknown` evidence and never as `failed`.

* A call that is not executed in place (recursive, effectful, too deep) is
  an uninterpreted value constrained by the callee's `ensures`. A callee's
  `requires` are obligations of the caller (`call_requires`) and are then
  assumed. A `@builtin`'s clauses are its axioms (S61) and are assumed
  whenever it returns; axioms are expanded two levels deep.
* A `while` loop is cut by its invariants: the locals it assigns take
  arbitrary values satisfying them, the body is checked once from there
  (`invariant_preserve`, `loop_decreases`), and the loop's exit continues
  with the invariants and the negated condition. Invariants are checked at
  runtime on entry and after each iteration (semantics 5.3), so they hold
  there.
* Capability operations return uninterpreted values constrained by their
  `ensures`; their `requires` are obligations.
* Text, bytes, lists of unknown length, generic values, `parallel`
  expressions, generators, and holes are uninterpreted values. A hole stands
  for any expression of its type, so a proof covers every completion.
* Products and quotients of two unknowns are fresh variables constrained by
  sign and magnitude facts (for example `a >= 1 and b >= 1` implies
  `a * b >= max(a, b)`, and `a == b * (a / b) + a % b` with the remainder's
  sign and bound). A model must also satisfy the products exactly; the
  verifier repairs one by fixing the offending factors and solving again,
  and otherwise reports `nonlinear_arithmetic`.

### Obligation sites and statuses

The verifier decides every obligation of toolchain 8.1 except
`refinement` (established by the type checker) and `termination` (a missing
measure): `ensures`, `decreases` (on entry the measure is nonnegative, and
at each direct recursive call the callee's measure is smaller than the
caller's), `invariant_init`, `invariant_preserve`, `loop_decreases`,
`call_requires`, `division`, `assert`, and `constructor_invariant` (struct
literals the checker did not establish by bounded evaluation). A function's
own `requires` are assumptions; clauses are evaluated in canonical order,
each assuming the earlier ones, as checked mode evaluates them.

| Status | Evidence |
| - | - |
| `proved` | `{"by": "linear_arithmetic", "paths": n}`; `{"by": "unreachable", "paths": 0}` when no path reaches the site |
| `failed` | `{"model": {param: value JSON}, "concrete": true}`: inputs that reach the site and violate the obligation |
| `unknown` | `null`, or `{"candidate": {param: value JSON}}` with the inputs of a model the verifier could not confirm |

| `unknown` reason | Meaning |
| - | - |
| `unsupported_expression` | the obligation depends on an abstracted construct (above) |
| `invariant_too_weak` | the only abstraction is a loop's invariants, and a state they allow violates the obligation: strengthen them |
| `nonlinear_arithmetic` | a model exists only for the linear relaxation of products or quotients |
| `integer_reasoning` | the rational relaxation is feasible but no integer model was found |
| `limit` | 4096 paths, 4096 solver cases, or 2000 derived constraints per case were exceeded |
| `indirect_recursion` | the function reaches itself through another function, so its `decreases` measure is not compared at a direct call |
| `has_errors` | the function's own declaration failed to check |
| `no_measure` | `termination`: recursive without a header `decreases` (unchanged) |

A function's status is `failed` when some obligation failed, `unknown` when
some obligation is unknown (the first one's reason) or termination is unknown
(a recursion or a `while` loop without a measure, reason `no_measure`), and
`proved` otherwise; a function without obligations is `proved`. The receipt's
verifier is `{"name": "cpl-prove", "version": "<crate version>",
"configuration": {"theory": "linear_integer_arithmetic"}}`.

`cpl check` reports the status of every `@prove` function as its
`verification_result` (with `obligation` and `model` for a failed one, and a
note at the failing site); `--release` accepts `proved` and rejects `unknown`
and `failed` (`verification_unsatisfied`, S36, whose message names the
reason). A release check's `hole_in_release` marks its definition failed only
at the release-policy stage, so both modes verify the same program (S47).
`cpl prove` exits 0 for `proved` and 1 otherwise (T29).

## Equivalence

`cpl_prove::equivalent(old, f, new, f', same)` decides whether `f'` behaves
like `f` on every input that satisfies `f`'s `requires`: the same returned
value or the same fault, after the same sequence of effects, in checked
mode (contracts are part of the behavior: `f'`'s stronger `requires`, or a
violated invariant, is a fault). Both versions are executed symbolically on
shared inputs; each pair of paths must agree wherever both are feasible.

* `same` names the definitions identical in both programs together with
  everything they call; calls to them are uninterpreted functions shared by
  both sides, with their contracts and the possibility of a fault inside.
  Other callees are executed in place per side.
* A recursive call of the function itself is the same uninterpreted
  function on both sides: by induction on the call depth this proves
  *partial* equivalence (agreement whenever both terminate), flagged as
  `"partial": true`.
* A loop whose source text, callees, and input locals are the same on both
  sides is one uninterpreted function of its inputs (also partial).
* Capability operations answer as a function of the operations performed so
  far, so equal traces receive equal answers; differing traces differ.
* Definitions are matched by stable identity and shape; a type whose
  definition differs is distinct on the two sides, and a changed parameter
  or return type is `unknown` (`signature_changed`).

The result is `proved` (`{"by": "symbolic_equivalence", "pairs": n,
"partial": bool}`), `failed` with `inputs` from an exact pair of paths, or
`unknown` with candidate inputs, which `cpl-evolve` runs on both versions
to confirm a difference (`spec/evolution.md`).

## Performance

Values defined by cases (`abs`, `min`, `max`, `and`/`or` over plain
values) are disjunctive facts on one path rather than forks, so the
number of paths grows only with the program's own branches. Each condition
is split into groups of facts that share no variable and decided group by
group (the obligation's group first, which alone settles a proof), with
the outcomes of identical groups shared across the paths and obligations of
a function. `cpl-index` keeps one report per function and snapshot.
`cargo run --release -p cpl-prove --example prove_bench [project] [iterations]`
times every function of a project; `examples/reader`'s `paginate`, whose
loop and builtin axioms produce the largest conditions there, takes about
2 ms.

## Dependency boundary

`cpl-prove` depends on `cpl-hir`, `cpl-syntax` (the operator types), and
`cpl-diag`; the checker is a dev-dependency of its tests. `cpl-sema`
(verification statuses), `cpl-index` (obligation statuses, receipts), and
`cpl-evolve` (equivalence) depend on it.
