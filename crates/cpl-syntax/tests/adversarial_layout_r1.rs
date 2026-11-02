//! Adversarial tests, round 1, lens: newlines and layout.
//!
//! Every expectation here encodes `spec/grammar.md` (edition 0.1), cited by
//! section or decision number in the test's comment: 1.8 step 2 and 2.1
//! (NEWLINE emission), 2.2 (parser newline modes), 2.3 (statement
//! termination, D11), 2.4 (header continuation lines, D12, D63), 2.5
//! (trailing commas), 3.11/D51 (empty blocks and tail expressions), 3.12
//! (`if`/`else`/`match` layout, D49, D55), 4.1-4.5 (`.work` and `.patch`
//! layout), 1.1 (CRLF, tabs, optional final terminator), and 1.2 (comments
//! and doc comments at the end of a file). Where grammar.md is silent the
//! comment says so and names the design-doc example the test follows.
//!
//! Diagnostic codes come from `spec/semantics.md` 9.4 through
//! `cpl_syntax::codes`. Tests never assert the parser's current behaviour;
//! a test that fails marks a parser defect (or a spec issue reported with
//! the round's results).

use cpl_diag::{DiagCode, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_syntax::ast::*;
use cpl_syntax::{
    codes, dump_module, dump_patch, dump_work, lex, parse_module, parse_patch, parse_work,
    TokenKind as T,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn render(src: &str, diagnostics: &[Diagnostic]) -> String {
    let mut map = SourceMap::new();
    map.add("test.cpl".to_string(), FileOrigin::Virtual, src.to_string());
    cpl_diag::render_all(diagnostics, &map)
}

/// Collapse the dump so shapes can be asserted as one-line S-expressions.
fn flat(dump: &str) -> String {
    dump.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a `.cpl` module that must produce zero diagnostics; return the AST.
fn module_ast(src: &str) -> Module {
    let r = parse_module(src, FileId(0));
    match r.value {
        Some(m) if r.diagnostics.is_empty() => m,
        _ => panic!(
            "expected a clean parse of:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

/// Parse a `.cpl` module that must produce zero diagnostics; return its flat dump.
fn module(src: &str) -> String {
    flat(&dump_module(&module_ast(src)))
}

/// Wrap statements in `module m` / `fn f() -> Int { ... }` and return the dump.
fn body(stmts: &str) -> String {
    module(&wrap(stmts))
}

fn wrap(stmts: &str) -> String {
    let indented: Vec<String> = stmts.lines().map(|l| format!("    {l}")).collect();
    format!(
        "module m\n\nfn f() -> Int {{\n{}\n}}\n",
        indented.join("\n")
    )
}

fn module_errors(src: &str) -> Vec<Diagnostic> {
    let r = parse_module(src, FileId(0));
    assert!(
        r.value.is_none() && r.diagnostics.iter().any(Diagnostic::is_error),
        "expected errors for:\n{src}"
    );
    check_codes(&r.diagnostics);
    r.diagnostics
}

fn body_errors(stmts: &str) -> Vec<Diagnostic> {
    module_errors(&wrap(stmts))
}

fn work(src: &str) -> String {
    let r = parse_work(src, FileId(0));
    match r.value {
        Some(w) if r.diagnostics.is_empty() => flat(&dump_work(&w)),
        _ => panic!(
            "expected a clean .work parse of:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn work_errors(src: &str) -> Vec<Diagnostic> {
    let r = parse_work(src, FileId(0));
    assert!(
        r.value.is_none() && r.diagnostics.iter().any(Diagnostic::is_error),
        "expected .work errors for:\n{src}"
    );
    check_codes(&r.diagnostics);
    r.diagnostics
}

fn patch(src: &str) -> String {
    let r = parse_patch(src, FileId(0));
    match r.value {
        Some(p) if r.diagnostics.is_empty() => flat(&dump_patch(&p)),
        _ => panic!(
            "expected a clean .patch parse of:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn patch_errors(src: &str) -> Vec<Diagnostic> {
    let r = parse_patch(src, FileId(0));
    assert!(
        r.value.is_none() && r.diagnostics.iter().any(Diagnostic::is_error),
        "expected .patch errors for:\n{src}"
    );
    check_codes(&r.diagnostics);
    r.diagnostics
}

fn check_codes(diags: &[Diagnostic]) {
    for d in diags {
        assert_eq!(d.category, codes::CATEGORY, "{d:#?}");
        assert!(
            codes::ALL.contains(&d.code),
            "unknown diagnostic code {}",
            d.code
        );
    }
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

#[track_caller]
fn assert_code(diags: &[Diagnostic], code: DiagCode) {
    assert!(
        has_code(diags, code),
        "expected a `{code}` diagnostic, got {:?}",
        diags
            .iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

fn kinds(src: &str) -> Vec<T> {
    let (tokens, diagnostics) = lex(src, FileId(0));
    assert!(
        diagnostics.is_empty(),
        "lexical errors for {src:?}: {diagnostics:#?}"
    );
    tokens.into_iter().map(|t| t.kind).collect()
}

/// Run every parser and the lexer over `src`; the only requirement is no
/// panic, known codes, and `value` present exactly when no error was reported.
fn no_panic(src: &str) {
    let m = parse_module(src, FileId(0));
    let w = parse_work(src, FileId(0));
    let p = parse_patch(src, FileId(0));
    check_codes(&m.diagnostics);
    check_codes(&w.diagnostics);
    check_codes(&p.diagnostics);
    assert_eq!(m.value.is_some(), !m.has_errors(), "module: {src:?}");
    assert_eq!(w.value.is_some(), !w.has_errors(), "work: {src:?}");
    assert_eq!(p.value.is_some(), !p.has_errors(), "patch: {src:?}");
    let (tokens, _) = lex(src, FileId(0));
    assert_eq!(tokens.last().map(|t| t.kind), Some(T::Eof), "{src:?}");
}

/// The same module with CRLF terminators must give the same dump (1.1: CRLF
/// is one terminator, and only the line terminator kind changes).
#[track_caller]
fn assert_crlf_invariant(src: &str) {
    let lf = module(src);
    let crlf = module(&src.replace('\n', "\r\n"));
    assert_eq!(lf, crlf, "CRLF changed the parse of:\n{src}");
}

// ---------------------------------------------------------------------------
// 1.8 step 2 / 2.1: NEWLINE emission
// ---------------------------------------------------------------------------

#[test]
fn newline_collapses_comment_only_and_blank_lines_into_one_token() {
    // 2.1: one NEWLINE per run of terminators, comment-only lines and blank lines.
    assert_eq!(
        kinds("a\n\n// c\n\n   \n\t\n// d\nb"),
        vec![T::LowerIdent, T::Newline, T::LowerIdent, T::Eof]
    );
    // A trailing comment on the same line as a token: the terminator still emits.
    assert_eq!(
        kinds("a // c\nb"),
        vec![T::LowerIdent, T::Newline, T::LowerIdent, T::Eof]
    );
}

#[test]
fn newline_is_not_emitted_before_the_first_token() {
    // 1.8 step 2: "no token has been emitted yet" means no NEWLINE for leading lines.
    assert_eq!(kinds("\n\n// c\n\n\t\nmodule"), vec![T::KwModule, T::Eof]);
    assert_eq!(kinds("\r\n\r\nmodule"), vec![T::KwModule, T::Eof]);
    // A module that starts after blank and comment lines parses cleanly (3.1).
    let d = module("\n\n// licence\n\n   \nmodule m\n");
    assert_contains(&d, "(module m");
}

#[test]
fn newline_at_end_of_file_is_a_real_token_and_eof_is_not_synthetic() {
    // 1.8 step 9: no synthetic NEWLINE at end of input.
    assert_eq!(kinds("a"), vec![T::LowerIdent, T::Eof]);
    assert_eq!(kinds("a\n"), vec![T::LowerIdent, T::Newline, T::Eof]);
    assert_eq!(kinds("a\n\n\n"), vec![T::LowerIdent, T::Newline, T::Eof]);
    // A comment after the last terminator adds no token.
    assert_eq!(kinds("a\n// c"), vec![T::LowerIdent, T::Newline, T::Eof]);
    assert_eq!(kinds("a\n// c\n"), vec![T::LowerIdent, T::Newline, T::Eof]);
    // A comment after the last token on its own line, no terminator anywhere.
    assert_eq!(kinds("a // c"), vec![T::LowerIdent, T::Eof]);
}

#[test]
fn newline_is_not_emitted_after_a_doc_comment_even_with_crlf_and_blank_lines() {
    // 2.1: none after DOC_COMMENT, nor for blank lines directly after it.
    assert_eq!(
        kinds("/// d\r\n\r\n\r\nfn"),
        vec![T::DocComment, T::KwFn, T::Eof]
    );
    assert_eq!(
        kinds("/// d\n// plain\n\nfn"),
        vec![T::DocComment, T::KwFn, T::Eof]
    );
    // Two doc comments: one token each, no NEWLINE between.
    assert_eq!(
        kinds("/// a\n/// b\nfn"),
        vec![T::DocComment, T::DocComment, T::KwFn, T::Eof]
    );
}

#[test]
fn tabs_are_horizontal_whitespace_only() {
    // 1.1: tab is horizontal whitespace; indentation is never significant.
    assert_eq!(kinds("a\t\tb"), vec![T::LowerIdent, T::LowerIdent, T::Eof]);
    assert_eq!(
        kinds("\t\ta\n\t\tb\t\n"),
        vec![T::LowerIdent, T::Newline, T::LowerIdent, T::Newline, T::Eof]
    );
    // A tab-only line is a blank line.
    assert_eq!(
        kinds("a\n\t\t\nb"),
        vec![T::LowerIdent, T::Newline, T::LowerIdent, T::Eof]
    );
}

#[test]
fn crlf_is_one_terminator_and_lone_cr_is_a_lexical_error() {
    // 1.1: CRLF is one terminator; CR alone is a lexical error.
    assert_eq!(
        kinds("a\r\n\r\nb\r\n"),
        vec![T::LowerIdent, T::Newline, T::LowerIdent, T::Newline, T::Eof]
    );
    let (_, d) = lex("a\rb", FileId(0));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, codes::INVALID_LINE_TERMINATOR);
    // A lone CR inside a module is rejected by the parser too.
    let e = module_errors("module m\rfn f() -> Int { 1 }\n");
    assert_code(&e, codes::INVALID_LINE_TERMINATOR);
    // CR before LF never splits a token: `-\r\n>` is not an arrow.
    assert_eq!(kinds("-\r\n>"), vec![T::Minus, T::Newline, T::Gt, T::Eof]);
}

// ---------------------------------------------------------------------------
// 2.2: parser newline modes
// ---------------------------------------------------------------------------

#[test]
fn newlines_inside_parentheses_are_insignificant_everywhere() {
    // 2.2: `(` pushes Insignificant; the fetch loop discards NEWLINE tokens.
    assert_contains(
        &body("let x = (\n\n    a\n\n)\nx"),
        "(let x (paren (name a)))",
    );
    assert_contains(&body("let x = f(\n)\nx"), "(let x (call (name f)))");
    // A newline before the comma, before the operator, and before `)`.
    assert_contains(
        &body("let x = f(a\n, b\n+ c\n)\nx"),
        "(let x (call (name f) (name a) (+ (name b) (name c))))",
    );
    // `return` and `assert` operands too.
    assert_contains(&body("return (\n    1\n)"), "(return (paren (int 1)))");
    assert_contains(&body("assert (\n    a\n)\n1"), "(assert (paren (name a)))");
}

#[test]
fn newlines_inside_brackets_are_insignificant() {
    assert_contains(&body("let x = [\n]\nx"), "(let x (list))");
    assert_contains(
        &body("let x = [\n    1\n    ,\n    2\n    ,\n]\nx"),
        "(let x (list (int 1) (int 2)))",
    );
    // Nested multi-line lists.
    assert_contains(
        &body("let x = [\n    [\n        1,\n        2,\n    ],\n    [3],\n]\nx"),
        "(let x (list (list (int 1) (int 2)) (list (int 3))))",
    );
}

#[test]
fn newlines_inside_a_struct_literal_are_insignificant() {
    // 2.2: the `{` of a struct literal pushes Insignificant.
    assert_contains(&body("let p = Page {\n}\np"), "(let p (struct-lit Page))");
    assert_contains(
        &body("let p = Page {\n    ..\n    base\n}\np"),
        "(let p (struct-lit Page (spread (name base))))",
    );
    // Newlines around `:` and `,` inside the literal are discarded too.
    assert_contains(
        &body("let p = Page {\n    text\n    :\n    t\n    ,\n    id: i\n}\np"),
        "(let p (struct-lit Page (field text (name t)) (field id (name i))))",
    );
    // Nested struct literal and a parenthesized field value spanning lines.
    assert_contains(
        &body(
            "let p = Outer {\n    inner: Inner {\n        a: (1\n            + 2),\n    },\n}\np",
        ),
        "(struct-lit Outer (field inner (struct-lit Inner (field a (paren (+ (int 1) (int 2)))))))",
    );
}

#[test]
fn newlines_inside_an_effects_set_and_a_use_list_are_insignificant() {
    // 2.2: `effects` set and `use` list braces push Insignificant.
    let d = module(
        "module m\n\nuse a.b.{\n    c,\n    d\n}\n\nfn f(using db: Db) -> Int\n    effects {\n        db.get,\n        db.put,\n    }\n{\n    1\n}\n",
    );
    assert_contains(&d, "(use a.b (items c d)");
    assert_contains(&d, "(effects db.get db.put)");
}

#[test]
fn newlines_inside_work_selector_sets_and_git_refs_are_insignificant() {
    // 2.2 / 4.2: selector sets open an insignificant-newline context.
    let d = work(
        "change c {\n    base git(\n        \"main\"\n    )\n    target git(\"main\")\n    task t {\n        write {\n        }\n        add {\n            tests.*\n            ,\n            other.*,\n        }\n        depend { a\n            .b\n            .body }\n        goal \"g\"\n    }\n    integrate {\n        require prove(\n            paginate\n        )\n    }\n}\n",
    );
    assert_contains(&d, "(base \"main\")");
    assert_contains(
        &d,
        "(task t (write) (add tests.* other.*) (depend a.b.body) (goal \"g\"))",
    );
    assert_contains(&d, "(require (prove paginate))");
}

#[test]
fn a_block_inside_parentheses_restores_significant_newlines() {
    // 2.2: the stack is restored on every bracket; `match` and `if` bodies
    // inside an argument list have significant newlines again.
    assert_contains(
        &body("let x = foo(match y {\n    A => 1\n    B => 2\n})\nx"),
        "(call (name foo) (match (name y) (arm (ctor A) (int 1)) (arm (ctor B) (int 2))))",
    );
    assert_contains(
        &body("let x = foo(if c {\n    let y = 1\n    y\n} else {\n    2\n})\nx"),
        "(if (name c) (block (let y (int 1)) (tail (name y))) (else (block (tail (int 2)))))",
    );
    // Two statements on one line inside such a block are still an error (2.3).
    let e = body_errors("let x = foo(if c { let y = 1 let z = 2 })\nx");
    assert_code(&e, codes::SYNTAX_ERROR);
    // A struct literal inside a block inside parentheses: three mode switches.
    assert_contains(
        &body("let x = f(if c { Page {\n    ..p\n} } else { p })\nx"),
        "(block (tail (struct-lit Page (spread (name p)))))",
    );
}

#[test]
fn the_mode_is_popped_on_the_closing_bracket() {
    // 2.2: after `)` or `]` the newline is significant again.
    assert_contains(
        &body("let x = (1)\nlet y = 2\nx"),
        "(let x (paren (int 1))) (let y (int 2))",
    );
    // `.len` on the next line after `]` is a new statement, and an error.
    let e = body_errors("let x = [1]\n    .len\nx");
    assert_code(&e, codes::SYNTAX_ERROR);
    // After the `}` of a struct literal the statement ends.
    let e = body_errors("let p = Page { ..p }\n    .text\np");
    assert_code(&e, codes::SYNTAX_ERROR);
}

#[test]
fn generic_lists_never_push_a_mode() {
    // 2.2 / 3.4: a line break inside a generic list is a syntax error where
    // newlines are significant ...
    assert_code(
        &body_errors("let x: List<\n    Int> = []\nx"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f() -> List<\n    Int> {\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f<T,\n    U>() -> Int {\n    1\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nstruct S<T\n> {\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // ... and a generic call split over a line at statement level fails its
    // speculation, leaving `check <` with nothing after it (D42, 2.3).
    let e = body_errors("check<\n    Positive>(n)");
    assert_code(&e, codes::SYNTAX_ERROR);
    // ... but inside brackets the break is discarded like any other.
    assert_contains(
        &module("module m\n\nfn f(x: List<\n    Int>) -> Int {\n    1\n}\n"),
        "(param x (ty List (ty Int)))",
    );
    assert_contains(
        &body("let x = f(check<\n    Positive>(n))\nx"),
        "(call (name f) (call (name check) (generics (ty Positive)) (name n)))",
    );
    assert_contains(
        &body("let x = [g<Int,\n    Text>(1)]\nx"),
        "(list (call (name g) (generics (ty Int) (ty Text)) (int 1)))",
    );
}

// ---------------------------------------------------------------------------
// 2.3 / D11: statement termination and non-continuation
// ---------------------------------------------------------------------------

#[test]
fn every_binary_operator_at_end_of_line_ends_the_expression() {
    // D11: a binary operator at the end of a line does not continue the
    // expression; `expected expression, found end of line`.
    for op in [
        "+", "-", "*", "/", "%", "++", "==", "!=", "<", "<=", ">", ">=", "and", "or",
    ] {
        let src = format!("let x = a {op}\n    b\nx");
        let e = body_errors(&src);
        assert!(
            has_code(&e, codes::SYNTAX_ERROR)
                && has_message(&e, "expected expression, found end of line"),
            "operator {op}: {:?}",
            e.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn prefix_operators_and_quantifier_colon_at_end_of_line_do_not_continue() {
    // 2.3: the expression parser never consumes a NEWLINE.
    assert!(has_message(
        &body_errors("let x = not\n    a\nx"),
        "expected expression, found end of line"
    ));
    assert!(has_message(
        &body_errors("let x = -\n    1\nx"),
        "expected expression, found end of line"
    ));
    assert!(has_message(
        &body_errors("let x = forall y in ys:\n    y > 0\nx"),
        "expected expression, found end of line"
    ));
    // `forall y in` then a newline.
    assert_code(
        &body_errors("let x = forall y in\n    ys: y > 0\nx"),
        codes::SYNTAX_ERROR,
    );
    // A trailing comment does not rescue the dangling operator.
    assert!(has_message(
        &body_errors("let x = a + // sum\n    b\nx"),
        "expected expression, found end of line"
    ));
}

#[test]
fn a_dangling_dot_or_leading_operator_line_is_an_error() {
    // D11: no leading-dot continuation; a line starting with an operator
    // cannot begin a statement.
    assert_code(&body_errors("let x = a.\n    text\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("let x = 1\n    + 2\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("let x = a\n    and b\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("let x = a\n    ?\nx"), codes::SYNTAX_ERROR);
    assert_code(
        &body_errors("let x = f\n    (1)\n    .len\nx"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_name_then_a_parenthesized_line_is_two_statements() {
    // 2.3: `let text = page` followed by a line is two statements; `(1)` is a
    // legal expression statement, so this parses as two statements.
    assert_contains(
        &body("let x = f\n(1)\nx"),
        "(block (let x (name f)) (expr (paren (int 1))) (tail (name x)))",
    );
    // Likewise a list literal on its own line is a statement of its own.
    assert_contains(
        &body("let x = f\n[1]\nx"),
        "(block (let x (name f)) (expr (list (int 1))) (tail (name x)))",
    );
}

#[test]
fn a_name_followed_by_a_brace_on_the_next_line_is_not_a_struct_literal() {
    // 2.3 / D11: the only `{` lookahead past a NEWLINE is for `if`, `else`
    // and `match` headers; `Page` ends the statement and a bare `{` cannot
    // begin one (D28).
    assert_code(
        &body_errors("let p = Page\n{ ..p }\np"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&body_errors("let p = Page\n{\n}\np"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("Page\n{ ..p }"), codes::SYNTAX_ERROR);
}

#[test]
fn two_statements_never_share_a_line() {
    // 2.3: there is no separator token.
    assert_code(&body_errors("assert a assert b\n1"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("return 1 return 2"), codes::SYNTAX_ERROR);
    assert_code(
        &body_errors("while c {} while d {}\n1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&body_errors("if c {} if d {}\n1"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("var x = 1 x = 2\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("f() g()"), codes::SYNTAX_ERROR);
    // Items too: two `use` declarations, or a header and a declaration.
    assert_code(
        &module_errors("module m\nuse a.b use c.d\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_errors("module m use a.b\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_errors("module m\nstruct A {} struct B {}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_statement_ends_before_the_closing_brace_or_at_eof() {
    // 2.3: a statement also ends immediately before a `}` or at EOF.
    assert_contains(
        &module("module m\nfn f() -> Unit { let x = 1 }\n"),
        "(fn f (params) (ret (ty Unit)) (block (let x (int 1))))",
    );
    assert_contains(
        &module("module m\nfn f() -> Int {\n    let x = 1 }\n"),
        "(block (let x (int 1)))",
    );
    assert_contains(
        &module("module m\nfn f() -> Int { let x = 1\n    x }\n"),
        "(block (let x (int 1)) (tail (name x)))",
    );
    // At EOF with no terminator at all (1.1: a final line terminator is optional).
    assert_contains(
        &module("module m\nfn f() -> Int { 1 }"),
        "(fn f (params) (ret (ty Int)) (block (tail (int 1))))",
    );
    assert_contains(&module("module m"), "(module m)");
    assert_contains(&module("module m\nuse a.b"), "(use a b)");
}

#[test]
fn return_without_a_value_ends_at_newline_or_brace() {
    // D47 / 3.11: `return` followed by NEWLINE or `}` has no value.
    let d = module(
        "module m\nfn f(c: Bool) -> Unit {\n    if c {\n        return\n    }\n    return\n}\n",
    );
    assert_contains(&d, "(if (name c) (block (return)))");
    assert_contains(&d, "(block (expr (if (name c) (block (return)))) (return))");
    // `return` with a comment after it still has no value.
    assert_contains(
        &module("module m\nfn f() -> Unit {\n    return // done\n}\n"),
        "(return (trivia (trail \"// done\")))",
    );
}

// ---------------------------------------------------------------------------
// 2.4: header continuation lines
// ---------------------------------------------------------------------------

#[test]
fn blank_lines_and_comments_between_a_fn_header_its_clauses_and_its_brace() {
    // 2.4: "Blank lines and ordinary comments between the header and its
    // continuation lines are permitted (they collapse into the single
    // NEWLINE the grammar names)."
    let src = "module m\n\nfn f(x: Int, using db: Db) -> Int\n\n    // bound\n    effects { db.get }\n\n\n    // pre\n\n    requires x > 0\n    // post\n    ensures result > 0\n\n    decreases x\n\n    // body follows\n\n{\n    x\n}\n";
    let m = module_ast(src);
    let DeclKind::Fn(f) = &m.declarations[0].kind else {
        panic!("expected fn")
    };
    assert_eq!(f.clauses.len(), 4);
    assert!(f.effects().is_some());
    assert_eq!(f.requires().count(), 1);
    assert_eq!(f.ensures().count(), 1);
    assert!(f.decreases().is_some());
    assert!(f.body.as_ref().unwrap().tail.is_some());
    assert_crlf_invariant(src);
}

#[test]
fn blank_lines_and_comments_between_a_header_and_its_brace_without_clauses() {
    // D63 + 2.4: `{` may start the next line after any header; blank and
    // comment lines in between collapse.
    let d = module(
        "module m\n\nfn f() -> Int\n\n// why\n\n{\n    1\n}\n\nstruct S\n\n// fields\n{\n    x: Int\n}\n\nenum E\n// variants\n{\n    A\n}\n\ncapability C\n\n{\n    fn get(id: Id) -> Int\n}\n\ntest \"t\"\n\n{\n    assert true\n}\n",
    );
    // (Comments and blank lines add `(trivia ...)` children to the dump, so
    // the shapes are asserted up to the point where trivia would appear.)
    assert_contains(&d, "(fn f (params) (ret (ty Int)) (block (tail (int 1)");
    assert_contains(&d, "(struct S (field x (ty Int)");
    assert_contains(&d, "(enum E (variant A");
    assert_contains(
        &d,
        "(capability C (op get (params (param id (ty Id))) (ret (ty Int))",
    );
    assert_contains(&d, "(test \"t\" (block (assert (bool true)");
    // A comment at the end of the header line is fine too.
    assert_contains(
        &module("module m\nfn f() -> Int // returns one\n{\n    1\n}\n"),
        "(block (tail (int 1)",
    );
}

#[test]
fn while_headers_accept_blank_lines_and_comments_around_their_clauses() {
    // 2.4 table: `while` continues with `invariant`, `decreases`, then `{`.
    let d = body("while c\n\n    // keep\n    invariant x > 0\n\n    invariant y > 0\n    // measure\n\n    decreases x\n\n{\n    x = x - 1\n}\n1");
    assert_contains(&d, "(invariant (> (name x) (int 0))");
    assert_contains(&d, "(invariant (> (name y) (int 0))");
    assert_contains(&d, "(decreases (name x)");
    assert_contains(&d, "(block (assign x (- (name x) (int 1))");
    // No clauses, `{` after a blank line.
    assert_contains(&body("while c\n\n{\n}\n1"), "(while (name c) (block))");
    assert_contains(&body("while c // spin\n{\n}\n1"), "(while (name c) (block");
}

#[test]
fn property_and_capability_headers_accept_blank_lines_around_clauses() {
    // 2.4 table: `property` continues with `for` then `{`; an operation with
    // `requires`/`ensures`.
    let d = module(
        "module m\n\nproperty \"p\"\n\n    // inputs\n    for x in xs\n\n    for y in ys\n\n    // body\n{\n    assert true\n}\n\ncapability C {\n    fn get(id: Id) -> Int\n\n        // pre\n        requires id != x\n\n        ensures result > 0\n\n    fn put(id: Id) -> Int\n}\n",
    );
    assert_contains(&d, "(for x (name xs)");
    assert_contains(&d, "(for y (name ys)");
    assert_contains(&d, "(requires (!= (name id) (name x))");
    assert_contains(&d, "(ensures (> (name result) (int 0))");
    assert_contains(&d, "(op put (params (param id (ty Id))) (ret (ty Int))");
}

#[test]
fn patch_headers_accept_blank_lines_and_comments_around_expect_and_brace() {
    // 4.5 / 2.4: `expect` is a clause and `patch selector` a header.
    let d = patch("patch f.body\n\n    // check\n\n    expect base(f.body)\n\n    // replace\n\n{\n\n    1\n\n}\n");
    assert_contains(&d, "(patch f.body (expect f.body");
    assert_contains(&d, "(block (tail (int 1)");
}

#[test]
fn work_headers_accept_the_brace_on_the_next_line() {
    // 4.1-4.3: `NEWLINE? "{"` after `change`, `task`, `task ... after`, `integrate`.
    let d = work(
        "change c\n{\n    base git(\"a\")\n    target git(\"b\")\n    task t\n    {\n        goal \"g\"\n    }\n    task u after t\n\n    // deps above\n    {\n        goal \"h\"\n    }\n    integrate\n    {\n        require typecheck\n    }\n}\n",
    );
    assert_contains(&d, "(task t (goal \"g\"))");
    assert_contains(&d, "(task u (after t) (item (goal \"h\")");
    assert_contains(&d, "(integrate (require typecheck))");
}

#[test]
fn if_else_and_match_headers_accept_the_brace_after_blank_and_comment_lines() {
    // 2.3: the headers of `if`, `else`, and `match` may be followed by a
    // NEWLINE before their `{`; blank/comment lines collapse into it (2.1).
    let d = body("if c\n\n{\n    1\n} else\n\n// otherwise\n{\n    2\n}");
    assert_contains(&d, "(if (name c) (block (tail (int 1)");
    assert_contains(&d, "(else (block (tail (int 2)");
    assert_contains(
        &body("match x\n\n// arms\n{\n    _ => 1\n}"),
        "(match (name x) (arm _ (int 1)",
    );
    // As an operand (2.3: `let label = if page.len > size` then `{` next line).
    assert_contains(
        &body("let x = 1 + if c\n{ 1 } else { 2 }\nx"),
        "(let x (+ (int 1) (if (name c) (block (tail (int 1))) (else (block (tail (int 2)))))))",
    );
    assert_contains(
        &body("let x = 1 + match y\n{\n    _ => 1\n}\nx"),
        "(let x (+ (int 1) (match (name y) (arm _ (int 1)))))",
    );
}

#[test]
fn rule_1_two_clauses_on_one_line_are_an_error() {
    // 2.4 rule 1: each clause occupies its own line.
    assert_code(
        &body_errors("while c\n    invariant a invariant b\n{\n}\n1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nproperty \"p\"\n    for a in xs for b in ys\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors(
            "module m\n\nfn f(x: Int, using db: Db) -> Int\n    effects { db.get } requires x > 0\n{\n    x\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        requires a ensures b\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn rule_2_the_body_brace_must_start_a_new_line_after_a_clause() {
    // 2.4 rule 2: when at least one clause is present, `{` starts a new line.
    assert_code(
        &body_errors("while c\n    invariant x {\n}\n1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("while c\n    decreases x {\n}\n1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors(
            "module m\n\nfn f(using db: Db) -> Int\n    effects { db.get } {\n    1\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f(x: Int) -> Int\n    ensures result > 0 {\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f(x: Int) -> Int\n    decreases x {\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nproperty \"p\"\n    for a in xs {\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch x\n    expect base(x) {\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn clause_keywords_cannot_be_separated_from_their_operands_by_a_newline() {
    // 2.4 rule 1: a clause's expression extends to the end of *its* line; the
    // grammar has no NEWLINE between the keyword and the expression or set.
    assert_code(
        &module_errors(
            "module m\n\nfn f(using db: Db) -> Int\n    effects\n    { db.get }\n{\n    1\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f(x: Int) -> Int\n    requires\n    x > 0\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f(x: Int) -> Int\n    decreases\n    x\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("while c\n    invariant\n    x\n{\n}\n1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nproperty \"p\"\n    for x in\n    xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch x\n    expect\n    base(x)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch x\n    expect base\n    (x)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_clause_expression_ends_at_its_line_so_a_continuation_line_is_an_error() {
    // 2.4 rule 1 + D11: `requires a` ends at NEWLINE; `and b` continues nothing.
    assert_code(
        &module_errors("module m\n\nfn f(a: Bool, b: Bool) -> Bool\n    requires a\n        and b\n{\n    a\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("while c\n    invariant a\n        and b\n{\n}\n1"),
        codes::SYNTAX_ERROR,
    );
    // Parenthesized, the clause may span lines (3.5 says the same for `where`).
    let d = module("module m\n\nfn f(a: Bool, b: Bool) -> Bool\n    requires (a\n        and b)\n{\n    a\n}\n\npub type P = Int where (self > 0\n    and self < 9)\n");
    assert_contains(&d, "(requires (paren (and (name a) (name b))))");
    assert_contains(
        &d,
        "(where (paren (and (> (name self) (int 0)) (< (name self) (int 9)))))",
    );
}

#[test]
fn rule_3_a_continuation_keyword_of_another_header_does_not_continue() {
    // 2.4 rule 3: only a continuation keyword *for this header* continues it.
    // `while` takes no `requires`; its body is mandatory (`expected { or a clause`).
    let e = body_errors("while c\n    requires x\n{\n}\n1");
    assert_code(&e, codes::SYNTAX_ERROR);
    let e = body_errors("while c\n    ensures x\n{\n}\n1");
    assert_code(&e, codes::SYNTAX_ERROR);
    let e = body_errors("while c\n    for x in xs\n{\n}\n1");
    assert_code(&e, codes::SYNTAX_ERROR);
    // `property` takes only `for`.
    assert_code(
        &module_errors("module m\n\nproperty \"p\"\n    requires x\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // `fn` takes no `invariant`/`for`: the header ends (bodiless, 3.9) and the
    // clause line is not a declaration.
    assert_code(
        &module_errors("module m\n\nfn f(x: Int) -> Int\n    invariant x > 0\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f(x: Int) -> Int\n    for x in xs\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // A `.work` task takes `after` only on the header line.
    assert_code(
        &work_errors("change c {\n    task t\n        after u\n    {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn rule_3_a_fn_header_followed_by_nothing_is_a_bodiless_fn_not_a_syntax_error() {
    // 3.9 (third `fn_tail` alternative): "A `fn` whose header (and clauses,
    // if any) is followed by neither a clause nor `{` is complete without a
    // body ... in a project file a bodiless `fn` parses and is the check
    // error `missing_body`, not a syntax error. The `expected { or a clause`
    // error of rule 3 therefore arises only for `while`, `property`, and
    // `patch` headers." The AST shape of a bodiless fn is not specified by
    // grammar.md, so only the absence of diagnostics is asserted.
    for src in [
        "module m\n\nfn f() -> Int\n\nfn g() -> Int { 1 }\n",
        "module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n\nfn g() -> Int { 1 }\n",
        "module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n    ensures result > 0\n",
        "module m\n\n@builtin(\"x\")\npub fn f() -> Int\n",
        "module m\nfn f() -> Int",
        "module m\n\nfn f() -> Int\n// end of file\n",
    ] {
        let r = parse_module(src, FileId(0));
        assert!(
            r.diagnostics.is_empty() && r.value.is_some(),
            "a bodiless fn parses (grammar 3.9):\n{src}\n{}",
            render(src, &r.diagnostics)
        );
    }
}

#[test]
fn rule_3_while_property_and_patch_headers_need_their_bodies() {
    // 2.4 rule 3 / 3.9: the `expected { or a clause` error arises for
    // `while`, `property`, and `patch` headers.
    let e = body_errors("while c\n1");
    assert_code(&e, codes::SYNTAX_ERROR);
    let e = body_errors("while c\n    invariant x\n1");
    assert_code(&e, codes::SYNTAX_ERROR);
    let e = module_errors("module m\n\nproperty \"p\"\n    for x in xs\n\nfn g() -> Int { 1 }\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    let e = module_errors("module m\n\nproperty \"p\"\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    let e = patch_errors("patch x\n    expect base(x)\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    let e = patch_errors("patch x\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    // `if`/`match` headers followed by a line that is not `{`.
    assert_code(&body_errors("if c\n    x = 1\n}\n1"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("match x\n    _ => 1\n}"), codes::SYNTAX_ERROR);
    // `test` and `struct` headers with nothing after them.
    assert_code(
        &module_errors("module m\n\ntest \"t\"\n\nfn g() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nstruct S\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_doc_comment_between_a_header_and_its_continuation_is_misplaced() {
    // 1.2: a DOC_COMMENT anywhere other than before a declaration or member
    // is `doc comment must precede a declaration` (code doc_comment_position,
    // semantics 9.4). Here it separates a header from its `{` or clause.
    assert_code(
        &module_errors("module m\n\nfn f() -> Int\n/// not here\n{\n    1\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &module_errors(
            "module m\n\nfn f(x: Int) -> Int\n/// not here\n    requires x > 0\n{\n    x\n}\n",
        ),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &body_errors("while c\n/// not here\n{\n}\n1"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &body_errors("match x {\n    A => 1\n    /// not here\n    B => 2\n}"),
        codes::DOC_COMMENT_POSITION,
    );
    // A doc comment with nothing following inside a struct body.
    assert_code(
        &module_errors("module m\n\nstruct S {\n    x: Int\n    /// dangling\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn d12_else_must_follow_the_closing_brace_on_the_same_line() {
    // D12 / 2.4 rule 4: a line beginning with `else` is misplaced_else.
    assert_code(
        &body_errors("if c { 1 }\nelse { 2 }"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if c { 1 }\n\n\nelse { 2 }"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if c { 1 } // then\nelse { 2 }"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if c { 1 }\n// otherwise\nelse { 2 }"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if c {\n    1\n}\nelse if d {\n    2\n}"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if c { 1 } else if d { 2 }\nelse { 3 }"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if c { 1 }\r\nelse { 2 }"),
        codes::MISPLACED_ELSE,
    );
    // In a `let` value.
    assert_code(
        &body_errors("let x = if c { 1 }\nelse { 2 }\nx"),
        codes::MISPLACED_ELSE,
    );
}

#[test]
fn d12_else_on_its_own_line_inside_a_match_body_is_misplaced() {
    // D12: wherever newlines are significant; a match body is significant
    // (2.2), so the arm `A => if c { 1 }` ends at the NEWLINE and the next
    // line begins with `else`.
    assert_code(
        &body_errors("match x {\n    A => if c { 1 }\n    else { 2 }\n    B => 3\n}"),
        codes::MISPLACED_ELSE,
    );
}

#[test]
fn d12_else_on_the_next_line_is_accepted_inside_brackets() {
    // 2.4 rule 4: inside an insignificant-newline context the NEWLINE is
    // discarded before the parser sees it.
    assert_contains(
        &body("let x = (if c { 1 }\nelse if d { 2 }\nelse { 3 })\nx"),
        "(paren (if (name c) (block (tail (int 1))) (else (if (name d) (block (tail (int 2))) (else (block (tail (int 3))))))))",
    );
    assert_contains(
        &body("let x = [if c { 1 }\nelse { 2 }]\nx"),
        "(list (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    assert_contains(
        &body("let p = Page { text: if c { a }\nelse { b } }\np"),
        "(field text (if (name c) (block (tail (name a))) (else (block (tail (name b))))))",
    );
}

#[test]
fn else_followed_by_a_newline_and_if_is_not_a_chain() {
    // 3.12: `"else" ( NEWLINE? block | if_expr )`: the optional NEWLINE
    // precedes a block only, never the `if` of a chain.
    assert_code(
        &body_errors("if c { 1 } else\nif d { 2 } else { 3 }"),
        codes::SYNTAX_ERROR,
    );
    // `else` then NEWLINE then `{` is fine (NEWLINE? block).
    assert_contains(
        &body("if c { 1 } else\n{ 2 }"),
        "(else (block (tail (int 2))))",
    );
    // `else if d` then NEWLINE then `{` is the nested if's `NEWLINE? block`.
    assert_contains(
        &body("if c { 1 } else if d\n{ 2 }"),
        "(else (if (name d) (block (tail (int 2)))))",
    );
}

// ---------------------------------------------------------------------------
// 2.5: trailing commas
// ---------------------------------------------------------------------------

#[test]
fn trailing_commas_are_accepted_in_every_bracketed_list() {
    // 2.5: commas(X)/commas1(X) everywhere in insignificant-newline contexts.
    let d = module(
        "module m\n\nuse a.b.{\n    c,\n}\n\n@id(\"x\",)\nfn f(\n    a: Int,\n    using db: Db,\n) -> Int\n    effects { db.get, }\n{\n    let p = Page { ..p, text: t, }\n    let q = Page { ..p, }\n    let r = Page { text: t, }\n    let l = [1,]\n    g(a, using db,)\n}\n\nfn h(using db: Db,) -> Int { 1 }\n\nenum E {\n    A(Int,)\n    B(Int, Text,)\n}\n",
    );
    assert_contains(&d, "(use a.b (items c)");
    assert_contains(&d, "(annotation id (text \"x\"))");
    assert_contains(
        &d,
        "(struct-lit Page (spread (name p)) (field text (name t)))",
    );
    assert_contains(&d, "(let q (struct-lit Page (spread (name p))))");
    assert_contains(&d, "(let r (struct-lit Page (field text (name t))))");
    assert_contains(&d, "(let l (list (int 1)))");
    assert_contains(&d, "(fn h (params (using db (ty Db))) (ret (ty Int))");
    assert_contains(&d, "(variant A (ty Int)) (variant B (ty Int) (ty Text))");
    // Patterns: `commas1(pattern)` inside parentheses.
    assert_contains(
        &body("match x {\n    Ok(v,) => v\n    Pair(a, b,) => a\n}"),
        "(arm (ctor Ok (bind v)) (name v)) (arm (ctor Pair (bind a) (bind b)) (name a))",
    );
    // Selector sets in .work files.
    assert_contains(
        &work("change c {\n    task t {\n        write { a.body, }\n        depend { b, }\n    }\n}\n"),
        "(task t (write a.body) (depend b))",
    );
}

#[test]
fn trailing_commas_are_rejected_where_the_grammar_has_no_commas_production() {
    // 2.5: not in generic lists or `after` lists; 4.1-4.3 single-item
    // parentheses (`git(TEXT)`, `prove(selector)`, `review(task)`) take none.
    assert_code(
        &module_errors("module m\n\nfn f<T,>() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nstruct S<A, B,> {\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("let x: Result<Int, E,> = y\nx"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t after a, b, {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    base git(\"a\",)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    integrate {\n        require prove(x,)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    integrate {\n        require review(t,)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch x\n    expect base(x,)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // A wrapper type has exactly one payload and no comma (D18).
    assert_code(
        &module_errors("module m\n\ntype A(Int,)\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_comma_is_never_a_line_separator() {
    // 2.3 / D55 / 3.6-3.8: items in significant-newline bodies are separated
    // by NEWLINE, never by commas.
    assert_code(&body_errors("let x = 1,\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("x = 1,\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("f(),\n1"), codes::SYNTAX_ERROR);
    assert_code(
        &module_errors("module m\n\nstruct S {\n    x: Int,\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nenum E {\n    A,\n    B\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\ncapability C {\n    fn a() -> Int,\n    fn b() -> Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    let e = body_errors("match x {\n    A => { 1 },\n    B => 2\n}");
    assert!(
        has_code(&e, codes::SYNTAX_ERROR)
            && has_message(&e, "match arms are separated by newlines"),
        "{e:#?}"
    );
    assert_code(
        &work_errors("change c {\n    workers 3,\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    integrate {\n        require typecheck,\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t {\n        goal \"g\",\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Lone or doubled commas inside brackets are errors too.
    assert_code(&body_errors("let x = [,]\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("let x = [1,,2]\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("f(,)"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("Page { , }"), codes::SYNTAX_ERROR);
    assert_code(
        &module_errors("module m\n\nfn f(using db: Db) -> Int\n    effects { , }\n{\n    1\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

// ---------------------------------------------------------------------------
// D51 / 3.11: empty blocks and tail expressions
// ---------------------------------------------------------------------------

#[test]
fn empty_blocks_in_every_spelling() {
    // D51: `{}` or `{` NEWLINE `}` has no statements and value Unit; blank
    // lines and comment-only lines collapse into that one NEWLINE (2.1).
    let d = module("module m\n\nfn a() -> Unit { }\nfn b() -> Unit {\n\n\n\n}\nfn c() -> Unit { // nothing\n}\nfn d() -> Unit {\n    // only a comment\n}\nfn e() -> Unit {\n\t\n\t\t\n}\n");
    for name in ["a", "b", "c", "d", "e"] {
        assert_contains(&d, &format!("(fn {name} (params) (ret (ty Unit)) (block"));
    }
    let m = module_ast("module m\n\nfn b() -> Unit {\n\n\n\n}\n");
    let DeclKind::Fn(f) = &m.declarations[0].kind else {
        panic!()
    };
    let fn_body = f.body.as_ref().unwrap();
    assert!(fn_body.is_empty() && fn_body.tail.is_none());
    // Empty `if`, `else`, `while`, `match`, `test`, and arm-body blocks.
    let d = body("if c {\n} else {\n}\nwhile c\n    invariant x\n{}\nmatch x {\n}\nmatch x {\n    _ => {\n    }\n}\n1");
    assert_contains(&d, "(if (name c) (block) (else (block)))");
    assert_contains(&d, "(while (name c) (invariant (name x)) (block))");
    assert_contains(&d, "(expr (match (name x)))");
    assert_contains(&d, "(match (name x) (arm _ (block)))");
    assert_contains(&module("module m\ntest \"t\" {}\n"), "(test \"t\" (block))");
    // `.work` and `.patch` bodies may be empty too (4.1-4.5).
    assert_contains(
        &work("change c {}\nchange d {\n    task t {}\n    integrate {\n    }\n}\n"),
        "(change c) (change d (task t) (integrate))",
    );
    assert_contains(
        &patch("patch x\n    expect base(x)\n{}\n"),
        "(expect x) (block))",
    );
    assert_crlf_invariant("module m\n\nfn a() -> Unit { }\nfn b() -> Unit {\n\n}\n");
}

#[test]
fn the_tail_expression_is_the_last_statement_only_when_it_is_an_expression() {
    // 3.11: if the last statement is an expr_stmt it is the tail; otherwise
    // the block's value is Unit.
    assert_contains(
        &body("let x = 1\nx"),
        "(block (let x (int 1)) (tail (name x)))",
    );
    assert_contains(
        &body("x\nlet y = 1"),
        "(block (expr (name x)) (let y (int 1)))",
    );
    assert_contains(
        &body("f()\ng()"),
        "(block (expr (call (name f))) (tail (call (name g))))",
    );
    assert_contains(&body("return 1"), "(block (return (int 1)))");
    assert_contains(&body("while c {}"), "(block (while (name c) (block)))");
    assert_contains(&body("x = 1"), "(block (assign x (int 1)))");
    // Trailing comment and blank lines after the tail do not change it.
    assert_contains(
        &body("let x = 1\nx // value\n\n\n"),
        "(tail (name x) (trivia (trail \"// value\")))",
    );
    // `if`/`match` as tails, on several lines.
    assert_contains(
        &body("if c {\n    1\n} else {\n    2\n}"),
        "(block (tail (if (name c) (block (tail (int 1))) (else (block (tail (int 2)))))))",
    );
    // An arm body block with statements and a tail across lines.
    assert_contains(
        &body("match x {\n    A => {\n        let y = 1\n        y\n    }\n}"),
        "(arm (ctor A) (block (let y (int 1)) (tail (name y))))",
    );
}

// ---------------------------------------------------------------------------
// 3.12 / D49: `if` / `else if` / `else` across lines
// ---------------------------------------------------------------------------

#[test]
fn if_chains_with_multi_line_blocks_parse_as_nested_ifs() {
    let d = body("if a {\n    1\n} else if b {\n    2\n} else if c {\n    3\n} else {\n    4\n}");
    assert_contains(
        &d,
        "(if (name a) (block (tail (int 1))) (else (if (name b) (block (tail (int 2))) (else (if (name c) (block (tail (int 3))) (else (block (tail (int 4)))))))))",
    );
    // With `{` on the line after each header (NEWLINE? block).
    let d = body("if a\n{\n    1\n} else if b\n{\n    2\n} else\n{\n    3\n}");
    assert_contains(
        &d,
        "(if (name a) (block (tail (int 1))) (else (if (name b) (block (tail (int 2))) (else (block (tail (int 3)))))))",
    );
    // Nested ifs inside branches across lines.
    let d =
        body("if a {\n    if b {\n        1\n    } else {\n        2\n    }\n} else {\n    3\n}");
    assert_contains(
        &d,
        "(if (name a) (block (tail (if (name b) (block (tail (int 1))) (else (block (tail (int 2))))))) (else (block (tail (int 3)))))",
    );
    assert_crlf_invariant(&wrap(
        "if a {\n    1\n} else if b {\n    2\n} else {\n    3\n}",
    ));
}

#[test]
fn if_conditions_may_span_lines_only_inside_brackets() {
    // 2.3: a header expression ends at its line unless a bracket is open.
    assert_contains(
        &body("if (a\n    and b) {\n    1\n}\n1"),
        "(if (paren (and (name a) (name b))) (block (tail (int 1))))",
    );
    assert_contains(
        &body("if f(\n    a,\n    b,\n) {\n    1\n}\n1"),
        "(if (call (name f) (name a) (name b)) (block (tail (int 1))))",
    );
    assert_code(
        &body_errors("if a and\n    b {\n    1\n}\n1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("if a\n    and b {\n    1\n}\n1"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_second_else_or_a_dangling_else_is_an_error() {
    assert_code(
        &body_errors("if a { 1 } else { 2 } else { 3 }"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&body_errors("if a { 1 } else"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("if a { 1 } else\n"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("if a { 1 } else if"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("else { 1 }"), codes::MISPLACED_ELSE);
}

// ---------------------------------------------------------------------------
// 3.12 / D55: match-arm layout
// ---------------------------------------------------------------------------

#[test]
fn match_arms_one_per_line_with_block_or_expression_bodies() {
    let d = body("match r {\n    Ok(v) => v\n    Err(e) => {\n        return Err(e)\n    }\n    _ => {\n    }\n}");
    assert_contains(
        &d,
        "(match (name r) (arm (ctor Ok (bind v)) (name v)) (arm (ctor Err (bind e)) (block (return (call (name Err) (name e))))) (arm _ (block)))",
    );
    // A single arm on the `{` line and on the `}` line (lines(X) has optional
    // leading and trailing NEWLINEs).
    assert_contains(
        &body("match x { A => 1\n    B => 2 }"),
        "(match (name x) (arm (ctor A) (int 1)) (arm (ctor B) (int 2)))",
    );
    assert_contains(
        &body("match x { A => 1 }"),
        "(match (name x) (arm (ctor A) (int 1)))",
    );
    // Blank lines and comments between arms collapse (2.1).
    assert_contains(
        &body("match x {\n\n    A => 1\n\n    // second\n\n    B => 2\n\n}"),
        "(arm (ctor B) (int 2)",
    );
    assert_crlf_invariant(&wrap(
        "match r {\n    Ok(v) => v\n    Err(e) => {\n        return Err(e)\n    }\n}",
    ));
}

#[test]
fn match_arm_bodies_may_continue_only_inside_brackets_or_a_block() {
    // 2.3: the arm's expression ends at its line.
    assert_contains(
        &body("match x {\n    A => (1\n        + 2)\n    B => Page {\n        ..p,\n        text: t,\n    }\n    C => [\n        1,\n    ]\n    D => f(\n        1,\n    )\n}"),
        "(arm (ctor A) (paren (+ (int 1) (int 2)))) (arm (ctor B) (struct-lit Page (spread (name p)) (field text (name t)))) (arm (ctor C) (list (int 1))) (arm (ctor D) (call (name f) (int 1)))",
    );
    // An arm body `if ... else` on one line.
    assert_contains(
        &body("match x {\n    A => if c { 1 } else { 2 }\n}"),
        "(arm (ctor A) (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    // A nested match as an arm body across lines.
    assert_contains(
        &body(
            "match x {\n    A => match y {\n        B => 1\n        _ => 2\n    }\n    _ => 3\n}",
        ),
        "(arm (ctor A) (match (name y) (arm (ctor B) (int 1)) (arm _ (int 2)))) (arm _ (int 3))",
    );
}

#[test]
fn match_arm_layout_errors() {
    // 3.12: `match_arm = pattern "=>" ( block | expr )` has no NEWLINE; the
    // expression parser never consumes one (2.3).
    assert!(has_message(
        &body_errors("match x {\n    A =>\n        1\n}"),
        "expected expression, found end of line"
    ));
    assert_code(
        &body_errors("match x {\n    A\n    => 1\n}"),
        codes::SYNTAX_ERROR,
    );
    // `=>` then NEWLINE then `{` is not `NEWLINE? block` (only headers have it).
    assert_code(
        &body_errors("match x {\n    A =>\n    {\n        1\n    }\n}"),
        codes::SYNTAX_ERROR,
    );
    // Two arms on one line, with a block body or an expression body.
    assert_code(
        &body_errors("match x {\n    A => { 1 } B => 2\n}"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("match x {\n    A => 1 B => 2\n}"),
        codes::SYNTAX_ERROR,
    );
    // An arm's `}` followed by another arm's start on the same line.
    assert_code(
        &body_errors("match x {\n    A => {\n        1\n    } B => 2\n}"),
        codes::SYNTAX_ERROR,
    );
    // A binary operator dangling at the end of an arm line.
    assert!(has_message(
        &body_errors("match x {\n    A => 1 +\n        2\n}"),
        "expected expression, found end of line"
    ));
}

// ---------------------------------------------------------------------------
// Multi-line struct and list literals in statement positions
// ---------------------------------------------------------------------------

#[test]
fn multi_line_literals_as_statement_values_and_arguments() {
    // 2.3 continuation examples, in every statement form.
    let d = body("var pages = [\n    [1, 2],\n    [3],\n]\npages = [\n]\nassert pages == [\n    [1, 2],\n]\nreturn Page {\n    ..base,\n    text: trim(\n        base.text,\n    ),\n}");
    assert_contains(
        &d,
        "(var pages (list (list (int 1) (int 2)) (list (int 3))))",
    );
    assert_contains(&d, "(assign pages (list))");
    assert_contains(
        &d,
        "(assert (== (name pages) (list (list (int 1) (int 2)))))",
    );
    assert_contains(
        &d,
        "(return (struct-lit Page (spread (name base)) (field text (call (name trim) (member (name base) text)))))",
    );
    // Comments inside multi-line literals attach to entries, not the shape.
    let d =
        body("let p = Page { // open\n    ..base, // spread\n    // the text\n    text: t,\n}\np");
    assert_contains(&d, "(let p (struct-lit Page (spread (name base)");
    assert_contains(&d, "(field text (name t)");
    let m = module_ast(&wrap(
        "let l = [ // open\n    1, // one\n    // two\n    2,\n]\nl",
    ));
    let DeclKind::Fn(f) = &m.declarations[0].kind else {
        panic!()
    };
    let Statement::Let(l) = &f.body.as_ref().unwrap().statements[0] else {
        panic!()
    };
    let ExprKind::List(list) = &l.value.kind else {
        panic!("expected a list")
    };
    assert_eq!(list.elements.len(), 2);
    assert_crlf_invariant(&wrap("let p = Page {\n    ..base,\n    text: t,\n}\np"));
}

#[test]
fn an_unclosed_multi_line_literal_reaches_eof_with_an_error() {
    // The bracket never closes, so the newline-insignificant context swallows
    // every following line and the file ends inside it (2.2 / D59).
    assert_code(
        &body_errors("let l = [\n    1,\n    2\nl"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("let p = Page {\n    ..base\np"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&body_errors("let x = f(\n    1\nx"), codes::SYNTAX_ERROR);
}

// ---------------------------------------------------------------------------
// 4.1-4.5: `.work` and `.patch` layout
// ---------------------------------------------------------------------------

#[test]
fn work_items_are_one_per_line_with_blank_and_comment_lines_between() {
    let src = "// header\nuse a.b.{f}\n\n\n// first change\nchange c {\n\n    base git(\"a\")\n    // the branch\n\n    target git(\"b\")\n\n    workers 2\n\n    task t {\n\n        write {\n            f.body,\n        }\n\n        goal \"g\"\n\n    }\n\n    integrate {\n\n        require typecheck\n\n        require no_holes\n\n    }\n\n}\n\n\nchange d {\n    base git(\"a\")\n    target git(\"b\")\n}\n// trailing\n";
    let d = work(src);
    assert_contains(&d, "(workers 2 (trivia blank)");
    // The blank line after an opening `{` is not recorded on the first item
    // of the body (spec/toolchain.md 2.3: `blank_before` is always false for
    // the first node of a sequence; 2.4 rule 4: no blank line after `{`),
    // so `write` and the first `require` carry no trivia while `goal` and
    // the second `require` do.
    assert_contains(
        &d,
        "(task t (write f.body) (item (goal \"g\") (trivia blank))",
    );
    assert_contains(
        &d,
        "(integrate (require typecheck) (require no_holes (trivia blank))",
    );
    assert_contains(&d, "(change d (base \"a\") (target \"b\")");
    assert_eq!(work(src), work(&src.replace('\n', "\r\n")));
}

#[test]
fn work_items_on_one_line_are_an_error() {
    // 2.3 applies to `.work` files (4: "share the .cpl lexer and layout rules").
    assert_code(
        &work_errors("change c {\n    base git(\"a\") target git(\"b\")\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t {\n        write { a } goal \"g\"\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors(
            "change c {\n    integrate {\n        require typecheck require no_holes\n    }\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n}\nchange d {\n} change e {\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("use a.b.{f} change c {\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn work_item_keywords_cannot_be_split_from_their_operands_by_a_newline() {
    // 4.1-4.2: `workers INT`, `goal TEXT`, `base git_ref`, `write selector_set`
    // name no NEWLINE between keyword and operand.
    assert_code(
        &work_errors("change c {\n    workers\n    3\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    base\n    git(\"a\")\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    base git\n    (\"a\")\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t {\n        goal\n        \"g\"\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t {\n        write\n        { a }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    integrate {\n        require\n        typecheck\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    integrate {\n        require prove\n        (x)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&work_errors("change\nc {\n}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_errors("change c {\n    task\n    t {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn an_after_list_stays_on_the_task_header_line() {
    // 4.2: `after_clause` is on the same line as `task` and is not bracketed,
    // so a line break inside it ends the header.
    assert_code(
        &work_errors("change c {\n    task t after a,\n        b {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t after a\n        , b {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t after\n        a {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // The `{` may follow on the next line after a complete `after` list.
    assert_contains(
        &work("change c {\n    task t after a, b\n    {\n        goal \"g\"\n    }\n}\n"),
        "(task t (after a b) (goal \"g\"))",
    );
}

#[test]
fn patch_forms_are_separated_by_newlines_and_their_layout_is_fixed() {
    // 4.5: `patch_file = patch_form ( NEWLINE patch_form )* NEWLINE? EOF`.
    let d = patch("patch a.body\n    expect base(a.body)\n{\n}\npatch b.body\n    expect base(b.body)\n{\n    1\n}\n\n\n// end\n");
    assert_contains(&d, "(patch a.body (expect a.body) (block)) (patch b.body (expect b.body) (block (tail (int 1))))");
    // Two forms with no blank line, no trailing newline, CRLF.
    let d = patch("patch a.body\r\n    expect base(a.body)\r\n{\r\n}\r\npatch b.body\r\n    expect base(b.body)\r\n{\r\n}");
    assert_contains(&d, "(patch b.body (expect b.body) (block))");
    // `}` and the next `patch` on one line.
    assert_code(
        &patch_errors("patch a.body\n    expect base(a.body)\n{\n} patch b.body\n    expect base(b.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // The body obeys the statement layout rules of 3.11 / D11.
    assert!(has_message(
        &patch_errors(
            "patch a.body\n    expect base(a.body)\n{\n    let x = a +\n        b\n    x\n}\n"
        ),
        "expected expression, found end of line"
    ));
}

// ---------------------------------------------------------------------------
// 1.1: CRLF, missing final terminator, tabs; 1.2: comments at end of file
// ---------------------------------------------------------------------------

#[test]
fn crlf_files_parse_identically_to_lf_files() {
    // 1.1: CRLF is one terminator. Each corpus-shaped construct is checked.
    for src in [
        "module m\n\nuse a.b.{c}\n\npub struct S {\n    x: Int\n}\n\nenum E {\n    A(Int)\n    B\n}\n",
        "module m\n\nfn f(x: Int, using db: Db) -> Int\n    effects { db.get }\n    requires x > 0\n    ensures result > 0\n    decreases x\n{\n    var y = x\n    while y > 0\n        invariant y >= 0\n        decreases y\n    {\n        y = y - 1\n    }\n    if y == 0 {\n        1\n    } else {\n        2\n    }\n}\n",
        "module m\n\nproperty \"p\"\n    for x in xs\n{\n    assert true\n}\n\ntest \"t\"\n{\n    let l = [\n        1,\n    ]\n    assert l == [1]\n}\n",
        "/// Module docs\nmodule m\n\n/// Docs\n@id(\"x\")\n/// more\n@prove\npub fn f() -> Int { 1 }\n",
    ] {
        assert_crlf_invariant(src);
    }
    // Doc and ordinary comment texts never include the CR.
    let m = module_ast("/// doc\r\nmodule m // header\r\n\r\n/// fn doc\r\nfn f() -> Int { 1 } // trailing\r\n// last\r\n");
    assert_eq!(m.docs.as_ref().map(|d| d.text()), Some("doc".to_string()));
    assert_eq!(m.trivia.trailing[0].text, "// header");
    assert_eq!(
        m.declarations[0].docs.as_ref().map(|d| d.text()),
        Some("fn doc".to_string())
    );
    assert_eq!(m.declarations[0].trivia.trailing[0].text, "// trailing");
    assert_eq!(m.trailing_comments[0].text, "// last");
    // A file mixing LF and CRLF terminators is fine.
    assert_contains(
        &module("module m\r\nfn f() -> Int {\n    1\r\n}\n"),
        "(block (tail (int 1)))",
    );
    // A BOM followed by CRLF.
    assert_contains(&module("\u{feff}module m\r\n"), "(module m");
}

#[test]
fn a_file_without_a_final_terminator_parses_in_every_form() {
    // 1.1: "A final line terminator is optional."
    assert_contains(
        &module("module m\n\nfn f() -> Int {\n    1\n}"),
        "(block (tail (int 1)))",
    );
    assert_contains(&module("module m\nfn f() -> Unit {}"), "(block))");
    assert_contains(
        &module("module m\n\nstruct S {\n    x: Int\n}"),
        "(field x (ty Int))",
    );
    assert_contains(
        &module("module m\ntest \"t\" { assert true }"),
        "(assert (bool true))",
    );
    assert_contains(
        &work("change c {\n    base git(\"a\")\n    target git(\"b\")\n}"),
        "(change c (base \"a\") (target \"b\"))",
    );
    assert_contains(&work("change c {}"), "(change c)");
    assert_contains(
        &patch("patch x\n    expect base(x)\n{\n    1\n}"),
        "(block (tail (int 1)))",
    );
    assert_contains(
        &patch("patch x\n    expect base(x)\n{}"),
        "(expect x) (block))",
    );
    // Trailing whitespace after the last token, with and without terminators.
    assert_contains(
        &module("module m\nfn f() -> Int { 1 }   "),
        "(tail (int 1))",
    );
    assert_contains(&module("module m\nfn f() -> Int { 1 }\t"), "(tail (int 1))");
    assert_contains(
        &module("module m\nfn f() -> Int { 1 }\n\n   \n\t\n"),
        "(tail (int 1))",
    );
    // A `.cpl` file whose last token is on the last line with no terminator and a `use`.
    assert_contains(&module("module m\nuse a.b.{c}"), "(use a.b (items c))");
}

#[test]
fn an_unterminated_body_at_end_of_file_is_an_error_not_a_panic() {
    // 2.3: EOF terminates the statement, but the block still needs its `}`.
    assert_code(
        &module_errors("module m\nfn f() -> Int {\n    1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\nfn f() -> Int {\n    1\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\nstruct S {\n    x: Int"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\nfn f() -> Int {\n    match x {\n        A => 1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    base git(\"a\")"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch x\n    expect base(x)\n{\n    1"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn tabs_indent_and_separate_tokens_without_changing_the_parse() {
    // 1.1: indentation is never significant; tabs are plain whitespace.
    let spaces = "module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n{\n    let y = x\n    if y > 0 {\n        y\n    } else {\n        0\n    }\n}\n";
    let tabs = "module m\n\nfn f(x: Int) -> Int\n\trequires x > 0\n{\n\tlet y = x\n\tif y > 0 {\n\t\ty\n\t} else {\n\t\t0\n\t}\n}\n";
    assert_eq!(module(spaces), module(tabs));
    // Tabs between tokens, before a comment, at the end of a line, and mixed with spaces.
    assert_contains(
        &module("module\tm\nfn\tf()\t->\tInt\t{\t// c\n  \t  let\tx\t=\t1\t\n\t  x\t\n}\t\n"),
        "(block (let x (int 1)",
    );
    // A tab-only line between statements is a blank line.
    let m = module_ast("module m\nfn f() -> Int {\n    let x = 1\n\t\n    x\n}\n");
    let DeclKind::Fn(f) = &m.declarations[0].kind else {
        panic!()
    };
    assert!(
        f.body
            .as_ref()
            .unwrap()
            .tail
            .as_ref()
            .unwrap()
            .trivia
            .blank_line_before
    );
    // Tabs in .work and .patch files.
    assert_contains(
        &work("change c {\n\tbase git(\"a\")\n\ttask t {\n\t\twrite {\n\t\t\ta.body,\n\t\t}\n\t}\n}\n"),
        "(task t (write a.body))",
    );
    assert_contains(
        &patch("patch x\n\texpect base(x)\n{\n\t1\n}\n"),
        "(block (tail (int 1)))",
    );
}

#[test]
fn comments_at_the_end_of_a_file_are_accepted_in_every_position() {
    // 1.2 / 2.1: an ordinary comment produces no token, so a comment after
    // the last declaration, with or without a final terminator, ends nothing.
    let m = module_ast("module m\nfn f() -> Int { 1 }\n// one\n// two");
    assert_eq!(m.trailing_comments.len(), 2);
    let m = module_ast("module m\nfn f() -> Int { 1 }// glued");
    assert_eq!(m.declarations[0].trivia.trailing[0].text, "// glued");
    let m = module_ast("module m\nfn f() -> Int { 1 } // same line");
    assert_eq!(m.declarations[0].trivia.trailing[0].text, "// same line");
    let m = module_ast("module m // only a header\n// and a comment");
    assert_eq!(m.trivia.trailing[0].text, "// only a header");
    assert_eq!(m.trailing_comments[0].text, "// and a comment");
    let m = module_ast("module m\nuse a.b\n// after the import");
    assert_eq!(m.trailing_comments[0].text, "// after the import");
    // Banner comments (D60) and non-ASCII comments (D1) at EOF.
    let m = module_ast("module m\n//// banner ////");
    assert_eq!(m.trailing_comments[0].text, "//// banner ////");
    let m = module_ast("module m\n// héllo 漢字");
    assert_eq!(m.trailing_comments[0].text, "// héllo 漢字");
    // CRLF-terminated comment at EOF, and a comment-only last line in .work/.patch.
    let m = module_ast("module m\r\n// end\r\n");
    assert_eq!(m.trailing_comments[0].text, "// end");
    assert_contains(
        &work("change c {}\n// end"),
        "(change c) (trailing-comments \"// end\")",
    );
    assert_contains(
        &patch("patch x\n    expect base(x)\n{\n}\n// end"),
        "(trailing-comments \"// end\")",
    );
    // A comment at the end of a block before `}`, without a newline before `}`.
    assert_contains(
        &body("let x = 1\nx\n// dangling"),
        "(trailing-comments \"// dangling\")",
    );
}

#[test]
fn a_doc_comment_at_the_end_of_a_file_is_an_error() {
    // 1.2: a DOC_COMMENT "at end of file with nothing following" is
    // `doc comment must precede a declaration`.
    for src in [
        "module m\nfn f() -> Int { 1 }\n/// dangling",
        "module m\nfn f() -> Int { 1 }\n/// dangling\n",
        "module m\nfn f() -> Int { 1 }\n/// dangling\n\n// plain after it\n",
        "module m\n///",
        "module m\r\n/// dangling\r\n",
    ] {
        let e = module_errors(src);
        assert!(
            has_code(&e, codes::DOC_COMMENT_POSITION),
            "{src:?}: {:?}",
            e.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
        );
    }
    let e = work_errors("change c {}\n/// dangling\n");
    assert_code(&e, codes::DOC_COMMENT_POSITION);
    let e = patch_errors("patch x\n    expect base(x)\n{\n}\n/// dangling\n");
    assert_code(&e, codes::DOC_COMMENT_POSITION);
    // A doc comment followed only by blank lines inside a `.work` change body.
    let e = work_errors("change c {\n    base git(\"a\")\n    /// dangling\n\n}\n");
    assert_code(&e, codes::DOC_COMMENT_POSITION);
}

#[test]
fn comment_only_and_whitespace_only_files_are_errors_without_panics() {
    // 3.1: the module header is mandatory; 4.1: at least one change; 4.5: at
    // least one patch form.
    for src in [
        "",
        "\n",
        "   ",
        "\t\t",
        "// c",
        "// c\n",
        "\n\n// a\n// b\n\n",
        "\r\n",
        "\u{feff}",
        "//// banner\n",
    ] {
        let r = parse_module(src, FileId(0));
        assert!(
            r.value.is_none() && has_code(&r.diagnostics, codes::SYNTAX_ERROR),
            "cpl {src:?}"
        );
        let r = parse_work(src, FileId(0));
        assert!(
            r.value.is_none() && has_code(&r.diagnostics, codes::SYNTAX_ERROR),
            "work {src:?}"
        );
        let r = parse_patch(src, FileId(0));
        assert!(
            r.value.is_none() && has_code(&r.diagnostics, codes::SYNTAX_ERROR),
            "patch {src:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Declaration prefix layout (1.2, 3.3): doc comments and annotations
// ---------------------------------------------------------------------------

#[test]
fn doc_comments_and_annotations_tolerate_blank_and_comment_lines_before_the_keyword() {
    // 2.1: no NEWLINE after a doc comment, nor for blank lines after it; 3.3:
    // an annotation ends with NEWLINE, into which blank/comment lines collapse.
    let m = module_ast("module m\n\n/// docs\n\n\n// note\n\n@id(\"x\")\n\n// another\n\n@prove\n\npub fn f() -> Int { 1 }\n");
    let d = &m.declarations[0];
    assert_eq!(d.docs.as_ref().map(|x| x.text()), Some("docs".to_string()));
    assert_eq!(d.annotations.len(), 2);
    assert!(d.is_pub());
    // Doc comments on members with blank lines after them.
    let d = module("module m\n\nstruct S {\n    /// the id\n\n    id: Id\n}\n\nenum E {\n    /// a\n\n    A\n}\n\ncapability C {\n    /// get\n\n    fn get(id: Id) -> Int\n}\n");
    assert_contains(&d, "(field id (ty Id) (docs \"the id\")");
    assert_contains(&d, "(variant A (docs \"a\")");
    assert_contains(
        &d,
        "(op get (params (param id (ty Id))) (ret (ty Int)) (docs \"get\")",
    );
    // In `.work` and `.patch` files.
    assert_contains(
        &work("/// change\n\nchange c {\n    /// task\n\n    task t {\n    }\n}\n"),
        "(change c (docs \"change\") (task t (docs \"task\"))",
    );
    assert_contains(
        &patch("/// form\n\npatch x\n    expect base(x)\n{\n}\n"),
        "(patch x (docs \"form\")",
    );
}

#[test]
fn an_annotation_must_occupy_its_own_line_and_be_followed_by_a_declaration() {
    // 3.3: `annotation = "@" LOWER_IDENT ( "(" commas(literal) ")" )? NEWLINE`.
    assert_code(
        &module_errors("module m\n\n@prove fn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\n@id(\"x\") @prove\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_errors("module m\n\n@prove"), codes::SYNTAX_ERROR);
    assert_code(&module_errors("module m\n\n@prove\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_errors("module m\n\n@prove\n// nothing follows\n"),
        codes::SYNTAX_ERROR,
    );
    // The argument list is bracketed, so it may span lines (2.2).
    assert_contains(
        &module("module m\n\n@id(\n    \"x\",\n)\nfn f() -> Int { 1 }\n"),
        "(annotation id (text \"x\"))",
    );
    // `@id` then NEWLINE then `(` is an annotation without arguments followed
    // by a line that cannot start a declaration.
    assert_code(
        &module_errors("module m\n\n@id\n(\"x\")\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
}

// ---------------------------------------------------------------------------
// No-panic sweep over layout-shaped fragments
// ---------------------------------------------------------------------------

const LAYOUT_FRAGMENTS: &[&str] = &[
    "module m\nfn f() -> Int\n{",
    "module m\nfn f() -> Int\n    requires",
    "module m\nfn f() -> Int\n    requires x\n",
    "module m\nfn f() -> Int\n    requires x\n{",
    "module m\nfn f() -> Int\n    effects {\n",
    "module m\nfn f() -> Int\n    effects {\n    db.get,\n",
    "module m\nfn f() -> Int\n\n\n",
    "module m\nfn f() -> Int {\n    while c\n        invariant",
    "module m\nfn f() -> Int {\n    while c\n        invariant x\n",
    "module m\nfn f() -> Int {\n    while c\n",
    "module m\nfn f() -> Int {\n    if c\n",
    "module m\nfn f() -> Int {\n    if c\n{",
    "module m\nfn f() -> Int {\n    if c { 1 } else\n",
    "module m\nfn f() -> Int {\n    if c { 1 } else\n{",
    "module m\nfn f() -> Int {\n    if c { 1 }\n    else",
    "module m\nfn f() -> Int {\n    else\n}",
    "module m\nfn f() -> Int {\n    match x\n",
    "module m\nfn f() -> Int {\n    match x\n{\n    A =>",
    "module m\nfn f() -> Int {\n    match x {\n        A =>\n",
    "module m\nfn f() -> Int {\n    match x {\n        =>\n",
    "module m\nfn f() -> Int {\n    [\n",
    "module m\nfn f() -> Int {\n    (\n",
    "module m\nfn f() -> Int {\n    Page {\n",
    "module m\nfn f() -> Int {\n    Page {\n    ..\n",
    "module m\nfn f() -> Int {\n    f(\n    ,\n",
    "module m\nfn f() -> Int {\n    let x = a +\n",
    "module m\nfn f() -> Int {\n    let x = a +\n\n\n",
    "module m\nfn f() -> Int {\n    let x =\n}",
    "module m\nfn f() -> Int {\n    x\n    .len\n}",
    "module m\nfn f() -> Int {\n    return\n",
    "module m\nfn f() -> Int {\n    return",
    "module m\nfn f() -> Int {\n    1 }",
    "module m\nfn f() -> Int { 1\n",
    "module m\nstruct S\n",
    "module m\nstruct S\n\n\n{",
    "module m\nenum E\n{\n    A\n    ,",
    "module m\ncapability C\n{\n    fn",
    "module m\ntest \"t\"\n",
    "module m\nproperty \"p\"\n    for",
    "module m\nproperty \"p\"\n    for x in xs\n",
    "module m\n@prove\n",
    "module m\n@prove\n\n\n",
    "module m\n/// d\n",
    "module m\n/// d\n\n\n",
    "module m\n///\n///\n///",
    "module m\nuse a.b.\n{c}",
    "module m\nuse a.b.{\n",
    "module m\nuse a.b.{\n    c\n",
    "module\nm",
    "module m\n.n",
    "change c\n",
    "change c\n{",
    "change c {\n    task t after\n",
    "change c {\n    task t after a,\n",
    "change c {\n    task t after a\n    {",
    "change c {\n    task t {\n        write {\n",
    "change c {\n    task t {\n        write {\n            a.\n",
    "change c {\n    integrate\n",
    "change c {\n    integrate\n{",
    "change c {\n    integrate {\n        require\n",
    "change c {\n    base git(\n",
    "change c {\n    base git(\n\"a\"\n",
    "change c {\n    workers\n",
    "patch x\n    expect",
    "patch x\n    expect base",
    "patch x\n    expect base(",
    "patch x\n    expect base(x)\n",
    "patch x\n    expect base(x)\n\n\n",
    "patch x\n    expect base(x)\n{\n    let x = a +\n",
    "patch x\n\n\n    expect base(x)\n{\n}",
    "\r\n\r\n\r\n",
    "\r",
    "\r\r\r",
    "module m\r",
    "module m\rfn f() -> Int { 1 }\r",
    "\t\t\t\n\t\t\t",
    "//",
    "//\n//\n//",
    "///\r\n",
    "\n\n\n\n\n\n\n\n\n\n",
    "{\n\n\n",
    "}\n\n\n",
    "module m\nfn f() -> Int {\n\n\n\n",
    "module m\nfn f() -> Int {\n    let x = [\n\n\n",
    "module m\nfn f() -> Int {\n    let x = (\n\n\n",
    "module m\nfn f() -> Int {\n    let x = Page {\n\n\n",
    "module m\nfn f() -> Int { 1 }\n\n\n\n// c",
    "module m\nfn f() -> Int { 1 }\n\n\n\n/// c",
    "module m\nfn f(x: List<\n",
    "module m\nfn f() -> List<\nInt> { x }",
    "module m\nfn f<T,\nU>() -> Int { 1 }",
    "module m\nfn f() -> Int {\n    check<\n    Positive>(n)\n}",
    "module m\nfn f() -> Int {\n    a <\n}",
    "module m\nfn f() -> Int {\n    a < B\n    > (c)\n}",
];

#[test]
fn layout_fragments_never_panic_in_any_parser() {
    for (i, src) in LAYOUT_FRAGMENTS.iter().enumerate() {
        no_panic(src);
        no_panic(&src.replace('\n', "\r\n"));
        no_panic(&src.replace("    ", "\t"));
        let _ = i;
    }
}

#[test]
fn every_prefix_of_a_layout_heavy_module_parses_without_panicking() {
    let src = "/// docs\nmodule m // h\n\nuse a.b.{\n    c,\n}\n\n@id(\"x\")\n@prove\npub fn f(\n    a: Int,\n    using db: Db,\n) -> Result<List<Int>, E>\n    effects { db.get, }\n\n    // pre\n    requires a > 0\n    ensures (result\n        == result)\n{\n    var l = [\n        1,\n    ]\n    while a > 0\n        invariant a >= 0\n        decreases a\n    {\n        l = l.push(a)\n    }\n    let p = Page {\n        ..base,\n        text: if c { \"a\" } else { \"b\" },\n    }\n    match p {\n        Page => {\n            return Err(e)\n        }\n        _ => if c\n        {\n            1\n        } else if d {\n            2\n        } else {\n            3\n        }\n    }\n    Ok(l)\n}\n// end\n";
    assert!(
        parse_module(src, FileId(0)).diagnostics.is_empty(),
        "{}",
        render(src, &parse_module(src, FileId(0)).diagnostics)
    );
    let crlf = src.replace('\n', "\r\n");
    assert_eq!(module(src), module(&crlf));
    for end in src
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(src.len()))
    {
        no_panic(&src[..end]);
    }
    for end in crlf
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(crlf.len()))
    {
        no_panic(&crlf[..end]);
    }
}
