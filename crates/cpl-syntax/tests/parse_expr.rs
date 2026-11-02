//! `parse_expr`: one expression with nothing after it, under the normal
//! expression grammar (grammar 3.12), for the constant-expression arguments
//! of `cpl run` (`spec/toolchain.md` 7.2; semantics S39 and 3.5). The
//! `ParseResult` contract is the file parsers' (grammar D59, D70): a value
//! exactly when there is no error, otherwise exactly one diagnostic, the
//! earliest.

use cpl_diag::{DiagCode, Diagnostic, FileId};
use cpl_syntax::ast::*;
use cpl_syntax::{codes, format_fragment, parse_expr, parse_module, Fragment, FragmentMode};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[track_caller]
fn expr(src: &str) -> Expr {
    let r = parse_expr(src, FileId(0));
    match r.value {
        Some(e) if r.diagnostics.is_empty() => e,
        _ => panic!("expected a clean parse of {src:?}: {:#?}", r.diagnostics),
    }
}

/// The canonical text of the parsed expression.
#[track_caller]
fn text(src: &str) -> String {
    format_fragment(Fragment::Expr(&expr(src)), FragmentMode::Canonical)
}

/// The single diagnostic of a failed parse.
#[track_caller]
fn error(src: &str) -> Diagnostic {
    let r = parse_expr(src, FileId(0));
    assert!(r.value.is_none(), "expected {src:?} to be rejected");
    assert!(r.has_errors());
    assert_eq!(
        r.diagnostics.len(),
        1,
        "D59: one diagnostic for {src:?}: {:#?}",
        r.diagnostics
    );
    let d = r.diagnostics.into_iter().next().expect("one diagnostic");
    assert_eq!(d.category, codes::CATEGORY);
    assert!(codes::ALL.contains(&d.code), "unknown code {}", d.code);
    d
}

#[track_caller]
fn assert_code(src: &str, code: DiagCode) -> Diagnostic {
    let d = error(src);
    assert_eq!(d.code, code, "{src:?}: {}", d.message);
    d
}

fn field<'a>(d: &'a Diagnostic, key: &str) -> Option<&'a str> {
    d.data.get(key).and_then(|v| v.as_str())
}

// ---------------------------------------------------------------------------
// Accepted forms
// ---------------------------------------------------------------------------

#[test]
fn literals() {
    let ExprKind::Literal(l) = expr("42").kind else {
        panic!("a literal")
    };
    assert_eq!(l.kind, LiteralKind::Int("42".to_string()));
    // Digit separators are dropped (grammar 1.6).
    let ExprKind::Literal(l) = expr("1_000_000").kind else {
        panic!("a literal")
    };
    assert_eq!(l.kind, LiteralKind::Int("1000000".to_string()));
    // Arbitrary precision: the digits are kept as written.
    let big = "123456789012345678901234567890";
    assert_eq!(text(big), big);
    let ExprKind::Literal(l) = expr("\"a\\tb\\u{41}\"").kind else {
        panic!("a literal")
    };
    assert_eq!(l.kind, LiteralKind::Text("a\tbA".to_string()));
    for (src, value) in [("true", true), ("false", false)] {
        let ExprKind::Literal(l) = expr(src).kind else {
            panic!("a literal")
        };
        assert_eq!(l.kind, LiteralKind::Bool(value));
    }
    // `Unit` is the constructor of the prelude's `Unit` (a name here).
    let ExprKind::Name(n) = expr("Unit").kind else {
        panic!("a name")
    };
    assert_eq!(n.text, "Unit");
}

#[test]
fn negative_numbers_are_unary_minus() {
    // Grammar 5 (j): a negative number is unary minus applied to a literal.
    let e = expr("-2");
    let ExprKind::Unary(u) = &e.kind else {
        panic!("unary minus")
    };
    assert_eq!(u.op, UnaryOp::Neg);
    assert!(
        matches!(&u.operand.kind, ExprKind::Literal(Literal { kind: LiteralKind::Int(d), .. }) if d == "2")
    );
    assert_eq!(text("-2"), "-2");
    assert_eq!(text("- 2"), "-2");
    assert_eq!(text("-(3)"), "-(3)");
    assert_eq!(text("--4"), "--4");
    assert_eq!(text("[-1, -0]"), "[-1, -0]");
    assert_eq!(text("not true"), "not true");
    assert_eq!(text("(1 + 2) * -3"), "(1 + 2) * -3");
}

#[test]
fn lists() {
    assert_eq!(text("[]"), "[]");
    assert_eq!(text("[1, 2, 3]"), "[1, 2, 3]");
    assert_eq!(text("[[1, 2], [3]]"), "[[1, 2], [3]]");
    // The `cpl run` example of toolchain 7.2.
    let ExprKind::List(list) = expr("[1, 2, 3]").kind else {
        panic!("a list")
    };
    assert_eq!(list.elements.len(), 3);
    // A trailing comma and line breaks inside the brackets (grammar 2.2, 2.5).
    assert_eq!(text("[\n    1,\n    2,\n]"), "[1, 2]");
    assert_eq!(text("[\"a\", \"b\"]"), "[\"a\", \"b\"]");
}

#[test]
fn struct_literals() {
    // An argument is not a header expression, so struct literals are
    // allowed at the top level (D41).
    let e = expr("Page { id: PageId(\"p\"), text: \"t\" }");
    let ExprKind::Struct(s) = &e.kind else {
        panic!("a struct literal")
    };
    assert_eq!(s.path.dotted(), "Page");
    assert_eq!(s.entries.fields.len(), 2);
    assert_eq!(
        text("Page{id:PageId(\"p\"),text:\"t\"}"),
        "Page { id: PageId(\"p\"), text: \"t\" }"
    );
    assert_eq!(text("Empty {}"), "Empty {}");
    assert_eq!(
        text("reader.pages.Page { id: PageId(\"p\"), text: \"\" }"),
        "reader.pages.Page { id: PageId(\"p\"), text: \"\" }"
    );
    assert_eq!(
        text("Dimensions {\n    width: 3,\n    height: 4,\n}"),
        "Dimensions { width: 3, height: 4 }"
    );
    assert_eq!(
        text("[Page { id: PageId(\"a\"), text: \"x\" }]"),
        "[Page { id: PageId(\"a\"), text: \"x\" }]"
    );
}

#[test]
fn constructors() {
    assert_eq!(text("None"), "None");
    assert_eq!(text("Unavailable"), "Unavailable");
    assert_eq!(text("Some(1)"), "Some(1)");
    assert_eq!(text("Ok([[1, 2], [3]])"), "Ok([[1, 2], [3]])");
    assert_eq!(
        text("Err(Missing(PageId(\"x\")))"),
        "Err(Missing(PageId(\"x\")))"
    );
    // A qualified constructor is a member chain called with its payload.
    let e = expr("StoreError.Missing(PageId(\"x\"))");
    let ExprKind::Call(c) = &e.kind else {
        panic!("a call")
    };
    assert!(matches!(&c.callee.kind, ExprKind::Member(m) if m.member.text == "Missing"));
    assert_eq!(text("Option.None"), "Option.None");
}

#[test]
fn wrapper_constructors() {
    // `PageId("page-1")` (design line 73).
    let e = expr("PageId(\"page-1\")");
    let ExprKind::Call(c) = &e.kind else {
        panic!("a call")
    };
    assert!(matches!(&c.callee.kind, ExprKind::Name(n) if n.text == "PageId"));
    assert_eq!(c.args.positional.len(), 1);
    assert_eq!(text("PageId(\"page-1\")"), "PageId(\"page-1\")");
    assert_eq!(
        text("[PageId(\"a\"), PageId(\"b\")]"),
        "[PageId(\"a\"), PageId(\"b\")]"
    );
}

#[test]
fn nested_generics() {
    // `>>>` closes three generic lists (grammar 3.4); the speculative
    // generic call of D42.
    let e = expr("check<List<List<Int>>>(x)");
    let ExprKind::Call(c) = &e.kind else {
        panic!("a call")
    };
    let g = c.generic_args.as_ref().expect("generic arguments");
    assert_eq!(g.args.len(), 1);
    assert_eq!(
        format_fragment(Fragment::Type(&g.args[0]), FragmentMode::Canonical),
        "List<List<Int>>"
    );
    assert_eq!(
        text("check<List<List<Int>>>(x)"),
        "check<List<List<Int>>>(x)"
    );
    assert_eq!(
        text("f<Result<Option<Int>, List<Text>>>([])"),
        "f<Result<Option<Int>, List<Text>>>([])"
    );
    // Without a call it is a comparison chain, which is not an expression.
    assert_code("a < b > c", codes::SYNTAX_ERROR);
    assert_eq!(text("a < b"), "a < b");
}

#[test]
fn every_expression_form_parses_without_a_constant_restriction() {
    // The caller's typing rejects what is not a constant expression
    // (semantics 3.5); the parser accepts the whole expression grammar.
    for (src, canonical) in [
        ("x", "x"),
        ("?todo", "?todo"),
        ("f(1, using db)", "f(1, using db)"),
        ("xs.len", "xs.len"),
        ("r?", "r?"),
        ("forall x in xs: x > 0", "forall x in xs: x > 0"),
        ("a ++ b", "a ++ b"),
        ("if c { 1 } else { 2 }", "if c {\n    1\n} else {\n    2\n}"),
        (
            "match r {\n    Ok(v) => v\n    Err(_) => 0\n}",
            "match r {\n    Ok(v) => v\n    Err(_) => 0\n}",
        ),
        ("a == b and not c or d", "a == b and not c or d"),
    ] {
        assert_eq!(text(src), canonical, "{src:?}");
    }
}

#[test]
fn surrounding_line_breaks_spaces_and_comments_are_allowed() {
    assert_eq!(text("  [1, 2]  "), "[1, 2]");
    assert_eq!(text("\n\n2\n\n"), "2");
    assert_eq!(text("2\r\n"), "2");
    assert_eq!(text("// the size\n2 // two\n"), "2");
    assert_eq!(text("\t-1\t"), "-1");
}

#[test]
fn the_tree_is_the_one_an_expression_statement_gets() {
    for src in [
        "[1, 2, 3]",
        "-2",
        "Page { id: PageId(\"p\"), text: \"t\" }",
        "Err(Missing(PageId(\"x\")))",
        "check<List<List<Int>>>(x)",
        "(1 + 2) * 3",
    ] {
        let m = parse_module(
            &format!("module m\n\nfn f() -> Unit {{\n    let x = {src}\n}}\n"),
            FileId(0),
        )
        .value
        .expect("the statement form parses");
        let DeclKind::Fn(f) = &m.declarations[0].kind else {
            panic!("a fn")
        };
        let Statement::Let(l) = &f.body.as_ref().expect("body").statements[0] else {
            panic!("a let")
        };
        assert_eq!(
            text(src),
            format_fragment(Fragment::Expr(&l.value), FragmentMode::Canonical),
            "{src:?}"
        );
    }
}

#[test]
fn spans_are_offsets_into_the_argument_text() {
    let e = expr("  [1, 22]");
    assert_eq!((e.span.start, e.span.end), (2, 9));
    assert_eq!(e.span.file, FileId(0));
    let r = parse_expr("Some(1)", FileId(7));
    assert_eq!(r.value.expect("parses").span.file, FileId(7));
    let r = parse_expr("1 2", FileId(7));
    assert_eq!(r.diagnostics[0].span.map(|s| s.file), Some(FileId(7)));
}

// ---------------------------------------------------------------------------
// Rejected input
// ---------------------------------------------------------------------------

#[test]
fn trailing_garbage_is_rejected() {
    for (src, found) in [
        ("1 2", "integer literal `2`"),
        ("1)", "`)`"),
        ("[1, 2]]", "`]`"),
        ("f(x) g", "identifier `g`"),
        ("1,", "`,`"),
        ("Some(1) Some(2)", "identifier `Some`"),
        ("Page {} {}", "`{`"),
        ("\"a\" \"b\"", "text literal \"b\""),
    ] {
        let d = assert_code(src, codes::SYNTAX_ERROR);
        assert_eq!(
            field(&d, "expected"),
            Some("end of input after the expression"),
            "{src:?}"
        );
        assert_eq!(field(&d, "found"), Some(found), "{src:?}");
        assert!(
            d.message
                .starts_with("expected end of input after the expression, found "),
            "{src:?}: {}",
            d.message
        );
    }
}

#[test]
fn a_line_break_ends_the_expression_outside_brackets() {
    // Grammar 2.3, as in an expression statement: a second line is trailing
    // garbage, and an operator cannot begin or end a line.
    let d = assert_code("1\n2", codes::SYNTAX_ERROR);
    assert_eq!(d.span.map(|s| s.start), Some(2));
    assert_code("1 // one\n2", codes::SYNTAX_ERROR);
    assert_code("1\n+ 2", codes::SYNTAX_ERROR);
    assert_code("1 +\n2", codes::SYNTAX_ERROR);
    // Inside brackets newlines are insignificant (grammar 2.2).
    assert_eq!(text("(1\n+ 2)"), "(1 + 2)");
    assert_eq!(text("f(\n1,\n2)"), "f(1, 2)");
}

#[test]
fn empty_input_and_non_expressions_are_rejected() {
    for src in ["", "   ", "\n\n", "// only a comment\n"] {
        let d = assert_code(src, codes::SYNTAX_ERROR);
        assert_eq!(field(&d, "found"), Some("end of file"), "{src:?}");
    }
    assert_code("let x = 1", codes::SYNTAX_ERROR);
    assert_code("1 +", codes::SYNTAX_ERROR);
    assert_code(")", codes::SYNTAX_ERROR);
    assert_code("[1, 2", codes::SYNTAX_ERROR);
    assert_code("Page { ..p, ..q }", codes::SYNTAX_ERROR);
    assert_code("/// docs\n1", codes::DOC_COMMENT_POSITION);
    assert_code("async", codes::RESERVED_WORD);
    assert_code("else", codes::SYNTAX_ERROR);
}

#[test]
fn lexical_errors_are_reported_with_their_codes() {
    assert_code("\"unterminated", codes::UNTERMINATED_TEXT_LITERAL);
    assert_code("1.5", codes::FLOAT_LITERAL_UNSUPPORTED);
    assert_code("2x", codes::LITERAL_ADJACENT_IDENTIFIER);
    assert_code("\"bad \\q escape\"", codes::INVALID_ESCAPE);
    assert_code("1 # 2", codes::INVALID_CHARACTER);
    assert_code("café", codes::NON_ASCII_OUTSIDE_TEXT);
}

#[test]
fn the_earliest_error_is_the_one_reported() {
    // D70: the error with the smallest start offset, lexical or syntactic.
    let d = assert_code("[1, 2 3] #", codes::SYNTAX_ERROR);
    assert_eq!(d.span.map(|s| s.start), Some(6));
    let d = assert_code("[1, #, 2 3]", codes::INVALID_CHARACTER);
    assert_eq!(d.span.map(|s| s.start), Some(4));
}
