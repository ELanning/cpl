//! Diagnostic codes for malformed input, the first-error rule of grammar
//! D59, and the no-panic guarantee on garbage and truncated inputs.

use cpl_diag::{DiagCode, Diagnostic, FileId};
use cpl_syntax::{codes, parse_module, parse_patch, parse_work};
use std::path::{Path, PathBuf};

fn errors(src: &str) -> Vec<Diagnostic> {
    let r = parse_module(src, FileId(0));
    assert!(r.value.is_none(), "expected errors for:\n{src}");
    assert!(!r.diagnostics.is_empty());
    for d in &r.diagnostics {
        assert_eq!(d.category, codes::CATEGORY);
        assert!(codes::ALL.contains(&d.code), "unknown code {}", d.code);
    }
    r.diagnostics
}

fn body_errors(stmts: &str) -> Vec<Diagnostic> {
    let indented: Vec<String> = stmts.lines().map(|l| format!("    {l}")).collect();
    errors(&format!(
        "module m\n\nfn f() -> Int {{\n{}\n}}\n",
        indented.join("\n")
    ))
}

fn code_list(diags: &[Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code.as_str()).collect()
}

fn has(diags: &[Diagnostic], code: DiagCode) -> bool {
    diags.iter().any(|d| d.code == code)
}

fn message(diags: &[Diagnostic], needle: &str) -> bool {
    diags.iter().any(|d| d.message.contains(needle))
}

#[test]
fn two_statements_on_one_line() {
    let e = body_errors("let x = 1 let y = 2\nx");
    assert_eq!(code_list(&e), vec![codes::SYNTAX_ERROR]);
    assert!(message(&e, "expected end of line after the statement"));
    assert!(e[0].data.get("found").is_some());
    let e = body_errors("x = 1 y = 2\nx");
    assert_eq!(code_list(&e), vec![codes::SYNTAX_ERROR]);
}

#[test]
fn chained_comparison() {
    let e = body_errors("a < b < c");
    assert_eq!(code_list(&e), vec![codes::SYNTAX_ERROR]);
    assert!(message(&e, "comparisons do not chain"));
    assert!(has(&body_errors("a == b == c"), codes::SYNTAX_ERROR));
    assert!(has(
        &body_errors("a <= b and b <= c < d"),
        codes::SYNTAX_ERROR
    ));
}

#[test]
fn struct_literal_in_a_condition() {
    let e = body_errors("if page == Page { ..p } {\n}\n1");
    assert!(has(&e, codes::SYNTAX_ERROR));
    let e =
        errors("module m\n\nfn f(p: Page) -> Int\n    requires p == Page { ..p }\n{\n    1\n}\n");
    assert!(has(&e, codes::SYNTAX_ERROR));
    let e = body_errors("while x == Page { ..p } {\n}\n1");
    assert!(has(&e, codes::SYNTAX_ERROR));
}

#[test]
fn bad_escape_and_unterminated_text() {
    assert!(has(&body_errors("\"\\q\""), codes::INVALID_ESCAPE));
    assert!(has(&body_errors("\"\\u{DFFF}\""), codes::INVALID_ESCAPE));
    let e = body_errors("\"abc");
    assert!(has(&e, codes::UNTERMINATED_TEXT_LITERAL));
    let e = errors("module m\n\ntest \"unterminated {\n    assert true\n}\n");
    assert!(has(&e, codes::UNTERMINATED_TEXT_LITERAL));
}

#[test]
fn misplaced_holes() {
    assert_eq!(code_list(&body_errors("?Foo")), vec![codes::HOLE_NAME_CASE]);
    assert!(has(&body_errors("?h.len"), codes::SYNTAX_ERROR));
    assert!(has(&body_errors("x?y"), codes::SYNTAX_ERROR));
    // A hole in a type position.
    assert!(has(
        &errors("module m\n\nfn f() -> ?t {\n    1\n}\n"),
        codes::SYNTAX_ERROR
    ));
    // A hole as a pattern.
    assert!(has(
        &body_errors("match x {\n    ?p => 1\n}"),
        codes::SYNTAX_ERROR
    ));
    // A hole as a statement target.
    assert!(has(&body_errors("?x = 1\n1"), codes::SYNTAX_ERROR));
}

#[test]
fn missing_return_type() {
    let e = errors("module m\n\nfn f() {\n    1\n}\n");
    assert_eq!(code_list(&e), vec![codes::SYNTAX_ERROR]);
    assert!(message(&e, "expected `->`"));
    assert!(has(
        &errors("module m\n\ncapability C {\n    fn get(id: Id)\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\nfn f() -> {\n    1\n}\n"),
        codes::SYNTAX_ERROR
    ));
}

#[test]
fn continuation_keyword_without_a_newline() {
    let e = errors("module m\n\nfn f(x: Int) -> Int requires x > 0\n{\n    1\n}\n");
    assert!(has(&e, codes::SYNTAX_ERROR) && message(&e, "a clause must start its own line"));
    let e = body_errors("while c invariant x\n{\n}\n1");
    assert!(message(&e, "a clause must start its own line"));
    let e = errors("module m\n\nproperty \"p\" for x in xs\n{\n    assert true\n}\n");
    assert!(message(&e, "a clause must start its own line"));
    let e = errors("module m\n\ncapability C {\n    fn get(id: Id) -> Int requires id != x\n}\n");
    assert!(message(&e, "a clause must start its own line"));
    // Two clauses on one line.
    let e = errors(
        "module m\n\nfn f(x: Int) -> Int\n    requires x > 0 ensures result > 0\n{\n    1\n}\n",
    );
    assert!(has(&e, codes::SYNTAX_ERROR));
    // The body's `{` on the clause line.
    let e = errors("module m\n\nfn f(x: Int) -> Int\n    requires x > 0 {\n    1\n}\n");
    assert!(message(&e, "must start a new line"));
    // A `fn` header with nothing after it is the bodiless form of grammar
    // 3.9 (third `fn_tail` alternative) and parses; `expected { or a clause`
    // arises only for `while`, `property`, and `patch` headers (3.9, 2.4
    // rule 3).
    let r = parse_module(
        "module m\n\nfn f(x: Int) -> Int\n\nfn g() -> Int { 1 }\n",
        FileId(0),
    );
    assert!(
        r.value.is_some() && r.diagnostics.is_empty(),
        "{:#?}",
        r.diagnostics
    );
    let e = body_errors("while c\n1");
    assert!(message(&e, "expected `{` or a clause"));
    let e = errors("module m\n\nproperty \"p\"\n\nfn g() -> Int { 1 }\n");
    assert!(has(&e, codes::SYNTAX_ERROR));
    let r = parse_patch("patch x expect base(x)\n{\n}\n", FileId(0));
    assert!(r.value.is_none() && message(&r.diagnostics, "`expect` must start its own line"));
}

#[test]
fn duplicate_clauses() {
    let e = errors("module m\n\nfn f(using db: Db) -> Int\n    effects { db.get }\n    effects { db.put }\n{\n    1\n}\n");
    assert_eq!(code_list(&e), vec![codes::DUPLICATE_CLAUSE]);
    assert!(message(&e, "duplicate effects clause"));
    let e =
        errors("module m\n\nfn f(x: Int) -> Int\n    decreases x\n    decreases x\n{\n    1\n}\n");
    assert!(message(&e, "duplicate decreases clause"));
    let e = body_errors("while c\n    decreases x\n    decreases y\n{\n}\n1");
    assert!(has(&e, codes::DUPLICATE_CLAUSE));
    // Several requires/ensures/invariant clauses are fine.
    let r = parse_module("module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n    requires x < 9\n    ensures result > 0\n    ensures result < 9\n{\n    x\n}\n", FileId(0));
    assert!(r.diagnostics.is_empty(), "{:#?}", r.diagnostics);
}

#[test]
fn ordering_and_placement_codes() {
    assert!(has(
        &errors("module m\n\nfn f() -> Int { 1 }\nuse a.b\n"),
        codes::USE_AFTER_DECLARATION
    ));
    assert!(has(
        &errors("module m\n\nfn f(using db: Db, x: Int) -> Int { 1 }\n"),
        codes::POSITIONAL_AFTER_USING
    ));
    assert!(has(
        &body_errors("f(using db, x)"),
        codes::POSITIONAL_AFTER_USING
    ));
    assert!(has(
        &body_errors("if c { 1 }\nelse { 2 }"),
        codes::MISPLACED_ELSE
    ));
    assert!(has(
        &body_errors("let loop = 1\nloop"),
        codes::RESERVED_WORD
    ));
    let e = errors("module m\n\nfn f() -> Int {\n    /// not here\n    1\n}\n");
    assert!(
        has(&e, codes::DOC_COMMENT_POSITION)
            && message(&e, "doc comment must precede a declaration")
    );
    assert!(has(
        &errors("module m\n\nfn f() -> Int { 1 }\n/// dangling at eof\n"),
        codes::DOC_COMMENT_POSITION
    ));
    assert!(has(
        &errors("module m\n\nfn f() -> Int { 1 } /// after code\n"),
        codes::DOC_COMMENT_POSITION
    ));
    assert!(has(
        &errors("module m\n\nfn f() -> Int { 1 }; fn g() -> Int { 2 }\n"),
        codes::INVALID_CHARACTER
    ));
    assert!(has(&body_errors("a | b"), codes::INVALID_CHARACTER));
    assert!(has(&body_errors("!a"), codes::INVALID_CHARACTER));
    assert!(has(&body_errors("007"), codes::INVALID_INT_LITERAL));
    assert!(has(&body_errors("1.5"), codes::FLOAT_LITERAL_UNSUPPORTED));
    assert!(has(&body_errors("2x"), codes::LITERAL_ADJACENT_IDENTIFIER));
    assert!(has(
        &body_errors("b\"x\""),
        codes::PREFIXED_LITERAL_RESERVED
    ));
    assert!(has(&body_errors("é"), codes::NON_ASCII_OUTSIDE_TEXT));
}

#[test]
fn work_and_patch_codes() {
    let w = |src: &str| {
        let r = parse_work(src, FileId(0));
        assert!(r.value.is_none(), "expected errors for {src}");
        r.diagnostics
    };
    let e = w("change c {\n    integrate {\n        require lint\n    }\n}\n");
    assert_eq!(code_list(&e), vec![codes::UNKNOWN_REQUIREMENT]);
    assert_eq!(
        e[0].data.get("found").and_then(|v| v.as_str()),
        Some("identifier `lint`")
    );
    let e = w("change c {\n    workers 1\n    workers 2\n}\n");
    assert_eq!(code_list(&e), vec![codes::DUPLICATE_WORK_ITEM]);
    assert_eq!(
        e[0].data.get("name").and_then(|v| v.as_str()),
        Some("workers")
    );
    let e = w("change c {\n    task t {\n        goal \"a\"\n        goal \"b\"\n    }\n}\n");
    assert!(has(&e, codes::DUPLICATE_WORK_ITEM));
    let e = w("change c {\n    integrate {\n    }\n    integrate {\n    }\n}\n");
    assert!(has(&e, codes::DUPLICATE_WORK_ITEM));
    let e = w("change c {\n    task t {\n        add { paginate.body, tests.* }\n    }\n}\n");
    assert_eq!(code_list(&e), vec![codes::SYNTAX_ERROR]);
    let e = w("change c {\n    frobnicate\n}\n");
    assert!(has(&e, codes::SYNTAX_ERROR));
    let e = w("change C {\n}\n");
    assert!(has(&e, codes::SYNTAX_ERROR));
}

#[test]
fn miscellaneous_syntax_errors() {
    assert!(has(
        &errors("module m\n\nenum E {\n    A()\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\ntype A = T\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\npub test \"t\" {}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\nfn f<T,>() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(&errors("module m\n\nuse trim\n"), codes::SYNTAX_ERROR));
    assert!(has(
        &body_errors("Page { text: t, ..p }"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(&body_errors("x.type"), codes::SYNTAX_ERROR));
    assert!(has(&body_errors("a..b"), codes::SYNTAX_ERROR));
    assert!(has(
        &body_errors("match x {\n    A => 1,\n    B => 2\n}"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &body_errors("if a { 1 } elif b { 2 }"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(&errors("fn f() -> Int { 1 }\n"), codes::SYNTAX_ERROR));
    assert!(has(
        &errors("module m\n\nfn f() -> Int {\n    fn g() -> Int { 1 }\n    1\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\n@prove pub fn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\n@id(x)\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\nfn f() -> Int { 1 } fn g() -> Int { 2 }\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\nstruct S {\n    x: Int, y: Int\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(
        &errors("module m\n\nstruct S {\n    x: int\n}\n"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(&body_errors("let X = 1\n1"), codes::SYNTAX_ERROR));
    assert!(has(&body_errors("return 1 2"), codes::SYNTAX_ERROR));
    assert!(has(&body_errors("assert"), codes::SYNTAX_ERROR));
    assert!(has(&body_errors("let x: = 1\nx"), codes::SYNTAX_ERROR));
    assert!(has(
        &body_errors("match x {\n    Ok() => 1\n}"),
        codes::SYNTAX_ERROR
    ));
    assert!(has(&body_errors("forall x in xs"), codes::SYNTAX_ERROR));
    assert!(has(&body_errors("{ 1 }"), codes::SYNTAX_ERROR));
}

#[test]
fn a_file_stops_at_its_first_error() {
    // Grammar D59: "A file stops at its first lexical or syntax error: 0.1
    // performs no error recovery or resynchronisation" (also toolchain.md
    // T6 and semantics.md 9.4). Several independent mistakes yield exactly
    // one diagnostic, the earliest in the source.
    let src = "module m\n\nfn a() -> Int {\n    let x = 1 let y = 2\n    a < b < c\n    x +\n    x\n}\n\nfn b() {\n    1\n}\n\nfn c(x: Int) -> Int requires x > 0\n{\n    x\n}\n\nfn d() -> Int { 1 }\n";
    let e = errors(src);
    assert_eq!(e.len(), 1, "{e:#?}");
    assert_eq!(code_list(&e), vec![codes::SYNTAX_ERROR]);
    assert!(message(&e, "expected end of line after the statement"));
    let second_let = src.find("let y").unwrap() as u32;
    assert_eq!(e[0].span.unwrap().start, second_let);
    // Three bad statements in one body: still one diagnostic.
    let e = errors("module m\n\nfn a() -> Int {\n    let = 1\n    var = 2\n    assert\n    1\n}\n\nfn b() -> Int {\n    let x = )\n    1\n}\n");
    assert_eq!(e.len(), 1, "{e:#?}");
    // A lexical error before a syntax error: the lexical one is reported.
    let e = errors("module m\n\nfn f() -> Int { 1; }\n\nfn g() -> Int { let x = ) }\n");
    assert_eq!(code_list(&e), vec![codes::INVALID_CHARACTER]);
    // A syntax error before a lexical error: the syntax one is reported, as
    // a single-pass implementation would meet it first.
    let e = errors("module m\n\nfn g() -> Int { let x = ) }\n\nfn f() -> Int { 1; }\n");
    assert_eq!(code_list(&e), vec![codes::SYNTAX_ERROR]);
    // The same rule for the other two file forms.
    let r = parse_work(
        "change c {\n    workers 1\n    workers 2\n    workers 3\n}\n",
        FileId(0),
    );
    assert!(r.value.is_none());
    assert_eq!(r.diagnostics.len(), 1, "{:#?}", r.diagnostics);
    let r = parse_patch(
        "patch a expect base(a)\n{\n}\npatch b expect base(b)\n{\n}\n",
        FileId(0),
    );
    assert!(r.value.is_none());
    assert_eq!(r.diagnostics.len(), 1, "{:#?}", r.diagnostics);
}

#[test]
fn the_earliest_error_is_reported_even_when_a_check_needs_later_tokens() {
    // Grammar D70 [D-226]: the one diagnostic of a failed file is the error
    // with the smallest start offset. A check that could only be made after
    // more of the input is read (the class of a use-path segment, the kind
    // of an `add` selector, a duplicate clause or task name) is made as soon
    // as its token is read, so a later error never takes its place.
    let at = |src: &str, needle: &str| src.find(needle).unwrap() as u32;
    // `use a.B.{}`: the upper-case path segment, not the empty list.
    let src = "module m\n\nuse a.B.{}\n";
    let e = errors(src);
    assert_eq!(code_list(&e), vec![codes::SYNTAX_ERROR]);
    assert_eq!(e[0].span.unwrap().start, at(src, "B"), "{e:#?}");
    assert!(message(&e, "expected a module name"), "{e:#?}");
    assert_eq!(
        e[0].data.get("found").and_then(|v| v.as_str()),
        Some("identifier `B`")
    );
    for src in ["module m\n\nuse a.B.\n", "module m\n\nuse a.B.{c,,}\n"] {
        let e = errors(src);
        assert_eq!(e[0].span.unwrap().start, at(src, "B"), "{src:?}: {e:#?}");
    }
    // An `add` selector that is not a namespace, before a broken selector.
    let src = "change c {\n    task t {\n        add { a.b, c. }\n    }\n}\n";
    let r = parse_work(src, FileId(0));
    assert_eq!(r.diagnostics.len(), 1, "{:#?}", r.diagnostics);
    assert!(r.diagnostics[0]
        .message
        .starts_with("add names a namespace; write a.b.*"));
    assert_eq!(r.diagnostics[0].span.unwrap().start, at(src, "a.b"));
    // A duplicate task name, before an error in that task's body.
    let src = "change c {\n    task t {}\n    task t {\n        goal 1\n    }\n}\n";
    let r = parse_work(src, FileId(0));
    assert_eq!(r.diagnostics.len(), 1, "{:#?}", r.diagnostics);
    assert_eq!(r.diagnostics[0].code, codes::DUPLICATE_WORK_ITEM);
    assert_eq!(r.diagnostics[0].span.unwrap().start, at(src, "t {\n"));
    // A second `effects` or `decreases` clause, at its keyword, before an
    // error inside it.
    let src = "module m\n\nfn f(using db: Db) -> Int\n    effects { db.get }\n    effects { db. }\n{\n    1\n}\n";
    let e = errors(src);
    assert_eq!(code_list(&e), vec![codes::DUPLICATE_CLAUSE]);
    assert_eq!(e[0].span.unwrap().start, at(src, "effects { db. }"));
    assert_eq!(e[0].span.unwrap().len(), "effects".len() as u32);
    let src = "module m\n\nfn f(x: Int) -> Int\n    decreases x\n    decreases (x\n{\n    1\n}\n";
    let e = errors(src);
    assert_eq!(code_list(&e), vec![codes::DUPLICATE_CLAUSE]);
    assert_eq!(e[0].span.unwrap().start, at(src, "decreases (x"));
    let e = body_errors("while c\n    decreases x\n    decreases )\n{\n}\n1");
    assert_eq!(code_list(&e), vec![codes::DUPLICATE_CLAUSE]);
}

#[test]
fn module_doc_comments_not_followed_by_the_header_are_misplaced() {
    // Grammar 1.2: doc comments before the `module` header document the
    // module; ones followed by anything else, the end of the file included,
    // are `doc comment must precede a declaration`, reported at the doc
    // comment, as in every other admitted position.
    for src in [
        "/// d",
        "/// d\n",
        "/// d\nfn f() -> Int { 1 }\n",
        "/// d\n\nelse\n",
    ] {
        let e = errors(src);
        assert_eq!(code_list(&e), vec![codes::DOC_COMMENT_POSITION], "{src:?}");
        assert_eq!(e[0].span.unwrap().start, 0, "{src:?}");
    }
    // Followed by the header (after blank and comment lines) they are the
    // module's docs.
    let r = parse_module("/// d\n\n// c\nmodule m\n", FileId(0));
    assert!(r.diagnostics.is_empty(), "{:#?}", r.diagnostics);
    assert_eq!(r.value.unwrap().docs.unwrap().text(), "d");
}

const GARBAGE: &[&str] = &[
    "",
    " ",
    "\n\n\n",
    "\u{0}\u{1}\u{2}\u{ff}",
    "{{{{{{{{{{",
    "}}}}}}}}}}",
    "((((((((((",
    "))))))))))",
    "][][][][",
    "module",
    "module .",
    "module a.",
    "module a..b",
    "module a b c",
    "fn",
    "fn f(",
    "fn f() ->",
    "fn f() -> Int",
    "fn f() -> Int {",
    "fn f() -> Int {\n    let",
    "fn f() -> Int {\n    let x =",
    "fn f() -> Int {\n    let x = (",
    "match x {",
    "match x { A =>",
    "\"unterminated",
    "\"\\",
    "\"\\u{",
    "@",
    "@id(",
    "@id(\"x\"",
    "@id(\"x\")",
    "use",
    "use a.b.{",
    "use a.b.{,}",
    "change x {",
    "change x {\n    task {",
    "task {",
    "integrate {",
    "patch",
    "patch x",
    "patch x\n    expect",
    "patch x\n    expect base(",
    "patch x\n    expect base(x)\n{",
    "///",
    "/// doc at eof",
    "?",
    "?Foo?",
    "??",
    "<<>><<>>",
    "+ - * / % ++ == != < <= > >= = -> => ? @ . .. , : ( ) [ ] { }",
    "漢字 ∀x∈S → y",
    "\u{feff}",
    "\u{feff}\u{feff}module",
    "a\rb\rc",
    "\t\t\t",
    "module m\nfn f() -> Int {\n    if\n}",
    "module m\nfn f() -> Int { if c { } else }",
    "module m\nstruct S {\n    x:\n}",
    "module m\nenum E {\n    A(\n}",
    "module m\npub pub pub fn",
    "module m\ntest {",
    "module m\nproperty \"p\"\n    for\n{",
    "module m\nfn f<>() -> Int {}",
    "module m\nfn f<T,>() -> Int {}",
    "module m\nfn f() -> List< {}",
    "module m\nfn f() -> Int {\n    x = \n}",
    "module m\nfn f() -> Int {\n    while\n}",
    "module m\nfn f() -> Int {\n    while c\n        invariant\n    {\n    }\n}",
    "module m\nfn f() -> Int {\n    forall\n}",
    "module m\nfn f() -> Int {\n    forall x in\n}",
    "module m\nfn f() -> Int {\n    Page {\n}",
    "module m\nfn f() -> Int {\n    Page { ..\n}",
    "module m\nfn f() -> Int {\n    f(using\n}",
    "module m\nfn f() -> Int {\n    match x {\n        Ok( => 1\n    }\n}",
    "module m\nfn f() -> Int {\n    not not not not not not not not not not not not not not not not not not\n}",
    "change c {\n    task t after {\n    }\n}",
    "change c {\n    integrate {\n        require prove(\n    }\n}",
    "change c {\n    task t {\n        write { a.b.c.\n    }\n}",
];

#[test]
fn no_panic_on_garbage_through_every_parser() {
    for (i, src) in GARBAGE.iter().enumerate() {
        let a = parse_module(src, FileId(0));
        let b = parse_work(src, FileId(0));
        let c = parse_patch(src, FileId(0));
        for r in [&a.diagnostics, &b.diagnostics, &c.diagnostics] {
            for d in r {
                assert!(
                    codes::ALL.contains(&d.code),
                    "garbage #{i}: unknown code {}",
                    d.code
                );
            }
        }
        let (tokens, _) = cpl_syntax::lex(src, FileId(0));
        assert_eq!(
            tokens.last().map(|t| t.kind),
            Some(cpl_syntax::TokenKind::Eof),
            "garbage #{i}"
        );
    }
}

fn wrap(inner: &str) -> String {
    format!("module m\n\nfn f() -> Int {{\n    {inner}\n}}\n")
}

fn deep_cases() -> Vec<String> {
    vec![
        wrap(&"(".repeat(5000)),
        wrap(&format!("{}1{}", "(".repeat(5000), ")".repeat(5000))),
        wrap(&"[".repeat(5000)),
        wrap(&"if c {\n".repeat(3000)),
        wrap(&format!("{}x", "not ".repeat(5000))),
        wrap(&format!("{}x", "- ".repeat(5000))),
        wrap(&format!("match x {{\n{}", "Ok(".repeat(5000))),
        wrap(&format!("Page {{ a: {}", "Page { a: ".repeat(3000))),
        wrap(&format!("f({}", "f(".repeat(5000))),
        wrap(&format!("if a {{ 1 }} {}", "else if a { 1 } ".repeat(3000))),
        wrap(&format!(
            "forall x in xs: {}true",
            "forall x in xs: ".repeat(3000)
        )),
        format!(
            "module m\n\nfn f() -> {}Int{} {{\n    1\n}}\n",
            "List<".repeat(5000),
            ">".repeat(5000)
        ),
        format!(
            "module m\n\nfn f(x: {}Int{}) -> Int {{\n    1\n}}\n",
            "List<".repeat(5000),
            ">".repeat(5000)
        ),
        wrap(&format!(
            "if {}c {{}} {}",
            "if ".repeat(3000),
            "{} ".repeat(3000)
        )),
        wrap(&format!("match {}x {{\n}}", "match ".repeat(3000))),
        wrap(&format!("{}1{}", "[".repeat(30), "]".repeat(30))),
    ]
}

fn run_deep_cases() {
    let cases = deep_cases();
    for (i, src) in cases.iter().enumerate() {
        let r = parse_module(src, FileId(0));
        let _ = parse_work(src, FileId(0));
        let _ = parse_patch(src, FileId(0));
        if i == cases.len() - 1 {
            assert!(
                r.value.is_some(),
                "30 levels of nesting are fine: {:#?}",
                r.diagnostics
            );
        } else {
            assert!(r.value.is_none(), "case #{i} should be rejected");
            assert!(
                r.diagnostics
                    .iter()
                    .any(|d| d.message.contains("nesting is too deep")),
                "case #{i}: {:#?}",
                r.diagnostics
            );
        }
    }
    // Long operator chains build deep trees; they are capped by the operator budget.
    for src in [
        wrap(&format!("1{}", " + 1".repeat(20_000))),
        wrap(&format!("a{}", ".b".repeat(20_000))),
        wrap(&format!("f{}", "()".repeat(20_000))),
        wrap(&format!("x{}", "?".repeat(20_000))),
        wrap(&format!("a{}", " and b or c < d * e + f".repeat(5_000))),
    ] {
        let r = parse_module(&src, FileId(0));
        assert!(r.value.is_none());
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("too many operators")),
            "{:#?}",
            r.diagnostics
        );
    }
    // Within the budget everything parses, and long flat statement lists are unbounded.
    let flat = wrap(&format!("1{}", " + 1".repeat(900)));
    assert!(parse_module(&flat, FileId(0)).value.is_some());
    let chain = wrap(&format!("a{}", ".b".repeat(900)));
    assert!(parse_module(&chain, FileId(0)).value.is_some());
    let stmts = wrap(&"let _ = 1\n    ".repeat(20_000));
    assert!(parse_module(&stmts, FileId(0)).value.is_some());
    let arms = wrap(&format!(
        "match x {{\n{}    }}",
        "        _ => 1\n".repeat(5_000)
    ));
    assert!(parse_module(&arms, FileId(0)).value.is_some());
}

#[test]
fn deep_nesting_is_rejected_without_a_crash() {
    run_deep_cases();
}

#[test]
fn deep_nesting_fits_in_one_mebibyte_of_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(run_deep_cases)
        .expect("spawn");
    handle
        .join()
        .expect("the parser must not overflow a 1 MiB stack");
}

fn corpus_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("examples dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples"),
        &mut files,
    );
    files.sort();
    files
}

#[test]
fn no_panic_on_every_truncation_of_the_corpus() {
    let mut checked = 0usize;
    for path in corpus_files() {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        if !matches!(ext.as_str(), "cpl" | "work" | "patch") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read corpus file");
        let mut boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        boundaries.push(text.len());
        for (n, &end) in boundaries.iter().enumerate() {
            let prefix = &text[..end];
            match ext.as_str() {
                "cpl" => {
                    parse_module(prefix, FileId(0));
                }
                "work" => {
                    parse_work(prefix, FileId(0));
                }
                _ => {
                    parse_patch(prefix, FileId(0));
                }
            }
            if n % 37 == 0 {
                parse_module(prefix, FileId(0));
                parse_work(prefix, FileId(0));
                parse_patch(prefix, FileId(0));
            }
            checked += 1;
        }
    }
    assert!(
        checked > 1000,
        "expected to check many prefixes, checked {checked}"
    );
}
