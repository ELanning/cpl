//! Adversarial tests, round 2, lens: newlines and layout.
//!
//! Every expectation encodes `spec/grammar.md` (edition 0.1), cited by
//! section or decision in the test's comment: 1.1 (terminators, horizontal
//! whitespace), 1.2 (doc comments), 1.8 step 2 and 2.1 (NEWLINE emission),
//! 2.2 (parser newline modes), 2.3 (statement termination, D11), 2.4
//! (header continuation, D12, D63), 2.5 (trailing commas), 3.11/D51
//! (blocks), 3.12 (`if`/`else`/`match` layout, D49, D55), 4.1-4.5 (`.work`
//! and `.patch` layout). Round 1 (`adversarial_layout_r1.rs`) already
//! covers the common shapes; this round probes the corners: constructs that
//! change meaning across a bracket boundary, doc comments in layout
//! positions, every statement form with a dangling operator, the diagnostic
//! span of a line-end error, generic headers with `{` on the next line,
//! `else` at line start in every significant-newline context, and the
//! canonical layout the formatter must restore (`spec/toolchain.md` 2.4,
//! which grammar 2.2 and 2.4 defer to). Where grammar.md is silent the
//! comment says so and names the design-doc example the test follows.
//!
//! Tests never assert the parser's current behaviour; a failing test marks a
//! parser defect or a spec issue reported with the round's results.
//!
//! Triage of the round (2026-10-29). Fifteen tests failed when the round's
//! session ended. One was a parser defect, now fixed:
//! `if_else_and_match_headers_followed_by_a_doc_comment_line_report_the_doc_comment`
//! (grammar 1.2 and 2.4 rule 3). The other fourteen asserted shapes the
//! specifications do not give, and were corrected with a `Triage:` comment
//! naming the section: a declaration, `use`, or clause preceded by a blank
//! line records it, and the dump prints `(trivia blank)` as the node's last
//! child (`spec/toolchain.md` 2.3, only the first node of a brace body or
//! file is exempt, and 12.1); a comment between a header and a `{` on the
//! next line is leading trivia of the first node inside the braces
//! (toolchain 2.3 rule C), and one after a patch target is the selector's
//! (rule B); a non-tail expression statement dumps as `(expr ...)` (12.1);
//! `else` before the `module` header is the generic syntax error for
//! `module` (grammar 2.4 rule 4); and an empty block prints `{}` (toolchain
//! 2.4 rule 8), on its own line after clauses (rule 7).

use cpl_diag::{DiagCode, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_syntax::ast::*;
use cpl_syntax::{
    codes, dump_module, dump_patch, dump_work, format_source, lex, parse_module, parse_patch,
    parse_work, FileKind, TokenKind as T,
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

fn module(src: &str) -> String {
    flat(&dump_module(&module_ast(src)))
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
        "expected errors for:\n{src}"
    );
    check_codes(&r.diagnostics);
    assert_eq!(
        r.diagnostics.len(),
        1,
        "D59: one diagnostic\n{src}\n{:#?}",
        r.diagnostics
    );
    r.diagnostics
}

fn body_errors(stmts: &str) -> Vec<Diagnostic> {
    module_errors(&wrap(stmts))
}

fn work_ast(src: &str) -> WorkFile {
    let r = parse_work(src, FileId(0));
    match r.value {
        Some(w) if r.diagnostics.is_empty() => w,
        _ => panic!(
            "expected a clean .work parse of:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn work(src: &str) -> String {
    flat(&dump_work(&work_ast(src)))
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

fn patch_ast(src: &str) -> PatchFile {
    let r = parse_patch(src, FileId(0));
    match r.value {
        Some(p) if r.diagnostics.is_empty() => p,
        _ => panic!(
            "expected a clean .patch parse of:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn patch(src: &str) -> String {
    flat(&dump_patch(&patch_ast(src)))
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

fn field<'a>(d: &'a Diagnostic, key: &str) -> Option<&'a str> {
    d.data.get(key).and_then(|v| v.as_str())
}

fn kinds(src: &str) -> Vec<T> {
    let (tokens, diagnostics) = lex(src, FileId(0));
    assert!(
        diagnostics.is_empty(),
        "lexical errors for {src:?}: {diagnostics:#?}"
    );
    tokens.into_iter().map(|t| t.kind).collect()
}

fn fmt_cpl(src: &str) -> String {
    format_source(src, FileKind::Cpl, FileId(0))
        .unwrap_or_else(|e| panic!("format failed for:\n{src}\n{}", render(src, &e)))
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
    for r in [&m.diagnostics, &w.diagnostics, &p.diagnostics] {
        assert!(
            r.len() <= 1,
            "D59: at most one diagnostic for {src:?}: {r:#?}"
        );
    }
    let (tokens, _) = lex(src, FileId(0));
    assert_eq!(tokens.last().map(|t| t.kind), Some(T::Eof), "{src:?}");
}

/// The same module with CRLF terminators must give the same dump (1.1).
#[track_caller]
fn assert_crlf_invariant(src: &str) {
    let lf = module(src);
    let crlf = module(&src.replace('\n', "\r\n"));
    assert_eq!(lf, crlf, "CRLF changed the parse of:\n{src}");
}

fn fn_of(m: &Module, i: usize) -> &FnDecl {
    match &m.declarations[i].kind {
        DeclKind::Fn(f) => f,
        other => panic!("expected a fn, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 1.8 step 2 / 2.1: NEWLINE emission is a lexer matter, modes are the parser's
// ---------------------------------------------------------------------------

#[test]
fn the_lexer_emits_newlines_inside_brackets_too() {
    // 2.2: "the mode is decided by the production being parsed, never by
    // the lexer" - so the token stream carries NEWLINE inside `(`, `[`, `{`
    // and the parser discards them.
    assert_eq!(
        kinds("f(\n1\n)"),
        vec![
            T::LowerIdent,
            T::LParen,
            T::Newline,
            T::Int,
            T::Newline,
            T::RParen,
            T::Eof
        ]
    );
    assert_eq!(
        kinds("[\n\n]"),
        vec![T::LBracket, T::Newline, T::RBracket, T::Eof]
    );
    assert_eq!(
        kinds("Page {\n..p\n}"),
        vec![
            T::UpperIdent,
            T::LBrace,
            T::Newline,
            T::DotDot,
            T::LowerIdent,
            T::Newline,
            T::RBrace,
            T::Eof
        ]
    );
}

#[test]
fn a_newline_is_emitted_before_a_doc_comment_but_never_after_one() {
    // 1.8 step 2: the token before the terminator is an identifier, so the
    // NEWLINE before the doc comment is emitted; the one after it is not.
    assert_eq!(
        kinds("a\n\n/// d\n\nfn"),
        vec![T::LowerIdent, T::Newline, T::DocComment, T::KwFn, T::Eof]
    );
    assert_eq!(
        kinds("a\n/// d"),
        vec![T::LowerIdent, T::Newline, T::DocComment, T::Eof]
    );
    // CRLF after the doc comment, then a comment-only line, then the keyword.
    assert_eq!(
        kinds("/// d\r\n// c\r\n\r\nstruct"),
        vec![T::DocComment, T::KwStruct, T::Eof]
    );
}

#[test]
fn a_doc_comment_after_a_bom_or_indentation_begins_its_line() {
    // 1.1: the BOM is skipped; 1.2: a `///` that is the first non-whitespace
    // on its line is a doc comment.
    assert_eq!(
        kinds("\u{feff}/// d\nmodule"),
        vec![T::DocComment, T::KwModule, T::Eof]
    );
    assert_eq!(
        kinds("\n\n\t/// d\nmodule"),
        vec![T::DocComment, T::KwModule, T::Eof]
    );
    let m = module_ast("\u{feff}/// d\nmodule m\n");
    assert_eq!(m.docs.as_ref().map(|d| d.text()), Some("d".to_string()));
    let m = module_ast("\t/// d\n\tmodule m\n");
    assert_eq!(m.docs.as_ref().map(|d| d.text()), Some("d".to_string()));
}

#[test]
fn an_ordinary_comment_or_blank_line_between_two_doc_comments_does_not_split_the_docs() {
    // 2.1: no NEWLINE after a DOC_COMMENT nor for comment-only or blank
    // lines after it, so `/// a`, `// plain`, `/// b` are two adjacent
    // DOC_COMMENT tokens and form one docs value (1.2: joined with `\n`).
    let m = module_ast("/// a\n// plain\n/// b\nmodule m\n");
    assert_eq!(m.docs.as_ref().map(|d| d.text()), Some("a\nb".to_string()));
    let m = module_ast("module m\n\n/// a\n\n// plain\n\n/// b\nfn f() -> Int { 1 }\n");
    assert_eq!(
        m.declarations[0].docs.as_ref().map(|d| d.text()),
        Some("a\nb".to_string())
    );
    // On a member too.
    let d = module("module m\n\nstruct S {\n    /// a\n    // plain\n    /// b\n    x: Int\n}\n");
    assert_contains(&d, "(field x (ty Int) (docs \"a\" \"b\")");
}

#[test]
fn a_doc_comment_strips_exactly_one_leading_space_and_keeps_a_tab() {
    // 1.2: "the text after `///` with one leading space removed if present".
    let m = module_ast("///\tdoc\n///  two\n///\n/// x\nmodule m\n");
    let lines: Vec<&str> = m
        .docs
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(lines, ["\tdoc", " two", "", "x"]);
}

#[test]
fn only_space_and_tab_are_horizontal_whitespace() {
    // 1.1: horizontal whitespace is U+0020 and U+0009; anything else outside
    // a text literal is a lexical error (1.6 `unexpected character`, or 1.1
    // non-ASCII).
    assert_code(&module_errors("module m\u{c}\n"), codes::INVALID_CHARACTER);
    assert_code(&module_errors("module m\u{b}\n"), codes::INVALID_CHARACTER);
    assert_code(
        &module_errors("module\u{a0}m\n"),
        codes::NON_ASCII_OUTSIDE_TEXT,
    );
    assert_code(&body_errors("let x =\u{c}1\nx"), codes::INVALID_CHARACTER);
    // A raw tab inside a text literal is permitted (D62 excludes U+0009).
    let (tokens, diags) = lex("\"a\tb\"", FileId(0));
    assert!(diags.is_empty(), "{diags:#?}");
    assert_eq!(tokens[0].kind, T::Text);
    assert_eq!(tokens[0].text, "a\tb");
    assert_contains(&body("let x = \"a\tb\"\nx"), "(let x (text \"a\\tb\"))");
}

#[test]
fn a_lone_cr_is_reported_wherever_it_appears() {
    // 1.1: a CR not followed by LF is a lexical error, also at end of file.
    assert_code(&module_errors("module m\r"), codes::INVALID_LINE_TERMINATOR);
    assert_code(
        &module_errors("module m\rfn f() -> Int { 1 }"),
        codes::INVALID_LINE_TERMINATOR,
    );
    assert_code(
        &work_errors("change c {\r}\n"),
        codes::INVALID_LINE_TERMINATOR,
    );
    // D62: a raw U+000D inside a text literal is `unterminated text literal`,
    // and (D59) it is the earliest error of the file, so the one reported.
    let e = module_errors("module m\n\nfn f() -> Int {\n    let x = \"a\rb\"\n    x\n}\n");
    assert_eq!(e[0].code, codes::UNTERMINATED_TEXT_LITERAL, "{e:#?}");
}

#[test]
fn the_newline_token_of_a_crlf_covers_both_bytes() {
    // 1.1: CRLF is one terminator; the token records its byte span.
    let (tokens, diags) = lex("a\r\nb", FileId(0));
    assert!(diags.is_empty());
    assert_eq!(tokens[1].kind, T::Newline);
    assert_eq!((tokens[1].span.start, tokens[1].span.end), (1, 3));
    let (tokens, _) = lex("a\nb", FileId(0));
    assert_eq!((tokens[1].span.start, tokens[1].span.end), (1, 2));
}

#[test]
fn a_bom_followed_by_blank_or_comment_lines_then_the_header() {
    // 1.1 + 1.8 step 2: BOM skipped, leading terminators emit no NEWLINE.
    assert_contains(&module("\u{feff}\r\n\r\nmodule m\r\n"), "(module m");
    assert_contains(&module("\u{feff}\n// c\n\nmodule m"), "(module m");
    assert_contains(&module("\u{feff}\t\n\tmodule m\n"), "(module m");
}

#[test]
fn only_double_slash_comments_exist() {
    // 1.2: comments are `//` to end of line; `/*`, `#` and friends are
    // operator tokens or unexpected characters, and `//` inside a text
    // literal is text.
    assert_code(&body_errors("let x = 1 /* c */\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("# c\n1"), codes::INVALID_CHARACTER);
    assert_contains(
        &body("let x = \"http://x\" // real comment\nx"),
        "(let x (text \"http://x\")",
    );
    assert_contains(&body("let x = \"//\"\nx"), "(let x (text \"//\"))");
}

// ---------------------------------------------------------------------------
// 2.2: constructs that change meaning across a bracket boundary
// ---------------------------------------------------------------------------

#[test]
fn a_struct_literal_brace_on_the_next_line_is_accepted_inside_brackets() {
    // 2.2: inside `(`, `[` the NEWLINE is discarded before the parser sees
    // it, so `Page` NEWLINE `{` is the struct literal (3.12 rule 2); at
    // statement level the same text is an error (2.3, round 1).
    assert_contains(
        &body("let x = f(Page\n{ ..p })\nx"),
        "(let x (call (name f) (struct-lit Page (spread (name p)))))",
    );
    assert_contains(
        &body("let x = [Page\n{ ..p }]\nx"),
        "(let x (list (struct-lit Page (spread (name p)))))",
    );
    assert_contains(
        &body("let x = (Page\n\n\n{ ..p })\nx"),
        "(let x (paren (struct-lit Page (spread (name p)))))",
    );
    // A qualified type path split around the newline inside parentheses.
    assert_contains(
        &body("let x = (a.b.Page\n{ text: t })\nx"),
        "(let x (paren (struct-lit a.b.Page (field text (name t)))))",
    );
}

#[test]
fn a_call_paren_on_the_next_line_is_a_call_inside_brackets() {
    // 2.2 / 2.3: outside brackets `f` NEWLINE `(1)` is two statements
    // (round 1); inside brackets it is the call `f(1)`.
    assert_contains(
        &body("let x = [f\n(1)]\nx"),
        "(let x (list (call (name f) (int 1))))",
    );
    assert_contains(
        &body("let x = (f\n(1))\nx"),
        "(let x (paren (call (name f) (int 1))))",
    );
    assert_contains(
        &body("let x = g(f\n(1))\nx"),
        "(let x (call (name g) (call (name f) (int 1))))",
    );
    assert_contains(
        &body("let x = g(h\n<Int>\n(1))\nx"),
        "(let x (call (name g) (call (name h) (generics (ty Int)) (int 1))))",
    );
}

#[test]
fn leading_dot_question_and_operator_lines_continue_inside_brackets() {
    // D11 forbids these continuations *outside* brackets; 2.2 discards the
    // NEWLINE inside them, so the same lines continue the expression.
    assert_contains(
        &body("let x = (a\n    .b\n    .c)\nx"),
        "(let x (paren (member (member (name a) b) c)))",
    );
    assert_contains(
        &body("let x = (f()\n    ?)\nx"),
        "(let x (paren (propagate (call (name f)))))",
    );
    assert_contains(
        &body("let x = [a\n    + b]\nx"),
        "(let x (list (+ (name a) (name b))))",
    );
    assert_contains(
        &body("let p = Page { text: a ++\n    b }\np"),
        "(let p (struct-lit Page (field text (++ (name a) (name b)))))",
    );
    assert_contains(
        &body("let x = (not\n    a)\nx"),
        "(let x (paren (not (name a))))",
    );
    assert_contains(
        &body("let x = (-\n    1)\nx"),
        "(let x (paren (neg (int 1))))",
    );
    assert_contains(
        &body("let x = f(a\n    .len\n    , b)\nx"),
        "(let x (call (name f) (member (name a) len) (name b)))",
    );
}

#[test]
fn forall_spans_lines_inside_parentheses() {
    // D45 + 2.2: the predicate extends "to the end of the line or the
    // enclosing bracket"; inside parentheses the line ends are discarded.
    assert_contains(
        &body("let x = (forall y in ys:\n    y > 0)\nx"),
        "(let x (paren (forall y (name ys) (> (name y) (int 0)))))",
    );
    assert_contains(
        &body("let x = (forall y in\n    ys\n    :\n    y > 0)\nx"),
        "(let x (paren (forall y (name ys) (> (name y) (int 0)))))",
    );
    assert_contains(
        &module("module m\n\nfn f(xs: List<Int>) -> Int\n    ensures (forall x in result:\n        x > 0)\n{\n    1\n}\n"),
        "(ensures (paren (forall x (name result) (> (name x) (int 0)))))",
    );
    assert_contains(
        &body("let x = [forall y in ys:\n    forall z in y:\n        z > 0]\nx"),
        "(list (forall y (name ys) (forall z (name y) (> (name z) (int 0)))))",
    );
}

#[test]
fn header_expressions_may_span_lines_inside_brackets() {
    // 2.3: a header expression ends at its line unless a bracket is open;
    // every header kind of the 2.4 table.
    assert_contains(
        &body("while (a\n    and b)\n{\n}\n1"),
        "(while (paren (and (name a) (name b))) (block))",
    );
    assert_contains(
        &body("while f(\n    a,\n) {\n}\n1"),
        "(while (call (name f) (name a)) (block))",
    );
    assert_contains(
        &body("match (x\n) {\n    _ => 1\n}"),
        "(match (paren (name x)) (arm _ (int 1)))",
    );
    assert_contains(
        &body("match f(\n    x,\n)\n{\n    _ => 1\n}"),
        "(match (call (name f) (name x)) (arm _ (int 1)))",
    );
    assert_contains(
        &module("module m\n\nproperty \"p\"\n    for x in gen.list(\n        gen.int,\n    )\n{\n    assert true\n}\n"),
        "(for x (call (member (name gen) list) (member (name gen) int)))",
    );
    assert_contains(
        &body("while c\n    decreases (x\n        - 1)\n{\n}\n1"),
        "(decreases (paren (- (name x) (int 1))))",
    );
    assert_contains(
        &module("module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        requires (id\n            != x)\n        ensures [result\n            ] == [1]\n}\n"),
        "(requires (paren (!= (name id) (name x)))) (ensures (== (list (name result)) (list (int 1))))",
    );
    assert_contains(
        &module("module m\n\npub type P = Int where f(\n    self,\n)\n"),
        "(refinement (ty Int) (where (call (name f) (name self))))",
    );
    assert_contains(
        &module("module m\n\nfn f(x: Int) -> Int\n    decreases [\n        x,\n    ].len\n{\n    x\n}\n"),
        "(decreases (member (list (name x)) len))",
    );
}

#[test]
fn struct_literals_inside_brackets_of_a_header_expression_span_lines() {
    // D41: struct literals are re-enabled inside any bracket within a header
    // expression; 2.2: those brackets also make the newlines insignificant.
    assert_contains(
        &body("if p == (Page {\n    ..q\n}) {\n    1\n}\n1"),
        "(if (== (name p) (paren (struct-lit Page (spread (name q))))) (block (tail (int 1))))",
    );
    assert_contains(
        &body("if xs == [Page {\n    ..q,\n}] {\n}\n1"),
        "(if (== (name xs) (list (struct-lit Page (spread (name q))))) (block))",
    );
    assert_contains(
        &body("while p == f(Page {\n    text: t,\n})\n{\n}\n1"),
        "(while (== (name p) (call (name f) (struct-lit Page (field text (name t))))) (block))",
    );
}

#[test]
fn significant_newlines_return_inside_bodies_nested_in_brackets_at_any_depth() {
    // 2.2: the stack is restored on every closing bracket; a match body or
    // block inside a struct literal inside an argument list has one arm or
    // statement per line again.
    assert_contains(
        &body("let x = f(Page { text: match y {\n    A => \"a\"\n    _ => \"b\"\n} })\nx"),
        "(call (name f) (struct-lit Page (field text (match (name y) (arm (ctor A) (text \"a\")) (arm _ (text \"b\"))))))",
    );
    assert_contains(
        &body("let x = f(Page { text: if c {\n    let y = \"a\"\n    y\n} else { \"b\" } })\nx"),
        "(field text (if (name c) (block (let y (text \"a\")) (tail (name y))) (else (block (tail (text \"b\"))))))",
    );
    assert_contains(
        &body("let x = [(g(if c {\n    1\n} else {\n    2\n}))]\nx"),
        "(list (paren (call (name g) (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))))",
    );
    // Four levels: paren, list, call, match body, block, paren again.
    assert_contains(
        &body("let x = ([f(match y {\n    A => {\n        let z = (1\n            + 2)\n        z\n    }\n})])\nx"),
        "(arm (ctor A) (block (let z (paren (+ (int 1) (int 2)))) (tail (name z))))",
    );
}

#[test]
fn else_on_its_own_line_inside_a_body_inside_brackets_is_misplaced() {
    // D12: wherever newlines are significant; a match body or block nested
    // inside brackets is significant again (2.2).
    assert_code(
        &body_errors("let x = f(match y {\n    A => if c { 1 }\n    else { 2 }\n})\nx"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("let x = [if c {\n    if d { 1 }\n    else { 2 }\n} else { 3 }]\nx"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors(
            "let p = Page { text: if c {\n    if d { a }\n    else { b }\n} else { c } }\np",
        ),
        codes::MISPLACED_ELSE,
    );
}

#[test]
fn two_items_on_one_line_inside_a_body_inside_brackets_are_an_error() {
    // 2.3 applies inside the nested significant context.
    assert_code(
        &body_errors("let x = f(match y {\n    A => 1 B => 2\n})\nx"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("let x = [if c { let a = 1 a }]\nx"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("let p = Page { text: if c { f() g() } else { b } }\np"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn doc_comments_inside_insignificant_contexts_are_misplaced() {
    // 1.2: a DOC_COMMENT anywhere other than the admitted positions is
    // `doc comment must precede a declaration` (code doc_comment_position).
    for src in [
        wrap("let x = [\n    /// d\n    1,\n]\nx"),
        wrap("let x = f(\n    /// d\n    a,\n)\nx"),
        wrap("let p = Page {\n    /// d\n    text: t,\n}\np"),
        wrap("let p = Page {\n    /// d\n    ..base,\n}\np"),
        wrap("let x = (\n    /// d\n    1\n)\nx"),
        "module m\n\nuse a.b.{\n    /// d\n    c,\n}\n".to_string(),
        "module m\n\nfn f(using db: Db) -> Int\n    effects {\n        /// d\n        db.get,\n    }\n{\n    1\n}\n".to_string(),
        "module m\n\nfn f(\n    /// d\n    x: Int,\n) -> Int { 1 }\n".to_string(),
        "module m\n\nenum E {\n    A(\n        /// d\n        Int,\n    )\n}\n".to_string(),
        "module m\n\n@id(\n    /// d\n    \"x\",\n)\nfn f() -> Int { 1 }\n".to_string(),
    ] {
        let e = module_errors(&src);
        assert_code(&e, codes::DOC_COMMENT_POSITION);
    }
    assert_code(
        &work_errors("change c {\n    task t {\n        write {\n            /// d\n            a.body,\n        }\n    }\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &work_errors("change c {\n    base git(\n        /// d\n        \"a\"\n    )\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &patch_errors("patch x\n    expect base(\n        /// d\n        x\n    )\n{\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn the_mode_stack_pops_exactly_once_per_closing_bracket() {
    // 2.2: `)` `]` `}` pop the matching mode; after the outermost closes,
    // the NEWLINE ends the statement again.
    assert_contains(
        &body("let x = [(1\n)]\nlet y = 2\nx"),
        "(let x (list (paren (int 1)))) (let y (int 2))",
    );
    assert_contains(
        &body("let x = f([\n])\nlet y = 2\nx"),
        "(let x (call (name f) (list))) (let y (int 2))",
    );
    assert_contains(
        &body("let p = Page { a: [\n] }\nlet y = 2\np"),
        "(let p (struct-lit Page (field a (list)))) (let y (int 2))",
    );
    // Mismatched or unclosed brackets.
    assert_code(&body_errors("let x = (1]\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("let x = [1)\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("let x = Page { ..p )\nx"), codes::SYNTAX_ERROR);
    // An unclosed paren swallows the following lines (they are no longer
    // statements), so the error is at `let` on the next line.
    let e = body_errors("let x = (1\nlet y = 2\nx");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert_eq!(field(&e[0], "found"), Some("`let`"), "{e:#?}");
}

// ---------------------------------------------------------------------------
// 2.3 / D11: statement termination in every statement and clause form
// ---------------------------------------------------------------------------

#[test]
fn a_binary_operator_at_end_of_line_ends_every_statement_and_clause_form() {
    // D11: no operator continuation anywhere outside brackets; the message
    // is the one 2.3 quotes.
    for stmts in [
        "var x = a +\n    b\nx",
        "x = a +\n    b\nx",
        "assert a ==\n    b\n1",
        "return a +\n    b",
        "let x = a or\n    b\nx",
        "let _ = a *\n    b\nx",
        "while a and\n    b {\n}\n1",
        "while c\n    invariant a or\n        b\n{\n}\n1",
        "while c\n    decreases x -\n        1\n{\n}\n1",
        "match a +\n    b {\n    _ => 1\n}",
        "f(a) ++\n    [b]",
        "let x = f()? +\n    1\nx",
    ] {
        let e = body_errors(stmts);
        assert!(
            has_code(&e, codes::SYNTAX_ERROR)
                && has_message(&e, "expected expression, found end of line"),
            "{stmts:?}: {:?}",
            e.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
    for src in [
        "module m\n\npub type P = Int where self > 0 and\n    self < 9\n",
        "module m\n\nproperty \"p\"\n    for x in xs ++\n        ys\n{\n}\n",
        "module m\n\nfn f(x: Int) -> Int\n    decreases x -\n        1\n{\n    x\n}\n",
        "module m\n\nfn f(x: Int) -> Int\n    ensures result ==\n        x\n{\n    x\n}\n",
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        requires id !=\n            x\n}\n",
    ] {
        let e = module_errors(src);
        assert!(
            has_code(&e, codes::SYNTAX_ERROR)
                && has_message(&e, "expected expression, found end of line"),
            "{src:?}: {:?}",
            e.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_line_end_error_points_at_the_terminator_and_carries_expected_and_found() {
    // D59: a syntax error carries the span of the offending token and the
    // expected set; 2.3: the offending token is the NEWLINE (the terminator).
    let src = wrap("let x = a +\n    b\nx");
    let e = module_errors(&src);
    let plus = src.find('+').unwrap();
    assert_eq!(e[0].span.unwrap().start as usize, plus + 1, "{e:#?}");
    assert_eq!(e[0].span.unwrap().end as usize, plus + 2, "{e:#?}");
    assert_eq!(field(&e[0], "found"), Some("end of line"));
    assert!(field(&e[0], "expected").is_some(), "{e:#?}");
    // With CRLF the terminator is two bytes (1.1: one terminator).
    let src = wrap("let x = a +\n    b\nx").replace('\n', "\r\n");
    let e = module_errors(&src);
    let plus = src.find('+').unwrap();
    assert_eq!(e[0].span.unwrap().start as usize, plus + 1, "{e:#?}");
    assert_eq!(e[0].span.unwrap().end as usize, plus + 3, "{e:#?}");
    assert_eq!(field(&e[0], "found"), Some("end of line"));
    // At end of file there is no terminator: the offending token is EOF.
    let e = module_errors("module m\n\nfn f() -> Int {\n    let x = a +");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert_eq!(field(&e[0], "found"), Some("end of file"), "{e:#?}");
    // Before a `}` on the same line the offending token is the brace.
    let e = module_errors("module m\n\nfn f() -> Int { let x = a + }\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert_eq!(field(&e[0], "found"), Some("`}`"), "{e:#?}");
}

#[test]
fn keywords_cannot_be_separated_from_their_operands_by_a_line_break() {
    // 3.11: the statement productions name no NEWLINE between a keyword, the
    // binder, `=`, and the expression.
    for stmts in [
        "let x\n    = 1\nx",
        "let x: Int\n    = 1\nx",
        "var x\n    = 1\nx",
        "let\n    x = 1\nx",
        "var\n    x = 1\nx",
        "x =\n    1\nx",
        "assert\n    a\n1",
        "let x: \n    Int = 1\nx",
        "let x =\n    if c { 1 } else { 2 }\nx",
    ] {
        assert_code(&body_errors(stmts), codes::SYNTAX_ERROR);
    }
    // `return` NEWLINE `(1)` is `return` without a value (D47) and then a
    // parenthesized expression statement: two statements.
    assert_contains(
        &body("return\n(1)"),
        "(block (return) (tail (paren (int 1))))",
    );
}

#[test]
fn a_line_that_cannot_begin_a_statement_after_a_complete_one() {
    // 2.3: `x` is a complete expression statement; `= 1`, `{}` (D28) and
    // `.len` (D11) cannot begin the next statement.
    assert_code(&body_errors("x\n    = 1\nx"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("y\n{}"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("f()\n{\n}\n1"), codes::SYNTAX_ERROR);
    // ... but `[0]` is a list literal and a legal expression statement, so
    // `let x = a` NEWLINE `[0]` is two statements (2.3), like `(1)` in round 1.
    assert_contains(
        &body("let x = a\n[0]\nx"),
        "(block (let x (name a)) (expr (list (int 0))) (tail (name x)))",
    );
    // 1.7: `x?` then a newline is propagation and the statement ends.
    assert_contains(
        &body("let x = f()?\ny"),
        "(block (let x (propagate (call (name f)))) (tail (name y)))",
    );
}

#[test]
fn a_statement_ends_at_a_closing_brace_so_nothing_may_follow_on_that_line() {
    // 2.3: a statement ends before a `}`; a second statement on the same
    // line as that `}` has no separator.
    assert_code(&body_errors("if c {\n} let y = 1\ny"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("match x {\n} y"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("while c {\n} x"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("if c { 1 } else { 2 } x"), codes::SYNTAX_ERROR);
    // `else` after the `}` of a *match* is not a line-initial `else` (D12
    // is about lines), just a token that cannot follow the statement.
    assert_code(&body_errors("match x {\n} else {\n}"), codes::SYNTAX_ERROR);
    // Nested blocks closing on one line are fine.
    assert_contains(
        &body("if c { if d { 1 } }"),
        "(if (name c) (block (tail (if (name d) (block (tail (int 1)))))))",
    );
    assert_contains(
        &module("module m\nfn f(p: Page) -> Page { Page { ..p } }\n"),
        "(block (tail (struct-lit Page (spread (name p)))))",
    );
    assert_contains(
        &body("match x { A => { 1 } }"),
        "(match (name x) (arm (ctor A) (block (tail (int 1)))))",
    );
}

#[test]
fn items_and_headers_end_at_the_line_in_every_file_form() {
    // 2.3 for items: the grammar names no NEWLINE inside `use_decl`,
    // `module_header`, `type_decl`, `fn` signatures, or `test` headers.
    assert_code(
        &module_errors("module m\nuse a.b.\n{c}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\nuse a.b\n.{c}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_errors("module\nm\n"), codes::SYNTAX_ERROR);
    assert_code(&module_errors("module m\n.n\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_errors("module m\n\npub\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn\nf() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f\n() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f()\n    -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f() ->\n    Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nstruct S\n<T> {\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\ntest\n\"t\" {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\ntype A\n(Text)\n"),
        codes::SYNTAX_ERROR,
    );
    // `type P = Int` ends at the line without `where` (D64 message).
    let e = module_errors("module m\n\ntype P = Int\n    where self > 0\n");
    assert!(
        has_code(&e, codes::SYNTAX_ERROR) && has_message(&e, "a type declaration is a wrapper"),
        "{e:#?}"
    );
    // `.work` and `.patch`.
    assert_code(
        &work_errors("change c {\n    task t after a\n    , b {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    integrate {\n        require test\n        (x)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch a\n    .body\n    expect base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch\na\n    expect base(a)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn clauses_cannot_follow_the_body() {
    // 2.4: clauses are continuation lines *before* the body; after the
    // closing `}` the declaration or statement is complete and a clause
    // keyword cannot begin an item or statement.
    assert_code(
        &module_errors("module m\n\nfn f(x: Int) -> Int { x }\n    requires x > 0\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nfn f(using db: Db) -> Int { 1 }\n    effects { db.get }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("while c {\n}\n    invariant x\n1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_errors("module m\n\nproperty \"p\"\n    for x in xs\n{\n}\n    for y in ys\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch x\n{\n}\n    expect base(x)\n"),
        codes::SYNTAX_ERROR,
    );
}

// ---------------------------------------------------------------------------
// 2.4: header continuation lines
// ---------------------------------------------------------------------------

#[test]
fn generic_headers_accept_the_brace_on_the_next_line() {
    // D63 / 3.6 / 3.7 / 3.9: `generic_params? NEWLINE? "{"`; the return type
    // may itself carry generic arguments before the line break.
    let d = module("module m\n\nstruct S<T>\n{\n    x: T\n}\n\nenum E<A, B>\n{\n    L(A)\n    R(B)\n}\n\nfn f<T>(x: T) -> List<List<T>>\n{\n    [[x]]\n}\n\npub fn g<T>(x: T) -> T\n\n    requires true\n\n{\n    x\n}\n");
    // Triage: every declaration here follows a blank line, and so does the
    // `requires` clause of `g`, so each records it and the dump ends the node
    // with `(trivia blank)` (toolchain 2.3, 12.1; the formatter prints no
    // blank line between clauses, 2.4 rule 4, but the tree records it).
    assert_contains(
        &d,
        "(struct S (generics T) (field x (ty T)) (trivia blank))",
    );
    assert_contains(
        &d,
        "(enum E (generics A B) (variant L (ty A)) (variant R (ty B)) (trivia blank))",
    );
    assert_contains(
        &d,
        "(fn f (generics T) (params (param x (ty T))) (ret (ty List (ty List (ty T)))) (block (tail (list (list (name x))))) (trivia blank))",
    );
    assert_contains(
        &d,
        "(fn g pub (generics T) (params (param x (ty T))) (ret (ty T)) (requires (bool true) (trivia blank)) (block (tail (name x))) (trivia blank))",
    );
    assert_crlf_invariant(
        "module m\n\nstruct S<T>\n{\n    x: T\n}\n\nfn f<T>(x: T) -> List<T>\n{\n    [x]\n}\n",
    );
}

#[test]
fn a_header_or_clause_ending_in_a_comment_still_continues() {
    // 2.1: an ordinary comment produces no token, so a comment at the end
    // of a header or clause line leaves exactly one NEWLINE before the
    // continuation.
    // Triage: where each comment goes is toolchain 2.3: one after the last
    // token of a header line followed by a clause is that clause's leading
    // comment (rule D), one followed by a `{` that begins the next line is
    // the leading comment of the first node inside the braces (rule C), one
    // after `{` on a header's line is the first statement's (rule D), and
    // one after a node's last token on its line is that node's trailing
    // comment (rule B; the patch target is a `Selector`, an attachable node).
    let d = module("module m\n\nfn f(x: Int) -> Int // header\n    requires x > 0 // pre\n    ensures result > 0 // post\n{ // open\n    x\n}\n");
    assert_contains(
        &d,
        "(requires (> (name x) (int 0)) (trivia (lead \"// header\") (trail \"// pre\")))",
    );
    assert_contains(
        &d,
        "(ensures (> (name result) (int 0)) (trivia (trail \"// post\")))",
    );
    assert_contains(&d, "(block (tail (name x) (trivia (lead \"// open\"))))");
    let d = work("change c // c\n{\n    task t after a, b // deps\n    {\n        goal \"g\"\n    }\n    integrate // i\n    {\n        require typecheck\n    }\n}\n");
    assert_contains(
        &d,
        "(task t (after a b) (item (goal \"g\") (trivia (lead \"// deps\"))) (trivia (lead \"// c\")))",
    );
    assert_contains(
        &d,
        "(integrate (require typecheck (trivia (lead \"// i\"))))",
    );
    let d = body("if c // c\n{\n    1\n} else // e\n{\n    2\n}");
    assert_contains(
        &d,
        "(if (name c) (block (tail (int 1) (trivia (lead \"// c\"))))",
    );
    assert_contains(&d, "(else (block (tail (int 2) (trivia (lead \"// e\")))))");
    let d = body("while c // spin\n    invariant x // inv\n{\n}\n1");
    assert_contains(
        &d,
        "(while (name c) (invariant (name x) (trivia (lead \"// spin\") (trail \"// inv\"))) (block))",
    );
    let d = patch("patch x // target\n    expect base(x) // base\n{ // body\n    1\n}\n");
    assert_contains(
        &d,
        "(patch (sel x (trivia (trail \"// target\"))) (expect x (trivia (trail \"// base\")))",
    );
    assert_contains(&d, "(block (tail (int 1) (trivia (lead \"// body\"))))");
}

#[test]
fn a_fn_with_clauses_may_be_bodiless_at_end_of_file_without_a_terminator() {
    // 3.9 third alternative + 1.1: EOF ends the clause and the declaration.
    assert_contains(
        &module("module m\nfn f(x: Int) -> Int\n    requires x > 0"),
        "(fn f (params (param x (ty Int))) (ret (ty Int)) (requires (> (name x) (int 0))) bodiless)",
    );
    assert_contains(
        &module("module m\nfn f(using db: Db) -> Int\n    effects {\n    }"),
        "(fn f (params (using db (ty Db))) (ret (ty Int)) (effects) bodiless)",
    );
    assert_contains(
        &module("module m\nfn f() -> Int\n    decreases 1 // c"),
        "(decreases (int 1) (trivia (trail \"// c\"))) bodiless)",
    );
    // A bodiless fn then a comment-only tail of the file.
    let m = module_ast("module m\n\nfn f() -> Int\n    requires true\n// after a bodiless fn\n");
    assert!(fn_of(&m, 0).is_bodiless());
    assert_eq!(m.trailing_comments.len(), 1);
}

#[test]
fn an_empty_effects_set_may_span_lines_before_the_body_brace() {
    // 2.2: the `effects` braces push Insignificant; 2.5: the set may be empty.
    // Triage: the function follows a blank line, so its dump ends with
    // `(trivia blank)` (toolchain 2.3, 12.1).
    assert_contains(
        &module("module m\n\nfn f(using db: Db) -> Int\n    effects {\n    }\n{\n    1\n}\n"),
        "(effects) (block (tail (int 1))) (trivia blank))",
    );
    assert_contains(
        &module("module m\n\nfn f(using db: Db) -> Int\n    effects {\n\n        // none\n\n    }\n\n{\n    1\n}\n"),
        "(effects (trailing-comments \"// none\")) (block (tail (int 1))) (trivia blank))",
    );
}

#[test]
fn capability_operations_take_only_requires_and_ensures_continuations() {
    // 2.4 table: an operation continues with `requires`/`ensures` only; 3.8:
    // no body, no effects.
    for src in [
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        decreases id\n}\n",
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n    {\n    }\n}\n",
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int {\n    }\n}\n",
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        invariant x\n}\n",
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        for x in xs\n}\n",
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        effects { a.b }\n}\n",
    ] {
        assert_code(&module_errors(src), codes::SYNTAX_ERROR);
    }
    // Two operations whose contracts are separated by blank and comment lines.
    // Triage: the `requires` clause follows a blank line and records it, and
    // so does `put` after its comment, whose own blank line is `lead-blank`
    // (toolchain 2.3, 12.1).
    let d = module("module m\n\ncapability C {\n    fn get(id: Id) -> Int\n\n        requires id != x\n\n    // second\n\n    fn put(id: Id) -> Int\n        ensures result > 0\n}\n");
    assert_contains(
        &d,
        "(op get (params (param id (ty Id))) (ret (ty Int)) (requires (!= (name id) (name x)) (trivia blank)))",
    );
    assert_contains(
        &d,
        "(op put (params (param id (ty Id))) (ret (ty Int)) (ensures (> (name result) (int 0))) (trivia blank (lead-blank \"// second\")))",
    );
}

#[test]
fn if_else_and_match_headers_followed_by_a_doc_comment_line_report_the_doc_comment() {
    // 2.3: the `NEWLINE?` before the `{` of an `if`, `else`, or `match`
    // header is admitted by the grammar and "decided by the Section 2.4
    // rule-3 lookahead", the same lookahead as `while`/`struct`/`test`
    // headers, for which round 1 fixed `while c` NEWLINE `/// x` NEWLINE
    // `{` to report the doc comment (1.2: `doc comment must precede a
    // declaration`, code doc_comment_position). The same shape after an
    // `if`, `else`, or `match` header must give the same diagnostic: the
    // NEWLINE is legal there and the doc comment is the offending token.
    // Triage: a parser defect (the three headers reported `expected `{`,
    // found end of line`), fixed in `Parser::expect_header_brace`; grammar
    // 1.2 also names "one between a header and its `{`".
    assert_code(
        &body_errors("if c\n/// d\n{\n}\n1"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &body_errors("if c { 1 } else\n/// d\n{ 2 }"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &body_errors("match x\n/// d\n{\n    _ => 1\n}"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &body_errors("let x = if c\n/// d\n{ 1 } else { 2 }\nx"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn every_other_header_followed_by_a_doc_comment_line_reports_the_doc_comment() {
    // 1.2 + 2.4 rule 3: the NEWLINE after a header is admitted; a doc
    // comment where the `{` or clause should be is the misplaced token.
    for src in [
        "module m\n\nstruct S\n/// d\n{\n}\n",
        "module m\n\nenum E\n/// d\n{\n    A\n}\n",
        "module m\n\ncapability C\n/// d\n{\n}\n",
        "module m\n\ntest \"t\"\n/// d\n{\n}\n",
        "module m\n\nproperty \"p\"\n    for x in xs\n/// d\n{\n}\n",
        "module m\n\nproperty \"p\"\n/// d\n    for x in xs\n{\n}\n",
        "module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n/// d\n{\n    x\n}\n",
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n    /// d\n        requires true\n}\n",
    ] {
        let e = module_errors(src);
        assert_code(&e, codes::DOC_COMMENT_POSITION);
    }
    for src in [
        "change c\n/// d\n{\n}\n",
        "change c {\n    task t\n/// d\n    {\n    }\n}\n",
        "change c {\n    task t after a\n/// d\n    {\n    }\n}\n",
        "change c {\n    integrate\n/// d\n    {\n    }\n}\n",
    ] {
        assert_code(&work_errors(src), codes::DOC_COMMENT_POSITION);
    }
    for src in [
        "patch x\n    expect base(x)\n/// d\n{\n}\n",
        "patch x\n/// d\n    expect base(x)\n{\n}\n",
    ] {
        assert_code(&patch_errors(src), codes::DOC_COMMENT_POSITION);
    }
    // A doc comment line between `}` and a line-initial `else`: the doc
    // comment comes first in the source, so it is the reported error (D59).
    assert_code(
        &body_errors("if c { 1 }\n/// d\nelse { 2 }"),
        codes::DOC_COMMENT_POSITION,
    );
    // A doc comment before `use` in a `.cpl` file (1.2 admits none there).
    assert_code(
        &module_errors("module m\n/// d\nuse a.b\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn an_if_header_inside_a_clause_or_arm_may_put_its_brace_on_the_next_line() {
    // 2.3: "the headers of `if`, `else`, and `match` may be followed by a
    // NEWLINE before their `{`" whether statement or operand; a clause
    // expression and an arm body are operand positions.
    assert_contains(
        &module("module m\n\nfn f(c: Bool) -> Int\n    requires if c\n    { true } else { false }\n{\n    1\n}\n"),
        "(requires (if (name c) (block (tail (bool true))) (else (block (tail (bool false))))))",
    );
    assert_contains(
        &body("match x {\n    A => if c\n    { 1 } else\n    { 2 }\n    B => match y\n    {\n        _ => 3\n    }\n}"),
        "(arm (ctor A) (if (name c) (block (tail (int 1))) (else (block (tail (int 2)))))) (arm (ctor B) (match (name y) (arm _ (int 3))))",
    );
    assert_contains(
        &body("assert if c\n{ true } else { false }\n1"),
        "(assert (if (name c) (block (tail (bool true))) (else (block (tail (bool false))))))",
    );
    assert_contains(
        &body("return match x\n{\n    _ => 1\n}"),
        "(return (match (name x) (arm _ (int 1))))",
    );
}

#[test]
fn an_upper_ident_header_expression_followed_by_a_brace_is_the_body() {
    // D41: struct literals are disabled at the top of a header expression,
    // so `Page` is a nullary constructor reference and the `{` (on the same
    // or the next line) opens the body.
    assert_contains(
        &body("if Page\n{\n    1\n}\n1"),
        "(if (name Page) (block (tail (int 1))))",
    );
    assert_contains(
        &body("match Page\n{\n    _ => 1\n}"),
        "(match (name Page) (arm _ (int 1)))",
    );
    assert_contains(
        &body("if p == Page {\n}\n1"),
        "(if (== (name p) (name Page)) (block))",
    );
    assert_contains(
        &body("while a.B\n{\n}\n1"),
        "(while (member (name a) B) (block))",
    );
}

#[test]
fn a_match_body_brace_on_the_next_line_with_arms_on_the_brace_lines() {
    // 3.12: `NEWLINE? "{" lines(match_arm) "}"`; `lines(X)` has optional
    // leading and trailing NEWLINEs.
    assert_contains(
        &body("match x\n{ A => 1\n    B => 2 }"),
        "(match (name x) (arm (ctor A) (int 1)) (arm (ctor B) (int 2)))",
    );
    assert_contains(
        &body("match x\n\n{\n\n    A => 1\n\n}"),
        "(match (name x) (arm (ctor A) (int 1)))",
    );
}

#[test]
fn continuation_keywords_never_continue_a_header_that_takes_none() {
    // 2.4 rule 3: only a continuation keyword *for this header* continues
    // it; `use`, `type`, `test`, `struct`, and `change` headers take none
    // (the `{`-only headers of the table take `{` only).
    for src in [
        "module m\n\nuse a.b\n    requires x\n",
        "module m\n\ntype A(Text)\n    ensures x\n",
        "module m\n\ntest \"t\"\n    ensures x\n{\n}\n",
        "module m\n\ntest \"t\"\n    effects { a.b }\n{\n}\n",
        "module m\n\nstruct S\n    requires x\n{\n}\n",
        "module m\n\nenum E\n    decreases x\n{\n}\n",
        "module m\n\ncapability C\n    requires x\n{\n}\n",
    ] {
        assert_code(&module_errors(src), codes::SYNTAX_ERROR);
    }
    for src in [
        "change c\n    after x\n{\n}\n",
        "change c {\n    task t\n        expect base(x)\n    {\n    }\n}\n",
        "change c {\n    integrate\n        requires x\n    {\n    }\n}\n",
    ] {
        assert_code(&work_errors(src), codes::SYNTAX_ERROR);
    }
    assert_code(
        &patch_errors("patch x\n    requires x\n    expect base(x)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_patch_expect_clause_may_span_lines_inside_its_parentheses() {
    // 4.5 + 2.2: `"(" selector ")"` opens an insignificant-newline context;
    // the `expect` keyword and `base` stay on the clause line.
    assert_contains(
        &patch("patch x.body\n    expect base(\n        x.body\n    )\n{\n}\n"),
        "(patch x.body (expect x.body) (block))",
    );
    assert_contains(
        &patch("patch x.body\n    expect base(x\n        .body)\n{\n    1\n}\n"),
        "(patch x.body (expect x.body) (block (tail (int 1))))",
    );
}

// ---------------------------------------------------------------------------
// D12: `else` at line start in every significant-newline context
// ---------------------------------------------------------------------------

#[test]
fn else_at_the_start_of_a_line_in_member_and_item_bodies_is_misplaced_else() {
    // D12 / 2.4 rule 4: "wherever newlines are significant" - member
    // bodies, `.work` bodies, and the top level of every file form (2.2).
    for src in [
        "module m\n\nstruct S {\n    else\n}\n",
        "module m\n\nenum E {\n    A\n    else\n}\n",
        "module m\n\ncapability C {\n    else\n}\n",
        "module m\nelse\n",
        "module m\n\nfn f() -> Int { 1 }\nelse {\n}\n",
    ] {
        assert_code(&module_errors(src), codes::MISPLACED_ELSE);
    }
    for src in [
        "else\n",
        "change c {\n    else\n}\n",
        "change c {\n    task t {\n        else\n    }\n}\n",
        "change c {\n    integrate {\n        else\n    }\n}\n",
        "change c {\n}\nelse\n",
    ] {
        assert_code(&work_errors(src), codes::MISPLACED_ELSE);
    }
    for src in [
        "else\n",
        "patch x\n    expect base(x)\n{\n    else\n}\n",
        "patch x\n    expect base(x)\n{\n}\nelse\n",
        "patch x\n    expect base(x)\n{\n    if c { 1 }\n    else { 2 }\n}\n",
    ] {
        assert_code(&patch_errors(src), codes::MISPLACED_ELSE);
    }
}

#[test]
fn else_where_one_particular_token_is_required_is_the_generic_syntax_error() {
    // Triage (the test was `else_before_the_module_header_is_misplaced_else`,
    // which expected `misplaced_else`): grammar 2.4 rule 4 names this case
    // explicitly, "Where the grammar requires one particular token instead
    // (the `module` header at the start of a `.cpl` file, the declaration
    // after an annotation line), a line beginning with `else` is the generic
    // syntax error for that token". The `.work` and `.patch` top levels
    // report `misplaced_else` because any item may begin a line there, while
    // a `.cpl` file must begin with its `module` header.
    for src in ["else\nmodule m\n", "else"] {
        let e = module_errors(src);
        assert_code(&e, codes::SYNTAX_ERROR);
        assert!(has_message(&e, "expected `module`"), "{e:#?}");
        assert_eq!(field(&e[0], "found"), Some("`else`"), "{e:#?}");
    }
    let e = module_errors("module m\n\n@prove\nelse\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert!(has_message(&e, "expected a declaration"), "{e:#?}");
    assert_eq!(field(&e[0], "found"), Some("`else`"), "{e:#?}");
}

#[test]
fn the_misplaced_else_diagnostic_points_at_the_else_token() {
    // D59: the span is that of the offending token.
    let src = wrap("if c { 1 }\n\n// otherwise\nelse { 2 }");
    let e = module_errors(&src);
    assert_code(&e, codes::MISPLACED_ELSE);
    assert_eq!(
        e[0].span.unwrap().start as usize,
        src.find("else").unwrap(),
        "{e:#?}"
    );
    assert_eq!(e[0].span.unwrap().len(), 4);
}

// ---------------------------------------------------------------------------
// 2.5: trailing commas
// ---------------------------------------------------------------------------

#[test]
fn trailing_commas_with_comments_and_using_arguments_before_the_closing_bracket() {
    // 2.5 / 3.12 `args`: the optional `","?` after the last positional or
    // `using` argument, with a trailing comment on the comma's line.
    let m = module_ast(&wrap("let l = [\n    1, // one\n]\nl"));
    let Statement::Let(l) = &fn_of(&m, 0).body.as_ref().unwrap().statements[0] else {
        panic!()
    };
    let ExprKind::List(list) = &l.value.kind else {
        panic!()
    };
    assert_eq!(list.elements.len(), 1);
    // Completed in triage: toolchain 2.3 rule B, "the separating comma does
    // not prevent the attachment", so the comment is the element's trailing
    // trivia and the list has no trailing comments.
    assert_eq!(list.elements[0].trivia.trailing.len(), 1);
    assert_eq!(list.elements[0].trivia.trailing[0].text, "// one");
    assert!(list.trailing_comments.is_empty());
    assert_contains(
        &body("let x = f(\n    a,\n    using db,\n)\nx"),
        "(call (name f) (name a) (using (name db)))",
    );
    assert_contains(
        &body("let x = f(\n    using db,\n    using ctx,\n)\nx"),
        "(call (name f) (using (name db)) (using (name ctx)))",
    );
    assert_contains(
        &module("module m\n\nfn f(\n    using db: Db,\n    using ctx: Ctx,\n) -> Int { 1 }\n"),
        "(params (using db (ty Db)) (using ctx (ty Ctx)))",
    );
}

#[test]
fn doubled_or_leading_commas_are_rejected_in_every_group_kind() {
    // 2.5: `commas(X)` admits one optional trailing comma, never two and
    // never a leading one.
    for src in [
        wrap("let x = [1,,]\nx"),
        wrap("f(a,,)"),
        wrap("let p = Page { ..p,, }\np"),
        wrap("let p = Page { text: t,, }\np"),
        wrap("match x {\n    Ok(v,,) => v\n}"),
        "module m\n\nuse a.b.{c,,}\n".to_string(),
        "module m\n\nuse a.b.{,c}\n".to_string(),
        "module m\n\nfn f(x: Int,,) -> Int { 1 }\n".to_string(),
        "module m\n\nfn f(,x: Int) -> Int { 1 }\n".to_string(),
        "module m\n\nfn f(using db: Db) -> Int\n    effects { db.get,, }\n{\n    1\n}\n"
            .to_string(),
        "module m\n\nenum E {\n    A(Int,,)\n}\n".to_string(),
        "module m\n\n@id(\"x\",,)\nfn f() -> Int { 1 }\n".to_string(),
    ] {
        assert_code(&module_errors(&src), codes::SYNTAX_ERROR);
    }
    assert_code(
        &work_errors("change c {\n    task t {\n        write { a,, }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t {\n        write { ,a }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_comma_after_an_unbracketed_expression_or_header_is_rejected() {
    // 2.5: trailing commas live only in bracketed lists; a clause
    // expression, a `where` predicate, a return type, a module path, and a
    // single-form `use` are not bracketed.
    for src in [
        "module m\n\npub type P = Int where self > 0,\n",
        "module m\n\nfn f(x: Int) -> Int\n    requires x > 0,\n{\n    x\n}\n",
        "module m\n\nfn f(x: Int) -> Int\n    decreases x,\n{\n    x\n}\n",
        "module m\n\nproperty \"p\"\n    for x in xs,\n{\n}\n",
        "module m\n\nfn f() -> Int,\n{\n    1\n}\n",
        "module m\n\nuse a.b.c,\n",
        "module m,\n",
        "module m\n\nstruct S,\n{\n}\n",
    ] {
        assert_code(&module_errors(src), codes::SYNTAX_ERROR);
    }
    assert_code(
        &body_errors("while c\n    invariant x,\n{\n}\n1"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&body_errors("return 1,"), codes::SYNTAX_ERROR);
    assert_code(&body_errors("assert a,\n1"), codes::SYNTAX_ERROR);
    assert_code(&work_errors("change c,\n{\n}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_errors("change c {\n    base git(\"a\"),\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch x,\n    expect base(x)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_errors("patch x\n    expect base(x),\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn a_trailing_comma_before_the_line_break_and_closing_bracket_in_every_group_kind() {
    // 2.5 + 2.2: the trailing comma and the closing bracket on its own line
    // for every `commas(X)` group, including a variant payload (3.7) and a
    // constructor pattern payload (3.13).
    let d = module("module m\n\nuse a.b.{\n    c,\n    D,\n}\n\n@id(\n    \"x\",\n)\nenum E {\n    A(\n        Int,\n    )\n    B(\n        Int,\n        Text,\n    )\n}\n\nfn f(\n    x: Int,\n) -> Int\n    effects {\n        db.get,\n    }\n{\n    match x {\n        Pair(\n            a,\n            b,\n        ) => g(\n            a,\n        )\n    }\n}\n");
    assert_contains(&d, "(use a.b (items c D)");
    assert_contains(&d, "(annotation id (text \"x\"))");
    assert_contains(&d, "(variant A (ty Int)) (variant B (ty Int) (ty Text))");
    assert_contains(&d, "(params (param x (ty Int)))");
    assert_contains(&d, "(effects db.get)");
    assert_contains(
        &d,
        "(arm (ctor Pair (bind a) (bind b)) (call (name g) (name a)))",
    );
}

// ---------------------------------------------------------------------------
// D51 / 3.6-3.8: empty bodies; 3.11: tail expressions
// ---------------------------------------------------------------------------

#[test]
fn empty_member_bodies_in_every_spelling() {
    // 3.6 "A struct may have zero fields"; `lines(X)` is possibly empty for
    // enums and capabilities too; blank and comment-only lines collapse (2.1).
    let d = module("module m\n\nstruct S {}\nstruct T {\n}\nstruct U {\n\n    // nothing\n\n}\nenum E {}\nenum F {\n\t\n}\ncapability C {}\ncapability D {\n    // none\n}\nstruct V\n{}\n");
    // Triage: `S` follows a blank line and records it (toolchain 2.3, 12.1);
    // the others are on adjacent lines.
    assert_contains(&d, "(struct S (trivia blank))");
    assert_contains(&d, "(struct T)");
    assert_contains(&d, "(struct U (trailing-comments \"// nothing\"))");
    assert_contains(&d, "(enum E)");
    assert_contains(&d, "(enum F)");
    assert_contains(&d, "(capability C)");
    assert_contains(&d, "(capability D (trailing-comments \"// none\"))");
    assert_contains(&d, "(struct V)");
    assert_crlf_invariant("module m\n\nstruct S {}\nstruct T {\n}\nenum E {\n\n}\n");
}

#[test]
fn members_may_share_the_lines_of_the_opening_and_closing_braces() {
    // Section 0: `lines(X) = NEWLINE? ( X ( NEWLINE X )* NEWLINE? )?`, so
    // the first member may follow `{` and the last precede `}` on one line.
    let d = module("module m\n\nstruct S { x: Int\n    y: Int }\nenum E { A\n    B(Int) }\ncapability C { fn a() -> Int\n    fn b() -> Int\n        requires true }\n");
    // Triage: `S` follows a blank line and records it (toolchain 2.3, 12.1).
    assert_contains(
        &d,
        "(struct S (field x (ty Int)) (field y (ty Int)) (trivia blank))",
    );
    assert_contains(&d, "(enum E (variant A) (variant B (ty Int)))");
    assert_contains(
        &d,
        "(capability C (op a (params) (ret (ty Int))) (op b (params) (ret (ty Int)) (requires (bool true))))",
    );
    assert_contains(
        &work("change c { base git(\"a\")\n    target git(\"b\") }\n"),
        "(change c (base \"a\") (target \"b\"))",
    );
}

#[test]
fn empty_else_and_else_if_blocks_across_lines() {
    // D51 + D49: every block of a chain may be empty.
    assert_contains(
        &body("if a {\n} else if b {\n} else {\n}\n1"),
        "(if (name a) (block) (else (if (name b) (block) (else (block)))))",
    );
    // Triage: the `if` is not the block's last statement, so it is an
    // expression statement and dumps as `(expr ...)` (grammar 3.11, toolchain
    // 12.1); `else_` is an ordinary identifier.
    assert_contains(
        &body("if a\n{\n} else if b\n{}\nelse_ = 1\n1"),
        "(expr (if (name a) (block) (else (if (name b) (block))))) (assign else_ (int 1))",
    );
}

#[test]
fn blank_and_comment_lines_around_the_tail_do_not_change_the_block() {
    // 2.1: they collapse into the one NEWLINE between statements; 3.11: the
    // last expression statement is the tail.
    let m = module_ast(&wrap("let x = 1\n\n\n// a\n\n// b\n\nx\n\n// c\n\n"));
    let b = fn_of(&m, 0).body.as_ref().unwrap();
    assert_eq!(b.statements.len(), 1);
    assert!(matches!(
        b.tail.as_ref().map(|t| &t.expr.kind),
        Some(ExprKind::Name(n)) if n.text == "x"
    ));
    assert_eq!(b.trailing_comments.len(), 1);
    // A tail that is an `if` whose `}` is followed only by comment lines.
    let d = body("if c {\n    1\n} else {\n    2\n}\n// done\n// really");
    assert_contains(
        &d,
        "(block (tail (if (name c) (block (tail (int 1))) (else (block (tail (int 2)))))) (trailing-comments \"// done\" \"// really\"))",
    );
}

// ---------------------------------------------------------------------------
// 3.12 / D49: `if` chains across lines
// ---------------------------------------------------------------------------

#[test]
fn else_if_chains_with_braces_on_following_lines_and_comments_between() {
    // D63 + 2.3: each header of the chain may take its `{` on the next line,
    // with blank and comment lines in between (2.1).
    let d = body("if a\n// one\n{\n    1\n} else if b\n\n{\n    2\n} else\n// three\n{\n    3\n}");
    assert_contains(&d, "(if (name a) (block (tail (int 1)");
    assert_contains(&d, "(else (if (name b) (block (tail (int 2)");
    assert_contains(&d, "(else (block (tail (int 3)");
    assert_crlf_invariant(&wrap(
        "if a\n{\n    1\n} else if b\n{\n    2\n} else\n{\n    3\n}",
    ));
}

#[test]
fn a_chain_broken_before_else_is_misplaced_whatever_separates_them() {
    // D12: the NEWLINE after `}` ends the statement; comments, blank lines,
    // tabs, and CRLF change nothing.
    assert_code(
        &body_errors("if a { 1 } else if b { 2 }\n// c\nelse if d { 3 } else { 4 }"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if a { 1 }\n\telse { 2 }"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if a {\n    1\n}\r\n\r\nelse {\n    2\n}"),
        codes::MISPLACED_ELSE,
    );
    assert_code(
        &body_errors("if a\n{\n    1\n}\nelse\n{\n    2\n}"),
        codes::MISPLACED_ELSE,
    );
}

#[test]
fn if_chains_as_tails_in_patch_bodies_and_test_bodies() {
    // 4.5: the patch block is the `.cpl` block production; 3.10: so is a
    // test body.
    assert_contains(
        &patch("patch f.body\n    expect base(f.body)\n{\n    if c {\n        1\n    } else if d {\n        2\n    } else {\n        3\n    }\n}\n"),
        "(block (tail (if (name c) (block (tail (int 1))) (else (if (name d) (block (tail (int 2))) (else (block (tail (int 3)))))))))",
    );
    // Triage: the test follows a blank line and records it (toolchain 2.3,
    // 12.1).
    assert_contains(
        &module("module m\n\ntest \"t\"\n{\n    if c\n    {\n        assert a\n    } else\n    {\n        assert b\n    }\n}\n"),
        "(test \"t\" (block (tail (if (name c) (block (assert (name a))) (else (block (assert (name b))))))) (trivia blank))",
    );
}

// ---------------------------------------------------------------------------
// 3.12 / D55: match-arm layout
// ---------------------------------------------------------------------------

#[test]
fn an_arm_body_block_followed_by_more_on_its_line_is_an_error() {
    // 3.12: `match_arm = pattern "=>" ( block | expr )` and a block is not
    // an expression (D28), so nothing may follow the block's `}` on the
    // arm's line; D55: a comma may not either.
    assert_code(
        &body_errors("match x {\n    A => { 1 } + 2\n}"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("match x {\n    A => { 1 }.len\n}"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("match x {\n    A => { 1 } ?\n}"),
        codes::SYNTAX_ERROR,
    );
    let e = body_errors("match x {\n    A => {\n        1\n    },\n    B => 2\n}");
    assert!(
        has_code(&e, codes::SYNTAX_ERROR)
            && has_message(&e, "match arms are separated by newlines"),
        "{e:#?}"
    );
}

#[test]
fn an_arm_expression_ending_in_a_name_does_not_take_a_brace_or_paren_from_the_next_line() {
    // 2.3: the arm's expression ends at its line; the next line must be an
    // arm (a pattern) and `{`/`(` cannot begin one.
    assert_code(
        &body_errors("match x {\n    A => Page\n    { ..p }\n}"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("match x {\n    A => f\n    (1)\n}"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_errors("match x {\n    A => f\n    .len\n}"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn an_arm_body_that_is_a_struct_literal_is_not_a_block() {
    // 3.12 rule 2: `Page {` after `=>` is the struct literal (struct
    // literals are enabled in arms, D41); a bare `{ ..p }` is a block whose
    // first token cannot begin a statement.
    assert_contains(
        &body("match x {\n    A => Page { ..p }\n    B => Page {\n        ..p,\n    }\n}"),
        "(arm (ctor A) (struct-lit Page (spread (name p)))) (arm (ctor B) (struct-lit Page (spread (name p))))",
    );
    assert_code(
        &body_errors("match x {\n    A => { ..p }\n}"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn arm_bodies_with_blocks_blank_lines_and_comments_between_arms() {
    let d = body("match r {\n    // ok\n    Ok(v) => {\n        let w = v\n        w\n    }\n\n    // err\n    Err(_) => {\n    }\n\n    _ => 0 // other\n}");
    // Completed in triage, toolchain 2.3: each own-line comment leads the
    // arm below it (rule D), the last one trails its arm (rule B), and a
    // blank line is recorded on the comment or arm it precedes; the first
    // arm records none.
    assert_contains(
        &d,
        "(arm (ctor Ok (bind v)) (block (let w (name v)) (tail (name w))) (trivia (lead \"// ok\")))",
    );
    assert_contains(
        &d,
        "(arm (ctor Err _) (block) (trivia (lead-blank \"// err\")))",
    );
    assert_contains(&d, "(arm _ (int 0) (trivia blank (trail \"// other\")))");
    assert_crlf_invariant(&wrap("match x { A => 1\n    B => {\n        2\n    } }"));
}

// ---------------------------------------------------------------------------
// Multi-line struct and list literals
// ---------------------------------------------------------------------------

#[test]
fn deeply_nested_multi_line_literals_and_calls() {
    // 2.2 continuation examples of 2.3, nested five deep.
    assert_contains(
        &body("let x = [\n    Page {\n        ..p,\n        text: f(\n            [\n                1,\n            ],\n        ),\n    },\n]\nx"),
        "(let x (list (struct-lit Page (spread (name p)) (field text (call (name f) (list (int 1)))))))",
    );
    assert_crlf_invariant(&wrap("let x = [\n    Page {\n        ..p,\n        text: f(\n            [\n                1,\n            ],\n        ),\n    },\n]\nx"));
}

#[test]
fn comments_and_blank_lines_everywhere_inside_a_multi_line_struct_literal() {
    let m = module_ast(&wrap("let p = Page { // open\n\n    // spread\n\n    ..base, // b\n\n    // field\n\n    text: t, // t\n\n    // end\n\n}\np"));
    let Statement::Let(l) = &fn_of(&m, 0).body.as_ref().unwrap().statements[0] else {
        panic!()
    };
    let ExprKind::Struct(s) = &l.value.kind else {
        panic!("expected a struct literal")
    };
    assert!(s.entries.spread.is_some());
    assert_eq!(s.entries.fields.len(), 1);
    assert_eq!(s.entries.fields[0].name.text, "text");
    // Completed in triage, toolchain 2.3: a comment after `{` and the one
    // above the spread lead the spread entry (rule D); one after an entry
    // and its comma trails it (rule B); the last is a trailing comment of
    // the entries (rule E). Each blank line is recorded on the comment it
    // precedes, never on a group element.
    let texts = |c: &[Comment]| {
        c.iter()
            .map(|c| (c.text.clone(), c.blank_line_before))
            .collect::<Vec<_>>()
    };
    let spread = &s.entries.spread_trivia;
    assert_eq!(
        texts(&spread.leading),
        [
            ("// open".to_string(), false),
            ("// spread".to_string(), true)
        ]
    );
    assert_eq!(texts(&spread.trailing), [("// b".to_string(), false)]);
    let field = &s.entries.fields[0].trivia;
    assert_eq!(texts(&field.leading), [("// field".to_string(), true)]);
    assert_eq!(texts(&field.trailing), [("// t".to_string(), false)]);
    assert!(!spread.blank_line_before && !field.blank_line_before);
    assert_eq!(
        texts(&s.entries.trailing_comments),
        [("// end".to_string(), true)]
    );
}

#[test]
fn an_empty_group_across_blank_and_comment_lines() {
    // 2.5 `commas(X)` may be empty; 2.1 collapses the lines inside.
    assert_contains(
        &body("let x = f(\n\n    // none\n\n)\nx"),
        "(let x (call (name f) (trailing-comments \"// none\")))",
    );
    assert_contains(&body("let l = [\n\n]\nl"), "(let l (list))");
    assert_contains(&body("let p = Page {\n\n}\np"), "(let p (struct-lit Page))");
    assert_contains(
        &module("module m\n\nfn f(\n\n) -> Int { 1 }\n"),
        "(fn f (params) (ret (ty Int))",
    );
    assert_contains(
        &work("change c {\n    task t {\n        write {\n\n\n        }\n    }\n}\n"),
        "(task t (write))",
    );
}

// ---------------------------------------------------------------------------
// 4.1-4.5: `.work` brace sets across lines
// ---------------------------------------------------------------------------

#[test]
fn selector_segments_and_the_star_may_sit_on_their_own_lines_inside_a_set() {
    // 4.2 / 2.2: a selector set opens an insignificant-newline context, so
    // the dotted path, the `*`, the facet, and the comma may each take a line.
    assert_contains(
        &work("change c {\n    task t {\n        add {\n            tests\n            .\n            pagination\n            .\n            *\n            ,\n        }\n        preserve {\n            a\n            .\n            effects\n            ,\n            b.\n            body\n        }\n    }\n}\n"),
        "(task t (add tests.pagination.*) (preserve a.effects b.body))",
    );
    // Inside `prove(...)` and `test(...)` parentheses too.
    assert_contains(
        &work("change c {\n    integrate {\n        require prove(a\n        .body)\n        require test(\n            tests\n            .*\n        )\n    }\n}\n"),
        "(integrate (require (prove a.body)) (require (test tests.*)))",
    );
}

#[test]
fn work_sets_and_requirements_across_lines_with_crlf() {
    let d = work("use a.b.{\r\n    f,\r\n}\r\nchange c {\r\n    task t {\r\n        write {\r\n            a.body,\r\n            b.contract,\r\n        }\r\n    }\r\n    integrate {\r\n        require test(\r\n            tests.*\r\n        )\r\n    }\r\n}\r\n");
    assert_contains(&d, "(use a.b (items f))");
    assert_contains(&d, "(write a.body b.contract)");
    assert_contains(&d, "(require (test tests.*))");
}

#[test]
fn a_selector_set_closing_brace_and_the_next_item_on_one_line_is_an_error() {
    // 2.3 for `.work` items (Section 4: the layout rules are shared).
    assert_code(
        &work_errors("change c {\n    task t {\n        write {\n        } goal \"g\"\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t {\n        write { a } depend { b }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    task t {\n    } task u {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_errors("change c {\n    integrate {\n    } workers 1\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

// ---------------------------------------------------------------------------
// 1.1: CRLF, missing final terminator, tabs; 1.2: comments at end of file
// ---------------------------------------------------------------------------

#[test]
fn crlf_between_headers_clauses_and_braces_in_every_header_kind() {
    assert_crlf_invariant("module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n    ensures result > 0\n{\n    x\n}\n\nstruct S\n{\n    x: Int\n}\n\nenum E\n{\n    A\n}\n\ncapability C\n{\n    fn get(id: Id) -> Int\n        requires true\n}\n\ntest \"t\"\n{\n    assert true\n}\n\nproperty \"p\"\n    for x in xs\n{\n    assert true\n}\n\nfn g() -> Int\n{\n    while c\n        invariant true\n        decreases x\n    {\n    }\n    if c\n    {\n        1\n    } else if d\n    {\n        2\n    } else\n    {\n        match x\n        {\n            _ => 3\n        }\n    }\n}\n");
    let d = work("change c\r\n{\r\n    task t after a, b\r\n    {\r\n        goal \"g\"\r\n    }\r\n    integrate\r\n    {\r\n        require typecheck\r\n    }\r\n}\r\n");
    assert_contains(
        &d,
        "(task t (after a b) (goal \"g\")) (integrate (require typecheck))",
    );
    // Triage: the `expect` clause follows a blank line and records it
    // (toolchain 2.3, 12.1).
    assert_contains(
        &patch("patch x\r\n\r\n    expect base(x)\r\n\r\n{\r\n    1\r\n}"),
        "(patch x (expect x (trivia blank)) (block (tail (int 1))))",
    );
}

#[test]
fn crlf_inside_multi_line_groups() {
    assert_crlf_invariant(&wrap(
        "let x = f(\n    a,\n    [\n        1,\n    ],\n    Page {\n        ..p,\n    },\n)\nx",
    ));
    assert_crlf_invariant("module m\n\nuse a.b.{\n    c,\n}\n\nfn f(\n    x: Int,\n    using db: Db,\n) -> Int\n    effects {\n        db.get,\n    }\n{\n    1\n}\n");
}

#[test]
fn a_crlf_file_without_a_final_terminator_in_every_form() {
    assert_contains(
        &module("module m\r\n\r\nfn f() -> Int {\r\n    1\r\n}"),
        "(block (tail (int 1)))",
    );
    assert_contains(
        &module("module m\r\nfn f() -> Int\r\n    requires true"),
        "bodiless",
    );
    assert_contains(
        &work("change c {\r\n    base git(\"a\")\r\n}"),
        "(change c (base \"a\"))",
    );
    assert_contains(
        &patch("patch x\r\n    expect base(x)\r\n{\r\n}"),
        "(patch x (expect x) (block))",
    );
}

#[test]
fn files_ending_after_a_prefix_or_brace_without_a_terminator() {
    // 1.1: a final terminator is optional; the last token may be anything
    // the grammar lets end an item. Triage: both functions follow a blank
    // line and record it (toolchain 2.3, 12.1).
    assert_contains(
        &module("module m\n\n@prove\nfn f() -> Int { 1 }"),
        "(fn f (annotation prove) (params) (ret (ty Int)) (block (tail (int 1))) (trivia blank))",
    );
    assert_contains(
        &module("module m\n\n/// d\nfn f() -> Int { 1 }"),
        "(fn f (docs \"d\") (params) (ret (ty Int)) (block (tail (int 1))) (trivia blank))",
    );
    assert_contains(&module("module m\ntype A"), "(type A (opaque))");
    assert_contains(
        &module("module m\npub type A(Text)"),
        "(type A pub (wrapper (ty Text)))",
    );
    assert_contains(&module("module m\nenum E { A }"), "(enum E (variant A))");
    assert_contains(&work("change c {}\nchange d {}"), "(change c) (change d)");
}

#[test]
fn tabs_as_the_only_separator_between_tokens_in_every_file_form() {
    // 1.1: tab is horizontal whitespace; indentation is never significant.
    let d = module("module\tm\n\nfn\tf(x:\tInt)\t->\tInt\n\trequires\tx\t>\t0\n{\n\tx\n}\n\nstruct\tS\n{\n\tx:\tInt\n}\n\npub\ttype\tP\t=\tInt\twhere\tself\t>\t0\n");
    // Triage: the three declarations follow blank lines and record them
    // (toolchain 2.3, 12.1).
    assert_contains(
        &d,
        "(fn f (params (param x (ty Int))) (ret (ty Int)) (requires (> (name x) (int 0))) (block (tail (name x))) (trivia blank))",
    );
    assert_contains(&d, "(struct S (field x (ty Int)) (trivia blank))");
    assert_contains(
        &d,
        "(type P pub (refinement (ty Int) (where (> (name self) (int 0)))) (trivia blank))",
    );
    assert_contains(
        &work("change\tc\t{\n\tbase\tgit(\"a\")\n\ttask\tt\tafter\ta,\tb\t{\n\t\twrite\t{\ta.body\t}\n\t}\n}\n"),
        "(change c (base \"a\") (task t (after a b) (write a.body)))",
    );
    assert_contains(
        &patch("patch\tx.body\n\texpect\tbase(x.body)\n{\n\t1\n}"),
        "(patch x.body (expect x.body) (block (tail (int 1))))",
    );
}

#[test]
fn tabs_before_doc_comments_annotations_and_declarations() {
    // 1.1 and 1.2: a tab is horizontal whitespace, so a `///` after it is
    // still the first non-whitespace of its line. Triage: both declarations
    // follow a blank line and record it (toolchain 2.3, 12.1).
    assert_contains(
        &module("module m\n\n\t/// d\n\t@prove\n\tfn f() -> Int { 1 }\n"),
        "(fn f (docs \"d\") (annotation prove) (params) (ret (ty Int)) (block (tail (int 1))) (trivia blank))",
    );
    assert_contains(
        &module("module m\n\nstruct S {\n\t/// x\n\tx: Int\n}\n"),
        "(struct S (field x (ty Int) (docs \"x\")) (trivia blank))",
    );
}

#[test]
fn comments_at_the_end_of_a_file_after_every_item_kind() {
    // 1.2 / 2.1: ordinary comments produce no token; the ones after the last
    // item belong to the file.
    let m = module_ast("module m\n\nstruct S {\n    x: Int\n}\n// after struct");
    assert_eq!(m.trailing_comments.len(), 1);
    let m = module_ast("module m\n\n@prove\nfn f() -> Int { 1 }\n// c1\n\n// c2");
    assert_eq!(m.trailing_comments.len(), 2);
    let m = module_ast("module m\n\npub type P = Int where self > 0\n// after a refinement\n");
    assert_eq!(m.trailing_comments.len(), 1);
    assert_contains(
        &work("change c {\n}\n\n// a\n\n// b"),
        "(change c) (trailing-comments \"// a\" \"// b\")",
    );
    assert_contains(
        &patch("patch x\n    expect base(x)\n{\n}\n// a\n// b"),
        "(trailing-comments \"// a\" \"// b\")",
    );
    // A comment on the closing brace line of the last body, no terminator.
    let m = module_ast("module m\nfn f() -> Int {\n    1\n} // done");
    assert_eq!(m.declarations[0].trivia.trailing[0].text, "// done");
    let w = work_ast("change c {\n} // done");
    assert_eq!(w.changes[0].trivia.trailing[0].text, "// done");
    let p = patch_ast("patch x\n    expect base(x)\n{\n} // done");
    assert_eq!(p.forms[0].trivia.trailing[0].text, "// done");
}

// ---------------------------------------------------------------------------
// Canonical layout (spec/toolchain.md 2.4, which grammar 2.2 and 2.4 defer to)
// ---------------------------------------------------------------------------

#[test]
fn the_formatter_joins_a_lone_brace_to_a_clause_free_header_and_separates_it_after_clauses() {
    // toolchain 2.4 rule 7 (grammar D63 accepts both spellings as input).
    assert_eq!(
        fmt_cpl("module m\n\nfn f() -> Int\n{\n    1\n}\n\nfn g(x: Int) -> Int\n    requires x > 0\n{\n    x\n}\n"),
        "module m\n\nfn f() -> Int {\n    1\n}\n\nfn g(x: Int) -> Int\n    requires x > 0\n{\n    x\n}\n"
    );
    assert_eq!(
        fmt_cpl("module m\n\nstruct S\n{\n    x: Int\n}\n\ntest \"t\"\n{\n    assert true\n}\n"),
        "module m\n\nstruct S {\n    x: Int\n}\n\ntest \"t\" {\n    assert true\n}\n"
    );
}

#[test]
fn the_formatter_normalises_crlf_tabs_and_header_brace_lines() {
    // toolchain 2.4 rule 1 (LF, four spaces, one final LF) and rule 8
    // (`if c {` ... `} else {`); grammar 1.1 makes CRLF and tabs equivalent
    // input, so the canonical output is the same.
    assert_eq!(
        fmt_cpl("module m\r\n\r\nfn f() -> Int\r\n{\r\n\t1\r\n}\r\n"),
        "module m\n\nfn f() -> Int {\n    1\n}\n"
    );
    assert_eq!(
        fmt_cpl(&wrap("if c\n{ 1 } else\n{ 2 }")),
        "module m\n\nfn f() -> Int {\n    if c {\n        1\n    } else {\n        2\n    }\n}\n"
    );
    // Triage: the empty body prints `{}` (toolchain 2.4 rule 8: "Blocks are
    // always multi-line except the empty block `{}`"; 1.5, `Fragment::Block`:
    // "`{` LF the statements one level deeper LF `}` (or `{}`)"), on its own
    // line because the loop has a clause (rule 7).
    assert_eq!(
        fmt_cpl(&wrap("while c\n    invariant x\n{\n}\nmatch x\n{ _ => 1 }")),
        "module m\n\nfn f() -> Int {\n    while c\n        invariant x\n    {}\n    match x {\n        _ => 1\n    }\n}\n"
    );
}

#[test]
fn the_formatter_keeps_a_broken_group_broken_and_a_flat_group_flat() {
    // toolchain 2.4 rule 6 / T12: a group stays broken once the author
    // broke it, and a flat group stays flat.
    assert_contains(
        &fmt_cpl(&wrap("let l = [\n    1,\n]\nl")),
        "    let l = [\n        1,\n    ]\n",
    );
    assert_contains(&fmt_cpl(&wrap("let l = [1, 2]\nl")), "    let l = [1, 2]\n");
    assert_contains(
        &fmt_cpl(&wrap("let p = Page { ..p,\n    text: t }\np")),
        "    let p = Page {\n        ..p,\n        text: t,\n    }\n",
    );
    assert_contains(
        &fmt_cpl(&wrap("let p = Page { ..p, text: t }\np")),
        "    let p = Page { ..p, text: t }\n",
    );
}

// ---------------------------------------------------------------------------
// No-panic sweeps over layout-shaped fragments
// ---------------------------------------------------------------------------

const LAYOUT_FRAGMENTS_R2: &[&str] = &[
    "module m\nfn f() -> Int {\n    if c\n/// d\n{",
    "module m\nfn f() -> Int {\n    if c { 1 } else\n/// d\n{",
    "module m\nfn f() -> Int {\n    match x\n/// d\n{",
    "module m\nfn f() -> Int {\n    let x = [Page\n",
    "module m\nfn f() -> Int {\n    let x = [Page\n{",
    "module m\nfn f() -> Int {\n    let x = (f\n(",
    "module m\nfn f() -> Int {\n    let x = (a\n    .",
    "module m\nfn f() -> Int {\n    let x = (forall y in\n",
    "module m\nfn f() -> Int {\n    let x = (1\nlet y = 2\n",
    "module m\nfn f() -> Int {\n    let x = (1]\n",
    "module m\nfn f() -> Int {\n    let x = Page { ..p )\n",
    "module m\nfn f() -> Int {\n    match x {\n        A => { 1 } +",
    "module m\nfn f() -> Int {\n    match x {\n        A => Page\n        {",
    "module m\nfn f() -> Int {\n    match x {\n        A => { ..",
    "module m\nfn f() -> Int {\n    return\n(",
    "module m\nfn f() -> Int {\n    x\n    = 1",
    "module m\nfn f() -> Int {\n    y\n{",
    "module m\nfn f() -> Int {\n    if c {\n} let",
    "module m\nfn f() -> Int {\n    match x {\n} else",
    "module m\nfn f() -> Int { 1 }\n    requires",
    "module m\nfn f() -> Int { 1 }\n    effects {",
    "module m\nfn f(x: Int) -> Int\n    requires x > 0",
    "module m\nfn f() -> Int\n    decreases 1 //",
    "module m\nfn f() ->\n",
    "module m\nfn f()\n    ->",
    "module m\nfn\n",
    "module m\nfn f\n(",
    "module m\npub\n",
    "module m\npub\nfn",
    "module m\nstruct S\n<",
    "module m\nstruct S<T>\n{",
    "module m\nstruct S {\n    else",
    "module m\nenum E {\n    A\n    else\n",
    "module m\ncapability C {\n    fn get(id: Id) -> Int\n    {",
    "module m\ncapability C {\n    fn get(id: Id) -> Int\n        decreases",
    "module m\ntest\n\"t\"",
    "module m\ntype A\n(",
    "module m\ntype P = Int\n    where",
    "module m\nuse a.b.\n{",
    "module m\nuse a.b\n.{",
    "module m\n/// d\nuse",
    "module m\nelse",
    "else\nmodule m",
    "else",
    "else\nelse",
    "}\nelse",
    "module m\nfn f() -> Int { let x = a + }",
    "module m\nfn f() -> Int {\n    let x = a +",
    "module m\nfn f() -> Int {\n    let x = a +\r\n",
    "module m\nfn f() -> Int {\n    let x = \"a\rb\"",
    "module m\nfn f() -> Int {\n    let x = \"a\tb\"",
    "module m\u{c}",
    "module m\u{b}\n",
    "module\u{a0}m",
    "module m\nfn f() -> Int { 1 /* c */ }",
    "module m\nfn f() -> Int {\n    # c\n}",
    "\u{feff}/// d",
    "\u{feff}/// d\nmodule m",
    "\u{feff}\r\n",
    "\u{feff}\t\n\tmodule m",
    "\t/// d\n\tmodule m",
    "///\tdoc\nmodule m",
    "/// a\n// plain\n/// b\nmodule m",
    "module m\n\n/// a\n\n// plain\n\n/// b\n",
    "module m\nfn f() -> Int {\n    let x = [\n    /// d\n",
    "module m\nfn f() -> Int {\n    let x = f(\n    /// d\n",
    "module m\nfn f(\n    /// d\n",
    "module m\nenum E {\n    A(\n        /// d\n",
    "module m\n@id(\n    /// d\n",
    "module m\nuse a.b.{\n    /// d\n",
    "module m\nfn f() -> Int\n    effects {\n        /// d\n",
    "module m\nfn f() -> Int {\n    let x = [1,,]",
    "module m\nfn f() -> Int {\n    f(,",
    "module m\nuse a.b.{,",
    "module m\nfn f(,",
    "module m,",
    "module m\nfn f() -> Int,",
    "module m\nstruct S,",
    "module m\nstruct S { x: Int\n    y: Int }",
    "module m\nenum E { A\n    B(Int) }",
    "module m\ncapability C { fn a() -> Int\n    fn b() -> Int\n        requires true }",
    "module m\nfn f() -> Int {\n    if a\n{\n} else if b\n{}\n",
    "module m\nfn f() -> Int {\n    match x\n{ A => 1\n    B => 2 }",
    "module m\nfn f() -> Int {\n    if Page\n{",
    "module m\nfn f() -> Int {\n    while a.B\n{",
    "module m\nfn f(c: Bool) -> Int\n    requires if c\n    { true } else { false }\n{",
    "module m\nfn f() -> Int {\n    assert if c\n{ true }",
    "module m\nfn f() -> Int {\n    return match x\n{",
    "module m\nfn f() -> Int {\n    let x = f(Page { text: match y {\n    A => \"a\"\n",
    "module m\nfn f() -> Int {\n    let x = f(match y {\n    A => if c { 1 }\n    else { 2 }\n",
    "module m\nfn f() -> Int {\n    let x = ([f(match y {\n    A => {\n        let z = (1\n            + 2)\n",
    "change c\n/// d\n{",
    "change c {\n    task t\n/// d\n    {",
    "change c {\n    task t after a\n/// d\n",
    "change c {\n    integrate\n/// d\n",
    "change c {\n    task t {\n        write {\n            /// d\n",
    "change c {\n    base git(\n        /// d\n",
    "change c {\n    task t {\n        add {\n            tests\n            .\n            *\n",
    "change c {\n    task t {\n        preserve {\n            a\n            .\n            effects\n",
    "change c {\n    integrate {\n        require prove(a\n        .body)\n",
    "change c {\n    integrate {\n        require test\n        (x)",
    "change c {\n    task t {\n        write {\n        } goal",
    "change c {\n    task t {\n    } task u {",
    "change c {\n    integrate {\n    } workers",
    "change c {\n    else",
    "change c {\n    task t {\n        else",
    "change c {\n    integrate {\n        else",
    "change c {\n}\nelse",
    "change c\n    after x\n{",
    "change c {\n    task t\n        expect base(x)\n",
    "change c,",
    "change c {\n    base git(\"a\"),",
    "change c {\r}",
    "use a.b.{\r\n    f,\r\n}\r\nchange c {\r\n",
    "change\tc\t{\n\tbase\tgit(\"a\")",
    "patch a\n    .body",
    "patch\na",
    "patch x\n{\n}\n    expect",
    "patch x\n    expect base(\n        /// d\n",
    "patch x\n    expect base(\n        x.body\n    )\n{",
    "patch x\n    expect base(x\n        .body)\n{",
    "patch x\n    requires x",
    "patch x,",
    "patch x\n    expect base(x),",
    "patch x\n    expect base(x)\n/// d\n{",
    "patch x\n/// d\n    expect",
    "patch x\n    expect base(x)\n{\n    else",
    "patch x\n    expect base(x)\n{\n}\nelse",
    "patch x // target\n    expect base(x) // base\n{ // body",
    "patch\tx.body\n\texpect\tbase(x.body)\n{",
    "module m\nfn f() -> Int { 1 }\n// c1\n\n// c2",
    "module m\nfn f() -> Int {\n    1\n} // done",
    "change c {\n} // done",
    "patch x\n    expect base(x)\n{\n} // done",
];

#[test]
fn layout_fragments_round_2_never_panic_in_any_parser() {
    for src in LAYOUT_FRAGMENTS_R2 {
        no_panic(src);
        no_panic(&src.replace('\n', "\r\n"));
        no_panic(&src.replace("    ", "\t"));
        no_panic(&format!("{src}\n"));
        no_panic(&format!("\u{feff}{src}"));
    }
    // Very long runs of blank lines and comment lines inside bodies and brackets.
    let blanks = "\n".repeat(20_000);
    no_panic(&format!(
        "module m\n\nfn f() -> Int {{{blanks}    1{blanks}}}\n"
    ));
    no_panic(&format!(
        "module m\n\nfn f() -> Int {{\n    let x = [{blanks}1,{blanks}]\n    x\n}}\n"
    ));
    no_panic(&format!("module m\n\nfn f() -> Int{blanks}{{\n    1\n}}\n"));
    let comments = "// c\n".repeat(5_000);
    no_panic(&format!(
        "module m\n{comments}fn f() -> Int {{\n{comments}    1\n{comments}}}\n{comments}"
    ));
    let r = parse_module(
        &format!("module m\n\nfn f() -> Int {{{blanks}    1{blanks}}}\n"),
        FileId(0),
    );
    assert!(r.value.is_some(), "{:#?}", r.diagnostics);
}

#[test]
fn every_prefix_of_layout_heavy_work_and_patch_files_parses_without_panicking() {
    let work_src = "// top\nuse a.b.{\n    f,\n    G,\n}\n\n/// The change.\nchange c // c\n{\n\n    base git(\n        \"main\"\n    )\n    target git(\"main\")\n    workers 3\n\n    /// A task.\n    task t after a,\tb\n    {\n        write {\n            f.body, // b\n            f.\n            contract,\n        }\n        add {\n            tests\n            .*\n            ,\n        }\n        preserve {}\n        goal \"g\"\n    }\n    integrate\n    {\n        require typecheck\n        require prove(\n            f\n        )\n        require test(tests.*)\n    }\n}\n\nchange d {}\n// end\n";
    assert!(parse_work(work_src, FileId(0)).diagnostics.is_empty());
    let patch_src = "/// form\npatch f.body\n\n    // check\n    expect base(\n        f.body\n    )\n\n{\n    var pages: List<List<T>> = []\n    while rest.len > size\n        invariant (flatten(pages)\n            ++ rest == items)\n        decreases rest.len\n    {\n        pages = pages.push(rest.take(size))\n    }\n    if not rest.is_empty()\n    {\n        pages = pages.push(rest)\n    } else if c\n    {\n        pages = []\n    } else\n    {\n    }\n    match pages\n    {\n        Empty => pages\n        _ => Page {\n            ..p,\n        }\n    }\n}\n\npatch g.body\n    expect base(g.body)\n{}\n";
    // Repaired in triage: the round's text had the arm pattern `[]`, which
    // is not a pattern (grammar D33), so the whole file was an error case.
    // The sweep now covers a valid file, like the `.work` one, and the list
    // pattern stays as the one error it is.
    assert!(parse_patch(patch_src, FileId(0)).diagnostics.is_empty());
    let with_list_pattern = patch_src.replace("Empty =>", "[] =>");
    let e = patch_errors(&with_list_pattern);
    assert_eq!(e.len(), 1, "{e:#?}");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert_eq!(field(&e[0], "found"), Some("`[`"), "{e:#?}");
    for src in [work_src, patch_src] {
        let crlf = src.replace('\n', "\r\n");
        for text in [src, crlf.as_str()] {
            for end in text
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(text.len()))
            {
                no_panic(&text[..end]);
            }
        }
    }
}
