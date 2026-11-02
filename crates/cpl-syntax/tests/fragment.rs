//! `format_fragment` (`spec/toolchain.md` 1.5 and 5.2): the canonical text
//! of every facet a declaration has (5.1), the canonical clause text of
//! single expressions (semantics 9.2 `clause`, the HIR's `Clause::text`,
//! toolchain 3.3), types and selectors, and the `Source` rendering that
//! patch application splices into a file (10.5).

use cpl_diag::{FileId, Span};
use cpl_syntax::ast::*;
use cpl_syntax::{
    format_fragment, format_module, parse_module, parse_patch, parse_work, Fragment, FragmentMode,
};
use std::path::Path;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[track_caller]
fn module(src: &str) -> Module {
    let r = parse_module(src, FileId(0));
    match r.value {
        Some(m) if r.diagnostics.is_empty() => m,
        _ => panic!("expected a clean parse of:\n{src}\n{:#?}", r.diagnostics),
    }
}

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn corpus(relative: &str) -> Module {
    module(&read(relative))
}

#[track_caller]
fn decl<'a>(m: &'a Module, name: &str) -> &'a Declaration {
    m.declarations
        .iter()
        .find(|d| d.display_name() == name)
        .unwrap_or_else(|| panic!("no declaration {name}"))
}

#[track_caller]
fn fn_of<'a>(m: &'a Module, name: &str) -> &'a FnDecl {
    match &decl(m, name).kind {
        DeclKind::Fn(f) => f,
        other => panic!("{name} is not a fn: {other:?}"),
    }
}

fn canonical(f: Fragment<'_>) -> String {
    format_fragment(f, FragmentMode::Canonical)
}

fn source(f: Fragment<'_>) -> String {
    format_fragment(f, FragmentMode::Source)
}

/// The canonical text of every `requires`/`ensures` expression of a fn, in
/// source order.
fn clause_texts(f: &FnDecl) -> Vec<String> {
    f.contract()
        .map(|c| canonical(Fragment::Expr(&c.expr)))
        .collect()
}

fn text_literal(value: &str) -> Expr {
    let span = Span::point(FileId(0), 0);
    Expr {
        span,
        kind: ExprKind::Literal(Literal {
            span,
            kind: LiteralKind::Text(value.to_string()),
        }),
    }
}

// ---------------------------------------------------------------------------
// Expressions: the canonical clause text
// ---------------------------------------------------------------------------

#[test]
fn canonical_clause_texts_of_the_corpus() {
    // The texts other crates' fixtures depend on (HIR `Clause::text`,
    // semantics 9.2 `clause`, the `contract` facet of 5.2).
    let pages = corpus("examples/reader/src/reader/pages.cpl");
    assert_eq!(
        clause_texts(fn_of(&pages, "paginate")),
        [
            "flatten(result) == items",
            "forall page in result: page.len > 0 and page.len <= size",
            "forall page in result.drop_last(): page.len == size",
        ]
    );
    assert_eq!(
        clause_texts(fn_of(&pages, "divide_exactly")),
        ["b != 0", "a % b == 0", "result * b == a"]
    );
    let holes = corpus("examples/holes/src/holes/pages.cpl");
    assert_eq!(
        clause_texts(fn_of(&holes, "paginate")),
        clause_texts(fn_of(&pages, "paginate"))
    );
    // The refinement predicate is a clause too (`RefinementDef::predicate`).
    let DeclKind::Type(TypeDecl {
        kind: TypeDeclKind::Refinement(r),
        ..
    }) = &decl(&pages, "Positive").kind
    else {
        panic!("Positive is a refinement")
    };
    assert_eq!(canonical(Fragment::Expr(&r.predicate)), "self > 0");
    // Loop invariants and measures (`While::invariants`, `decreases`).
    let body = fn_of(&pages, "paginate").body.as_ref().expect("body");
    let Statement::While(w) = &body.statements[2] else {
        panic!("the third statement is the loop")
    };
    let invariants: Vec<String> = w
        .invariants()
        .map(|c| canonical(Fragment::Expr(&c.expr)))
        .collect();
    assert_eq!(
        invariants,
        [
            "flatten(pages) ++ rest == items",
            "forall page in pages: page.len == size",
        ]
    );
    assert_eq!(
        canonical(Fragment::Expr(&w.decreases().expect("decreases").expr)),
        "rest.len"
    );
}

#[test]
fn canonical_clause_text_ignores_spacing_line_breaks_and_comments() {
    // Semantics 9.2: `requires b  !=  0` and `requires b != 0` both yield
    // "b != 0"; 5.2: source line breaks are not reproduced, parentheses are
    // kept exactly as written.
    let m = module(concat!(
        "module m\n\n",
        "fn f(a: Int, b: Int) -> List<Int>\n",
        "    requires b  !=  0\n",
        "    requires (a > 0\n",
        "        and b > 0)\n",
        "    requires g(\n",
        "        a, // first\n",
        "        // second\n",
        "        b,\n",
        "    ) == ((a))\n",
        "    ensures result  ==  [\n",
        "        1,\n",
        "        2,\n",
        "    ]\n",
        "    ensures not   (-a < b)\n",
        "    ensures (Pair { ..p,\n",
        "        first: a }) == p\n",
        "    ensures \"a\\u{9}b\\u{41}\" ++ t == u\n",
        "    ensures check<List<List<Int>>>(x)?.len == ?hole\n",
        "{\n",
        "    [a, b]\n",
        "}\n",
    ));
    assert_eq!(
        clause_texts(fn_of(&m, "f")),
        [
            "b != 0",
            "(a > 0 and b > 0)",
            "g(a, b) == ((a))",
            "result == [1, 2]",
            "not (-a < b)",
            "(Pair { ..p, first: a }) == p",
            "\"a\\tbA\" ++ t == u",
            "check<List<List<Int>>>(x)?.len == ?hole",
        ]
    );
    // No clause is broken for width either: a clause is one line.
    let long_name = "a_rather_long_name_that_takes_up_a_good_part_of_the_line";
    let src = format!(
        "module m\n\nfn f() -> Bool\n    ensures g({long_name}, {long_name}, {long_name})\n{{\n    true\n}}\n"
    );
    let m = module(&src);
    let text = &clause_texts(fn_of(&m, "f"))[0];
    assert!(text.len() > 100 && !text.contains('\n'), "{text}");
}

#[test]
fn source_expression_text_keeps_comments_and_layout_memory() {
    let m = module(concat!(
        "module m\n\n",
        "fn f(a: Int, b: Int) -> Bool\n",
        "    requires (a > 0\n",
        "        and b > 0)\n",
        "    requires g(\n",
        "        a, // first\n",
        "        b,\n",
        "    ) == 1\n",
        "    requires h(a, b)\n",
        "{\n",
        "    true\n",
        "}\n",
    ));
    let f = fn_of(&m, "f");
    let texts: Vec<String> = f
        .contract()
        .map(|c| source(Fragment::Expr(&c.expr)))
        .collect();
    // Rule 9: the break before `and` inside the parentheses is kept, one
    // level deeper than the line of the `(` (level 0 here).
    assert_eq!(texts[0], "(a > 0\n    and b > 0)");
    // Rule 6: a group with a comment or written broken stays broken.
    assert_eq!(texts[1], "g(\n    a, // first\n    b,\n) == 1");
    assert_eq!(texts[2], "h(a, b)");
}

#[test]
fn an_expression_containing_a_block_spans_lines() {
    // Rule 8: blocks are always multi-line and match arms take a line each,
    // so these are the one place where canonical expression text has line
    // breaks; the arms are indented from level 0.
    let m = module(concat!(
        "module m\n\n",
        "fn f(r: Result<Int, Int>) -> Int\n",
        "    ensures match result { Ok(v) => v > 0\n",
        "        Err(_) => true }\n",
        "    ensures if r.is_ok() { true } else { false }\n",
        "{\n",
        "    1\n",
        "}\n",
    ));
    assert_eq!(
        clause_texts(fn_of(&m, "f")),
        [
            "match result {\n    Ok(v) => v > 0\n    Err(_) => true\n}",
            "if r.is_ok() {\n    true\n} else {\n    false\n}",
        ]
    );
}

#[test]
fn the_name_facet_of_a_test_is_its_escaped_text_literal() {
    // 5.2: the `name` of a test or property is the canonically escaped text
    // literal with its quotes, the rendering of a `Text` literal expression.
    let tests = corpus("examples/reader/src/tests/pagination.cpl");
    let names: Vec<String> = tests
        .declarations
        .iter()
        .map(|d| canonical(Fragment::Expr(&text_literal(d.display_name()))))
        .collect();
    assert_eq!(
        names,
        [
            "\"pagination preserves a short tail\"",
            "\"pagination handles empty input\"",
            "\"pagination preserves input\"",
        ]
    );
    assert_eq!(
        canonical(Fragment::Expr(&text_literal(
            "tab\there \"q\" \\ \u{1}\u{7f}é"
        ))),
        "\"tab\\there \\\"q\\\" \\\\ \\u{1}\\u{7f}é\""
    );
}

// ---------------------------------------------------------------------------
// Signature
// ---------------------------------------------------------------------------

#[test]
fn signatures_of_the_corpus_declarations() {
    let pages = corpus("examples/reader/src/reader/pages.cpl");
    let sig = |name: &str| canonical(Fragment::Signature(decl(&pages, name)));
    assert_eq!(
        sig("paginate"),
        "pub fn _<T>(items: List<T>, size: Positive) -> List<List<T>>"
    );
    assert_eq!(sig("PageId"), "pub type _(Text)");
    assert_eq!(sig("Positive"), "pub type _ = Int");
    assert_eq!(
        sig("Page"),
        "pub struct _ {\n    id: PageId\n    text: Text\n}"
    );
    assert_eq!(
        sig("StoreError"),
        "pub enum _ {\n    Missing(PageId)\n    Unavailable\n}"
    );
    assert_eq!(
        sig("PageRead"),
        "pub capability _ {\n    fn get(id: PageId) -> Result<Page, StoreError>\n}"
    );
    assert_eq!(sig("clean"), "fn _(page: Page) -> Page");
    // Parameters are always flat in the signature, `using` ones included,
    // although the corpus breaks these parameter lists (T12).
    assert_eq!(
        sig("load_page"),
        "pub fn _(id: PageId, using db: PageRead) -> Result<Page, StoreError>"
    );
    assert_eq!(
        sig("paginate_ints"),
        "pub fn _(items: List<Int>, size: Int) -> Result<List<List<Int>>, ConstraintError>"
    );
    // A property's signature is its `for` clauses; a test has none.
    let tests = corpus("examples/reader/src/tests/pagination.cpl");
    assert_eq!(
        canonical(Fragment::Signature(decl(
            &tests,
            "pagination preserves input"
        ))),
        "for items in gen.list(gen.int)\nfor size in gen.positive_int"
    );
    assert_eq!(
        canonical(Fragment::Signature(decl(
            &tests,
            "pagination handles empty input"
        ))),
        ""
    );
}

#[test]
fn signatures_of_std_stubs_and_member_docs() {
    // std opaque types (`pub type _<T>`), generic enums, bodiless functions;
    // member doc comments belong to `docs`, not to the signature (T25).
    let prelude = corpus("std/prelude.cpl");
    let sig = |name: &str| canonical(Fragment::Signature(decl(&prelude, name)));
    assert_eq!(sig("List"), "pub type _<T>");
    assert_eq!(sig("Int"), "pub type _");
    assert_eq!(sig("Option"), "pub enum _<T> {\n    Some(T)\n    None\n}");
    assert_eq!(
        sig("ConstraintError"),
        "pub struct _ {\n    type_name: Text\n    message: Text\n}"
    );
    assert_eq!(
        sig("flatten"),
        "pub fn _<T>(lists: List<List<T>>) -> List<T>"
    );
    let gen = corpus("std/gen.cpl");
    assert_eq!(
        canonical(Fragment::Signature(decl(&gen, "int"))),
        "pub fn _() -> Gen<Int>"
    );
}

#[test]
fn signatures_drop_comments_and_layout_but_source_mode_keeps_them() {
    let m = module(concat!(
        "module m\n\n",
        "/// Docs are the docs facet.\n",
        "@id(\"s_1\")\n",
        "struct Pair<A, B> { // the pair\n",
        "    /// The first.\n",
        "    first: A // left\n",
        "\n",
        "    // the second\n",
        "    second: B\n",
        "    // dangling\n",
        "}\n\n",
        "enum E {\n",
        "    /// A variant.\n",
        "    V(\n",
        "        Int,\n",
        "        Text,\n",
        "    )\n",
        "    W\n",
        "}\n\n",
        "capability C {\n",
        "    /// Reads.\n",
        "    fn get(id: Int) -> Int // after get\n",
        "    fn put(id: Int) -> Int\n",
        "        // the precondition\n",
        "        requires id > 0\n",
        "}\n\n",
        "struct Empty {}\n\n",
        "enum NoVariants {\n",
        "    // nothing yet\n",
        "}\n",
    ));
    let pair = decl(&m, "Pair");
    assert_eq!(
        canonical(Fragment::Signature(pair)),
        "struct _<A, B> {\n    first: A\n    second: B\n}"
    );
    assert_eq!(
        source(Fragment::Signature(pair)),
        "struct _<A, B> {\n    // the pair\n    first: A // left\n\n    // the second\n    second: B\n    // dangling\n}"
    );
    assert_eq!(
        canonical(Fragment::Signature(decl(&m, "E"))),
        "enum _ {\n    V(Int, Text)\n    W\n}"
    );
    assert_eq!(
        source(Fragment::Signature(decl(&m, "E"))),
        "enum _ {\n    V(\n        Int,\n        Text,\n    )\n    W\n}"
    );
    // Operations without their contracts; the clause's comment goes with
    // the clause.
    assert_eq!(
        canonical(Fragment::Signature(decl(&m, "C"))),
        "capability _ {\n    fn get(id: Int) -> Int\n    fn put(id: Int) -> Int\n}"
    );
    assert_eq!(
        source(Fragment::Signature(decl(&m, "C"))),
        "capability _ {\n    fn get(id: Int) -> Int // after get\n    fn put(id: Int) -> Int\n}"
    );
    // Empty bodies print as the formatter prints them.
    assert_eq!(
        canonical(Fragment::Signature(decl(&m, "Empty"))),
        "struct _ {}"
    );
    assert_eq!(
        canonical(Fragment::Signature(decl(&m, "NoVariants"))),
        "enum _ {}"
    );
    assert_eq!(
        source(Fragment::Signature(decl(&m, "NoVariants"))),
        "enum _ {\n    // nothing yet\n}"
    );
}

#[test]
fn a_rename_leaves_the_signature_unchanged() {
    // T24: the name is replaced by `_`, so a rename is `identity_renamed`
    // only.
    let a = module("module m\n\npub fn before<T>(x: T) -> T {\n    x\n}\n");
    let b = module("module m\n\npub fn after<T>(x: T) -> T {\n    x\n}\n");
    assert_eq!(
        canonical(Fragment::Signature(&a.declarations[0])),
        canonical(Fragment::Signature(&b.declarations[0]))
    );
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[test]
fn contracts_of_the_corpus_declarations() {
    let pages = corpus("examples/reader/src/reader/pages.cpl");
    let contract = |name: &str| canonical(Fragment::Contract(decl(&pages, name)));
    // The 5.2 example.
    assert_eq!(
        contract("paginate"),
        "ensures flatten(result) == items\nensures forall page in result: page.len > 0 and page.len <= size\nensures forall page in result.drop_last(): page.len == size"
    );
    assert_eq!(
        contract("divide_exactly"),
        "requires b != 0\nrequires a % b == 0\nensures result * b == a"
    );
    assert_eq!(contract("Positive"), "where self > 0");
    // No clauses, or no contract facet: the empty string.
    for name in [
        "clean",
        "load_page",
        "PageId",
        "Page",
        "StoreError",
        "PageRead",
    ] {
        assert_eq!(contract(name), "", "{name}");
    }
}

#[test]
fn contract_clauses_print_in_canonical_order() {
    // requires in source order, then ensures in source order, then
    // decreases (5.2, T23), each on its own line without indentation; the
    // effects clause is not part of the contract.
    let m = module(concat!(
        "module m\n\n",
        "fn f(n: Int, using db: Db) -> Int\n",
        "    decreases n\n",
        "    // why the result is small\n",
        "    ensures result < 10 // bound\n",
        "    effects { db.get }\n",
        "    requires n >= 0\n",
        "    ensures result >= 0\n",
        "    requires n < 100\n",
        "{\n",
        "    0\n",
        "}\n\n",
        "capability Store {\n",
        "    fn get(id: Int) -> Int\n",
        "    fn put(id: Int) -> Int\n",
        "        ensures result == id\n",
        "        requires id > 0\n",
        "    fn del(id: Int) -> Int\n",
        "        requires id != 0\n",
        "}\n\n",
        "capability Plain {\n",
        "    fn get(id: Int) -> Int\n",
        "}\n",
    ));
    assert_eq!(
        canonical(Fragment::Contract(decl(&m, "f"))),
        "requires n >= 0\nrequires n < 100\nensures result < 10\nensures result >= 0\ndecreases n"
    );
    assert_eq!(
        source(Fragment::Contract(decl(&m, "f"))),
        "requires n >= 0\nrequires n < 100\n// why the result is small\nensures result < 10 // bound\nensures result >= 0\ndecreases n"
    );
    assert_eq!(
        canonical(Fragment::Contract(decl(&m, "Store"))),
        "fn put\n    requires id > 0\n    ensures result == id\nfn del\n    requires id != 0"
    );
    assert_eq!(canonical(Fragment::Contract(decl(&m, "Plain"))), "");
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

#[test]
fn effects_are_sorted_and_deduplicated_in_canonical_text() {
    let m = module(concat!(
        "module m\n\n",
        "fn f(using db: Db, using codec: Codec) -> Int\n",
        "    effects { db.put, codec.dimensions, db.get, db.put }\n",
        "{\n",
        "    0\n",
        "}\n\n",
        "fn g(using db: Db) -> Int\n",
        "    effects {\n",
        "        // writes first\n",
        "        db.put,\n",
        "        db.get, // then reads\n",
        "    }\n",
        "{\n",
        "    0\n",
        "}\n\n",
        "fn h() -> Int\n",
        "    effects {}\n",
        "{\n",
        "    0\n",
        "}\n",
    ));
    let effects = |name: &str| fn_of(&m, name).effects().expect("effects");
    // T26: a set; the author's order survives only in source (T13).
    assert_eq!(
        canonical(Fragment::Effects(effects("f"))),
        "effects { codec.dimensions, db.get, db.put }"
    );
    assert_eq!(
        source(Fragment::Effects(effects("f"))),
        "effects { db.put, codec.dimensions, db.get, db.put }"
    );
    assert_eq!(
        canonical(Fragment::Effects(effects("g"))),
        "effects { db.get, db.put }"
    );
    assert_eq!(
        source(Fragment::Effects(effects("g"))),
        "effects {\n    // writes first\n    db.put,\n    db.get, // then reads\n}"
    );
    // An empty set has empty canonical text (5.2).
    assert_eq!(canonical(Fragment::Effects(effects("h"))), "");
    assert_eq!(source(Fragment::Effects(effects("h"))), "effects {}");
    // The corpus.
    let pages = corpus("examples/reader/src/reader/pages.cpl");
    assert_eq!(
        canonical(Fragment::Effects(
            fn_of(&pages, "load_page").effects().expect("effects")
        )),
        "effects { db.get }"
    );
}

// ---------------------------------------------------------------------------
// Block (the body facet)
// ---------------------------------------------------------------------------

const BODY: &str = concat!(
    "module m\n\n",
    "fn f(xs: List<Int>) -> Int {\n",
    "    // setup\n",
    "    var total = 0 // running sum\n",
    "\n",
    "    let parts = g(\n",
    "        xs,\n",
    "        1,\n",
    "    )\n",
    "    while total < 10 // loop\n",
    "        invariant total >= 0\n",
    "        decreases 10 - total\n",
    "    {\n",
    "        total = total + (1\n",
    "            + 1)\n",
    "    }\n",
    "    // dangling\n",
    "    total\n",
    "}\n",
);

#[test]
fn canonical_body_text_has_no_comments_blank_lines_or_source_breaks() {
    let m = module(BODY);
    let body = fn_of(&m, "f").body.as_ref().expect("body");
    assert_eq!(
        canonical(Fragment::Block(body)),
        concat!(
            "{\n",
            "    var total = 0\n",
            "    let parts = g(xs, 1)\n",
            "    while total < 10\n",
            "        invariant total >= 0\n",
            "        decreases 10 - total\n",
            "    {\n",
            "        total = total + (1 + 1)\n",
            "    }\n",
            "    total\n",
            "}",
        )
    );
}

#[test]
fn source_body_text_is_the_formatters_rendering() {
    let m = module(BODY);
    let body = fn_of(&m, "f").body.as_ref().expect("body");
    let text = source(Fragment::Block(body));
    assert_eq!(
        text,
        concat!(
            "{\n",
            "    // setup\n",
            "    var total = 0 // running sum\n",
            "\n",
            "    let parts = g(\n",
            "        xs,\n",
            "        1,\n",
            "    )\n",
            "    while total < 10\n",
            "        // loop\n",
            "        invariant total >= 0\n",
            "        decreases 10 - total\n",
            "    {\n",
            "        total = total + (1\n",
            "            + 1)\n",
            "    }\n",
            "    // dangling\n",
            "    total\n",
            "}",
        )
    );
    // (The comment after the `while` header is the first clause's leading
    // comment, 2.3 rule D, so it prints above the clause.) A top-level function's body is at level 0 in its file, so the source
    // text is a substring of the formatted file.
    assert!(format_module(&m).contains(&text));
}

#[test]
fn empty_blocks() {
    let m = module("module m\n\nfn f() -> Unit {}\n\nfn g() -> Unit {\n    // later\n}\n");
    let body = |name: &str| fn_of(&m, name).body.as_ref().expect("body");
    assert_eq!(canonical(Fragment::Block(body("f"))), "{}");
    assert_eq!(source(Fragment::Block(body("f"))), "{}");
    assert_eq!(canonical(Fragment::Block(body("g"))), "{}");
    assert_eq!(source(Fragment::Block(body("g"))), "{\n    // later\n}");
}

#[test]
fn the_corpus_patch_body_matches_the_paginate_body() {
    // 5.5 and 10.5: `pagination.patch` replaces `paginate`'s body with the
    // same text, so the patch body's canonical text is the baseline body
    // facet, and its source text is what application writes back.
    let pages = corpus("examples/reader/src/reader/pages.cpl");
    let paginate = fn_of(&pages, "paginate").body.as_ref().expect("body");
    let patch = parse_patch(&read("examples/reader/work/pagination.patch"), FileId(0))
        .value
        .expect("the corpus patch parses");
    let replacement = &patch.forms[0].body;
    assert_eq!(
        canonical(Fragment::Block(replacement)),
        canonical(Fragment::Block(paginate))
    );
    let text = source(Fragment::Block(replacement));
    assert_eq!(text, source(Fragment::Block(paginate)));
    // Splicing it over the body's bytes reproduces the file.
    let file = read("examples/reader/src/reader/pages.cpl");
    let (start, end) = (paginate.span.start as usize, paginate.span.end as usize);
    let spliced = format!("{}{}{}", &file[..start], text, &file[end..]);
    assert_eq!(spliced, file);
    // The canonical body keeps the loop clauses (T23) and drops blank lines.
    assert!(canonical(Fragment::Block(paginate)).contains("\n        decreases rest.len\n"));
    assert!(!canonical(Fragment::Block(paginate)).contains("\n\n"));
}

// ---------------------------------------------------------------------------
// Docs and annotations
// ---------------------------------------------------------------------------

#[test]
fn docs_text_is_the_doc_lines_joined() {
    let prelude = corpus("std/prelude.cpl");
    let docs = |name: &str| decl(&prelude, name).docs.as_ref().expect("docs");
    assert_eq!(
        canonical(Fragment::Docs(docs("Int"))),
        "Arbitrary-precision integer."
    );
    let m = module(concat!(
        "module m\n\n",
        "/// First line.   \n",
        "///\n",
        "///    indented\n",
        "@prove\n",
        "///after the annotation\n",
        "fn f() -> Int {\n",
        "    1\n",
        "}\n",
    ));
    let d = m.declarations[0].docs.as_ref().expect("docs");
    // Grammar 1.2: one docs value across the annotation; trailing whitespace
    // is removed as the formatter removes it (rule 1).
    let text = "First line.\n\n   indented\nafter the annotation";
    assert_eq!(canonical(Fragment::Docs(d)), text);
    assert_eq!(source(Fragment::Docs(d)), text);
    // Member docs (the caller assembles `member <name>` sections, 5.2).
    let ce = decl(&prelude, "ConstraintError");
    let DeclKind::Struct(s) = &ce.kind else {
        panic!("a struct")
    };
    assert_eq!(
        canonical(Fragment::Docs(s.fields[0].docs.as_ref().expect("docs"))),
        "Name of the type whose constraint failed (stdlib.md S44); stable."
    );
}

#[test]
fn annotation_fragments() {
    let pages = corpus("examples/reader/src/reader/pages.cpl");
    let paginate = decl(&pages, "paginate");
    assert_eq!(
        canonical(Fragment::Annotations(&paginate.annotations)),
        "@id(\"pgn_7c2d\")\n@prove"
    );
    let m = module(concat!(
        "module m\n\n",
        "@method()\n",
        "@property // a keyword name (D71)\n",
        "@id(\n",
        "    \"x\",\n",
        ")\n",
        "fn f() -> Int {\n",
        "    1\n",
        "}\n",
    ));
    let anns = &m.declarations[0].annotations;
    assert_eq!(
        canonical(Fragment::Annotations(anns)),
        "@method()\n@property\n@id(\"x\")"
    );
    assert_eq!(
        source(Fragment::Annotations(anns)),
        "@method()\n@property // a keyword name (D71)\n@id(\n    \"x\",\n)"
    );
    assert_eq!(canonical(Fragment::Annotations(&[])), "");
}

// ---------------------------------------------------------------------------
// Types and selectors
// ---------------------------------------------------------------------------

#[test]
fn type_fragments() {
    let m = module("module m\n\nfn f(a: List< List<T> >, b: std.text.Text, c: Result<Int,ConstraintError>) -> Map<K, Option<V>> {\n    a\n}\n");
    let f = fn_of(&m, "f");
    let types: Vec<String> = f
        .params
        .positional
        .iter()
        .map(|p| canonical(Fragment::Type(&p.ty)))
        .collect();
    assert_eq!(
        types,
        [
            "List<List<T>>",
            "std.text.Text",
            "Result<Int, ConstraintError>"
        ]
    );
    assert_eq!(canonical(Fragment::Type(&f.ret)), "Map<K, Option<V>>");
    assert_eq!(source(Fragment::Type(&f.ret)), "Map<K, Option<V>>");
}

#[test]
fn selector_fragments() {
    let work = parse_work(&read("examples/reader/work/pagination.work"), FileId(0))
        .value
        .expect("the corpus work file parses");
    let change = &work.changes[0];
    let mut texts: Vec<String> = Vec::new();
    for task in change.tasks() {
        for item in &task.items {
            if let TaskItemKind::Write(set)
            | TaskItemKind::Add(set)
            | TaskItemKind::Depend(set)
            | TaskItemKind::Preserve(set) = &item.kind
            {
                texts.extend(
                    set.selectors
                        .iter()
                        .map(|s| canonical(Fragment::Selector(s))),
                );
            }
        }
    }
    assert_eq!(
        texts,
        [
            "paginate.body",
            "paginate.contract",
            "Positive.definition",
            "paginate.signature",
            "paginate.contract",
            "paginate.effects",
            "paginate.verification_policy",
            "tests.pagination.*",
        ]
    );
    let integrate = change.integrate().expect("integrate");
    let RequirementKind::Prove(s) = &integrate.requirements[1].requirement.kind else {
        panic!("require prove(paginate)")
    };
    assert_eq!(canonical(Fragment::Selector(s)), "paginate");
    // Source mode keeps a selector's comments.
    let w = parse_work(
        "change c {\n    task t {\n        preserve {\n            // the signature\n            f.signature, // frozen\n        }\n    }\n}\n",
        FileId(0),
    )
    .value
    .expect("parses");
    let task = w.changes[0].tasks().next().expect("task");
    let TaskItemKind::Preserve(set) = &task.items[0].kind else {
        panic!("preserve")
    };
    assert_eq!(
        canonical(Fragment::Selector(&set.selectors[0])),
        "f.signature"
    );
    assert_eq!(
        source(Fragment::Selector(&set.selectors[0])),
        "// the signature\nf.signature // frozen"
    );
}

// ---------------------------------------------------------------------------
// Declarations, and properties over every corpus declaration
// ---------------------------------------------------------------------------

#[test]
fn declaration_fragments() {
    let m = module(concat!(
        "module m\n\n",
        "// above\n",
        "\n",
        "/// Docs.\n",
        "@id(\"f_1\")\n",
        "pub fn f(\n",
        "    a: Int, // the input\n",
        ") -> Int\n",
        "    requires a > 0\n",
        "{\n",
        "    let b = a\n",
        "\n",
        "    b // the result\n",
        "} // after\n",
    ));
    let f = &m.declarations[0];
    assert_eq!(
        canonical(Fragment::Declaration(f)),
        "/// Docs.\n@id(\"f_1\")\npub fn f(a: Int) -> Int\n    requires a > 0\n{\n    let b = a\n    b\n}"
    );
    assert_eq!(
        source(Fragment::Declaration(f)),
        "// above\n\n/// Docs.\n@id(\"f_1\")\npub fn f(\n    a: Int, // the input\n) -> Int\n    requires a > 0\n{\n    let b = a\n\n    b // the result\n} // after"
    );
}

/// Every declaration of every corpus and std file.
fn all_corpus_modules() -> Vec<(String, Module)> {
    let files = [
        "examples/reader/src/reader/pages.cpl",
        "examples/reader/src/reader/images.cpl",
        "examples/reader/src/tests/pagination.cpl",
        "examples/holes/src/holes/pages.cpl",
        "std/prelude.cpl",
        "std/text.cpl",
        "std/bytes.cpl",
        "std/foreign.cpl",
        "std/gen.cpl",
    ];
    files.iter().map(|f| (f.to_string(), corpus(f))).collect()
}

#[test]
fn source_declarations_are_the_formatted_file_text() {
    // The corpus is canonical, so every declaration's source rendering (with
    // its comments) appears in the formatted file verbatim.
    for (file, m) in all_corpus_modules() {
        let formatted = format_module(&m);
        for d in &m.declarations {
            let text = source(Fragment::Declaration(d));
            assert!(
                formatted.contains(&text),
                "{file}: {} not found:\n{text}",
                d.display_name()
            );
        }
    }
}

#[test]
fn canonical_texts_are_stable_and_one_line_where_5_2_says_so() {
    for (file, m) in all_corpus_modules() {
        for d in &m.declarations {
            let name = d.display_name();
            for f in [
                Fragment::Declaration(d),
                Fragment::Signature(d),
                Fragment::Contract(d),
            ] {
                let text = canonical(f);
                // No blank line and no ordinary comment; doc comments
                // (`/// text`, `///`) are part of a declaration.
                let comment = |l: &str| {
                    let l = l.trim_start();
                    l.starts_with("//") && !l.starts_with("/// ") && l != "///"
                };
                assert!(
                    !text.ends_with('\n') && !text.contains("\n\n") && !text.lines().any(comment),
                    "{file}: {name}: {text:?}"
                );
            }
            // The canonical declaration parses back to the same canonical
            // text (canonical text is a fixpoint).
            let text = canonical(Fragment::Declaration(d));
            let again = module(&format!("module m\n\n{text}\n"));
            assert_eq!(
                canonical(Fragment::Declaration(&again.declarations[0])),
                text,
                "{file}: {name}"
            );
            if let DeclKind::Fn(f) = &d.kind {
                for c in f.contract() {
                    let clause = canonical(Fragment::Expr(&c.expr));
                    assert!(!clause.contains('\n'), "{file}: {name}: {clause}");
                }
                if let Some(e) = f.effects() {
                    assert!(!canonical(Fragment::Effects(e)).contains('\n'));
                }
                assert!(!canonical(Fragment::Signature(d)).contains('\n'));
            }
        }
    }
}

#[test]
fn canonical_texts_ignore_comments_and_layout() {
    // The same declarations with and without comments, blank lines, and
    // broken groups have equal canonical texts for every fragment.
    let plain = module(concat!(
        "module m\n\n",
        "fn f(a: Int, using db: Db) -> Int\n",
        "    effects { db.get }\n",
        "    requires a > 0\n",
        "{\n",
        "    let b = g(a, 1)\n",
        "    b\n",
        "}\n",
    ));
    let noisy = module(concat!(
        "module m\n\n",
        "// leading\n",
        "fn f(\n",
        "    a: Int, // a\n",
        "    using db: Db,\n",
        ") -> Int // header\n",
        "    // effects next\n",
        "    effects {\n",
        "        db.get, // read\n",
        "    }\n",
        "    requires a  >  0 // positive\n",
        "{\n",
        "    // first\n",
        "    let b = g(\n",
        "        a,\n",
        "        1,\n",
        "    )\n",
        "\n",
        "\n",
        "    b\n",
        "    // end\n",
        "} // after\n",
    ));
    let (p, n) = (&plain.declarations[0], &noisy.declarations[0]);
    for (fp, fn_) in [
        (Fragment::Declaration(p), Fragment::Declaration(n)),
        (Fragment::Signature(p), Fragment::Signature(n)),
        (Fragment::Contract(p), Fragment::Contract(n)),
    ] {
        assert_eq!(canonical(fp), canonical(fn_));
    }
    let (fp, fn_) = (fn_of(&plain, "f"), fn_of(&noisy, "f"));
    assert_eq!(
        canonical(Fragment::Effects(fp.effects().unwrap())),
        canonical(Fragment::Effects(fn_.effects().unwrap()))
    );
    assert_eq!(
        canonical(Fragment::Block(fp.body.as_ref().unwrap())),
        canonical(Fragment::Block(fn_.body.as_ref().unwrap()))
    );
    assert_ne!(
        source(Fragment::Block(fp.body.as_ref().unwrap())),
        source(Fragment::Block(fn_.body.as_ref().unwrap()))
    );
}
