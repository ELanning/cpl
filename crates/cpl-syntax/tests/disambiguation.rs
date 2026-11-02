//! Positive and negative cases for every ruling of `spec/grammar.md`
//! section 5 ((a) to (m) and the further rulings), plus the layout rules of
//! section 2 they depend on. Tree shapes are asserted through the dump.

use cpl_diag::{DiagCode, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_syntax::{
    codes, dump_module, dump_patch, dump_work, parse_module, parse_patch, parse_work,
};

fn render(src: &str, diagnostics: &[Diagnostic]) -> String {
    let mut map = SourceMap::new();
    map.add("test.cpl".to_string(), FileOrigin::Virtual, src.to_string());
    cpl_diag::render_all(diagnostics, &map)
}

/// Collapse the dump's line breaks and indentation so shapes can be asserted
/// as single-line S-expressions.
fn flat(dump: &str) -> String {
    dump.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a module and return its flattened dump; panic with rendered
/// diagnostics on error.
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

/// Wrap statements in `module m` / `fn f() -> Int { ... }` and return the dump.
fn body(stmts: &str) -> String {
    let indented: Vec<String> = stmts.lines().map(|l| format!("    {l}")).collect();
    module(&format!(
        "module m\n\nfn f() -> Int {{\n{}\n}}\n",
        indented.join("\n")
    ))
}

fn module_errors(src: &str) -> Vec<Diagnostic> {
    let r = parse_module(src, FileId(0));
    assert!(
        r.value.is_none() && r.diagnostics.iter().any(Diagnostic::is_error),
        "expected errors for:\n{src}"
    );
    r.diagnostics
}

fn body_errors(stmts: &str) -> Vec<Diagnostic> {
    let indented: Vec<String> = stmts.lines().map(|l| format!("    {l}")).collect();
    module_errors(&format!(
        "module m\n\nfn f() -> Int {{\n{}\n}}\n",
        indented.join("\n")
    ))
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
        "expected to find\n  {needle}\nin\n{haystack}"
    );
}

// (a) struct literal `Type {` versus a block `{` -------------------------------

#[test]
fn a_struct_literals_are_disabled_only_in_header_expressions() {
    let d = module(
        "module m\n\nfn relabel(c: Bool, p: Page) -> Page {\n    if c {\n        let _ = 1\n    }\n    Page { ..p, text: \"x\" }\n}\n",
    );
    assert_contains(
        &d,
        "(tail (struct-lit Page (spread (name p)) (field text (text \"x\"))))",
    );

    let d = module(
        "module m\n\nfn pick(r: Result<Page, StoreError>, p: Page) -> Page {\n    match r {\n        Ok(q) => Page { ..q, text: \"x\" }\n        Err(_) => p\n    }\n}\n",
    );
    assert_contains(
        &d,
        "(arm (ctor Ok (bind q)) (struct-lit Page (spread (name q)) (field text (text \"x\"))))",
    );

    // `if page == Page {}` is an `if` with an empty body; `Page` is a constructor reference.
    assert_contains(
        &body("if page == Page {}\n1"),
        "(expr (if (== (name page) (name Page)) (block)))",
    );
    // The intended form is parenthesized; brackets re-enable struct literals.
    assert_contains(
        &body("if page == (Page { ..p }) {}\n1"),
        "(paren (struct-lit Page (spread (name p))))",
    );
    assert_contains(
        &body("if f(Page { ..p }) {}\n1"),
        "(if (call (name f) (struct-lit Page (spread (name p)))) (block))",
    );
    assert_contains(
        &body("if [Page { ..p }] == xs {}\n1"),
        "(list (struct-lit Page (spread (name p))))",
    );
    // while and match headers are header expressions too.
    assert_contains(
        &body("while x == Page {}\n1"),
        "(while (== (name x) (name Page)) (block))",
    );
    assert_contains(
        &body("match Page {\n    _ => 1\n}"),
        "(match (name Page) (arm _ (int 1)))",
    );
    // Contract clauses are header expressions: `Page {` would open nothing sensible, so parenthesize.
    let d = module(
        "module m\n\nfn g(p: Page) -> Page\n    ensures result == (Page { ..p })\n{\n    p\n}\n",
    );
    assert_contains(
        &d,
        "(ensures (== (name result) (paren (struct-lit Page (spread (name p))))))",
    );
    // Negative: `..` cannot begin a statement once `{` opened the `if` body.
    let e = body_errors("if page == Page { ..p } {\n}\n1");
    assert!(has_code(&e, codes::SYNTAX_ERROR));
    // A chain with an UPPER segment before the last is not a type path (D20).
    let e = body_errors("StoreError.Missing { id: x }");
    assert!(has_message(
        &e,
        "a struct literal names a type; variants have positional payloads"
    ));
    // A qualified type path is fine.
    assert_contains(
        &body("reader.pages.Page { ..p }"),
        "(struct-lit reader.pages.Page (spread (name p)))",
    );
}

// (b) `ident<Type>(` generic call versus `<` comparison ----------------------

#[test]
fn b_generic_calls_are_speculative_and_authoritative() {
    assert_contains(
        &body("check<Positive>(n)"),
        "(tail (call (name check) (generics (ty Positive)) (name n)))",
    );
    assert_contains(
        &body("std.check<Positive>(n)"),
        "(call (member (name std) check) (generics (ty Positive)) (name n))",
    );
    assert_contains(
        &body("x.convert<Int>()"),
        "(call (member (name x) convert) (generics (ty Int)))",
    );
    assert_contains(
        &body("f<List<Int>, Text>(x)"),
        "(call (name f) (generics (ty List (ty Int)) (ty Text)) (name x))",
    );
    assert_contains(&body("a < b"), "(tail (< (name a) (name b)))");
    assert_contains(&body("a < B"), "(tail (< (name a) (name B)))");
    assert_contains(
        &body("a < b and c > d"),
        "(and (< (name a) (name b)) (> (name c) (name d)))",
    );
    // One argument: the generic call, never two comparisons.
    assert_contains(
        &body("f(a < B, C > (d))"),
        "(call (name f) (call (name a) (generics (ty B) (ty C)) (name d)))",
    );
    // Parenthesized: two comparisons.
    assert_contains(
        &body("f((a < B), C > (d))"),
        "(call (name f) (paren (< (name a) (name B))) (> (name C) (paren (name d))))",
    );
    // Generic arguments after something other than a name chain are an error.
    let e = body_errors("f(x)<T>(y)");
    assert!(has_code(&e, codes::SYNTAX_ERROR));
    // Speculation failing leaves `<` to the comparison without diagnostics.
    assert_contains(&body("a < b.len"), "(< (name a) (member (name b) len))");
}

#[test]
fn b_every_lt_is_decided_on_its_own_tokens() {
    // D42 decides each `<` after a name chain by whether `generic_args "("`
    // follows it. The lookahead from `a <` runs through `B, C<D>` and finds
    // `(` where `,` or `>` must follow, so that `<` is a comparison; the `<`
    // after `C` opens the generic call `C<D>(x)`.
    assert_contains(
        &body("f(a < B, C<D>(x))"),
        "(call (name f) (< (name a) (name B)) (call (name C) (generics (ty D)) (name x)))",
    );
    assert_contains(
        &body("f(A < B, C<D, E>(x), G < H)"),
        "(call (name f) (< (name A) (name B)) (call (name C) (generics (ty D) (ty E)) (name x)) (< (name G) (name H)))",
    );
    assert_contains(
        &body("f(A < B, C < D)"),
        "(call (name f) (< (name A) (name B)) (< (name C) (name D)))",
    );
    assert_contains(
        &body("[A < B, C < D, E<F>(x)]"),
        "(list (< (name A) (name B)) (< (name C) (name D)) (call (name E) (generics (ty F)) (name x)))",
    );
    assert_contains(
        &body("f(A<B, C<D>>(x))"),
        "(call (name f) (call (name A) (generics (ty B) (ty C (ty D))) (name x)))",
    );
}

// (c) prefix `?name` versus postfix `?` ---------------------------------------

#[test]
fn c_holes_and_propagation_are_decided_by_the_lexer() {
    assert_contains(
        &body("let n = parse_size(text)?\nn"),
        "(let n (propagate (call (name parse_size) (name text))))",
    );
    assert_contains(&body("?build_pages"), "(tail (hole build_pages))");
    assert_contains(
        &body("let x = f()??\nx"),
        "(propagate (propagate (call (name f))))",
    );
    assert_contains(&body("not ?h"), "(not (hole h))");
    assert_contains(&body("?a + ?b"), "(+ (hole a) (hole b))");
    assert!(has_code(&body_errors("x?y"), codes::SYNTAX_ERROR));
    assert!(has_code(&body_errors("x? y"), codes::SYNTAX_ERROR));
    assert!(has_message(
        &body_errors("?h.len"),
        "a hole admits no postfix operator"
    ));
    assert!(has_message(
        &body_errors("?h(1)"),
        "a hole admits no postfix operator"
    ));
}

// (d) `len` as a property versus method calls --------------------------------

#[test]
fn d_member_access_has_one_form() {
    assert_contains(&body("page.len"), "(tail (member (name page) len))");
    assert_contains(
        &body("page.is_empty()"),
        "(tail (call (member (name page) is_empty)))",
    );
    assert_contains(
        &body("rest.take(size)"),
        "(call (member (name rest) take) (name size))",
    );
    assert_contains(
        &body("result.drop_last()"),
        "(call (member (name result) drop_last))",
    );
    assert_contains(
        &body("pages.push(rest.take(size))"),
        "(call (member (name pages) push) (call (member (name rest) take) (name size)))",
    );
    // `page.len()` parses; the typechecker rejects it (D43).
    assert_contains(&body("page.len()"), "(call (member (name page) len))");
    assert_contains(
        &body("a.b.c.d"),
        "(member (member (member (name a) b) c) d)",
    );
}

// (e) enum variant resolution -------------------------------------------------

#[test]
fn e_constructors_parse_uniformly_before_resolution() {
    let d = module(
        "module m\n\nfn label(result: Result<Page, StoreError>) -> Text {\n    match result {\n        Ok(page) => page.text\n        Err(Missing(_)) => \"Missing page\"\n        Err(Unavailable) => \"Storage unavailable\"\n        Err(e) => e\n        reader.pages.StoreError.Missing(id) => id\n        StoreError.Unavailable => \"u\"\n        PageId(t) => t\n    }\n}\n",
    );
    assert_contains(&d, "(arm (ctor Ok (bind page)) (member (name page) text))");
    assert_contains(
        &d,
        "(arm (ctor Err (ctor Missing _)) (text \"Missing page\"))",
    );
    assert_contains(
        &d,
        "(arm (ctor Err (ctor Unavailable)) (text \"Storage unavailable\"))",
    );
    assert_contains(&d, "(arm (ctor Err (bind e)) (name e))");
    assert_contains(
        &d,
        "(arm (ctor reader.pages.StoreError.Missing (bind id)) (name id))",
    );
    assert_contains(&d, "(arm (ctor StoreError.Unavailable) (text \"u\"))");
    assert_contains(&d, "(arm (ctor PageId (bind t)) (name t))");
    assert_contains(
        &body("StoreError.Missing(id)"),
        "(call (member (name StoreError) Missing) (name id))",
    );
    assert_contains(
        &body("PageId(\"page-1\")"),
        "(call (name PageId) (text \"page-1\"))",
    );
    assert_contains(&body("Unit"), "(tail (name Unit))");
    assert_contains(&body("None"), "(tail (name None))");
    // A constructor path ends in a PascalCase name.
    assert!(has_code(
        &body_errors("match x {\n    Foo.bar => 1\n}"),
        codes::SYNTAX_ERROR
    ));
}

// (f) extent of the `forall` predicate ----------------------------------------

#[test]
fn f_forall_extends_to_the_end_and_is_never_an_operand() {
    let d = module(
        "module m\n\nfn f(result: List<Int>, size: Int) -> Int\n    ensures forall page in result: page.len > 0 and page.len <= size\n    ensures forall a in xs: forall b in ys: a == b\n    ensures a and (forall x in c: x > 0)\n    ensures forall x in xs.drop(1): x or y\n{\n    1\n}\n",
    );
    assert_contains(
        &d,
        "(ensures (forall page (name result) (and (> (member (name page) len) (int 0)) (<= (member (name page) len) (name size)))))",
    );
    assert_contains(
        &d,
        "(forall a (name xs) (forall b (name ys) (== (name a) (name b))))",
    );
    assert_contains(
        &d,
        "(and (name a) (paren (forall x (name c) (> (name x) (int 0)))))",
    );
    assert_contains(
        &d,
        "(forall x (call (member (name xs) drop) (int 1)) (or (name x) (name y)))",
    );
    let e = module_errors(
        "module m\n\nfn f() -> Bool\n    ensures a and forall x in c: p\n{\n    true\n}\n",
    );
    assert!(has_code(&e, codes::SYNTAX_ERROR));
    assert!(has_code(
        &body_errors("1 + forall x in c: p"),
        codes::SYNTAX_ERROR
    ));
}

// (g) `if` as an expression versus a statement --------------------------------

#[test]
fn g_if_is_always_an_expression() {
    assert_contains(
        &body("let label = if page.len > size { \"long\" } else { \"short\" }\nlabel"),
        "(let label (if (> (member (name page) len) (name size)) (block (tail (text \"long\"))) (else (block (tail (text \"short\"))))))",
    );
    assert_contains(
        &body("1 + if c { 1 } else { 2 }"),
        "(+ (int 1) (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    assert_contains(
        &body("if c {\n    x = 1\n}\n1"),
        "(expr (if (name c) (block (assign x (int 1)))))",
    );
    // The header's `{` may start the next line (rule-3 lookahead).
    assert_contains(
        &body("let x = if c\n{ 1 } else { 2 }\nx"),
        "(let x (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    assert_contains(
        &body("if c\n{\n    x = 1\n}\n1"),
        "(expr (if (name c) (block (assign x (int 1)))))",
    );
    // Inside an insignificant-newline context `else` on the next line is accepted (2.4 rule 4).
    assert_contains(
        &body("foo(if c { 1 }\nelse { 2 })"),
        "(call (name foo) (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    // D12: where newlines are significant, `else` must follow `}` on the same line.
    assert!(has_code(
        &body_errors("if c { 1 }\nelse { 2 }"),
        codes::MISPLACED_ELSE
    ));
    assert!(has_code(
        &body_errors("let x = if c { 1 }\nelse { 2 }\nx"),
        codes::MISPLACED_ELSE
    ));
    // `else` then `{` on the next line is `NEWLINE? block`.
    assert_contains(
        &body("if c { 1 } else\n{ 2 }"),
        "(else (block (tail (int 2))))",
    );
}

// (h) empty list `[]` ----------------------------------------------------------

#[test]
fn h_empty_list_literal_parses() {
    assert_contains(
        &body("let items: List<Int> = []\nitems.len"),
        "(let items (ty List (ty Int)) (list))",
    );
    assert_contains(
        &body("assert paginate(items, 2) == []\n1"),
        "(assert (== (call (name paginate) (name items) (int 2)) (list)))",
    );
    assert_contains(
        &body("[[1, 2], [3]]"),
        "(list (list (int 1) (int 2)) (list (int 3)))",
    );
    assert_contains(&body("[1, 2,]"), "(list (int 1) (int 2))");
}

// (i) `return` with or without a value ----------------------------------------

#[test]
fn i_return_value_is_optional() {
    let d = module("module m\n\nfn f(c: Bool) -> Unit {\n    if c {\n        return\n    }\n    return Unit\n}\n\nfn g() -> Unit { return }\n");
    assert_contains(&d, "(if (name c) (block (return)))");
    assert_contains(&d, "(return (name Unit))");
    assert_contains(&d, "(fn g (params) (ret (ty Unit)) (block (return))");
    // D26: `return` is a statement, so inside an arm it needs a block.
    assert_contains(
        &body("match r {\n    Err(e) => { return Err(e) }\n    Ok(v) => v\n}"),
        "(arm (ctor Err (bind e)) (block (return (call (name Err) (name e)))))",
    );
    assert!(has_code(
        &body_errors("let x = return 1\nx"),
        codes::SYNTAX_ERROR
    ));
}

// (j) negative literals and unary minus ---------------------------------------

#[test]
fn j_negative_numbers_are_unary_minus_except_in_patterns() {
    assert_contains(&body("-5"), "(tail (neg (int 5)))");
    assert_contains(&body("-x.len"), "(neg (member (name x) len))");
    assert_contains(&body("2 * -3"), "(* (int 2) (neg (int 3)))");
    assert_contains(&body("a -1"), "(- (name a) (int 1))");
    assert_contains(&body("- 5"), "(neg (int 5))");
    assert_contains(&body("- -a"), "(neg (neg (name a)))");
    assert_contains(&body("not not a"), "(not (not (name a)))");
    // `not` binds tighter than comparison.
    assert_contains(&body("not a == b"), "(== (not (name a)) (name b))");
    assert_contains(
        &body("not (a == b)"),
        "(not (paren (== (name a) (name b))))",
    );
    assert_contains(
        &body("not rest.is_empty()"),
        "(not (call (member (name rest) is_empty)))",
    );
    let d =
        body("match n {\n    -5 => 1\n    5 => 2\n    \"a\" => 3\n    true => 4\n    _ => 5\n}");
    assert_contains(&d, "(arm (pat-int -5) (int 1))");
    assert_contains(&d, "(arm (pat-int 5) (int 2))");
    assert_contains(&d, "(arm (pat-text \"a\") (int 3))");
    assert_contains(&d, "(arm (pat-bool true) (int 4))");
    assert_contains(&d, "(arm _ (int 5))");
    assert!(has_code(
        &body_errors("match n {\n    -x => 1\n}"),
        codes::SYNTAX_ERROR
    ));
}

// (k) `else if` chains ----------------------------------------------------------

#[test]
fn k_else_if_chains() {
    assert_contains(
        &body("if a { 1 } else if b { 2 } else { 3 }"),
        "(if (name a) (block (tail (int 1))) (else (if (name b) (block (tail (int 2))) (else (block (tail (int 3)))))))",
    );
    assert_contains(
        &body("if a { 1 } else if b { 2 }"),
        "(else (if (name b) (block (tail (int 2)))))",
    );
    assert!(has_code(
        &body_errors("if a { 1 } elif b { 2 }"),
        codes::SYNTAX_ERROR
    ));
}

// (l) `result` and `self` -------------------------------------------------------

#[test]
fn l_result_and_self_are_ordinary_identifiers() {
    let d = module(
        "module m\n\npub type Positive = Int where self > 0\n\nfn label(result: Result<Page, StoreError>) -> Text {\n    result.text\n}\n\nfn g(x: Int) -> Int\n    ensures result == x\n{\n    let self = x\n    self\n}\n",
    );
    assert_contains(
        &d,
        "(type Positive pub (refinement (ty Int) (where (> (name self) (int 0))))",
    );
    assert_contains(&d, "(param result (ty Result (ty Page) (ty StoreError)))");
    assert_contains(&d, "(ensures (== (name result) (name x)))");
    assert_contains(&d, "(let self (name x))");
}

// (m) empty blocks ---------------------------------------------------------------

#[test]
fn m_empty_blocks() {
    let d = module("module m\n\nfn f() -> Unit {}\n\nfn g() -> Unit {\n}\n\nfn h(c: Bool) -> Unit {\n    if c {}\n    while c {}\n}\n");
    assert_contains(&d, "(fn f (params) (ret (ty Unit)) (block)");
    assert_contains(&d, "(fn g (params) (ret (ty Unit)) (block)");
    assert_contains(&d, "(expr (if (name c) (block)))");
    assert_contains(&d, "(while (name c) (block))");
    let d = module("module m\n\nstruct Empty {}\n\nenum Never {\n}\n\ncapability Nothing {}\n");
    assert_contains(&d, "(struct Empty");
    assert_contains(&d, "(enum Never");
    assert_contains(&d, "(capability Nothing");
}

// Further rulings ---------------------------------------------------------------

#[test]
fn d52_wildcard_versus_underscore_identifiers() {
    assert_contains(&body("let _ = f()\n1"), "(let _ (call (name f)))");
    assert_contains(&body("let _x = 1\n_x"), "(let _x (int 1))");
    assert!(has_code(&body_errors("var _ = 1\n1"), codes::SYNTAX_ERROR));
    assert!(has_code(&body_errors("_ = 1\n1"), codes::SYNTAX_ERROR));
}

#[test]
fn d53_member_names_are_never_keywords() {
    let e = body_errors("x.type");
    assert!(has_code(&e, codes::SYNTAX_ERROR) && has_message(&e, "expected a member name"));
    assert!(has_code(&body_errors("x.if"), codes::SYNTAX_ERROR));
    assert_contains(&body("x.Type"), "(member (name x) Type)");
    // The one exception is the `effects` facet in selectors.
    let w = parse_work(
        "use a.b.{f}\n\nchange c {\n    base git(\"main\")\n    target git(\"main\")\n    task t {\n        preserve { f.effects, f.body, f }\n        goal \"g\"\n    }\n}\n",
        FileId(0),
    );
    assert!(w.diagnostics.is_empty(), "{:#?}", w.diagnostics);
    assert_contains(
        &flat(&dump_work(&w.value.unwrap())),
        "(preserve f.effects f.body f)",
    );
}

#[test]
fn d54_dotdot_is_only_the_spread() {
    assert!(has_code(&body_errors("a..b"), codes::SYNTAX_ERROR));
    assert!(has_code(
        &body_errors("match x {\n    1..5 => 1\n}"),
        codes::SYNTAX_ERROR
    ));
    // D29: the spread is first and unique.
    assert!(has_message(
        &body_errors("Page { text: t, ..p }"),
        "must be the first entry"
    ));
    assert!(has_code(
        &body_errors("Page { ..p, ..q }"),
        codes::SYNTAX_ERROR
    ));
    assert_contains(
        &body("Page { ..p, }"),
        "(struct-lit Page (spread (name p)))",
    );
    assert_contains(
        &body("Page { text: t, id: i, }"),
        "(struct-lit Page (field text (name t)) (field id (name i)))",
    );
    // D30: no punning, no type arguments.
    assert!(has_code(
        &body_errors("Page { id, text }"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &body_errors("Pair<Int, Text> { a: 1 }"),
        codes::SYNTAX_ERROR
    ));
}

#[test]
fn d55_match_arms_are_separated_by_newlines() {
    let e = body_errors("match x {\n    A => 1,\n    B => 2\n}");
    assert!(
        has_code(&e, codes::SYNTAX_ERROR)
            && has_message(&e, "match arms are separated by newlines")
    );
    assert!(has_code(
        &body_errors("match x { A => 1 B => 2 }"),
        codes::SYNTAX_ERROR
    ));
    assert_contains(
        &body("match x\n{\n    A => 1\n}"),
        "(match (name x) (arm (ctor A) (int 1)))",
    );
}

#[test]
fn d56_test_is_reserved_and_tests_is_not() {
    module("module tests.pagination\n");
    assert!(has_code(
        &module_errors("module test.x\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\nfn test() -> Unit {}\n"),
        codes::SYNTAX_ERROR
    ));
    assert_contains(&body("tests.run()"), "(call (member (name tests) run))");
}

#[test]
fn d4_reserved_future_words_are_errors() {
    for src in ["let loop = 1\n1", "let x = as\nx", "ghost"] {
        assert!(has_code(&body_errors(src), codes::RESERVED_WORD), "{src}");
    }
    assert!(has_code(
        &module_errors("module m\n\nfn async() -> Unit {}\n"),
        codes::RESERVED_WORD
    ));
}

#[test]
fn d11_no_operator_or_leading_dot_continuation() {
    let e = body_errors("let total = a +\n    b");
    assert!(has_message(&e, "expected expression, found end of line"));
    assert!(has_code(
        &body_errors("let text = page\n    .text"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_message(
        &body_errors("let x =\n    compute()"),
        "expected expression, found end of line"
    ));
    // Continuations inside brackets.
    assert_contains(
        &body("let total = (a\n    + b)\ntotal"),
        "(let total (paren (+ (name a) (name b))))",
    );
    assert_contains(
        &body("let pages = paginate(\n    items,\n    size,\n)\npages"),
        "(let pages (call (name paginate) (name items) (name size)))",
    );
    assert_contains(
        &body("let labels = [\n    \"a\",\n    \"b\",\n]\nlabels"),
        "(let labels (list (text \"a\") (text \"b\")))",
    );
    assert_contains(
        &body("let page = Page {\n    ..base,\n    text: trim(base.text),\n}\npage"),
        "(let page (struct-lit Page (spread (name base)) (field text (call (name trim) (member (name base) text)))))",
    );
    // A block inside parentheses has significant newlines again.
    assert_contains(
        &body("foo(if c {\n    x = 1\n    2\n} else { 3 })"),
        "(block (assign x (int 1)) (tail (int 2)))",
    );
}

#[test]
fn d63_and_d66_headers_clauses_and_bodies() {
    // `{` may start the next line after any header when there are no clauses.
    let d = module("module m\n\nfn f() -> Int\n{\n    1\n}\n\nstruct S\n{\n    x: Int\n}\n\ntest \"t\"\n{\n    assert true\n}\n");
    assert_contains(&d, "(fn f (params) (ret (ty Int)) (block (tail (int 1)))");
    assert_contains(&d, "(struct S (field x (ty Int))");
    // Clauses in any order are accepted and kept in source order.
    let d = module("module m\n\nfn f(a: Int, using db: Db) -> Int\n    ensures result > 0\n    decreases a\n    effects { db.get }\n    requires a > 0\n{\n    a\n}\n");
    assert_contains(&d, "(ensures (> (name result) (int 0))) (decreases (name a)) (effects db.get) (requires (> (name a) (int 0)))");
    // With clauses the `{` must start a new line.
    assert!(has_code(
        &module_errors("module m\n\nfn f(x: Int) -> Int\n    requires x > 0 {\n    1\n}\n"),
        codes::SYNTAX_ERROR
    ));
    // A clause on the header line is an error.
    assert!(has_message(
        &module_errors("module m\n\nfn f(x: Int) -> Int requires x > 0\n{\n    1\n}\n"),
        "a clause must start its own line"
    ));
    // Blank lines and comments between header and clauses are fine.
    let d = module("module m\n\nfn f(x: Int) -> Int\n\n    // precondition\n    requires x > 0\n\n{\n    x\n}\n");
    assert_contains(
        &d,
        "(requires (> (name x) (int 0)) (trivia (lead-blank \"// precondition\")))",
    );
    // Capability operations carry contracts but no body, effects, or using parameters.
    let d = module("module m\n\npub capability Db {\n    fn get(id: Id) -> Result<Page, E>\n        requires id != x\n        ensures result != y\n    fn put(page: Page) -> Result<Unit, E>\n}\n");
    assert_contains(&d, "(op get (params (param id (ty Id))) (ret (ty Result (ty Page) (ty E))) (requires (!= (name id) (name x))) (ensures (!= (name result) (name y))))");
    assert_contains(
        &d,
        "(op put (params (param page (ty Page))) (ret (ty Result (ty Unit) (ty E))))",
    );
    assert!(has_code(
        &module_errors("module m\n\ncapability Db {\n    fn get(using x: Id) -> Int\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors(
            "module m\n\ncapability Db {\n    fn get(x: Id) -> Int\n        effects { x.y }\n}\n"
        ),
        codes::SYNTAX_ERROR
    ));
    // Property: for clauses then `{` on its own line.
    let d = module("module m\n\nproperty \"p\"\n    for items in gen.list(gen.int)\n    for size in gen.positive_int\n{\n    assert true\n}\n");
    assert_contains(&d, "(property \"p\" (for items (call (member (name gen) list) (member (name gen) int))) (for size (member (name gen) positive_int)) (block (assert (bool true)))");
    assert!(has_code(
        &module_errors("module m\n\nproperty \"p\" {\n    assert true\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\nproperty \"p\" for x in xs\n{\n    assert true\n}\n"),
        codes::SYNTAX_ERROR
    ));
}

#[test]
fn trailing_commas_where_the_grammar_allows_them() {
    let d = module(
        "module m\n\nuse a.b.{c,}\n\n@id(\"x\",)\nfn f(\n    a: Int,\n    using db: Db,\n) -> Int\n    effects { db.get, }\n{\n    g(a, using db,)\n}\n",
    );
    assert_contains(&d, "(use a.b (items c)");
    assert_contains(&d, "(annotation id (text \"x\"))");
    assert_contains(&d, "(params (param a (ty Int)) (using db (ty Db)))");
    assert_contains(&d, "(effects db.get)");
    assert_contains(&d, "(call (name g) (name a) (using (name db)))");
    assert!(has_code(
        &module_errors("module m\n\nfn f<T,>() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\nfn f() -> List<Int,> { x }\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &body_errors("let x: List<Int,> = []\nx"),
        codes::SYNTAX_ERROR
    ));
}

#[test]
fn using_arguments_and_parameters() {
    assert_contains(
        &body("load_page(id, using db)"),
        "(call (name load_page) (name id) (using (name db)))",
    );
    assert_contains(
        &body("f(using ctx.reader)"),
        "(call (name f) (using (member (name ctx) reader)))",
    );
    assert!(has_code(
        &body_errors("f(using db, id)"),
        codes::POSITIONAL_AFTER_USING
    ));
    assert!(has_code(
        &module_errors("module m\n\nfn f(using db: Db, id: Id) -> Int { 1 }\n"),
        codes::POSITIONAL_AFTER_USING
    ));
    let d = module("module m\n\nfn f(using db: Db) -> Int { 1 }\n");
    assert_contains(&d, "(params (using db (ty Db)))");
}

#[test]
fn use_declarations_forms() {
    let d = module("module m\n\nuse std.text.{trim, parse_int}\nuse std.text.trim\nuse reader.pages.Positive\n\nfn f() -> Int { 1 }\n");
    assert_contains(&d, "(use std.text (items trim parse_int)");
    assert_contains(&d, "(use std.text trim)");
    assert_contains(&d, "(use reader.pages Positive)");
    assert!(has_code(
        &module_errors("module m\n\nuse trim\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\nuse a.b.{}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\nuse a.*\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\nuse a.b.c as d\n"),
        codes::RESERVED_WORD
    ));
    assert!(has_code(
        &module_errors("module m\n\nfn f() -> Int { 1 }\n\nuse a.b\n"),
        codes::USE_AFTER_DECLARATION
    ));
}

#[test]
fn type_declarations_and_generics() {
    let d = module("module m\n\npub type PageId(Text)\npub type Positive = Int where self > 0\nstruct Pair<A, B> {\n    first: A\n    second: B\n}\nenum Opt<T> {\n    Some(T)\n    None\n}\n");
    assert_contains(&d, "(type PageId pub (wrapper (ty Text))");
    assert_contains(
        &d,
        "(struct Pair (generics A B) (field first (ty A)) (field second (ty B)))",
    );
    assert_contains(
        &d,
        "(enum Opt (generics T) (variant Some (ty T)) (variant None))",
    );
    assert!(has_message(
        &module_errors("module m\n\npub type A = T\n"),
        "a type declaration is a wrapper"
    ));
    assert!(has_code(
        &module_errors("module m\n\ntype Pair(Int, Int)\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_message(
        &module_errors("module m\n\nenum E {\n    A()\n}\n"),
        "omits the parentheses"
    ));
    assert!(has_code(
        &module_errors("module m\n\nenum E {\n    A(Int), B\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\nstruct S {\n    pub x: Int\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\npub test \"t\" {}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\ntype a(Int)\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &module_errors("module m\n\nfn F() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR
    ));
}

#[test]
fn work_file_rulings() {
    let src = "use reader.pages.{paginate, Positive}\n\nchange pagination_update {\n    workers 3\n    integrate {\n        require typecheck\n        require prove(paginate)\n        require test(tests.pagination.*)\n        require review(audit)\n        require no_holes\n        require no_unresolved_assumptions\n    }\n    base git(\"main\")\n    task audit after implementation, examples {\n        write {}\n        goal \"Review.\"\n    }\n    target git(\"v1.0\")\n    task implementation {\n        add { tests.pagination.* }\n        depend { paginate.contract, Positive.definition, Positive }\n        goal \"g\"\n        write { paginate.body }\n    }\n}\n\nchange second {\n    base git(\"a\")\n    target git(\"b\")\n}\n";
    let r = parse_work(src, FileId(0));
    assert!(r.diagnostics.is_empty(), "{}", render(src, &r.diagnostics));
    let d = flat(&dump_work(&r.value.unwrap()));
    assert_contains(&d, "(use reader.pages (items paginate Positive))");
    assert_contains(&d, "(workers 3)");
    assert_contains(&d, "(require (prove paginate))");
    assert_contains(&d, "(require (test tests.pagination.*))");
    assert_contains(&d, "(require (review audit))");
    assert_contains(&d, "(require no_unresolved_assumptions)");
    assert_contains(
        &d,
        "(task audit (after implementation examples) (write) (goal \"Review.\"))",
    );
    assert_contains(
        &d,
        "(depend paginate.contract Positive.definition Positive)",
    );
    assert_contains(&d, "(change second (base \"a\") (target \"b\")");

    let errs = |src: &str| {
        let r = parse_work(src, FileId(0));
        assert!(r.value.is_none(), "expected errors for {src}");
        r.diagnostics
    };
    let e = errs("change c {\n    base git(\"a\")\n    base git(\"b\")\n}\n");
    assert!(has_code(&e, codes::DUPLICATE_WORK_ITEM) && has_message(&e, "duplicate base"));
    let e = errs("change c {\n    task t {\n        write { a }\n        write { b }\n    }\n}\n");
    assert!(has_message(&e, "duplicate write set"));
    let e = errs("change c {\n    task t {\n        goal \"a\"\n    }\n    task t {\n        goal \"b\"\n    }\n}\n");
    assert!(has_message(&e, "duplicate task name"));
    let e = errs("change c {\n    integrate {\n        require lint\n    }\n}\n");
    assert!(has_code(&e, codes::UNKNOWN_REQUIREMENT));
    let e = errs("change c {\n    task t {\n        add { paginate.body }\n    }\n}\n");
    assert!(has_message(&e, "add names a namespace; write paginate.*"));
    assert!(has_code(
        &errs("change c {\n    task t after a, {\n    }\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &errs("change c {\n    base main\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &errs("change c {\n    task t {\n        write { a.*.b }\n    }\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &errs("change c {\n    task t {\n        write { paginate.effects.body }\n    }\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(
        &errs("change c {\n    base git(\"a\")\n}\nuse a.b\n"),
        codes::USE_AFTER_DECLARATION
    ));
    assert!(has_code(
        &errs("fn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(&errs(""), codes::SYNTAX_ERROR));
    // `change` is only contextual: a function named `change` is fine in .cpl files.
    assert_contains(&body("let change = 1\nchange"), "(let change (int 1))");
}

#[test]
fn patch_file_rulings() {
    let src = "patch paginate.body\n    expect base(paginate.body)\n{\n    var pages: List<List<T>> = []\n    pages\n}\n\npatch reader.pages.other.body\n    expect base(reader.pages.other.body)\n{\n}\n";
    let r = parse_patch(src, FileId(0));
    assert!(r.diagnostics.is_empty(), "{}", render(src, &r.diagnostics));
    let d = flat(&dump_patch(&r.value.unwrap()));
    assert_contains(&d, "(patch paginate.body (expect paginate.body) (block (var pages (ty List (ty List (ty T))) (list)) (tail (name pages))))");
    assert_contains(
        &d,
        "(patch reader.pages.other.body (expect reader.pages.other.body) (block)",
    );
    let errs = |src: &str| {
        let r = parse_patch(src, FileId(0));
        assert!(r.value.is_none(), "expected errors for {src}");
        r.diagnostics
    };
    assert!(has_message(
        &errs("patch x expect base(x) {\n}\n"),
        "`expect` must start its own line"
    ));
    assert!(has_message(
        &errs("patch x\n    expect base(x) {\n}\n"),
        "must start a new line"
    ));
    assert!(has_code(&errs("patch x\n{\n}\n"), codes::SYNTAX_ERROR));
    assert!(has_code(
        &errs("patch x\n    expect head(x)\n{\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has_code(&errs("module m\n"), codes::SYNTAX_ERROR));
    assert!(has_code(&errs(""), codes::SYNTAX_ERROR));
    // Inside the block, `patch`, `expect`, and `base` are ordinary identifiers.
    let r = parse_patch(
        "patch f.body\n    expect base(f.body)\n{\n    let patch = expect + base\n    patch\n}\n",
        FileId(0),
    );
    assert!(r.diagnostics.is_empty(), "{:#?}", r.diagnostics);
}
