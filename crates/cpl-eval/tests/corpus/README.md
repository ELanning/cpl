# cpl-eval interpreter corpus

Spec-derived fixtures for the reference evaluator (`cpl-eval`). Written on
2026-10-28 from the specifications alone, before a typechecker or an
interpreter existed, so every expected value below was derived by hand from
`spec/semantics.md`, `spec/stdlib.md`, `spec/toolchain.md`, and
`spec/grammar.md`.

Status (2026-10-29): `crates/cpl-eval/tests/corpus.rs` is the harness of
`spec/toolchain.md` 12.5 (T49). It checks each module with the real checker
(`cpl-sema`, loaded as `corpus.<name>` at `src/corpus/<name>.cpl` with a
synthetic `cpl.toml`) and runs every case through `run_function` with
`RunArg::Json` in checked mode with the default `EvalConfig`: 74 cases, 74
pass. The same file also compares the fields the `// why:` comments state
for each fault case (`the_fault_fields_the_why_comments_state`) and the
diagnostics of the "checking" row below
(`the_modules_check_with_exactly_the_diagnostics_named_here`). The earlier
run of these cases on a hand-built lowering of the modules is retired.

## 1. Format

This is the "interpreter corpus" row of `spec/toolchain.md` Section 12:
`crates/cpl-eval/tests/corpus/*.cpl` plus `*.run.json`, "each case
`{ "function", "args", "expect": { "value": json } | { "fault": code } }`".

- `<name>.cpl` is a complete module whose header is `module corpus.<name>`.
- `<name>.run.json` is a JSON array of case objects with exactly the three
  keys `function`, `args`, `expect`; `expect` has exactly one key, `value`
  or `fault`. No other field appears anywhere (see A1 below for why).

`spec/toolchain.md` 12.5 (T49, [D-237]) now fixes how each field is read;
the table below restates those readings as this corpus applies them:

| Field | Reading |
| - | - |
| `function` | the *qualified name* of a function declared in the paired module (`corpus.lists.take_n`), the form `cpl run <function>` and `cpl_eval::run::run_function(SymbolPath)` take (toolchain 7.2, 1.7). Public and private functions alike (semantics S39). |
| `args` | a JSON array with one *value JSON* (toolchain 7.3, semantics S40) per positional parameter, decoded against the parameter's declared type exactly as `cpl run --arg JSON` decodes it. A refinement-typed parameter has its predicate evaluated during decoding; an argument that fails it is the fault `refinement_violation` (semantics S39 (a); A3 below). |
| `expect.value` | the value JSON of the returned value, encoded against the function's *declared return type*: a refined `Int` is a plain number, `$type` of a prelude type is its bare name (`Result`, `Option`, `Unit`, `Bytes`, `ConstraintError`), `$type` of a corpus type is qualified (`corpus.values.Point`), `$payload` is omitted for payload-less variants. An `Err(...)` result is a value, not a fault (semantics 6.6, S39 (d)). Large `Int`s are plain JSON numbers with every digit; the reader must preserve precision (toolchain 7.3: `serde_json` `arbitrary_precision`). |
| `expect.fault` | one fault code of semantics 6.7 / 9.4 (`contract_violation`, `division_by_zero`, `assertion_failed`, `unresolved_hole`, `refinement_violation`). Only the code is compared by the format; the other fields the fault must carry (`clause_kind`, `clause`, `symbol`, `attributed_to`, `input`, `actual`, `iteration`, ...) are stated in the `// why:` comment directly above the function in the `.cpl` file and summarised in Section 2, and the `corpus.contracts` functions are written so that each can violate exactly one clause kind, so the code plus the function name identify the kind. |
| execution | checked mode (the only Milestone 1 mode, semantics 5.2), default `EvalConfig` (`max_call_depth` 10000), the five `std/*.cpl` stubs loaded, each case a fresh evaluation, and no calling declaration (so a top-level `requires` violation carries no `caller`, semantics S39 (c)). |
| checking | every module must pass `cpl check` with no errors. `holes.cpl` reports two `hole` (info) and one `verification_result` (info); `reader.cpl` reports one `verification_result` (info). Every other module checks with no diagnostic at all (confirmed by the checker). |

JSON string escapes are used for every non-ASCII scalar in `*.run.json`
(`"\u00e9"`, `"e\u0301"`, `"\ud83d\ude00"`) so the files are ASCII; the
`.cpl` files are ASCII too (non-ASCII is only legal inside text literals and
comments, grammar 1.1) and spell scalars with `\u{...}` in comments.

## 2. Cases by module (74 cases, 12 modules)

The "spec" column names the rules each case pins. "CV" abbreviates the fault
`contract_violation`; the clause kind, symbol, and clause text in that column
are what the fault must carry (semantics 5.6, 6.10, 9.4), beyond what the
`.run.json` compares.

### `ints` (7) - Int arithmetic

| Case | Expected | Spec |
| - | - | - |
| `div(-7, 2)`, `div(7, -2)` | `-3`, `-3` | truncation toward zero: semantics 6.3 / S37, stdlib 2.2 worked values; floor division gives -4 |
| `rem(-7, 2)`, `rem(7, -2)` | `-1`, `1` | remainder has the dividend's sign: S37, stdlib S8; floored gives 1 and -1, Euclidean 1 and 1 |
| `div(1, 0)` | fault `division_by_zero` | an intrinsic, so never a CV (stdlib S8); category runtime, `symbol` `corpus.ints.div`, `input {"a": 1, "b": 0}` |
| `mul(12345678901234567890, 10)` | `123456789012345678900` | arbitrary precision (stdlib 12 row); JSON number with every digit (toolchain 7.3) |
| `add(9223372036854775807, 1)` | `9223372036854775808` | i64::MAX + 1: a machine-integer fast path wraps or panics |

### `text` (4) - Text as scalar values

| Case | Expected | Spec |
| - | - | - |
| `concat_len("\u{e9}", "\u{1f600}")` | `2` | `++` on Text (semantics S21, stdlib S12); `len` counts scalars, not bytes (6) or UTF-16 units (3): semantics 6.4, stdlib 3.3 |
| `length("h\u{e9}llo")` | `5` | stdlib 3.3 / 12 row |
| `length("e\u{301}")` | `2` | combining mark is one scalar; no grapheme clustering |
| `same("\u{e9}", "e\u{301}")` | `false` | no normalization, ever: stdlib 3.3, semantics 6.5 |

### `std_funcs` (5) - std.text / std.bytes through `use` and through the `std` root

| Case | Expected | Spec |
| - | - | - |
| `trimmed("\u{a0}x\u{a0}")` | `"x"` | S22 White_Space set includes U+00A0; stdlib 12 row |
| `pieces("aaa", "aa")` | `["", "a"]` | leftmost-first non-overlapping split, S24 worked value |
| `parsed("007")` | `Ok(7)` | S25 accepts leading zeros in data; `007` as a source literal is a lexical error (grammar 1.5) |
| `parse_failure_type("-")` | `"Int"` | S44 bare prelude name; only `type_name` is stable (stdlib 1.5), so the message is not compared |
| `pack([104, 105])` | `{"$type": "Bytes", "hex": "6869"}` | Bytes value JSON (toolchain 7.3); `std.bytes.from_list` reached through the always-in-scope `std` root (stdlib S1) |

### `lists` (10) - the design's List operations at their boundaries

| Case | Expected | Spec |
| - | - | - |
| `take_n([1, 2, 3], 5)` | `[1, 2, 3]` | clamps at the length (design; stdlib S16) |
| `take_n([1, 2], 10000000000000000000000)` | `[1, 2]` | clamp *before* converting the count to a machine index (stdlib 4.1 implementation note) |
| `take_n([1], -1)` | CV: `requires`, `symbol` `std.prelude.List.take`, `clause "n >= 0"`, `attributed_to caller`, `caller corpus.lists.take_n`, `input {"self": [1], "n": -1}` | negative is a fault, not a clamp: stdlib S16, semantics 6.10, S58 |
| `without_last([])` | `[]` | design: `drop_last()` of an empty list is empty |
| `push_keeps_original([1], 2)` | `[[1], [1, 2]]` | immutability: semantics 6.1, stdlib 4.2 |
| `flat([[1, 2], [], [3]])` | `[1, 2, 3]` | stdlib 1.6, law L7 |
| `element([1, 2, 3], -1)` | `None` | negative index is `None`, not from-the-end indexing and not a fault (stdlib 4.5); `None` value JSON has no `$payload` |
| `all_divide([])` | `true` | vacuous truth (stdlib S17, semantics 6.2) |
| `all_divide([-1, 0])` | `false` | left to right, stops at the first `false` before reaching `10 / 0` (S17) |
| `all_divide([1, 0])` | fault `division_by_zero` | a fault inside the predicate is the fault of the enclosing expression (S17) |

### `values` (8) - structs, wrappers, Unit, equality

| Case | Expected | Spec |
| - | - | - |
| `moved(Point{1,2}, 5)` | `[Point{1,2}, Point{6,2}]` | spread copies and replaces; the base is unchanged (semantics 6.1); struct value JSON with fields in declaration order (toolchain 7.3, T30) |
| `points_equal(Point{1,2}, Point{1,2})` | `true` | structural equality, field by field (semantics 6.5, S17) |
| `wrap(3)` | `{"$type": "corpus.values.Meters", "$payload": [3]}` | a wrapper is distinct from its payload (semantics 6.1); wrapper value JSON (toolchain 7.3) |
| `unwrap(Meters(7))` | `7` | one-arm match on a wrapper, exhaustive (semantics S23, S25) |
| `same_tag(Tag("a"), Tag("a"))` | `true` | wrappers compare by payload (stdlib 1.7, semantics 6.5) |
| `nothing()` | `{"$type": "Unit"}` | the constructor `Unit` (stdlib S2); Unit value JSON |
| `noop()` | `{"$type": "Unit"}` | a block without a tail expression has value `Unit` (grammar 3.11, semantics 3.10) |
| `nested_equal([[1],[2]], [[1],[2]])` | `true` | elementwise, recursive (stdlib 12 row) |

### `enums` (6) - match, `?`, `if` as an expression

| Case | Expected | Spec |
| - | - | - |
| `describe(Ok(Some(3)))` | `"3"` | nested constructor patterns (grammar 3.13); nested tagged value JSON (toolchain 7.3) |
| `word(-1)` | `"minus one"` | `-1` is one literal pattern in pattern position (grammar 3.13, D48) |
| `sign(0)` | `0` | `else if` chain, final `else` (D49); `if` as an expression (D46) |
| `bump(true, 1)` | `2` | `if` is a primary and may be an operand of `+` (grammar 3.12) |
| `early(-1, 0)` | `Err("not positive")` | `?` on `Err` exits at once with that `Err`; `1 / 0` is never evaluated (semantics 6.2, 3.8) |
| `early(1, 0)` | fault `division_by_zero` | `?` on `Ok(1)` yields 1 and the body continues |

### `control` (5) - loops that satisfy their clauses, early return, var rebinding

| Case | Expected | Spec |
| - | - | - |
| `sum_to(4)` | `10` | invariants hold before the first test and after every body; measure 4, 3, 2, 1, not evaluated after exit (semantics 5.3); ensures holds (5.2 step 5) |
| `find_index([5, 6, 7], 6)` | `1` | `return` inside `while` exits the function (D26); ensures still runs on that exit (5.2 step 5) |
| `clamp_pos(5)` | `5` | tail exit |
| `clamp_pos(-2)` | CV: `ensures`, `clause "result > 0"`, `attributed_to body`, `actual -2`, `input {"n": -2}` | the early `return` is an exit and ensures runs with `result == -2` (5.2 step 5) |
| `rebind([])` | `[[], [1]]` | a `var` is a rebindable name, not a mutable cell (semantics 6.1) |

### `contracts` (10) - every clause kind as a fault, plus the two non-CV outcomes

| Case | Expected | Spec |
| - | - | - |
| `recip(0)` | CV: `requires`, `clause "n != 0"`, `attributed_to caller`, no `caller`, `input {"n": 0}` | the precondition is checked before the body (5.2 step 3), so this is *not* `division_by_zero` |
| `half(5)` | CV: `ensures`, `clause "result * 2 == n"`, `attributed_to body`, `actual 2` | 5 / 2 == 2 (S37); 2 * 2 != 5 |
| `passthrough(Err("e"))` | CV: `ensures`, `clause "result.is_ok()"`, `actual Err("e")` | a propagated `Err` is an exit and ensures runs on it (5.2 step 5; design "checked as a whole, including error variants") |
| `bad_loop(3)` | CV: `invariant`, `clause "i < 3"`, `iteration 3`, `actual {"bindings": {...}}` | invariant checked after every iteration body (5.3); holds at entry and after bodies 1 and 2 |
| `stuck(1)` | `1` | the measure is evaluated only after the condition held and before the body, never after exit (5.3), so one iteration with a constant measure is not a fault |
| `stuck(2)` | CV: `decreases`, `clause "n"`, `iteration 1`, `actual {"bindings": {...}, "previous": 2, "measure": 2}` | not strictly decreasing (5.3) |
| `neg_measure(1)` | CV: `decreases`, `clause "i - 1"`, `iteration 0`, `actual {"bindings": {...}, "measure": -1}` | negative measure (5.3) |
| `bad_rec(1)` | CV: `decreases`, `clause "n"`, `actual {"previous": 1, "measure": 1}` | header `decreases` compared with the nearest activation of the same function at entry (5.4 / S35); detected at the second entry, so no runaway recursion |
| `count_down(3)` | `3` | measures 3, 2, 1, 0 each strictly smaller than the nearest activation's (S35), evaluated after `requires` held (5.2 step 3) |
| `faulting_clause(0)` | fault `division_by_zero`, `facet contract`, `clause "10 / n > 0"` | a fault while evaluating a clause is that fault, not a CV (S34) |

### `refine` (7) - check<R>, chains, ConstraintError, refinement flow

| Case | Expected | Spec |
| - | - | - |
| `to_positive(3)` | `Ok(3)` | refinements are erased: the payload is the Int (semantics 6.1); `check` returns the same value (stdlib 1.4) |
| `to_positive(0)` | `Err(ConstraintError { type_name: "corpus.refine.Positive", message: "0 does not satisfy self > 0" })` | a value, not a fault (6.6); `type_name` per S44; `message` per semantics 6.10; `$type "ConstraintError"` bare (T30) |
| `to_small(0)` | `Err(... "corpus.refine.Positive", "0 does not satisfy self > 0")` | chain evaluated root first; the root predicate fails first (S44 worked example, 6.10) |
| `to_small(20)` | `Err(... "corpus.refine.Small", "20 does not satisfy self < 10")` | root holds, `Small`'s predicate fails |
| `to_nz(0)` | fault `division_by_zero`, `symbol corpus.refine.Nz`, `facet contract`, `clause "10 / self > 0"` | a fault inside a predicate propagates (6.10, S34) |
| `double(0)` | fault `refinement_violation`, `expected_type Positive`, `actual 0`, `clause "self > 0"` | a run argument failing its refinement (S39 (a), toolchain 7.3); see A3 |
| `doubled(4)` | `Ok(8)` | a `Positive` from `check` flows into a `Positive` parameter unchecked; operator results are base-typed (stdlib 0.3) |

### `faults` (2) - assert in a function

| Case | Expected | Spec |
| - | - | - |
| `checked_div(6, 3)` | `2` | assertion holds |
| `checked_div(1, 0)` | fault `assertion_failed`, `clause "b != 0"`, `bindings` (b, a, innermost first) | `assert` is a statement in any body, functions included (semantics 7.1, S41; grammar 3.11); the division is never reached |

### `holes` (1) - the holes project's runnable row

| Case | Expected | Spec |
| - | - | - |
| `summarize(Page { id: PageId("p"), text: "t" })` | fault `unresolved_hole`, `hole "summary"`, `symbol corpus.holes.summarize`, `expected_type Text`, `bindings [page]` | semantics 6.7, 8.1; toolchain 12.3 |

### `reader` (9) - the reader project's runnable rows (toolchain 12.3, semantics 11)

| Case | Expected | Spec |
| - | - | - |
| `paginate_ints([1, 2, 3], 2)` | `Ok([[1, 2], [3]])` | 12.3 row; the design's completed `paginate` with every invariant and ensures holding (stdlib 4.3 worked check) |
| `paginate_ints([], 2)` | `Ok([])` | 12.3 row; empty input, empty result |
| `paginate_ints([1], 0)` | `Err(ConstraintError { type_name: "corpus.reader.Positive", message: "0 does not satisfy self > 0" })` | 12.3 row ("Library: payload shape") and semantics 11 |
| `divide_exactly(6, 3)` | `2` | 12.3 row |
| `divide_exactly(7, 0)` | CV: `requires`, `clause "b != 0"`, `input {"a": 7, "b": 0}`, `attributed_to caller`, no `caller` | 12.3 row, semantics 11; the second clause is never evaluated |
| `divide_exactly(7, 2)` | CV: `requires`, `clause "a % b == 0"` | 12.3 row; sequential preconditions (design, semantics 5.1) |
| `positive_size("  12 ")` | `Ok(12)` | 12.3 row; trim, parse_int, check |
| `positive_size("-3")` | `Err(... "corpus.reader.Positive", "-3 does not satisfy self > 0")` | 12.3 row (`Err(...)`), made exact by S25 (sign accepted) and 6.10 |
| `label(Err(Missing(PageId("p"))))` | `"Missing page"` | design's `label`; three nested tagged objects in the argument |

## 3. The toolchain.md 12.3 table

The corpus format pairs a `.run.json` with one corpus module and cannot
reference `examples/reader` or `examples/holes`, so the design's functions
are reproduced byte-for-byte in `reader.cpl` and `holes.cpl` with two
changes: the module header (`corpus.reader`, `corpus.holes`) and the
removal of `@id("pgn_7c2d")` (a copied definition is a new identity, and
two corpus modules linked into one program must not share an `@id`,
semantics S2). Every qualified name in the expectations therefore reads
`corpus.reader.X` where 12.3 and semantics 11 say `reader.pages.X`.

| 12.3 row | Corpus case |
| - | - |
| `paginate_ints([1, 2, 3], 2)` -> `Ok([[1, 2], [3]])` | `reader` case 1 |
| `paginate_ints([], 2)` -> `Ok([])` | `reader` case 2 |
| `paginate_ints([1], 0)` -> `Err(ConstraintError...)` | `reader` case 3 (payload made exact) |
| `divide_exactly(6, 3)` -> `2` | `reader` case 4 |
| `divide_exactly(7, 0)` -> CV `b != 0`, `input {"a": 7, "b": 0}` | `reader` case 5 |
| `divide_exactly(7, 2)` -> CV `a % b == 0` | `reader` case 6 |
| `positive_size("  12 ")` -> `Ok(12)` | `reader` case 7 |
| `positive_size("-3")` -> `Err(...)` | `reader` case 8 (payload made exact) |
| `holes.pages.summarize(Page{...})` -> `unresolved_hole`, `hole: "summary"` | `holes` case 1 |
| `holes.pages.paginate` via `cpl run` -> `cannot_run_generic` (exit 2) | not expressible: `cannot_run_generic` is a `RunError` / usage outcome, not a fault (toolchain 1.7 `RunError::Generic`, 7.2); `tests/examples.rs` checks it on `examples/holes` |
| `cpl test` on `examples/reader` with `--seed 1` | not expressible (test runner); `tests/examples.rs` runs it (and the CLI tests will) |
| `cpl check` / `cpl check --release` rows | not expressible (checker and CLI layer) |

## 4. Discrepancies and ambiguities found, and the rulings applied

Conflicts between documents (D) and silences the corpus had to resolve (A),
each followed by the ruling of the specification editor (2026-10-28), with
the decision id of `spec/decisions.md` where a new decision was recorded.
No fixture here changed: every ruling confirms the reading the corpus
encoded.

**D1. `@property` is unparseable under grammar.md's own lexical rules.**
grammar 1.4 reserves `property` as one of the 33 keywords (`KwProperty`);
grammar 3.3 defines `annotation = "@" LOWER_IDENT ...` and, in the same
section, lists `@property` in the annotation table; stdlib 0.1, 10.2, 11.1
and 11.5 and toolchain 3.6 all use `@property`. A keyword is never a
`LOWER_IDENT`, so the current `cpl-syntax` parser rejects every
`@property` line of `std/prelude.cpl` and `std/gen.cpl` with
`syntax_error: expected an annotation name (snake_case identifier), found
'property'` (verified). Reading: the stubs keep `@property`, the spelling
four documents agree on; the grammar needs the ruling that the
annotation-name position also admits the `property` keyword token, exactly
as D53 already admits the `effects` keyword in selector facet position.
Precedence (grammar over stdlib) does not decide this, because grammar.md
is internally inconsistent.
*Ruling* [D-227] (grammar D71, 3.3, 1.4): an annotation name is a
`LOWER_IDENT` or any reserved keyword token, so `@property` parses as
written; the stubs stay as they are, and the current parser already
accepts it (re-checked 2026-10-28).

**D2. Three std stubs violate `recursive_contract`.** semantics 5.1 / S33:
"a clause of declaration `d` may not call `d` itself" is the check error
`recursive_contract`. stdlib 6.1 and 11.2 give `trim` the clause `ensures
trim(result) == result`, `to_upper` the clause `ensures to_upper(result) ==
result`, and `to_lower` the clause `ensures to_lower(result) == result`.
Reading: the stub files keep the clauses verbatim (Section 11 is the single
normative stub text, stdlib S35), and the checker must exempt `@builtin`
stubs from S33, whose rationale (an ensures that re-enters itself forever
in checked mode) cannot apply to a builtin because builtin `ensures`
clauses are never evaluated (stdlib S39, semantics S58). The alternative is
to move the three clauses to the laws table, where X1 (`trim(trim(t)) ==
trim(t)`) already states the first and `to_lower(to_upper(t)) ==
to_lower(t)` the spirit of the others. The spec owners must choose; the
stubs follow the normative text.
*Ruling* [D-228] (semantics S61; stdlib 10.4 cross-references it): a clause
of a `@builtin` declaration may name the declaration itself; the checker
exempts `@builtin` declarations from `recursive_contract`, and the
verifier treats such clauses as axioms (stdlib S41). The stubs stay as
written.

**D3. The parser crate does not implement G1/G2.** grammar 3.5 and 3.9
(with stdlib 10.2 and toolchain 3.6) make a bodiless `fn` and a
definition-less `type` legal parses, but `cpl_syntax::ast::FnDecl` has
`body: Block` (not optional) and `TypeDeclKind` has only `Wrapper` and
`Refinement`. The std stub files therefore cannot be parsed by the current
crate at all. This is an implementation gap rather than a specification
conflict; it was worked around for validation by parsing a transform of the
stubs in which every bodiless `fn` received an empty body and every opaque
`type` a wrapper payload (everything else byte-identical). After masking
`@property` (D1) all five transformed files parse cleanly, so the newline
rules, clause placement, doc-comment positions, and struct/enum bodies of
the stubs are well-formed.
*Ruling*: none needed; the grammar (3.5, 3.9) stands, and the current
parser implements G1/G2 (`FnDecl::body: Option<Block>`, `OpaqueDecl`), so
the stub files now parse without the transform described above.

**D4. `Gen` is both "reserved and not declared in the stub files" and
declared in `std/gen.cpl`.** stdlib S11: "The reserved names are not
declared in the stub files"; the reserved list includes `Gen` (S28). stdlib
8.5 / 11.5 declare `@builtin("gen") pub type Gen<T>`. Reading: `Gen` is
reserved as a *prelude* name (a user cannot declare or import it) and is an
ordinary `std.gen` item declared in its stub; S11's sentence
over-generalises to the ten integer and float names.
*Ruling*: confirmed; stdlib S11 (2.5) now says the ten numeric names are
undeclared and `Gen` is declared in `std/gen.cpl` as the `@builtin("gen")`
type while remaining reserved for user code.

**D5. `ConstraintError.message` is "informative only" and also fixed.**
stdlib 1.5 marks `message` informative and changeable between toolchain
versions; semantics 6.10 fixes the `check` message to `"<value text of v>
does not satisfy <predicate text>"`, and semantics 11 and toolchain 12.3
compare it (`"0 does not satisfy self > 0"`). Reading: the `check` message
is specification-fixed and this corpus compares it (`refine`, `reader`);
the messages of the standard parsers (`parse_int`, `to_text`) are not
fixed anywhere, so the corpus compares only their `type_name`
(`std_funcs.parse_failure_type`).
*Ruling*: confirmed; stdlib 1.5's stability column now distinguishes the
fixed `check` message (semantics 6.10) from the informative parser
messages.

**D6. `is_ok` is the only prelude member without an `ensures` clause**
(stdlib 5.2 and 11.1 agree; `is_err` is specified relative to it). Not a
conflict; recorded because an auditor comparing the two sections will
notice it.

**D7. Doc-comment wording differs between stdlib 8.5 and 11.5 for
`Positive`** ("Structurally identical to any user refinement ..." versus
"Structurally the same type as any user refinement ..."). Signatures and
clauses are identical; Section 11 was copied verbatim.
*Ruling*: 8.5 now carries 11.5's wording (Section 11 is normative, S35).

**A1. Extra fields in `.run.json`.** toolchain 12 gives the closed shape
`{ "function", "args", "expect": { "value": json } | { "fault": code } }`
and, unlike semantics 9.2 for diagnostics, says nothing about extra fields.
Reading: none are permitted. The expected `clause_kind`, `clause`,
`symbol`, `attributed_to`, `input`, and `actual` of every fault case are in
the `// why:` comments of the `.cpl` files and in Section 2, and the
`corpus.contracts` functions each have exactly one clause kind they can
violate, so a harness that only compares the code still exercises every
kind separately.
*Ruling* [D-237] (toolchain 12.5, T49): confirmed; the three keys and the
one-key `expect` are the closed shape, and any other key is a harness
error.

**A2. What `function` names.** Reading: the qualified name (see Section 1).
A harness that wants bare names can strip the `corpus.<name>.` prefix,
which equals the paired module's header in every case.
*Ruling* [D-237]: the qualified name, public or private.

**A3. Refinement-typed parameters.** toolchain 7.3: a refined `Int`
argument that fails its predicate is "`refinement_violation` for `cpl run`,
`usage_bad_argument` elsewhere"; the corpus harness is neither named.
Reading: the corpus runs cases through `run_function` with
`RunArg::Json`, whose semantics are S39's, so the failure is the fault
`refinement_violation`. Exactly one case depends on it
(`refine.double(0)`); if the harness decodes arguments itself and reports
`usage_bad_argument`, that case must be re-expressed.
*Ruling* [D-237]: the harness decodes through `run_function` with
`RunArg::Json`, so the outcome is the fault `refinement_violation`;
`refine.double(0)` stands.

**A4. Harness context for faults.** The `caller` field of a top-level
`requires` violation is absent because the harness has no calling
declaration (S39 (c)); `actual.bindings` of a loop violation is "every
local binding visible at the loop" (semantics 5.3) and the `// why:`
comments assume that includes the parameters (as S41 states explicitly
for `assert`). Neither is compared by the format.
*Ruling*: none needed (semantics S39 (c) and 5.3 already say so).

**A5. `@id` and `@prove` in the copied modules.** `@id("pgn_7c2d")` is
dropped (Section 3). `@prove` is kept: in Milestone 1 it changes no runtime
behaviour (semantics S36 (a)) and only adds a `verification_result` info
diagnostic; the corpus is a development-mode program, so `--release`
rejection does not apply.
*Ruling*: none needed.

**A6. Rows of stdlib 12 that need the property runner** (`for x in
gen.range(5, 1)` faulting at property setup with `symbol std.gen.range`)
are not expressible here; `tests/properties.rs` covers them.
*Ruling*: none needed; T49 scopes the format to `run_function` cases.

## 5. Standard-library stub audit (`std/*.cpl`)

The five files were extracted mechanically from stdlib.md Section 11 (the
text between each `### 11.N` heading's code fence) and are byte-identical
to it (verified by re-extraction and `diff`). They were then audited:

- **Registry (stdlib 10.6):** the stubs carry 59 distinct `@builtin` ids;
  the registry table lists 59; the sets are equal, with no duplicates and
  no unknown id.
- **Signatures and contracts (stdlib 1.6, 2.3, 3.3, 3.4, 4.2, 4.5, 5.1,
  5.2, 6.1, 7, 8.5):** every operation's name, type parameters, parameter
  names and types, return type, and `requires`/`ensures` clauses in Section
  11 are identical to the detailed sections; only doc-comment wording
  differs (D7). No case arose where Section 11 disagrees with Sections
  1-10, so no file deviates from the normative text.
- **Member placement (stdlib 0.1, 10.2, 10.3):** 32 `@method` stubs, 3
  receiver `@property` stubs (`len` on `List`, `Text`, `Bytes`), and 4
  zero-parameter `@property` generator properties (`gen.int`,
  `positive_int`, `bool`, `text`, S40). Every receiver head is one of
  `Int`, `Text`, `Bytes`, `List`, `Option`, `Result`; no `@method` has zero
  parameters, no receiver `@property` has more than one, no `fn` carries
  both, and every `@method`/`@property` fn also carries `@builtin`.
  `flatten` and the `std.text`/`std.bytes`/`std.gen` functions carry
  neither, as specified. `len` is declared three times and `unwrap_or`
  twice, as S37 requires.
- **Prelude contents (semantics 1.7, stdlib 1.1):** `std/prelude.cpl`
  declares exactly `Int`, `Bool`, `Text`, `Bytes`, `List<T>`, `Unit`,
  `Option<T>`, `Result<T, E>`, `ConstraintError`, `flatten`, and the
  members; `check` has no stub (intrinsic, stdlib 1.4); `gen` is bound by
  the loader (S1); the reserved names of S11 are declared nowhere (D4 for
  `Gen`).
- **Grammar:** all five files are ASCII-only, LF-only, end in a newline,
  begin with module doc comments followed by `module std.<name>`
  (toolchain 3.6 file names and headers), place every clause on its own
  line, and keep every `///` before a declaration, field, or variant. Under
  the current parser they parse once G1/G2 are simulated (D3) and
  `@property` is masked (D1); nothing else was flagged.
- **Open check-time issues the stubs carry into `cpl check`:** D1
  (`@property` versus the keyword) and D2 (`recursive_contract` on `trim`,
  `to_upper`, `to_lower`).
