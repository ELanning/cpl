//! Adversarial tests, round 1: expressions and patterns.
//!
//! Lens: `spec/grammar.md` 3.12 (precedence table, postfix rules 1-4),
//! 3.13 (patterns), 1.5 (Int and Text literals), 1.7 (the `?` rule), 2.2
//! (generic lists and newlines), section 5 rulings (a), (b), (c), (f), (g),
//! (h), (j), and decisions D29, D30, D41, D42, D45, D48, D59.
//!
//! Every expectation encodes the grammar's answer, never the parser's
//! current behaviour. Where the grammar is silent the test's comment names
//! the design-doc example that was followed. Tree shapes are asserted
//! through `dump_module` (collapsed to one line).

use cpl_diag::{Category, DiagCode, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_syntax::{codes, dump_module, parse_module, parse_patch, parse_work};

/// The `parse` codes of `spec/semantics.md` 9.4, the catalogue D59 names
/// for every lexical and syntax diagnostic.
const SPEC_PARSE_CODES: &[&str] = &[
    "invalid_utf8",
    "invalid_line_terminator",
    "non_ascii_outside_text",
    "invalid_character",
    "unterminated_text_literal",
    "invalid_escape",
    "invalid_int_literal",
    "float_literal_unsupported",
    "literal_adjacent_identifier",
    "prefixed_literal_reserved",
    "doc_comment_position",
    "reserved_word",
    "hole_name_case",
    "hole_name_keyword",
    "syntax_error",
    "duplicate_clause",
    "use_after_declaration",
    "positional_after_using",
    "misplaced_else",
    "unknown_requirement",
    "duplicate_work_item",
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn render(src: &str, diagnostics: &[Diagnostic]) -> String {
    let mut map = SourceMap::new();
    map.add("test.cpl".to_string(), FileOrigin::Virtual, src.to_string());
    cpl_diag::render_all(diagnostics, &map)
}

fn flat(dump: &str) -> String {
    dump.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a whole module that must produce zero diagnostics; return the
/// flattened dump.
fn module(src: &str) -> String {
    let r = parse_module(src, FileId(0));
    match r.value {
        Some(m) if r.diagnostics.is_empty() => flat(&dump_module(&m)),
        _ => panic!(
            "expected a clean parse of:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn wrap(stmts: &str) -> String {
    let indented: Vec<String> = stmts.lines().map(|l| format!("    {l}")).collect();
    format!(
        "module m\n\nfn f() -> Int {{\n{}\n}}\n",
        indented.join("\n")
    )
}

/// Statements inside `fn f() -> Int { ... }`; zero diagnostics required.
fn body(stmts: &str) -> String {
    module(&wrap(stmts))
}

fn module_errors(src: &str) -> Vec<Diagnostic> {
    let r = parse_module(src, FileId(0));
    assert!(
        r.value.is_none() && r.diagnostics.iter().any(Diagnostic::is_error),
        "expected a syntax or lexical error for:\n{src}\n(got {} diagnostics)",
        r.diagnostics.len()
    );
    for d in &r.diagnostics {
        assert_eq!(d.category, Category::Parse, "category of {d:?}");
        assert!(
            SPEC_PARSE_CODES.contains(&d.code.as_str()),
            "code `{}` is not in spec/semantics.md 9.4 (parse)",
            d.code
        );
    }
    r.diagnostics
}

fn body_errors(stmts: &str) -> Vec<Diagnostic> {
    module_errors(&wrap(stmts))
}

fn has_code(diags: &[Diagnostic], code: DiagCode) -> bool {
    diags.iter().any(|d| d.code == code)
}

fn has_message(diags: &[Diagnostic], needle: &str) -> bool {
    diags.iter().any(|d| d.message.contains(needle))
}

#[track_caller]
fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected to find\n  {needle}\nin\n  {haystack}"
    );
}

/// The statements' last expression must dump as `(tail <expected>)`.
#[track_caller]
fn tail(stmts: &str, expected: &str) {
    assert_contains(&body(stmts), &format!("(tail {expected})"));
}

/// A rejected expression statement: at least one error, all with codes of
/// the catalogue; `code` is the code the grammar names for the case.
#[track_caller]
fn rejected(stmts: &str, code: DiagCode) -> Vec<Diagnostic> {
    let e = body_errors(stmts);
    assert!(
        has_code(&e, code),
        "expected code `{code}` for:\n{stmts}\ngot {:?}",
        e.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    e
}

/// A match with one arm `<pat> => 1`; returns the dump.
fn arm(pat: &str) -> String {
    body(&format!("match x {{\n{pat} => 1\n}}"))
}

#[track_caller]
fn arm_is(pat: &str, expected: &str) {
    assert_contains(&arm(pat), &format!("(arm {expected} (int 1))"));
}

#[track_caller]
fn arm_rejected(pat: &str) -> Vec<Diagnostic> {
    body_errors(&format!("match x {{\n{pat} => 1\n}}"))
}

// ---------------------------------------------------------------------------
// 3.12 precedence table: levels 3 to 7, associativity
// ---------------------------------------------------------------------------

#[test]
fn multiplicative_binds_tighter_than_additive() {
    tail("a + b * c", "(+ (name a) (* (name b) (name c)))");
    tail("a * b + c", "(+ (* (name a) (name b)) (name c))");
    tail("a - b / c", "(- (name a) (/ (name b) (name c)))");
    tail(
        "a % b + c % d",
        "(+ (% (name a) (name b)) (% (name c) (name d)))",
    );
    tail(
        "a * b ++ c * d",
        "(++ (* (name a) (name b)) (* (name c) (name d)))",
    );
}

#[test]
fn additive_and_multiplicative_are_left_associative() {
    tail("a - b - c", "(- (- (name a) (name b)) (name c))");
    tail("a / b / c", "(/ (/ (name a) (name b)) (name c))");
    tail(
        "a % b * c / d",
        "(/ (* (% (name a) (name b)) (name c)) (name d))",
    );
    tail(
        "a + b - c ++ d",
        "(++ (- (+ (name a) (name b)) (name c)) (name d))",
    );
    tail("1 + 2 + 3", "(+ (+ (int 1) (int 2)) (int 3))");
    tail("a - (b - c)", "(- (name a) (paren (- (name b) (name c))))");
}

#[test]
fn and_binds_tighter_than_or_and_both_are_left_associative() {
    tail("a and b or c", "(or (and (name a) (name b)) (name c))");
    tail("a or b and c", "(or (name a) (and (name b) (name c)))");
    tail("a or b or c", "(or (or (name a) (name b)) (name c))");
    tail("a and b and c", "(and (and (name a) (name b)) (name c))");
    tail(
        "a or b and c or d",
        "(or (or (name a) (and (name b) (name c))) (name d))",
    );
}

#[test]
fn comparisons_sit_between_arithmetic_and_boolean_operators() {
    tail(
        "a == b and c != d",
        "(and (== (name a) (name b)) (!= (name c) (name d)))",
    );
    tail(
        "a + b < c * d",
        "(< (+ (name a) (name b)) (* (name c) (name d)))",
    );
    tail(
        "a <= b or c >= d",
        "(or (<= (name a) (name b)) (>= (name c) (name d)))",
    );
    tail(
        "a * b == c + d and e",
        "(and (== (* (name a) (name b)) (+ (name c) (name d))) (name e))",
    );
    tail("a < b or c", "(or (< (name a) (name b)) (name c))");
    tail("a ++ b == c", "(== (++ (name a) (name b)) (name c))");
}

#[test]
fn parentheses_override_precedence_and_are_kept_as_paren_nodes() {
    tail("(a + b) * c", "(* (paren (+ (name a) (name b))) (name c))");
    tail("a * (b + c)", "(* (name a) (paren (+ (name b) (name c))))");
    tail(
        "(a or b) and c",
        "(and (paren (or (name a) (name b))) (name c))",
    );
    tail("((a))", "(paren (paren (name a)))");
    tail("(a) + (b)", "(+ (paren (name a)) (paren (name b)))");
}

#[test]
fn all_levels_in_one_expression() {
    // level 1 postfix, 2 prefix, 3 `*`, 4 `+`, 5 `<`, 6 `and`, 7 `or`
    tail(
        "a or not b.c * -d + 1 < e and f",
        "(or (name a) (and (< (+ (* (not (member (name b) c)) (neg (name d))) (int 1)) (name e)) (name f)))",
    );
    tail(
        "f(x) + g(y) * h(z)?",
        "(+ (call (name f) (name x)) (* (call (name g) (name y)) (propagate (call (name h) (name z)))))",
    );
}

#[test]
fn dangling_binary_operators_are_errors() {
    for src in [
        "a +",
        "a *",
        "a and",
        "a or",
        "a ==",
        "a ++",
        "a <",
        "* a",
        "a + + b",
        "a and and b",
        "== a",
        "a + * b",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
    // `+` is not a prefix operator (3.12 lists only `not` and `-`).
    rejected("+a", codes::SYNTAX_ERROR);
    rejected("a ++ ++ b", codes::SYNTAX_ERROR);
}

// ---------------------------------------------------------------------------
// 3.12 level 5: comparisons do not chain
// ---------------------------------------------------------------------------

#[test]
fn chained_comparisons_are_syntax_errors() {
    // D59: a chained comparison is a `syntax_error`.
    for src in [
        "a < b < c",
        "a == b != c",
        "a < b == c",
        "a >= b <= c",
        "a != b == c",
        "a > b > c",
        "not a == b == c",
        "f(x) == g(y) == 1",
        "a + b < c < d",
        "a == (b == c) == d",
        "a < b and c < d < e",
        "x == 1 or y == 2 == z",
    ] {
        let e = rejected(src, codes::SYNTAX_ERROR);
        assert!(
            e.iter().all(|d| d.code == codes::SYNTAX_ERROR),
            "{src}: only syntax_error expected, got {e:#?}"
        );
    }
}

#[test]
fn comparisons_nest_through_parentheses_brackets_and_conjunctions() {
    tail("(a < b) < c", "(< (paren (< (name a) (name b))) (name c))");
    tail("a < (b < c)", "(< (name a) (paren (< (name b) (name c))))");
    tail(
        "a < b and b < c",
        "(and (< (name a) (name b)) (< (name b) (name c)))",
    );
    tail(
        "[a < b, b < c]",
        "(list (< (name a) (name b)) (< (name b) (name c)))",
    );
    tail(
        "f(a == b, c == d)",
        "(call (name f) (== (name a) (name b)) (== (name c) (name d)))",
    );
    tail(
        "(a == b) == (c == d)",
        "(== (paren (== (name a) (name b))) (paren (== (name c) (name d))))",
    );
    // every comparison operator once
    for op in ["==", "!=", "<", "<=", ">", ">="] {
        tail(&format!("a {op} b"), &format!("({op} (name a) (name b))"));
    }
}

#[test]
fn chained_comparison_diagnostic_carries_the_generic_fields() {
    // D59: an error case the grammar names without quoting a message (the
    // chained comparison is its first example) reports the generic
    // `expected <tokens>, found <token>` and, for a syntax error, the set of
    // expected tokens; semantics 9.4 lists `expected` and `found` as the
    // fields of `syntax_error`.
    let e = rejected("a < b < c", codes::SYNTAX_ERROR);
    let d = e.iter().find(|d| d.code == codes::SYNTAX_ERROR).unwrap();
    assert!(
        d.data.get("found").is_some(),
        "the chained-comparison syntax_error must carry `found` (semantics 9.4): {d:#?}"
    );
    assert!(
        d.data.get("expected").is_some(),
        "the chained-comparison syntax_error must carry `expected` (D59): {d:#?}"
    );
    assert!(
        d.message.starts_with("expected "),
        "D59: the message is the generic `expected <tokens>, found <token>`, got {:?}",
        d.message
    );
}

// ---------------------------------------------------------------------------
// 5 (j) D48: unary minus and negative literals
// ---------------------------------------------------------------------------

#[test]
fn unary_minus_binds_tighter_than_multiplication_but_looser_than_postfix() {
    tail("-x * y", "(* (neg (name x)) (name y))");
    tail("x * -y", "(* (name x) (neg (name y)))");
    tail("-x.y", "(neg (member (name x) y))");
    tail("-f(x).y", "(neg (member (call (name f) (name x)) y))");
    tail("-a?", "(neg (propagate (name a)))");
    tail("-5.abs()", "(neg (call (member (int 5) abs)))");
    tail("(-5).abs()", "(call (member (paren (neg (int 5))) abs))");
    tail("-a ++ b", "(++ (neg (name a)) (name b))");
    tail("-a < -b", "(< (neg (name a)) (neg (name b)))");
    tail("-a * -b", "(* (neg (name a)) (neg (name b)))");
}

#[test]
fn double_minus_and_minus_minus_literals() {
    tail("- -x", "(neg (neg (name x)))");
    tail("--5", "(neg (neg (int 5)))");
    tail("2 - -3", "(- (int 2) (neg (int 3)))");
    tail("2--3", "(- (int 2) (neg (int 3)))");
    tail("a - -b - c", "(- (- (name a) (neg (name b))) (name c))");
    tail("a * - b", "(* (name a) (neg (name b)))");
    tail("-(x)", "(neg (paren (name x)))");
    tail(
        "-(x + y) * z",
        "(* (neg (paren (+ (name x) (name y)))) (name z))",
    );
    tail("-0", "(neg (int 0))");
    tail("- 5", "(neg (int 5))");
    tail("-1_000", "(neg (int 1000))");
}

#[test]
fn unary_operators_compose_right_nested() {
    tail("-not a", "(neg (not (name a)))");
    tail("not -a", "(not (neg (name a)))");
    tail("-?h", "(neg (hole h))");
    tail("- - - x", "(neg (neg (neg (name x))))");
}

#[test]
fn negative_literals_in_statement_positions() {
    assert_contains(&body("let x = -1\nx"), "(let x (neg (int 1)))");
    assert_contains(&body("x = -1\n1"), "(assign x (neg (int 1)))");
    tail("f(-1, -x)", "(call (name f) (neg (int 1)) (neg (name x)))");
    tail("[-1, -2]", "(list (neg (int 1)) (neg (int 2)))");
    assert_contains(&body("return -1"), "(return (neg (int 1)))");
    assert_contains(
        &body("assert -1 < 0\n1"),
        "(assert (< (neg (int 1)) (int 0)))",
    );
}

#[test]
fn malformed_unary_minus() {
    for src in ["-", "a -", "- * a", "-,", "a -> b", "- -"] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

// ---------------------------------------------------------------------------
// 3.12 level 2: `not` relative to comparisons and `and`
// ---------------------------------------------------------------------------

#[test]
fn not_binds_tighter_than_every_binary_operator() {
    tail("not a and b", "(and (not (name a)) (name b))");
    tail("not a or b", "(or (not (name a)) (name b))");
    tail("a and not b", "(and (name a) (not (name b)))");
    tail("not a < b", "(< (not (name a)) (name b))");
    tail("not a != b", "(!= (not (name a)) (name b))");
    tail("not a + b", "(+ (not (name a)) (name b))");
    tail("not a * b", "(* (not (name a)) (name b))");
    tail("not a ++ b", "(++ (not (name a)) (name b))");
    tail(
        "not a and not b or not c",
        "(or (and (not (name a)) (not (name b))) (not (name c)))",
    );
}

#[test]
fn not_applies_to_a_whole_postfix_chain() {
    tail("not f(x)?", "(not (propagate (call (name f) (name x))))");
    tail("not a.b.c", "(not (member (member (name a) b) c))");
    tail(
        "not xs.is_empty()",
        "(not (call (member (name xs) is_empty)))",
    );
    tail(
        "not check<T>(x)",
        "(not (call (name check) (generics (ty T)) (name x)))",
    );
    tail("not not not a", "(not (not (not (name a))))");
    tail(
        "not (a or b) and c",
        "(and (not (paren (or (name a) (name b)))) (name c))",
    );
    tail("not true", "(not (bool true))");
    tail("not [a]", "(not (list (name a)))");
}

#[test]
fn malformed_not() {
    for src in [
        "not",
        "not and a",
        "a not b",
        "not = a",
        "not not",
        "a and not",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

// ---------------------------------------------------------------------------
// 3.12 level 4: `++` at the additive level
// ---------------------------------------------------------------------------

#[test]
fn concat_is_at_the_additive_level_and_left_associative() {
    tail("a ++ b + c", "(+ (++ (name a) (name b)) (name c))");
    tail("a + b ++ c", "(++ (+ (name a) (name b)) (name c))");
    tail("a ++ b ++ c", "(++ (++ (name a) (name b)) (name c))");
    tail("a ++ b * c", "(++ (name a) (* (name b) (name c)))");
    tail(
        "a ++ b == c ++ d",
        "(== (++ (name a) (name b)) (++ (name c) (name d)))",
    );
    tail("a ++ -b", "(++ (name a) (neg (name b)))");
    tail("[1] ++ [2]", "(++ (list (int 1)) (list (int 2)))");
    tail(
        "a ++ b.take(1)",
        "(++ (name a) (call (member (name b) take) (int 1)))",
    );
    // E5: `flatten(pages) ++ rest == items` is `(flatten(pages) ++ rest) == items`.
    tail(
        "flatten(pages) ++ rest == items",
        "(== (++ (call (name flatten) (name pages)) (name rest)) (name items))",
    );
}

#[test]
fn malformed_concat() {
    // `+++` lexes as `++` `+` (longest match), and `+` is not a prefix operator.
    for src in ["a +++ b", "++ a", "a ++", "a ++ ++ b", "++"] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

// ---------------------------------------------------------------------------
// 3.12 level 1: postfix chains
// ---------------------------------------------------------------------------

#[test]
fn postfix_operators_apply_left_to_right_in_any_order() {
    tail(
        "a.b(c).d?",
        "(propagate (member (call (member (name a) b) (name c)) d))",
    );
    tail("a?.b", "(member (propagate (name a)) b)");
    tail("f?()", "(call (propagate (name f)))");
    tail(
        "f()?.g()?",
        "(propagate (call (member (propagate (call (name f))) g)))",
    );
    tail("x???", "(propagate (propagate (propagate (name x))))");
    tail(
        "a.b().c().d",
        "(member (call (member (call (member (name a) b)) c)) d)",
    );
    tail(
        "a.b.c.d()",
        "(call (member (member (member (name a) b) c) d))",
    );
}

#[test]
fn a_call_result_may_be_called_again() {
    // 3.12 rule 4: any postfix expression may be followed by `(args)`; the
    // typechecker, not the parser, rejects non-callable targets.
    tail("f(x)(y)", "(call (call (name f) (name x)) (name y))");
    tail(
        "f(x)(y)(z)",
        "(call (call (call (name f) (name x)) (name y)) (name z))",
    );
    tail("f()()", "(call (call (name f)))");
    tail("(f)(x)", "(call (paren (name f)) (name x))");
}

#[test]
fn postfix_applies_to_every_primary() {
    tail("(a).b", "(member (paren (name a)) b)");
    tail(
        "[1, 2].take(1)",
        "(call (member (list (int 1) (int 2)) take) (int 1))",
    );
    tail("\"x\".len", "(member (text \"x\") len)");
    tail("1.len", "(member (int 1) len)");
    tail("1_000.len", "(member (int 1000) len)");
    tail("true.x", "(member (bool true) x)");
    tail("5?", "(propagate (int 5))");
    tail("[]?", "(propagate (list))");
    tail("[].len", "(member (list) len)");
    // `if` and `match` are primaries (3.12), so the EBNF admits postfixes on them.
    tail(
        "if c { 1 } else { 2 }.len",
        "(member (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))) len)",
    );
    tail(
        "match x { _ => 1 }.len",
        "(member (match (name x) (arm _ (int 1))) len)",
    );
    tail(
        "Page { id: 1 }.id",
        "(member (struct-lit Page (field id (int 1))) id)",
    );
    tail(
        "Page { id: 1 }?",
        "(propagate (struct-lit Page (field id (int 1))))",
    );
}

#[test]
fn member_names_may_be_either_case_and_whitespace_around_dot_is_free() {
    tail("a.Upper.lower", "(member (member (name a) Upper) lower)");
    tail("x. y", "(member (name x) y)");
    tail("x .y", "(member (name x) y)");
    tail("x . y . z", "(member (member (name x) y) z)");
    tail(
        "StoreError.Missing(id)",
        "(call (member (name StoreError) Missing) (name id))",
    );
}

#[test]
fn malformed_member_access() {
    for src in [
        "a.0", "a.\"b\"", "a.", "a.b.", "a.(b)", "a.if", "a.(", ".a", "a.b c", "a.[0]", "a.?h",
        "a.true",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn malformed_calls() {
    for src in [
        "f(,)",
        "f(x,,y)",
        "f(x y)",
        "f(x",
        "f)",
        "f(using)",
        "f(x, using)",
        "f(using using db)",
        "f(x,)(",
        "f(;)",
    ] {
        body_errors(src);
    }
    rejected("f(x, using db, y)", codes::POSITIONAL_AFTER_USING);
    tail(
        "f(using x, using y)",
        "(call (name f) (using (name x)) (using (name y)))",
    );
    tail("f(using x,)", "(call (name f) (using (name x)))");
    tail("f(x,)", "(call (name f) (name x))");
}

// ---------------------------------------------------------------------------
// 5 (b) D42: generic calls versus comparisons
// ---------------------------------------------------------------------------

#[test]
fn generic_call_reading_is_authoritative_whatever_the_spacing() {
    // D42: when `generic_args "("` parses, the generic call is taken even
    // where a comparison reading exists; whitespace does not change tokens.
    // `(c)` is the call's argument list (`generic_args "(" args ")"`), not a paren_expr.
    tail("a<B>(c)", "(call (name a) (generics (ty B)) (name c))");
    tail("a < B > (c)", "(call (name a) (generics (ty B)) (name c))");
    tail("a <B> (c)", "(call (name a) (generics (ty B)) (name c))");
    tail("A<B>(c)", "(call (name A) (generics (ty B)) (name c))");
    tail(
        "a < B, C > (d)",
        "(call (name a) (generics (ty B) (ty C)) (name d))",
    );
    tail(
        "a < b.C > (d)",
        "(call (name a) (generics (ty b.C)) (name d))",
    );
    tail(
        "a.b < C > (d)",
        "(call (member (name a) b) (generics (ty C)) (name d))",
    );
    tail(
        "a<B>(c) > (d)",
        "(> (call (name a) (generics (ty B)) (name c)) (paren (name d)))",
    );
    tail(
        "a < B > (c) and d",
        "(and (call (name a) (generics (ty B)) (name c)) (name d))",
    );
    tail(
        "a < B > ((c))",
        "(call (name a) (generics (ty B)) (paren (name c)))",
    );
}

#[test]
fn lowercase_type_shaped_names_make_it_a_chained_comparison() {
    // `b` cannot be a type (3.4: type_path ends in UPPER_IDENT), so the
    // speculation fails, `a < b` is a comparison, and the `>` chains it.
    for src in [
        "a < b > (c)",
        "a<b>(c)",
        "a < b.c > (d)",
        "x < y > (z)",
        "f<a>(x)",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn speculation_fails_without_a_following_paren() {
    tail("a < List", "(< (name a) (name List))");
    tail("a < B.c", "(< (name a) (member (name B) c))");
    tail(
        "a < B and C > (d)",
        "(and (< (name a) (name B)) (> (name C) (paren (name d))))",
    );
    tail(
        "a < b and c > d",
        "(and (< (name a) (name b)) (> (name c) (name d)))",
    );
    for src in [
        "a < B > c",
        "a < B >",
        "a < B >= (c)",
        "a <= B > (c)",
        "a<B>>(d)",
        "f<T>",
        "a < B > (c) > (d) > e",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
    // `a<B<C>(d)`: speculation on the first `<` fails (no `>` before `(`),
    // so it is a comparison whose right operand is the generic call `B<C>(d)`.
    tail(
        "a<B<C>(d)",
        "(< (name a) (call (name B) (generics (ty C)) (name d)))",
    );
}

#[test]
fn generic_call_after_a_broken_chain_is_an_error() {
    // 3.12 rule 3: generic_args only after a pure name chain.
    for src in [
        "f(x)<T>(y)",
        "(f)<T>(y)",
        "1 < B > (c)",
        "(a) < B > (c)",
        "x?.convert<Int>()",
        "f(x).g<T>(y)",
        "f<T>(x)<U>(y)",
        "x.y<T>(z).w<U>(v)",
        "[f]<T>(x)",
        "\"s\"<T>(x)",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn generic_argument_lists_in_calls() {
    tail(
        "check<List<Int>>(x)",
        "(call (name check) (generics (ty List (ty Int))) (name x))",
    );
    tail(
        "check<List<List<Int>>>(x)",
        "(call (name check) (generics (ty List (ty List (ty Int)))) (name x))",
    );
    tail(
        "f<Result<List<T>, E>, U>(x)",
        "(call (name f) (generics (ty Result (ty List (ty T)) (ty E)) (ty U)) (name x))",
    );
    tail(
        "f<a.b.C>(x)",
        "(call (name f) (generics (ty a.b.C)) (name x))",
    );
    tail("f<T>()", "(call (name f) (generics (ty T)))");
    tail("f<T>()?", "(propagate (call (name f) (generics (ty T))))");
    tail(
        "f<T>(using db)",
        "(call (name f) (generics (ty T)) (using (name db)))",
    );
    tail(
        "f<T>(x)(y)",
        "(call (call (name f) (generics (ty T)) (name x)) (name y))",
    );
    tail(
        "f<T>(x).g(y)",
        "(call (member (call (name f) (generics (ty T)) (name x)) g) (name y))",
    );
    tail(
        "f<T>(x) < g<U>(y)",
        "(< (call (name f) (generics (ty T)) (name x)) (call (name g) (generics (ty U)) (name y)))",
    );
    assert_contains(
        &body("let s = check<Positive>(size)?\ns"),
        "(let s (propagate (call (name check) (generics (ty Positive)) (name size))))",
    );
}

#[test]
fn malformed_generic_argument_lists_in_calls() {
    // 2.5 and 3.4: no trailing comma, at least one type, types only.
    for src in [
        "f<T,>(x)",
        "f<>(x)",
        "f<T, >(x)",
        "f<1>(x)",
        "f<T U>(x)",
        "f<T,,U>(x)",
        "f<(T)>(x)",
        "f<T<>>(x)",
        "f<?t>(x)",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn generic_lists_never_change_the_newline_mode() {
    // 2.2: a line break inside a generic list where newlines are significant
    // ends the statement (a syntax error); inside brackets it is discarded.
    rejected("let x = f<T,\n    U>(x)\nx", codes::SYNTAX_ERROR);
    rejected("let x = f<T\n>(x)\nx", codes::SYNTAX_ERROR);
    tail(
        "g(f<T,\n    U>(x))",
        "(call (name g) (call (name f) (generics (ty T) (ty U)) (name x)))",
    );
    tail(
        "[f<T,\n    U>(x)]",
        "(list (call (name f) (generics (ty T) (ty U)) (name x)))",
    );
}

#[test]
fn generic_calls_and_comparisons_inside_argument_lists() {
    tail(
        "f(a < b, c > d)",
        "(call (name f) (< (name a) (name b)) (> (name c) (name d)))",
    );
    tail(
        "f(a < B, c > (d))",
        "(call (name f) (< (name a) (name B)) (> (name c) (paren (name d))))",
    );
    tail(
        "f(a < B, C > d)",
        "(call (name f) (< (name a) (name B)) (> (name C) (name d)))",
    );
    tail(
        "[a < B, C > (d)]",
        "(list (call (name a) (generics (ty B) (ty C)) (name d)))",
    );
    tail(
        "f(a < B, C > (d), e)",
        "(call (name f) (call (name a) (generics (ty B) (ty C)) (name d)) (name e))",
    );
}

#[test]
fn generic_calls_and_comparisons_in_header_expressions() {
    assert_contains(
        &body("if a < B {}\n1"),
        "(if (< (name a) (name B)) (block))",
    );
    assert_contains(
        &body("if a < B > (c) {}\n1"),
        "(if (call (name a) (generics (ty B)) (name c)) (block))",
    );
    assert_contains(
        &body("while check<T>(x) {}\n1"),
        "(while (call (name check) (generics (ty T)) (name x)) (block))",
    );
    assert_contains(
        &body("match check<T>(x) {\n    _ => 1\n}"),
        "(match (call (name check) (generics (ty T)) (name x)) (arm _ (int 1)))",
    );
}

// ---------------------------------------------------------------------------
// 5 (a) D41, D29, D30: struct literals
// ---------------------------------------------------------------------------

#[test]
fn struct_literals_in_statement_positions() {
    assert_contains(
        &body("let p = Page { id: 1 }\np"),
        "(let p (struct-lit Page (field id (int 1))))",
    );
    assert_contains(
        &body("var p = Page { ..q }\np"),
        "(var p (struct-lit Page (spread (name q))))",
    );
    assert_contains(
        &body("p = Page { ..q }\n1"),
        "(assign p (struct-lit Page (spread (name q))))",
    );
    assert_contains(
        &body("return Page { ..q }"),
        "(return (struct-lit Page (spread (name q))))",
    );
    // `assert` is a statement, not a header expression (D41 lists the headers).
    assert_contains(
        &body("assert p == Page { id: 1 }\n1"),
        "(assert (== (name p) (struct-lit Page (field id (int 1)))))",
    );
    tail("Page {}", "(struct-lit Page)");
}

#[test]
fn struct_literals_as_operands_and_arguments() {
    tail(
        "f(Page { id: 1 }, Inner { x: 2 })",
        "(call (name f) (struct-lit Page (field id (int 1))) (struct-lit Inner (field x (int 2))))",
    );
    tail(
        "[Page { id: 1 }]",
        "(list (struct-lit Page (field id (int 1))))",
    );
    tail(
        "Page { id: 1 } == p",
        "(== (struct-lit Page (field id (int 1))) (name p))",
    );
    tail(
        "-Page { id: 1 }",
        "(neg (struct-lit Page (field id (int 1))))",
    );
    tail(
        "f(using Ctx { db: d })",
        "(call (name f) (using (struct-lit Ctx (field db (name d)))))",
    );
    tail(
        "Page { inner: Inner { x: 1 } }",
        "(struct-lit Page (field inner (struct-lit Inner (field x (int 1)))))",
    );
    tail(
        "Page { id: if c { 1 } else { 2 } }",
        "(struct-lit Page (field id (if (name c) (block (tail (int 1))) (else (block (tail (int 2)))))))",
    );
    tail(
        "Page { id: match x { _ => 1 } }",
        "(struct-lit Page (field id (match (name x) (arm _ (int 1)))))",
    );
    tail(
        "Page { ok: forall x in xs: p }",
        "(struct-lit Page (field ok (forall x (name xs) (name p))))",
    );
}

#[test]
fn spread_is_any_expression_and_comes_first() {
    tail("Page { ..base }", "(struct-lit Page (spread (name base)))");
    tail(
        "Page { ..base, text: t }",
        "(struct-lit Page (spread (name base)) (field text (name t)))",
    );
    tail(
        "Page { ..f(x).y, id: 1 }",
        "(struct-lit Page (spread (member (call (name f) (name x)) y)) (field id (int 1)))",
    );
    tail(
        "Page { ..Page { id: 1 }, text: t }",
        "(struct-lit Page (spread (struct-lit Page (field id (int 1)))) (field text (name t)))",
    );
    tail(
        "Page { ..p? }",
        "(struct-lit Page (spread (propagate (name p))))",
    );
    tail("Page { ..if c { a } else { b } }", "(struct-lit Page (spread (if (name c) (block (tail (name a))) (else (block (tail (name b)))))))");
    // E1: `Page { ..page, text: trim(page.text) }`
    tail(
        "Page { ..page, text: trim(page.text) }",
        "(struct-lit Page (spread (name page)) (field text (call (name trim) (member (name page) text))))",
    );
}

#[test]
fn malformed_struct_literal_entries() {
    // D29: the spread is first and unique; 3.12: field_init is LOWER_IDENT ":" expr.
    for src in [
        "Page { id: 1, ..p }",
        "Page { ..p, ..q }",
        "Page { .. }",
        "Page { ..p ..q }",
        "Page { ..p id: 1 }",
        "Page { ..p, id: 1, ..q }",
        "Page { id: 1 id: 2 }",
        "Page { X: 1 }",
        "Page { id = 1 }",
        "Page { id }",
        "Page { id: }",
        "Page { : 1 }",
        "Page { , }",
        "Page { id: 1,, }",
        "Page { ..,  }",
        "Page { 1: 2 }",
        "Page { \"id\": 1 }",
        "Page { id: 1 ",
        "Page { ..p",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn struct_literal_requires_a_type_path_on_the_same_line() {
    // 3.12 rule 2: the chain must be `( LOWER_IDENT "." )* UPPER_IDENT`.
    rejected("page { id: 1 }", codes::SYNTAX_ERROR);
    rejected("a.b { id: 1 }", codes::SYNTAX_ERROR);
    let e = rejected("a.B.C { x: 1 }", codes::SYNTAX_ERROR);
    assert!(has_message(
        &e,
        "a struct literal names a type; variants have positional payloads"
    ));
    // D30: no type arguments.
    rejected("Pair<Int, Text> { a: 1 }", codes::SYNTAX_ERROR);
    // 2.3: the `{` of a struct literal is not a header `{`; it cannot start the next line.
    rejected("Page\n{ id: 1 }", codes::SYNTAX_ERROR);
    tail("a.b.C { x: 1 }", "(struct-lit a.b.C (field x (int 1)))");
}

#[test]
fn struct_literals_are_rejected_at_the_top_of_header_expressions() {
    // D41: condition of if/while, scrutinee of match, for range, clause expressions.
    rejected("if p == Page { id: 1 } {\n}\n1", codes::SYNTAX_ERROR);
    rejected("while p == Page { id: 1 } {\n}\n1", codes::SYNTAX_ERROR);
    rejected("match Page { id: 1 } {\n    _ => 1\n}", codes::SYNTAX_ERROR);
    rejected(
        "match p == Page { id: 1 } {\n    _ => 1\n}",
        codes::SYNTAX_ERROR,
    );
    rejected(
        "while c\n    invariant p == Page { id: 1 }\n{\n}\n1",
        codes::SYNTAX_ERROR,
    );
    rejected(
        "while c\n    decreases Page { id: 1 }\n{\n}\n1",
        codes::SYNTAX_ERROR,
    );
    module_errors(
        "module m\n\nfn f(p: Page) -> Int\n    requires p == Page { id: 1 }\n{\n    1\n}\n",
    );
    module_errors(
        "module m\n\nfn f(p: Page) -> Int\n    ensures result == Page { id: 1 }\n{\n    1\n}\n",
    );
    module_errors("module m\n\nfn f(p: Page) -> Int\n    decreases Page { id: 1 }\n{\n    1\n}\n");
    module_errors(
        "module m\n\nproperty \"p\"\n    for x in Gen { seed: 1 }\n{\n    assert true\n}\n",
    );
    // The `where` predicate of a refinement (3.5) is NOT in D41's list of
    // header expressions (no `{` can follow it), so a struct literal is legal there.
    assert_contains(
        &module("module m\n\npub type P = Page where self == Page { id: 1 }\n"),
        "(refinement (ty Page) (where (== (name self) (struct-lit Page (field id (int 1))))))",
    );
}

#[test]
fn struct_literals_are_re_enabled_inside_brackets_and_bodies() {
    assert_contains(
        &body("if (Page { id: 1 }) == p {}\n1"),
        "(if (== (paren (struct-lit Page (field id (int 1)))) (name p)) (block))",
    );
    assert_contains(
        &body("if c { Page { ..p } } else { q }"),
        "(if (name c) (block (tail (struct-lit Page (spread (name p))))) (else (block (tail (name q)))))",
    );
    assert_contains(
        &body("if c {} else { Page { ..p } }"),
        "(else (block (tail (struct-lit Page (spread (name p))))))",
    );
    assert_contains(
        &body("while c { p = Page { ..p } }\n1"),
        "(while (name c) (block (assign p (struct-lit Page (spread (name p))))))",
    );
    assert_contains(
        &body("match r { Ok(q) => Page { ..q } }"),
        "(match (name r) (arm (ctor Ok (bind q)) (struct-lit Page (spread (name q)))))",
    );
    assert_contains(
        &body("match [Page { id: 1 }] {\n    _ => 1\n}"),
        "(match (list (struct-lit Page (field id (int 1)))) (arm _ (int 1)))",
    );
    let d = module("module m\n\nfn f(p: Page) -> Page\n    requires xs == [Page { ..p }]\n    ensures result == make(Page { ..p })\n{\n    p\n}\n");
    assert_contains(
        &d,
        "(requires (== (name xs) (list (struct-lit Page (spread (name p))))))",
    );
    assert_contains(
        &d,
        "(ensures (== (name result) (call (name make) (struct-lit Page (spread (name p))))))",
    );
    // The flag is restored after the header: the next statement sees `true`.
    assert_contains(
        &body("if c {\n}\nPage { ..p }"),
        "(tail (struct-lit Page (spread (name p))))",
    );
    // A `forall` collection is an or_expr; outside a header it may be a struct literal.
    assert_contains(
        &body("let ok = forall x in Pages { items: xs }: x > 0\nok"),
        "(let ok (forall x (struct-lit Pages (field items (name xs))) (> (name x) (int 0))))",
    );
}

// ---------------------------------------------------------------------------
// 1.7 D10, 5 (c): holes versus postfix `?`
// ---------------------------------------------------------------------------

#[test]
fn holes_are_operands_in_every_expression_position() {
    tail("?build_pages", "(hole build_pages)");
    assert_contains(&body("let x = ?todo\nx"), "(let x (hole todo))");
    assert_contains(&body("return ?v"), "(return (hole v))");
    assert_contains(&body("x = ?v\n1"), "(assign x (hole v))");
    assert_contains(&body("assert ?p\n1"), "(assert (hole p))");
    tail("f(?x, ?y)", "(call (name f) (hole x) (hole y))");
    tail("f(using ?db)", "(call (name f) (using (hole db)))");
    tail("[?a, ?b]", "(list (hole a) (hole b))");
    tail("Page { id: ?id }", "(struct-lit Page (field id (hole id)))");
    assert_contains(
        &body("if ?c { 1 }\n1"),
        "(if (hole c) (block (tail (int 1))))",
    );
    assert_contains(
        &body("match ?s { _ => 1 }"),
        "(match (hole s) (arm _ (int 1)))",
    );
    tail("?x == ?y", "(== (hole x) (hole y))");
    tail("?x * 2 + 1", "(+ (* (hole x) (int 2)) (int 1))");
    tail("forall x in ?xs: ?p", "(forall x (hole xs) (hole p))");
    tail("(?x)", "(paren (hole x))");
    // D50: `self` and `result` are ordinary identifiers, so these are holes; `_x` is a LOWER_IDENT.
    tail("?self", "(hole self)");
    tail("?result", "(hole result)");
    tail("?_x", "(hole _x)");
    tail("?add", "(hole add)"); // contextual keywords are ordinary in .cpl
}

#[test]
fn propagation_is_a_question_not_followed_by_an_identifier_start() {
    tail("x?", "(propagate (name x))");
    tail("x ?", "(propagate (name x))");
    tail("f(x)?", "(propagate (call (name f) (name x)))");
    tail("x? + 1", "(+ (propagate (name x)) (int 1))");
    tail("x?+1", "(+ (propagate (name x)) (int 1))");
    tail("x?.y?", "(propagate (member (propagate (name x)) y))");
    tail("(x)?", "(propagate (paren (name x)))");
    tail("x?(1)", "(call (propagate (name x)) (int 1))");
}

#[test]
fn question_adjacent_to_an_identifier_is_a_hole_and_breaks_the_statement() {
    // 1.7 consequences: `x?y` lexes as `x HOLE(y)` and is a syntax error;
    // `x? y` is propagation followed by a syntax error.
    for src in ["x?y", "x ?y", "x? y", "f()?y", "x?y?", "a + b?c"] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn a_hole_admits_no_postfix_operator() {
    for src in [
        "?x?",
        "?x.y",
        "?x(1)",
        "?x<T>(1)",
        "?x { a: 1 }",
        "?x?.y",
        "?x()?",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
    // Parenthesized, postfixes apply to the parenthesis (3.12: "parenthesize if ever needed").
    tail("(?x)?", "(propagate (paren (hole x)))");
    tail("(?x).y", "(member (paren (hole x)) y)");
}

#[test]
fn lone_question_marks_are_errors() {
    for src in ["?", "? x", "??", "? ?x", "x ? y", "(?)", "f(?)"] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn hole_names_must_be_snake_case() {
    // 1.7: `?Foo` and `?_` are `hole names are snake_case`; semantics 9.4 gives the code and `name` field.
    for (src, name) in [
        ("?Foo", "Foo"),
        ("?_", "_"),
        ("?X", "X"),
        ("let y = ?Bad\ny", "Bad"),
    ] {
        let e = body_errors(src);
        let d = e
            .iter()
            .find(|d| d.code == "hole_name_case")
            .unwrap_or_else(|| panic!("{src}: expected hole_name_case, got {e:#?}"));
        assert_eq!(d.message, "hole names are snake_case", "{src}");
        assert_eq!(
            d.data.get("name").and_then(|v| v.as_str()),
            Some(name),
            "{src}: `name` field"
        );
    }
}

#[test]
fn hole_names_cannot_be_keywords() {
    // 1.7: `a hole name cannot be a keyword` for reserved keywords and
    // reserved-future words. D59: the code is the one semantics 9.4 assigns
    // to that message, which is `hole_name_keyword` (distinct from
    // `hole_name_case`).
    for (src, name) in [
        ("?if", "if"),
        ("?loop", "loop"),
        ("?test", "test"),
        ("?forall", "forall"),
        ("?not", "not"),
        ("?as", "as"),
    ] {
        let e = body_errors(src);
        let d = e
            .iter()
            .find(|d| d.message == "a hole name cannot be a keyword")
            .unwrap_or_else(|| panic!("{src}: expected the keyword-hole message, got {e:#?}"));
        assert_eq!(
            d.code, "hole_name_keyword",
            "{src}: semantics 9.4 assigns `hole_name_keyword` to this message"
        );
        assert_eq!(
            d.data.get("name").and_then(|v| v.as_str()),
            Some(name),
            "{src}: `name` field"
        );
    }
}

#[test]
fn holes_are_not_binders_types_or_patterns() {
    rejected("let ?x = 1\n1", codes::SYNTAX_ERROR);
    rejected("var ?x = 1\n1", codes::SYNTAX_ERROR);
    rejected("let x: ?t = 1\nx", codes::SYNTAX_ERROR);
    rejected("forall ?x in xs: p", codes::SYNTAX_ERROR);
    rejected("Page { ?id: 1 }", codes::SYNTAX_ERROR);
    rejected("f(?x: 1)", codes::SYNTAX_ERROR);
    arm_rejected("?p");
    arm_rejected("Ok(?p)");
    module_errors("module m\n\nfn ?f() -> Int {\n    1\n}\n");
}

// ---------------------------------------------------------------------------
// 5 (f) D45: forall
// ---------------------------------------------------------------------------

#[test]
fn forall_predicate_extends_to_the_end_of_the_expression() {
    assert_contains(
        &body("let ok = forall p in xs: a and b or c\nok"),
        "(let ok (forall p (name xs) (or (and (name a) (name b)) (name c))))",
    );
    tail(
        "forall x in xs: x > 0 and x < 9",
        "(forall x (name xs) (and (> (name x) (int 0)) (< (name x) (int 9))))",
    );
    tail(
        "forall x in xs: (a and b) or c",
        "(forall x (name xs) (or (paren (and (name a) (name b))) (name c)))",
    );
    tail(
        "forall x in xs: x?",
        "(forall x (name xs) (propagate (name x)))",
    );
    tail(
        "forall x in xs: not x",
        "(forall x (name xs) (not (name x)))",
    );
    tail(
        "forall x in xs: p.len",
        "(forall x (name xs) (member (name p) len))",
    );
    tail(
        "forall x in xs: if c { a } else { b }",
        "(forall x (name xs) (if (name c) (block (tail (name a))) (else (block (tail (name b))))))",
    );
}

#[test]
fn forall_collection_is_an_or_expr() {
    tail(
        "forall x in a or b: p",
        "(forall x (or (name a) (name b)) (name p))",
    );
    tail(
        "forall x in a < b: p",
        "(forall x (< (name a) (name b)) (name p))",
    );
    tail(
        "forall x in [1, 2]: x > 0",
        "(forall x (list (int 1) (int 2)) (> (name x) (int 0)))",
    );
    tail(
        "forall x in xs.drop(1): x",
        "(forall x (call (member (name xs) drop) (int 1)) (name x))",
    );
    tail(
        "forall x in f(xs)?: x",
        "(forall x (propagate (call (name f) (name xs))) (name x))",
    );
    tail(
        "forall x in (xs): x",
        "(forall x (paren (name xs)) (name x))",
    );
}

#[test]
fn forall_nests_only_as_a_predicate() {
    tail(
        "forall x in xs: forall y in ys: x == y and z",
        "(forall x (name xs) (forall y (name ys) (and (== (name x) (name y)) (name z))))",
    );
    tail(
        "forall x in xs: forall y in ys: forall z in zs: true",
        "(forall x (name xs) (forall y (name ys) (forall z (name zs) (bool true))))",
    );
    // D45: `forall` is never a left or right operand; parenthesize it.
    for src in [
        "forall x in forall y in ys: q: p",
        "forall x in xs: p and forall y in ys: q",
        "forall x in xs: p or forall y in ys: q",
        "(forall x in xs: p) and forall y in ys: q",
        "a and forall x in xs: p",
        "a or forall x in xs: p",
        "1 + forall x in xs: p",
        "forall x in xs: p ++ forall y in ys: q",
        "not forall x in xs: p",
        "-forall x in xs: p",
        "f(x) == forall x in xs: p",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn parenthesized_forall_is_an_operand() {
    tail(
        "(forall x in xs: x > 0) and b",
        "(and (paren (forall x (name xs) (> (name x) (int 0)))) (name b))",
    );
    tail(
        "a and (forall x in xs: p) or c",
        "(or (and (name a) (paren (forall x (name xs) (name p)))) (name c))",
    );
    tail(
        "not (forall x in xs: p)",
        "(not (paren (forall x (name xs) (name p))))",
    );
    tail(
        "(forall x in xs: p).len",
        "(member (paren (forall x (name xs) (name p))) len)",
    );
    tail(
        "(forall x in xs: p) == (forall y in ys: q)",
        "(== (paren (forall x (name xs) (name p))) (paren (forall y (name ys) (name q))))",
    );
}

#[test]
fn forall_is_a_complete_expression_in_every_expression_position() {
    // expr = forall_expr | or_expr: wherever `expr` appears, a forall may.
    assert_contains(
        &body("assert forall x in xs: x > 0\n1"),
        "(assert (forall x (name xs) (> (name x) (int 0))))",
    );
    assert_contains(
        &body("return forall x in xs: p"),
        "(return (forall x (name xs) (name p)))",
    );
    assert_contains(
        &body("ok = forall x in xs: p\n1"),
        "(assign ok (forall x (name xs) (name p)))",
    );
    tail(
        "f(forall x in xs: x > 0)",
        "(call (name f) (forall x (name xs) (> (name x) (int 0))))",
    );
    tail(
        "f(forall x in xs: x > 0, 1)",
        "(call (name f) (forall x (name xs) (> (name x) (int 0))) (int 1))",
    );
    tail(
        "f(1, forall x in xs: p)",
        "(call (name f) (int 1) (forall x (name xs) (name p)))",
    );
    tail(
        "f(using forall x in xs: p)",
        "(call (name f) (using (forall x (name xs) (name p))))",
    );
    tail(
        "[forall x in xs: p]",
        "(list (forall x (name xs) (name p)))",
    );
    tail(
        "[forall x in xs: p, forall y in ys: q]",
        "(list (forall x (name xs) (name p)) (forall y (name ys) (name q)))",
    );
    assert_contains(
        &body("if forall x in xs: x > 0 { 1 }\n1"),
        "(if (forall x (name xs) (> (name x) (int 0))) (block (tail (int 1))))",
    );
    assert_contains(
        &body("while forall x in xs: x > 0 {}\n1"),
        "(while (forall x (name xs) (> (name x) (int 0))) (block))",
    );
    assert_contains(
        &body("match x { _ => forall y in ys: y > 0 }"),
        "(arm _ (forall y (name ys) (> (name y) (int 0))))",
    );
    // E3/E5: as the whole expression of ensures and invariant clauses.
    let d = module("module m\n\nfn f(xs: List<Int>) -> Int\n    ensures forall x in result: x > 0\n{\n    while c\n        invariant forall x in xs: x == 1\n    {\n    }\n    1\n}\n");
    assert_contains(
        &d,
        "(ensures (forall x (name result) (> (name x) (int 0))))",
    );
    assert_contains(&d, "(invariant (forall x (name xs) (== (name x) (int 1))))");
}

#[test]
fn malformed_forall() {
    for src in [
        "forall X in xs: p",
        "forall x in xs",
        "forall x in xs:",
        "forall x in xs:\n    p",
        "forall in xs: p",
        "forall x xs: p",
        "forall x in : p",
        "forall x in xs p",
        "forall x, y in xs: p",
        "forall x in xs: p,",
        "forall",
        "forall x",
        "forall x in",
        "forall _ in xs: p",
        "forall 1 in xs: p",
        "forall x in xs => p",
        "f(forall)",
        "forall x in xs: p.",
        "forall ?x in xs: p",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

// ---------------------------------------------------------------------------
// 5 (g), 3.12: `if` and `match` as operands
// ---------------------------------------------------------------------------

#[test]
fn if_and_match_as_arguments_and_elements() {
    tail(
        "f(if c { 1 } else { 2 }, 3)",
        "(call (name f) (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))) (int 3))",
    );
    tail(
        "f(match x {\n    A => 1\n    B => 2\n}, 2)",
        "(call (name f) (match (name x) (arm (ctor A) (int 1)) (arm (ctor B) (int 2))) (int 2))",
    );
    tail(
        "[if a { 1 } else { 2 }, 3]",
        "(list (if (name a) (block (tail (int 1))) (else (block (tail (int 2))))) (int 3))",
    );
    tail("f(using if c { a } else { b })", "(call (name f) (using (if (name c) (block (tail (name a))) (else (block (tail (name b)))))))");
}

#[test]
fn if_and_match_as_the_value_of_a_let_var_assign_or_return() {
    assert_contains(
        &body("let v = match x {\n    A => 1\n    _ => 2\n}\nv"),
        "(let v (match (name x) (arm (ctor A) (int 1)) (arm _ (int 2))))",
    );
    assert_contains(
        &body("let v = match x\n{\n    A => 1\n}\nv"),
        "(let v (match (name x) (arm (ctor A) (int 1))))",
    );
    assert_contains(
        &body("var v = if c { 1 } else { 2 }\nv"),
        "(var v (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    assert_contains(
        &body("v = if c { 1 } else { 2 }\n1"),
        "(assign v (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    assert_contains(
        &body("return match x { _ => 1 }"),
        "(return (match (name x) (arm _ (int 1))))",
    );
    // The typechecker, not the parser, demands the `else` of a value `if` (D46).
    assert_contains(
        &body("let x = if c { 1 }\nx"),
        "(let x (if (name c) (block (tail (int 1)))))",
    );
}

#[test]
fn if_and_match_are_primaries_so_operators_may_follow_them() {
    tail(
        "x + match y { _ => 1 }",
        "(+ (name x) (match (name y) (arm _ (int 1))))",
    );
    tail(
        "if c { 1 } else { 2 } + 3",
        "(+ (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))) (int 3))",
    );
    tail(
        "match x { _ => 1 } == 1",
        "(== (match (name x) (arm _ (int 1))) (int 1))",
    );
    tail(
        "if c { 1 } else { 2 }?",
        "(propagate (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    tail(
        "-if c { 1 } else { 2 }",
        "(neg (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    tail(
        "not if c { true } else { false }",
        "(not (if (name c) (block (tail (bool true))) (else (block (tail (bool false))))))",
    );
    tail("1 + if c { 1 } else { 2 } * 3", "(+ (int 1) (* (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))) (int 3)))");
}

#[test]
fn if_and_match_as_header_expressions_of_each_other() {
    assert_contains(
        &body("if match x { _ => true } { 1 }\n1"),
        "(if (match (name x) (arm _ (bool true))) (block (tail (int 1))))",
    );
    assert_contains(
        &body("if if a { b } else { c } { 1 }\n1"),
        "(if (if (name a) (block (tail (name b))) (else (block (tail (name c))))) (block (tail (int 1))))",
    );
    assert_contains(
        &body("match if a { b } else { c } {\n    _ => 1\n}"),
        "(match (if (name a) (block (tail (name b))) (else (block (tail (name c))))) (arm _ (int 1)))",
    );
}

#[test]
fn match_bodies_and_arm_bodies() {
    // lines(match_arm) admits zero arms; exhaustiveness is the typechecker's.
    tail("match x {}", "(match (name x))");
    tail("match x {\n}", "(match (name x))");
    assert_contains(
        &body("match x { A => { 1 } }"),
        "(arm (ctor A) (block (tail (int 1))))",
    );
    assert_contains(
        &body("match x { A => 1 + 2 }"),
        "(arm (ctor A) (+ (int 1) (int 2)))",
    );
    assert_contains(
        &body("match x { A => if c { 1 } else { 2 } }"),
        "(arm (ctor A) (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    assert_contains(
        &body("match x { A => f(y)? }"),
        "(arm (ctor A) (propagate (call (name f) (name y))))",
    );
    assert_contains(
        &body("match x {\n    A => {\n        let y = 1\n        y\n    }\n    _ => 0\n}"),
        "(arm (ctor A) (block (let y (int 1)) (tail (name y))))",
    );
    // An arm body that is a block may be empty.
    assert_contains(&body("match x { A => {} }"), "(arm (ctor A) (block))");
}

#[test]
fn malformed_if_and_match() {
    for src in [
        "match x { _ => }",
        "match x { => 1 }",
        "match x { _ 1 }",
        "match x { A => 1 B => 2 }",
        "match x { A => return 1 }",
        "match {\n    _ => 1\n}",
        "match x\n    _ => 1",
        "match x { _ -> 1 }",
        "match x { _ = 1 }",
        "if { 1 }",
        "if c 1",
        "if c { 1 } else",
        "if c { 1 } else 2",
        "if c { 1 } else if { 2 }",
        "else { 1 }",
        "let x = if\nx",
        "if c then { 1 }",
        "if (c { 1 }",
        "match x { _ => 1 }}",
        "if c { 1 } { 2 }",
    ] {
        body_errors(src);
    }
}

// ---------------------------------------------------------------------------
// 5 (h), 3.12: list literals
// ---------------------------------------------------------------------------

#[test]
fn list_literals_empty_nested_and_mixed() {
    tail("[]", "(list)");
    tail("[[], []]", "(list (list) (list))");
    tail("[[[]]]", "(list (list (list)))");
    tail(
        "[1, \"a\", true, x, f(y), -1, ?h, [2]]",
        "(list (int 1) (text \"a\") (bool true) (name x) (call (name f) (name y)) (neg (int 1)) (hole h) (list (int 2)))",
    );
    tail("[1,]", "(list (int 1))");
    tail(
        "[[1, 2,], [3,],]",
        "(list (list (int 1) (int 2)) (list (int 3)))",
    );
    tail("[\n    1,\n    2\n]", "(list (int 1) (int 2))");
    tail("[\n    1,\n    2,\n]", "(list (int 1) (int 2))");
    tail(
        "[a + b, c and d, e < f]",
        "(list (+ (name a) (name b)) (and (name c) (name d)) (< (name e) (name f)))",
    );
    tail(
        "[1, 2] ++ [3]",
        "(++ (list (int 1) (int 2)) (list (int 3)))",
    );
    tail(
        "[1, 2] == [1, 2]",
        "(== (list (int 1) (int 2)) (list (int 1) (int 2)))",
    );
    tail(
        "[Page { id: 1 }, Page { id: 2 }]",
        "(list (struct-lit Page (field id (int 1))) (struct-lit Page (field id (int 2))))",
    );
    tail(
        "[if c { 1 } else { 2 }]",
        "(list (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    // E8: `[[1, 2], [3]]`
    tail(
        "[[1, 2], [3]]",
        "(list (list (int 1) (int 2)) (list (int 3)))",
    );
}

#[test]
fn malformed_list_literals() {
    for src in [
        "[,]",
        "[1,, 2]",
        "[1 2]",
        "[1, 2",
        "[..xs]",
        "[1, 2]3",
        "]",
        "[1\n2]",
        "[1, 2,,]",
        "[[1]",
        "[1]]",
        "[,1]",
        "[using x]",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
    rejected("[1;2]", codes::INVALID_CHARACTER);
}

// ---------------------------------------------------------------------------
// 3.4, 1.6, 2.2: nested generics in types
// ---------------------------------------------------------------------------

#[test]
fn nested_generic_types_split_their_closing_angles() {
    assert_contains(
        &body("let x: List<List<List<Int>>> = []\nx"),
        "(let x (ty List (ty List (ty List (ty Int)))) (list))",
    );
    assert_contains(
        &body("var p: Option<Result<Page, StoreError>> = None\np"),
        "(var p (ty Option (ty Result (ty Page) (ty StoreError))) (name None))",
    );
    let d = module("module m\n\nfn f(m: Map<K, List<V>>, x: a.b.List<c.D>) -> Result<List<List<Int>>, ConstraintError> {\n    1\n}\n");
    assert_contains(&d, "(param m (ty Map (ty K) (ty List (ty V))))");
    assert_contains(&d, "(param x (ty a.b.List (ty c.D)))");
    assert_contains(
        &d,
        "(ret (ty Result (ty List (ty List (ty Int))) (ty ConstraintError)))",
    );
    let d = module("module m\n\nfn f<T, U>(x: Pair<T, U>) -> Pair<U, T> {\n    x\n}\n");
    assert_contains(
        &d,
        "(generics T U) (params (param x (ty Pair (ty T) (ty U)))) (ret (ty Pair (ty U) (ty T)))",
    );
    let d = module("module m\n\nstruct S {\n    x: List<List<Option<Int>>>\n}\n\nenum E {\n    A(List<List<Int>>, Result<Unit, E>)\n}\n");
    assert_contains(&d, "(field x (ty List (ty List (ty Option (ty Int)))))");
    assert_contains(
        &d,
        "(variant A (ty List (ty List (ty Int))) (ty Result (ty Unit) (ty E)))",
    );
    let d = module("module m\n\npub type W(List<List<Int>>)\n");
    assert_contains(&d, "(wrapper (ty List (ty List (ty Int))))");
}

#[test]
fn generic_type_lists_and_newlines() {
    // 2.2: a line break inside a generic list is a syntax error where
    // newlines are significant and is discarded inside brackets.
    rejected("let x: List<\n    Int> = []\nx", codes::SYNTAX_ERROR);
    module_errors("module m\n\nfn f() -> List<Int\n> {\n    x\n}\n");
    let d = module("module m\n\nfn f(x: List<\n    Int>) -> Int {\n    1\n}\n");
    assert_contains(&d, "(param x (ty List (ty Int)))");
    let d = module("module m\n\nfn f(x: Map<K,\n    V>) -> Int {\n    1\n}\n");
    assert_contains(&d, "(param x (ty Map (ty K) (ty V)))");
}

#[test]
fn malformed_generic_type_lists() {
    for src in [
        "let x: List<> = []\nx",
        "let x: List<Int,> = []\nx",
        "let x: List<int> = []\nx",
        "let x: List<Int = []\nx",
        "let x: List<<Int>> = []\nx",
        "let x: List<Int>> = []\nx",
        "let x: List<Int, > = []\nx",
        "let x: List<1> = []\nx",
        "let x: List<Int List<T>> = []\nx",
        "let x: List<Int>(x) = 1\nx",
        "let x: list = 1\nx",
        "let x: 1 = 2\nx",
        "let x: List<(Int)> = []\nx",
        "let x: List<Int>.len = 1\nx",
        "let x: List<?t> = []\nx",
    ] {
        rejected(src, codes::SYNTAX_ERROR);
    }
}

// ---------------------------------------------------------------------------
// 1.5: Int literals
// ---------------------------------------------------------------------------

#[test]
fn int_literals_carry_digits_without_separators_and_have_no_range() {
    tail("1_000_000", "(int 1000000)");
    tail("1_0_0", "(int 100)");
    tail("0", "(int 0)");
    tail("42", "(int 42)");
    let huge = "123456789012345678901234567890123456789012345678901234567890";
    tail(huge, &format!("(int {huge})"));
    tail(
        "99_999_999_999_999_999_999_999_999",
        "(int 99999999999999999999999999)",
    );
    // `_1` is an identifier (1.3, D52), not a literal.
    tail("_1", "(name _1)");
    tail("_1_000", "(name _1_000)");
    tail("1 + 1_000", "(+ (int 1) (int 1000))");
}

#[test]
fn int_literal_separator_errors() {
    // 1.5: `_` only between two digits.
    for src in [
        "1__2",
        "1_",
        "1__",
        "1_000_",
        "let x = 1__2\nx",
        "f(1_)",
        "1_000__000",
    ] {
        rejected(src, codes::INVALID_INT_LITERAL);
    }
}

#[test]
fn int_literal_leading_zero_errors() {
    // 1.5: `0` is the only literal that may begin with `0`.
    for src in ["01", "00", "007", "0_0", "0_1", "0000000", "-01", "f(007)"] {
        let e = rejected(src, codes::INVALID_INT_LITERAL);
        assert!(
            has_message(&e, "leading zeros are not permitted"),
            "{src}: {e:#?}"
        );
    }
}

#[test]
fn int_literal_adjacency_and_float_errors() {
    for src in ["1e5", "2x", "1_000x", "0x1F", "1b", "3abc", "1_e"] {
        rejected(src, codes::LITERAL_ADJACENT_IDENTIFIER);
    }
    for src in ["1.5", "0.0", "1_000.5", "1.0.len", "-1.5", "f(2.5)"] {
        let e = rejected(src, codes::FLOAT_LITERAL_UNSUPPORTED);
        assert!(
            has_message(
                &e,
                "floating-point literals are not supported in edition 0.1"
            ),
            "{src}"
        );
    }
    // `1.` followed by a non-digit is INT then `.`: a member access missing its name.
    rejected("1.", codes::SYNTAX_ERROR);
    rejected("1. + 2", codes::SYNTAX_ERROR);
}

// ---------------------------------------------------------------------------
// 1.5: Text literals and escapes
// ---------------------------------------------------------------------------

fn text_lit(src: &str, decoded: &str) {
    tail(src, &format!("(text {decoded:?})"));
}

#[test]
fn every_valid_escape_decodes() {
    text_lit(r#""a\\b""#, "a\\b");
    text_lit(r#""a\"b""#, "a\"b");
    text_lit(r#""a\nb""#, "a\nb");
    text_lit(r#""a\rb""#, "a\rb");
    text_lit(r#""a\tb""#, "a\tb");
    text_lit(r#""\u{41}""#, "A");
    text_lit(r#""\u{0041}""#, "A");
    text_lit(r#""\u{000041}""#, "A");
    text_lit(r#""\u{e9}""#, "\u{e9}");
    text_lit(r#""\u{1F600}""#, "\u{1F600}");
    text_lit(r#""\u{10FFFF}""#, "\u{10FFFF}");
    text_lit(r#""\u{0}""#, "\0");
    text_lit(r#""\u{D7FF}\u{E000}""#, "\u{D7FF}\u{E000}");
    text_lit(r#""\\\"\n\r\t\u{41}""#, "\\\"\n\r\t\u{41}");
    text_lit(r#""""#, "");
    text_lit(r#""\\""#, "\\");
    text_lit(r#""\"""#, "\"");
    // Raw non-ASCII and raw tabs are permitted (D62); C1 controls too.
    text_lit(
        "\"h\u{e9}llo \u{6f22}\u{5b57}\"",
        "h\u{e9}llo \u{6f22}\u{5b57}",
    );
    text_lit("\"a\tb\"", "a\tb");
    text_lit("\"\u{85}\"", "\u{85}");
}

#[test]
fn text_literals_in_expressions() {
    tail(r#""a" ++ "b""#, "(++ (text \"a\") (text \"b\"))");
    tail(
        r#"f("x", "y")"#,
        "(call (name f) (text \"x\") (text \"y\"))",
    );
    tail(r#""a" == "a""#, "(== (text \"a\") (text \"a\"))");
    tail(r#"["a", "b",]"#, "(list (text \"a\") (text \"b\"))");
    assert_contains(&body("let s = \"x\"\ns"), "(let s (text \"x\"))");
    // Two adjacent literals are two tokens on one statement: an error.
    rejected(r#""a" "b""#, codes::SYNTAX_ERROR);
}

#[test]
fn invalid_escapes_are_lexical_errors() {
    // 1.5: exactly six escapes; anything else after `\` is an error;
    // `\u{}` is bounded to 1..=6 hex digits and must be a scalar value.
    for src in [
        r#""\a""#,
        r#""\0""#,
        r#""\x41""#,
        r#""\'""#,
        r#""\ ""#,
        r#""\N""#,
        r#""\U{41}""#,
        r#""\u{}""#,
        r#""\u{1234567}""#,
        r#""\u{0000041}""#,
        r#""\u{D800}""#,
        r#""\u{DFFF}""#,
        r#""\u{DBFF}""#,
        r#""\u{110000}""#,
        r#""\u{FFFFFF}""#,
        r#""\u41""#,
        r#""\u{41""#,
        r#""\u{4G}""#,
        r#""\u{ 41}""#,
        r#""\u{41 }""#,
        r#""\u""#,
        r#""\u{-1}""#,
        r#""\/""#,
        r#""\b""#,
        r#""\f""#,
        r#""\v""#,
        r#""\e""#,
    ] {
        rejected(src, codes::INVALID_ESCAPE);
    }
    let e = rejected(r#""\u{1234567}""#, codes::INVALID_ESCAPE);
    assert!(has_message(
        &e,
        "unicode escape has more than six hex digits"
    ));
}

#[test]
fn unterminated_text_literals() {
    for src in [
        "\"abc",
        "\"abc\n1",
        "\"abc\\\"\n1",
        "\"\\",
        "\"",
        "let s = \"abc\ns",
        "f(\"x)",
        "\"abc\\n",
    ] {
        rejected(src, codes::UNTERMINATED_TEXT_LITERAL);
    }
    let e = rejected("\"abc", codes::UNTERMINATED_TEXT_LITERAL);
    assert!(has_message(&e, "unterminated text literal"));
}

#[test]
fn control_characters_in_text_literals() {
    // D62: C0 controls other than tab, and DEL, are `control character in text literal`.
    for src in [
        "\"a\u{1}b\"",
        "\"a\u{7f}b\"",
        "\"\u{0}\"",
        "\"\u{1b}\"",
        "\"\u{b}\"",
        "\"\u{c}\"",
    ] {
        let e = rejected(src, codes::INVALID_CHARACTER);
        assert!(
            has_message(&e, "control character in text literal"),
            "{src:?}"
        );
    }
    // A raw line terminator is `unterminated text literal`, not a control-character error.
    let e = rejected("\"a\nb\"", codes::UNTERMINATED_TEXT_LITERAL);
    assert!(!has_message(&e, "control character"));
}

#[test]
fn prefixed_text_literals_are_reserved() {
    for src in ["b\"x\"", "r\"x\"", "f\"x\"", "x\"\""] {
        rejected(src, codes::PREFIXED_LITERAL_RESERVED);
    }
    // With a space it is two tokens (and a syntax error at the parser).
    rejected("b \"x\"", codes::SYNTAX_ERROR);
}

// ---------------------------------------------------------------------------
// 3.13: patterns
// ---------------------------------------------------------------------------

#[test]
fn nested_constructor_patterns() {
    arm_is(
        "Ok(Some(Pair(a, b)))",
        "(ctor Ok (ctor Some (ctor Pair (bind a) (bind b))))",
    );
    arm_is(
        "Err(Missing(PageId(id)))",
        "(ctor Err (ctor Missing (ctor PageId (bind id))))",
    );
    arm_is(
        "A(B(C(D(E(x)))))",
        "(ctor A (ctor B (ctor C (ctor D (ctor E (bind x))))))",
    );
    arm_is(
        "Pair(Some(_), None)",
        "(ctor Pair (ctor Some _) (ctor None))",
    );
    arm_is("Ok(Err(e))", "(ctor Ok (ctor Err (bind e)))");
    // E2: `Err(Missing(_))`, `Err(Unavailable)`
    arm_is("Err(Missing(_))", "(ctor Err (ctor Missing _))");
    arm_is("Err(Unavailable)", "(ctor Err (ctor Unavailable))");
}

#[test]
fn qualified_constructor_patterns() {
    arm_is(
        "StoreError.Missing(x)",
        "(ctor StoreError.Missing (bind x))",
    );
    arm_is("StoreError.Unavailable", "(ctor StoreError.Unavailable)");
    arm_is("a.b.C.D(x)", "(ctor a.b.C.D (bind x))");
    arm_is("a.B(x)", "(ctor a.B (bind x))");
    arm_is("A.B", "(ctor A.B)");
    arm_is("A.B.C", "(ctor A.B.C)");
    arm_is(
        "reader.pages.StoreError.Missing(id)",
        "(ctor reader.pages.StoreError.Missing (bind id))",
    );
    arm_is(
        "reader.pages.PageId(t)",
        "(ctor reader.pages.PageId (bind t))",
    );
    arm_is("Ok(a.B.C(x))", "(ctor Ok (ctor a.B.C (bind x)))");
}

#[test]
fn literal_patterns() {
    arm_is("0", "(pat-int 0)");
    arm_is("1_000", "(pat-int 1000)");
    arm_is("-5", "(pat-int -5)");
    arm_is("- 5", "(pat-int -5)");
    arm_is("-0", "(pat-int -0)");
    arm_is("-1_000", "(pat-int -1000)");
    arm_is(
        "123456789012345678901234567890",
        "(pat-int 123456789012345678901234567890)",
    );
    arm_is("\"a b\"", "(pat-text \"a b\")");
    arm_is(r#""\n""#, &format!("(pat-text {:?})", "\n"));
    arm_is(r#""\u{41}""#, "(pat-text \"A\")");
    arm_is("\"\"", "(pat-text \"\")");
    arm_is("true", "(pat-bool true)");
    arm_is("false", "(pat-bool false)");
    // Literals nested in constructors.
    arm_is("Some(0)", "(ctor Some (pat-int 0))");
    arm_is("Ok(-1)", "(ctor Ok (pat-int -1))");
    arm_is("Err(\"x\")", "(ctor Err (pat-text \"x\"))");
    arm_is("Pair(true, _)", "(ctor Pair (pat-bool true) _)");
    arm_is("Pair(1, \"a\")", "(ctor Pair (pat-int 1) (pat-text \"a\"))");
}

#[test]
fn binding_and_wildcard_patterns() {
    arm_is("x", "(bind x)");
    arm_is("_x", "(bind _x)");
    arm_is("_", "_");
    arm_is("Some(_x)", "(ctor Some (bind _x))");
    arm_is("Some(_)", "(ctor Some _)");
    // D50: `self` and `result` are ordinary identifiers; D5: work words are ordinary in .cpl.
    arm_is("self", "(bind self)");
    arm_is("Some(result)", "(ctor Some (bind result))");
    arm_is("Some(add)", "(ctor Some (bind add))");
    arm_is("tests", "(bind tests)");
    // A bare UPPER_IDENT is a nullary constructor (D2).
    arm_is("None", "(ctor None)");
    arm_is("Unit", "(ctor Unit)");
    arm_is("Unavailable", "(ctor Unavailable)");
}

#[test]
fn constructor_arity_is_not_a_parser_matter() {
    // 3.13: `ctor_path ( "(" commas1(pattern) ")" )?` - any arity of one or
    // more parses; `pattern_arity_mismatch` is a `type` code (semantics 9.4).
    arm_is("Ok(a, b)", "(ctor Ok (bind a) (bind b))");
    arm_is("None(x)", "(ctor None (bind x))");
    arm_is("Some(a, b, c)", "(ctor Some (bind a) (bind b) (bind c))");
    arm_is("PageId(a, b)", "(ctor PageId (bind a) (bind b))");
    arm_is("Ok(_, _)", "(ctor Ok _ _)");
    arm_is("Unit(x)", "(ctor Unit (bind x))");
}

#[test]
fn pattern_payload_lists_allow_trailing_commas_and_newlines() {
    arm_is("Pair(a, b,)", "(ctor Pair (bind a) (bind b))");
    arm_is("Ok(x,)", "(ctor Ok (bind x))");
    assert_contains(
        &body("match x {\n    Pair(\n        a,\n        b,\n    ) => 1\n}"),
        "(arm (ctor Pair (bind a) (bind b)) (int 1))",
    );
    assert_contains(
        &body("match x {\n    Ok(Some(\n        v)) => 1\n}"),
        "(arm (ctor Ok (ctor Some (bind v))) (int 1))",
    );
}

#[test]
fn malformed_literal_patterns() {
    // 3.13: only `"-"? INT` may be negated.
    for pat in [
        "-x",
        "-\"a\"",
        "-true",
        "--5",
        "-_",
        "-None",
        "-",
        "- -5",
        "-(5)",
        "1 + 2",
        "\"a\" ++ \"b\"",
        "1..5",
        "- 1_",
        "-01",
    ] {
        arm_rejected(pat);
    }
}

#[test]
fn patterns_outside_the_0_1_language_are_errors() {
    // D33: no struct, list, or-, range, `@`, or guard patterns; D54: no ranges.
    arm_rejected("Page { id }");
    arm_rejected("Page { id: x }");
    arm_rejected("[x]");
    arm_rejected("[]");
    arm_rejected("[x, ..rest]");
    arm_rejected("(x)");
    arm_rejected("(A)");
    arm_rejected("Some(x) if x > 0");
    arm_rejected("x @ Some(y)");
    arm_rejected("1..5");
    arm_rejected("A(x) | B(x)");
    assert!(has_code(&arm_rejected("A | B"), codes::INVALID_CHARACTER));
    arm_rejected("Some(x) or None");
    arm_rejected("A & B");
    arm_rejected("..");
}

#[test]
fn malformed_constructor_paths() {
    // ctor_path = ( LOWER_IDENT "." )* UPPER_IDENT ( "." UPPER_IDENT )*
    for pat in [
        "A.b", "x.y", "a.b", "A.", "a.", ".A", "A..B", "A.1", "A.\"b\"", "A.(B)", "x.y(z)",
        "A.b(x)", "a.B.c", "A.B.c(x)", "_.A", "_(x)", "self.x",
    ] {
        arm_rejected(pat);
    }
    // Keywords and reserved words are not patterns.
    for pat in [
        "if", "test", "match", "loop", "as", "forall", "true(x)", "false.A",
    ] {
        arm_rejected(pat);
    }
}

#[test]
fn malformed_constructor_payloads() {
    for pat in [
        "Ok()",
        "Ok(,)",
        "Ok x",
        "Ok(?x)",
        "Ok(x)(y)",
        "Ok(x).y",
        "Ok(-)",
        "Ok(x y)",
        "Ok(x,, y)",
        "Ok(x",
        "Ok(x))",
        "Ok(x) ?",
        "Ok((x))",
        "Ok(x: y)",
        "Ok(using x)",
        "Ok(x)?",
    ] {
        arm_rejected(pat);
    }
    // 5(x), "a"(x) and the like: a literal pattern takes no payload.
    for pat in ["5(x)", "\"a\"(x)", "true(x)", "_(x)", "-5(x)"] {
        arm_rejected(pat);
    }
}

#[test]
fn arm_body_follows_the_fat_arrow() {
    for pat in [
        "Ok(x) -> 1",
        "Ok(x) = 1",
        "Ok(x) : 1",
        "Ok(x) 1",
        "Ok(x) =>",
        "=> 1",
    ] {
        body_errors(&format!("match x {{\n{pat}\n}}"));
    }
    assert_contains(
        &body("match x { Ok(x) => x }"),
        "(arm (ctor Ok (bind x)) (name x))",
    );
}

// ---------------------------------------------------------------------------
// D59: one diagnostic per file
// ---------------------------------------------------------------------------

#[test]
fn the_first_expression_error_stops_the_file() {
    // D59: "A file stops at its first lexical or syntax error: 0.1 performs
    // no error recovery or resynchronisation."
    let e = body_errors("a < b < c\nlet x = )\nx +\n1");
    assert_eq!(
        e.len(),
        1,
        "D59: exactly one diagnostic (the chained comparison), got {e:#?}"
    );
    assert_eq!(e[0].code, codes::SYNTAX_ERROR);
    let e = body_errors("f(1,, 2)\n[,]\nforall x in xs");
    assert_eq!(e.len(), 1, "D59: exactly one diagnostic, got {e:#?}");
}

// ---------------------------------------------------------------------------
// No panics
// ---------------------------------------------------------------------------

const ADVERSARIAL_EXPRESSIONS: &[&str] = &[
    "a < b < c < d < e < f",
    "a<B>(c)<D>(e)<F>(g)",
    "a < B > (c) < D > (e)",
    "f<T<U<V<W<X>>>>>(x)",
    "f<T>(x",
    "f<T(x)",
    "f<<T>>(x)",
    "f<>()",
    "f<,>()",
    "f<T,,>()",
    "a <<<< b",
    "a >>>> b",
    "<T>(x)",
    "?",
    "??",
    "???",
    "?x?y?z",
    "?x.?y",
    "?x<?y>(?z)",
    "?_?_",
    "?Foo.Bar(baz)",
    "?if?else",
    "- - - - -",
    "not not not not",
    "not -not -not",
    "-?-?-?",
    "a ++ ++ ++ b",
    "+++",
    "a +-* b",
    "a ** b",
    "a // b",
    "a %% b",
    "a == == b",
    "a =< b",
    "a => b",
    "a -> b",
    "a <> b",
    "a =! b",
    "a === b",
    "a !== b",
    "forall",
    "forall forall forall",
    "forall x in forall y in forall z in zs: q: p: o",
    "forall x in xs: forall y in ys: forall z in zs:",
    "forall x in xs: p and forall y in ys: q or forall z in zs: r",
    "(forall x in xs: (forall y in ys: (forall z in zs: true)))",
    "forall x in x: x",
    "forall in in in: in",
    "Page { ..Page { ..Page { ..Page { .. } } } }",
    "Page { a: Page { b: Page { c: } } }",
    "Page { ..Page { ..p },, }",
    "Page {{}}",
    "Page { } { }",
    "Page { id: 1 } { id: 2 }",
    "a.B.C.D.E { x: 1 }",
    "Page<T> { x: 1 }",
    "Page { x: 1 }<T>(y)",
    "if if if if c {} {} {} {}",
    "if c { if c { if c { if c { } } } } else { } else { }",
    "if c {} else if c {} else if c {} else",
    "match match match x {} {} {}",
    "match x { match y { _ => 1 } => 2 }",
    "match x { _ => match y { _ => match z { _ => 1 } } }",
    "match x { A => 1\n B => 2 }",
    "match x { A => , }",
    "match x { A =>\n 1 }",
    "match x { Ok(Ok(Ok(Ok(Ok(Ok(Ok(Ok(x)))))))) => 1 }",
    "match x { -5 => -5\n - 5 => - 5 }",
    "match x { \"\\u{D800}\" => 1 }",
    "match x { 1__2 => 1 }",
    "match x { 007 => 1 }",
    "[[[[[[[[[[]]]]]]]]]]",
    "[[[[[[[[[[",
    "]]]]]]]]]]",
    "[1, 2, 3",
    "[,,,]",
    "[..]",
    "[?]",
    "[using]",
    "f(using using using)",
    "f(using, using)",
    "f(x, using db, y, using e)",
    "f(\"unterminated",
    "f(\"\\",
    "\"\\u{",
    "\"\\u{FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF}\"",
    "\"\\u{0000000000000000041}\"",
    "\"\\\\\\\\\\\\\\\\\\\"",
    "1__________1",
    "1_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000_000",
    "0000000000000000000000000000000000000000000000000000000000000",
    "1.2.3.4.5",
    "1e1e1e1",
    "1x1x1x1",
    "1._1",
    "1.len.len.len",
    "x.1.2.3",
    "a.b.c.d.e.f.g.h.i.j.k.l.m.n.o.p.q.r.s.t.u.v.w.x.y.z",
    "a?????????????",
    "f()()()()()()()()()()",
    "f(f(f(f(f(f(f(f(f(f(x))))))))))",
    "x = y = z",
    "let x = let y = 1",
    "let x = var y = 1",
    "let x = return",
    "let x = assert y",
    "let x = while c {}",
    "let _ = _",
    "_ + _",
    "_.len",
    "_()",
    "a.b = c",
    "a[0]",
    "a[0] = 1",
    "a{0}",
    "f{x}",
    "{ x }",
    "{}",
    "(x, y)",
    "()",
    "(",
    ")",
    "(()",
    "())",
    "((((((((((1))))))))))",
    "a; b",
    "a | b || c",
    "a & b && c",
    "!a",
    "~a",
    "a ^ b",
    "a # b",
    "a $ b",
    "a ` b",
    "a \\ b",
    "a ' b",
    "é + ü",
    "\u{2200}x\u{2208}xs: p",
    "x\u{a0}+\u{a0}y",
    "\"\u{0}\"",
    "\"\u{7f}\"",
    "a\rb",
    "\u{feff}a",
    "x = ?",
    "x = ?x = ?y",
    "check<Positive>(n)?.is_ok()?.not",
    "parse_size(text)?.is_ok()",
    "flatten(pages) ++ rest == items ++ [] == []",
    "self.result.self",
    "true.false",
    "None.Some(x)",
    "Unit()",
    "Unit(Unit(Unit))",
    "Ok(Err(Ok(Err(x))))",
    "loop",
    "async + await",
    "as",
    "ghost lemma",
    "break continue",
];

#[test]
fn adversarial_expressions_never_panic_and_use_catalogue_codes() {
    for (i, expr) in ADVERSARIAL_EXPRESSIONS.iter().enumerate() {
        let src = wrap(expr);
        let r = parse_module(&src, FileId(0));
        for d in &r.diagnostics {
            assert_eq!(
                d.category,
                Category::Parse,
                "#{i} {expr:?}: category of {d:?}"
            );
            assert!(
                SPEC_PARSE_CODES.contains(&d.code.as_str()),
                "#{i} {expr:?}: code `{}` is not in semantics 9.4",
                d.code
            );
            assert!(
                d.span.is_some(),
                "#{i} {expr:?}: every diagnostic has a span (D59)"
            );
        }
        if let Some(m) = &r.value {
            assert!(
                r.diagnostics.is_empty(),
                "#{i} {expr:?}: a value with error diagnostics"
            );
            let dump = dump_module(m);
            assert!(!dump.is_empty());
        } else {
            assert!(
                !r.diagnostics.is_empty(),
                "#{i} {expr:?}: no value and no diagnostics"
            );
        }
        // The same text through the other two parsers.
        let _ = parse_work(&src, FileId(0));
        let _ = parse_patch(&src, FileId(0));
        let _ = parse_work(expr, FileId(0));
        let _ = parse_patch(expr, FileId(0));
        let _ = parse_module(expr, FileId(0));
    }
}

#[test]
fn every_truncation_of_dense_expressions_is_handled() {
    let dense = [
        wrap("let ok = forall p in xs: a and not b.c<D>(e)? or -f(g, using h) ++ [Page { ..p, id: ?id }] == \"\\u{41}\\n\"\nmatch ok {\n    Ok(Some(Pair(-5, \"a\"))) => check<List<List<Int>>>(x)?.len\n    reader.pages.StoreError.Missing(id) => if c { 1 } else { 2 } + 3\n    _ => 1_000\n}"),
        wrap("f(a < B, C > (d), (a < b) < c, [forall x in xs: x?])"),
        wrap("Page { inner: Inner { x: if c { Page { ..p } } else { q } } }.inner.x?"),
    ];
    let mut checked = 0;
    for src in &dense {
        let boundaries: Vec<usize> = src
            .char_indices()
            .map(|(i, _)| i)
            .chain([src.len()])
            .collect();
        for &end in &boundaries {
            let prefix = &src[..end];
            let r = parse_module(prefix, FileId(0));
            for d in &r.diagnostics {
                assert!(
                    SPEC_PARSE_CODES.contains(&d.code.as_str()),
                    "{prefix:?}: code {}",
                    d.code
                );
            }
            if let Some(m) = &r.value {
                let _ = dump_module(m);
            }
            let _ = parse_work(prefix, FileId(0));
            let _ = parse_patch(prefix, FileId(0));
            checked += 1;
        }
    }
    assert!(checked > 300);
}

#[test]
fn the_dense_expression_file_parses_cleanly() {
    // The full file above, with its whole-tree shape pinned.
    let d = body("let ok = forall p in xs: a and not b.c<D>(e)? or -f(g, using h) ++ [Page { ..p, id: ?id }] == \"\\u{41}\"\nmatch ok {\n    Ok(Some(Pair(-5, \"a\"))) => check<List<List<Int>>>(x)?.len\n    reader.pages.StoreError.Missing(id) => if c { 1 } else { 2 } + 3\n    _ => 1_000\n}");
    assert_contains(
        &d,
        "(let ok (forall p (name xs) (or (and (name a) (not (propagate (call (member (name b) c) (generics (ty D)) (name e))))) (== (++ (neg (call (name f) (name g) (using (name h)))) (list (struct-lit Page (spread (name p)) (field id (hole id))))) (text \"A\")))))",
    );
    assert_contains(
        &d,
        "(arm (ctor Ok (ctor Some (ctor Pair (pat-int -5) (pat-text \"a\")))) (member (propagate (call (name check) (generics (ty List (ty List (ty Int)))) (name x))) len))",
    );
    assert_contains(
        &d,
        "(arm (ctor reader.pages.StoreError.Missing (bind id)) (+ (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))) (int 3)))",
    );
    assert_contains(&d, "(arm _ (int 1000))");
}
