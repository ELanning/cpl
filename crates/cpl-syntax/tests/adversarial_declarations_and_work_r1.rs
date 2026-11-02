//! Adversarial round 1: declarations, `.work`, and `.patch` files.
//!
//! Every expectation here is `spec/grammar.md`'s answer (sections cited in
//! each test), never the parser's current behaviour. Where the grammar is
//! silent the test says so and follows `design-doc.md`'s examples.

use cpl_diag::{Category, DiagCode, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_syntax::ast::*;
use cpl_syntax::{
    codes, dump_module, dump_patch, dump_work, parse_module, parse_patch, parse_work, ParseResult,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn render(src: &str, diagnostics: &[Diagnostic]) -> String {
    let mut map = SourceMap::new();
    map.add("input".to_string(), FileOrigin::Virtual, src.to_string());
    cpl_diag::render_all(diagnostics, &map)
}

fn flat(dump: &str) -> String {
    dump.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `value` is present exactly when no error was reported (lib.rs contract).
fn check_consistency<T>(src: &str, r: &ParseResult<T>) {
    let has_errors = r.diagnostics.iter().any(Diagnostic::is_error);
    assert_eq!(
        r.value.is_some(),
        !has_errors,
        "value/diagnostics inconsistency for:\n{src}\n{}",
        render(src, &r.diagnostics)
    );
    for d in &r.diagnostics {
        assert_eq!(d.category, codes::CATEGORY, "category of {d:?}");
        assert!(
            codes::ALL.contains(&d.code),
            "unknown code {} for:\n{src}",
            d.code
        );
    }
}

fn module_ast(src: &str) -> Module {
    let r = parse_module(src, FileId(0));
    check_consistency(src, &r);
    match r.value {
        Some(m) if r.diagnostics.is_empty() => m,
        _ => panic!(
            "expected zero diagnostics for:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn module_ok(src: &str) -> String {
    flat(&dump_module(&module_ast(src)))
}

fn module_err(src: &str) -> Vec<Diagnostic> {
    let r = parse_module(src, FileId(0));
    check_consistency(src, &r);
    assert!(
        r.value.is_none() && !r.diagnostics.is_empty(),
        "expected a rejection of:\n{src}"
    );
    r.diagnostics
}

fn work_ast(src: &str) -> WorkFile {
    let r = parse_work(src, FileId(0));
    check_consistency(src, &r);
    match r.value {
        Some(w) if r.diagnostics.is_empty() => w,
        _ => panic!(
            "expected zero diagnostics for:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn work_ok(src: &str) -> String {
    flat(&dump_work(&work_ast(src)))
}

fn work_err(src: &str) -> Vec<Diagnostic> {
    let r = parse_work(src, FileId(0));
    check_consistency(src, &r);
    assert!(
        r.value.is_none() && !r.diagnostics.is_empty(),
        "expected a rejection of:\n{src}"
    );
    r.diagnostics
}

fn patch_ast(src: &str) -> PatchFile {
    let r = parse_patch(src, FileId(0));
    check_consistency(src, &r);
    match r.value {
        Some(p) if r.diagnostics.is_empty() => p,
        _ => panic!(
            "expected zero diagnostics for:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn patch_ok(src: &str) -> String {
    flat(&dump_patch(&patch_ast(src)))
}

fn patch_err(src: &str) -> Vec<Diagnostic> {
    let r = parse_patch(src, FileId(0));
    check_consistency(src, &r);
    assert!(
        r.value.is_none() && !r.diagnostics.is_empty(),
        "expected a rejection of:\n{src}"
    );
    r.diagnostics
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

#[track_caller]
fn assert_code(diags: &[Diagnostic], code: DiagCode) {
    assert!(
        has_code(diags, code),
        "expected code `{code}`, got {:?}",
        diags
            .iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

fn decl(m: &Module, i: usize) -> &Declaration {
    &m.declarations[i]
}

// ===========================================================================
// 3.1 Module header
// ===========================================================================

#[test]
fn module_header_is_the_first_token_and_a_dotted_lower_path() {
    assert_contains(&module_ok("module m"), "(module m)");
    assert_contains(&module_ok("module a.b.c\n"), "(module a.b.c)");
    assert_contains(
        &module_ok("module tests.pagination\n\n\n"),
        "(module tests.pagination)",
    );
    // Leading blank lines and ordinary comments are not tokens (1.8 step 2).
    let m = module_ast("\n\n// license\n\nmodule m\n");
    assert_eq!(m.header.path.dotted(), "m");
    assert_eq!(m.trivia.leading.len(), 1);
    // A BOM is skipped (1.1).
    assert_contains(&module_ok("\u{feff}module m\n"), "(module m)");
    // CRLF is one terminator (1.1).
    assert_contains(
        &module_ok("module m\r\nfn f() -> Int { 1 }\r\n"),
        "(fn f (params) (ret (ty Int)) (block (tail (int 1))))",
    );
    // `_x` is a LOWER_IDENT (1.3), so it is a legal segment.
    assert_contains(&module_ok("module _x.y_2\n"), "(module _x.y_2)");
}

#[test]
fn module_header_missing_or_malformed_is_rejected() {
    // 3.1: the header is mandatory and comes first.
    assert_code(&module_err("fn f() -> Int { 1 }\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("use a.b\nmodule m\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("pub module m\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module .a\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module a.\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module a..b\n"), codes::SYNTAX_ERROR);
    // Segments are LOWER_IDENT (1.3 table).
    assert_code(&module_err("module A\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module a.B\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module _\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module \"m\"\n"), codes::SYNTAX_ERROR);
    // Nothing else may share the header line.
    assert_code(&module_err("module a b\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m fn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // A second header is not a declaration (3.3).
    assert_code(&module_err("module m\nmodule n\n"), codes::SYNTAX_ERROR);
    // `test` is reserved (D56); `loop` is reserved for a future edition (D4).
    assert_code(&module_err("module test\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module a.loop\n"), codes::RESERVED_WORD);
}

#[test]
fn module_docs_precede_the_header() {
    let m = module_ast("/// The module.\n/// More.\nmodule m\n");
    assert_eq!(
        m.docs.as_ref().map(|d| d.text()),
        Some("The module.\nMore.".to_string())
    );
    assert_contains(
        &flat(&dump_module(&m)),
        "(module m (docs \"The module.\" \"More.\"))",
    );
    // Blank lines after a doc comment produce no NEWLINE (1.2), so it still attaches.
    let m = module_ast("/// doc\n\n\nmodule m\n");
    assert_eq!(m.docs.as_ref().map(|d| d.text()), Some("doc".to_string()));
    // `////` is an ordinary comment, not documentation (D60).
    let m = module_ast("//// banner\nmodule m\n");
    assert!(m.docs.is_none());
}

// ===========================================================================
// 3.2 Imports
// ===========================================================================

#[test]
fn use_list_form_single_item_and_trailing_comma() {
    let d = module_ok("module m\nuse std.text.{trim}\nuse a.b.{c,}\nuse a.b.{C, d, E}\n");
    assert_contains(&d, "(use std.text (items trim))");
    assert_contains(&d, "(use a.b (items c))");
    assert_contains(&d, "(use a.b (items C d E))");
}

#[test]
fn use_list_spans_lines_inside_the_braces() {
    // 2.2: the `{` of a use list opens an insignificant-newline context.
    let d = module_ok("module m\nuse a.b.{\n    c,\n    D,\n}\n");
    assert_contains(&d, "(use a.b (items c D))");
}

#[test]
fn use_single_form_per_d15() {
    let d = module_ok("module m\nuse a.b.c\nuse a.B\nuse a.b.c.d\nuse a.b\n");
    assert_contains(&d, "(use a.b c)");
    assert_contains(&d, "(use a B)");
    assert_contains(&d, "(use a.b.c d)");
    assert_contains(&d, "(use a b)");
    // The path may be a single segment when the list form is used.
    assert_contains(&module_ok("module m\nuse a.{b}\n"), "(use a (items b))");
}

#[test]
fn use_ast_records_braced_versus_single_form() {
    let m = module_ast("module m\n\nuse a.b.{c}\nuse a.b.c\n");
    assert!(m.uses[0].braced);
    assert!(!m.uses[1].braced);
    assert_eq!(m.uses[0].items.len(), 1);
    assert_eq!(m.uses[1].items.len(), 1);
    assert_eq!(m.uses[1].path.dotted(), "a.b");
}

#[test]
fn use_empty_braces_are_rejected_d16() {
    assert_code(&module_err("module m\n\nuse a.b.{}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\nuse a.b.{,}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nuse a.b.{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn use_malformed_paths_are_rejected() {
    // A path needs at least one module segment before the item (3.2).
    assert_code(&module_err("module m\n\nuse trim\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nuse Trim\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nuse {trim}\n"), codes::SYNTAX_ERROR);
    // Path segments are LOWER_IDENT.
    assert_code(&module_err("module m\n\nuse A.b\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nuse a.B.c\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\nuse a.B.{c}\n"),
        codes::SYNTAX_ERROR,
    );
    // Items are single identifiers.
    assert_code(
        &module_err("module m\n\nuse a.b.{c.d}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nuse a.b.{c d}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nuse a.b.{\"c\"}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nuse a.b.{c}}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_err("module m\n\nuse a.b.{c\n"), codes::SYNTAX_ERROR);
    // No globs, no nested lists, no `as` (D16).
    assert_code(&module_err("module m\n\nuse a.b.*\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\nuse a.b.{*}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nuse a.{b.{c}}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nuse a.b.{c as d}\n"),
        codes::RESERVED_WORD,
    );
    // Dangling dots.
    assert_code(&module_err("module m\n\nuse a.\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nuse a.b.\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nuse .a.b\n"), codes::SYNTAX_ERROR);
    // Two imports on one line.
    assert_code(
        &module_err("module m\n\nuse a.b use c.d\n"),
        codes::SYNTAX_ERROR,
    );
    // No `pub use`.
    assert_code(
        &module_err("module m\n\npub use a.b\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn use_after_a_declaration_is_d13_error() {
    let e = module_err("module m\n\nstruct S {}\n\nuse a.b\n");
    assert_code(&e, codes::USE_AFTER_DECLARATION);
    let e = module_err("module m\n\ntest \"t\" {}\nuse a.b.{c}\n");
    assert_code(&e, codes::USE_AFTER_DECLARATION);
    // Between the header and the first declaration it is fine.
    module_ok("module m\nuse a.b\nuse c.d.{e}\nfn f() -> Int { 1 }\n");
}

// ===========================================================================
// 3.3 Declarations, annotations, pub, doc comments
// ===========================================================================

#[test]
fn annotations_with_literal_arguments() {
    let d = module_ok(
        "module m\n\n@id(\"pgn_7c2d\")\n@prove\n@unknown(1, 2)\n@mixed(true, false, \"s\", 3_0,)\n@empty()\npub fn f() -> Int { 1 }\n",
    );
    assert_contains(&d, "(annotation id (text \"pgn_7c2d\"))");
    assert_contains(&d, "(annotation prove)");
    assert_contains(&d, "(annotation unknown (int 1) (int 2))");
    assert_contains(
        &d,
        "(annotation mixed (bool true) (bool false) (text \"s\") (int 30))",
    );
    // `commas(literal)` admits the empty list; the dump records the
    // parentheses, which the formatter keeps (`spec/toolchain.md` T14).
    assert_contains(&d, "(annotation empty ())");
    // The annotations precede `pub` and are recorded in source order.
    assert_contains(
        &d,
        "(fn f pub (annotation id (text \"pgn_7c2d\")) (annotation prove) (annotation unknown",
    );
}

#[test]
fn annotation_accessors_on_the_ast() {
    let m = module_ast("module m\n\n@id(\"x\")\n@prove\nfn f() -> Int { 1 }\n");
    let d = decl(&m, 0);
    assert_eq!(d.id(), Some("x"));
    assert!(d.has_annotation("prove"));
    assert!(!d.has_annotation("id2"));
    assert_eq!(d.annotations.len(), 2);
    assert!(!d.is_pub());
}

#[test]
fn annotations_admitted_on_every_declaration_kind() {
    // D17: the grammar is open; the checker owns the vocabulary and placement.
    let d = module_ok(
        "module m\n@id(\"a\")\ntype A(Int)\n@id(\"b\")\nstruct B {}\n@id(\"c\")\nenum C {}\n@id(\"d\")\ncapability D {}\n@id(\"e\")\ntest \"e\" {}\n@id(\"f\")\nproperty \"f\"\n    for x in xs\n{\n}\n@prove\nstruct G {}\n",
    );
    assert_contains(
        &d,
        "(type A (annotation id (text \"a\")) (wrapper (ty Int)))",
    );
    assert_contains(&d, "(struct B (annotation id (text \"b\")))");
    assert_contains(&d, "(enum C (annotation id (text \"c\")))");
    assert_contains(&d, "(capability D (annotation id (text \"d\")))");
    assert_contains(&d, "(test \"e\" (annotation id (text \"e\")) (block))");
    assert_contains(
        &d,
        "(property \"f\" (annotation id (text \"f\")) (for x (name xs)) (block))",
    );
    assert_contains(&d, "(struct G (annotation prove))");
}

#[test]
fn annotation_arguments_must_be_literals_d61() {
    assert_code(
        &module_err("module m\n\n@id(x)\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@id(1 + 2)\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // `-1` is unary minus on a literal, not a literal (D48).
    assert_code(
        &module_err("module m\n\n@id(-1)\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@id(\"a\" \"b\")\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@id([1])\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@id(,)\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@id(\"x\"\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn annotation_name_is_lower_ident_and_owns_its_line() {
    assert_code(
        &module_err("module m\n\n@Id(\"x\")\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@\"id\"\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@1\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // `@` and the name are separate tokens; horizontal whitespace between them
    // is insignificant (1.1), unlike the adjacency rule of `?name` (1.7).
    assert_contains(
        &module_ok("module m\n@ prove\nfn f() -> Int { 1 }\n"),
        "(annotation prove)",
    );
    // Each annotation occupies its own line (3.3).
    assert_code(
        &module_err("module m\n\n@prove fn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@id(\"x\") @prove\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\n@prove pub fn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // An annotation precedes `pub`, never follows it.
    assert_code(
        &module_err("module m\n\npub @prove\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\npub\n@prove\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // `@loop` names a reserved-future word.
    assert_code(
        &module_err("module m\n\n@loop\nfn f() -> Int { 1 }\n"),
        codes::RESERVED_WORD,
    );
}

#[test]
fn annotation_on_a_non_declaration_is_rejected() {
    // decl_prefix only precedes the seven declaration kinds (3.3).
    assert_code(
        &module_err("module m\n\n@id(\"x\")\nuse a.b\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_err("@id(\"x\")\nmodule m\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\n@prove\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\n@prove\n\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\nfn f() -> Int {\n    @prove\n    1\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    @id(\"x\")\n    x: Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    @id(\"x\")\n    A\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability C {\n    @prove\n    fn get(id: Id) -> Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Annotations do not exist in `.work` or `.patch` files (section 4).
    assert_code(&work_err("@id(\"x\")\nchange c {}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_err("change c {\n    @id(\"x\")\n    task t {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("@id(\"x\")\npatch f.body\n    expect base(f.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn pub_is_accepted_before_type_struct_enum_capability_fn_only() {
    let d = module_ok(
        "module m\npub type A(Int)\npub type P = Int where self > 0\npub struct S {}\npub enum E {}\npub capability C {}\npub fn f() -> Int { 1 }\nfn g() -> Int { 1 }\n",
    );
    assert_contains(&d, "(type A pub (wrapper (ty Int)))");
    assert_contains(
        &d,
        "(type P pub (refinement (ty Int) (where (> (name self) (int 0)))))",
    );
    assert_contains(&d, "(struct S pub)");
    assert_contains(&d, "(enum E pub)");
    assert_contains(&d, "(capability C pub)");
    assert_contains(&d, "(fn f pub (params)");
    assert_contains(&d, "(fn g (params)");
    let m = module_ast("module m\n\npub fn f() -> Int { 1 }\nfn g() -> Int { 1 }\n");
    assert!(decl(&m, 0).is_pub());
    assert!(!decl(&m, 1).is_pub());
}

#[test]
fn pub_misplacements_are_rejected() {
    // D67: tests and properties take no `pub`.
    assert_code(
        &module_err("module m\n\npub test \"t\" {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\npub property \"p\"\n    for x in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // `pub` only directly before the keyword.
    assert_code(
        &module_err("module m\n\nfn pub f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\npub pub fn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\npub\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_err("module m\n\npub\n"), codes::SYNTAX_ERROR);
    // D65: no per-member `pub`.
    assert_code(
        &module_err("module m\n\nstruct S {\n    pub x: Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    pub A\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // 3.8: operations take no `pub`.
    assert_code(
        &module_err("module m\n\ncapability C {\n    pub fn get(id: Id) -> Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // No `pub` on statements or on `.work` items.
    assert_code(
        &module_err("module m\n\nfn f() -> Int {\n    pub let x = 1\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&work_err("pub change c {}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_err("change c {\n    pub task t {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn doc_comments_attach_to_the_declaration_that_follows() {
    let m = module_ast(
        "module m\n\n/// first\nfn a() -> Int { 1 }\n\n/// second a\n/// second b\n\n\nstruct B {}\nenum C {}\n/// fourth\n@id(\"d\")\ntest \"d\" {}\n",
    );
    assert_eq!(
        decl(&m, 0).docs.as_ref().map(|d| d.text()),
        Some("first".to_string())
    );
    // Blank lines after a doc comment emit no NEWLINE (1.2), so it still attaches.
    assert_eq!(
        decl(&m, 1).docs.as_ref().map(|d| d.text()),
        Some("second a\nsecond b".to_string())
    );
    assert!(decl(&m, 2).docs.is_none());
    assert_eq!(
        decl(&m, 3).docs.as_ref().map(|d| d.text()),
        Some("fourth".to_string())
    );
    assert_eq!(decl(&m, 3).id(), Some("d"));
}

#[test]
fn doc_comments_interleaved_with_annotations_form_one_docs_value() {
    // 1.2 / 3.3: any interleaving; docs concatenated in source order.
    let m = module_ast(
        "module m\n\n@id(\"x\")\n/// a\n@prove\n/// b\n/// c\npub fn f() -> Int { 1 }\n",
    );
    let d = decl(&m, 0);
    assert_eq!(
        d.docs.as_ref().map(|d| d.text()),
        Some("a\nb\nc".to_string())
    );
    assert_eq!(d.annotations.len(), 2);
    assert!(d.is_pub());
    // The dump places docs before annotations (canonical order).
    assert_contains(
        &flat(&dump_module(&m)),
        "(fn f pub (docs \"a\" \"b\" \"c\") (annotation id (text \"x\")) (annotation prove)",
    );
}

#[test]
fn doc_comments_on_members() {
    let m = module_ast(
        "module m\n\nstruct S {\n    /// the id\n    id: Int\n    text: Text\n}\n\nenum E {\n    /// missing\n    /// (two lines)\n    Missing(Int)\n    Other\n}\n\ncapability C {\n    /// gets\n    fn get(id: Int) -> Int\n    fn put(id: Int) -> Int\n}\n",
    );
    let DeclKind::Struct(s) = &decl(&m, 0).kind else {
        panic!()
    };
    assert_eq!(
        s.fields[0].docs.as_ref().map(|d| d.text()),
        Some("the id".to_string())
    );
    assert!(s.fields[1].docs.is_none());
    let DeclKind::Enum(e) = &decl(&m, 1).kind else {
        panic!()
    };
    assert_eq!(
        e.variants[0].docs.as_ref().map(|d| d.text()),
        Some("missing\n(two lines)".to_string())
    );
    assert!(e.variants[1].docs.is_none());
    let DeclKind::Capability(c) = &decl(&m, 2).kind else {
        panic!()
    };
    assert_eq!(
        c.operations[0].docs.as_ref().map(|d| d.text()),
        Some("gets".to_string())
    );
    assert!(c.operations[1].docs.is_none());
}

#[test]
fn doc_comment_text_strips_exactly_one_leading_space() {
    let m = module_ast("module m\n\n///no space\n///  two spaces\n///\nfn f() -> Int { 1 }\n");
    assert_eq!(
        decl(&m, 0).docs.as_ref().map(|d| d.text()),
        Some("no space\n two spaces\n".to_string())
    );
}

#[test]
fn doc_comments_elsewhere_are_doc_comment_position_errors() {
    // 1.2: a DOC_COMMENT anywhere but the listed places is
    // `doc comment must precede a declaration` (code doc_comment_position).
    assert_code(
        &module_err("module m\n/// not before a use\nuse a.b\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &module_err("module m\n\n/// dangling\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &module_err("module m\n\nfn f() -> Int {\n    /// stmt\n    1\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int {\n    match x {\n        /// arm\n        _ => 1\n    }\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comment_before_the_closing_brace_of_a_struct_body() {
    // 1.2: a DOC_COMMENT with no member following is `doc comment must precede a declaration`.
    assert_code(
        &module_err("module m\n\nstruct S {\n    x: Int\n    /// trailing\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comment_before_the_closing_brace_of_an_enum_body() {
    assert_code(
        &module_err("module m\n\nenum E {\n    A\n    /// trailing\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comment_before_the_closing_brace_of_a_capability_body() {
    assert_code(
        &module_err("module m\n\ncapability C {\n    /// trailing\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comment_before_a_fn_clause() {
    assert_code(
        &module_err(
            "module m\n\nfn f(x: Int) -> Int\n    /// why\n    requires x > 0\n{\n    x\n}\n",
        ),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comments_in_work_and_patch_files_only_before_change_task_patch() {
    // 1.2 table: before `change` or `task` in a `.work` file, before `patch` in a `.patch` file.
    let w = work_ast("/// The change.\nchange c {\n    /// The task.\n    task t {}\n}\n");
    assert_eq!(
        w.changes[0].docs.as_ref().map(|d| d.text()),
        Some("The change.".to_string())
    );
    let ChangeItemKind::Task(t) = &w.changes[0].items[0].kind else {
        panic!()
    };
    assert_eq!(
        t.docs.as_ref().map(|d| d.text()),
        Some("The task.".to_string())
    );
    let p = patch_ast("/// Replace.\npatch f.body\n    expect base(f.body)\n{\n}\n");
    assert_eq!(
        p.forms[0].docs.as_ref().map(|d| d.text()),
        Some("Replace.".to_string())
    );

    // Anywhere else: `doc comment must precede a declaration`. The positions
    // the parser handles through `lines(X)` of task items and requirements:
    assert_code(
        &work_err(
            "change c {\n    task t {\n        /// before write\n        write {}\n    }\n}\n",
        ),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        /// before require\n        require typecheck\n    }\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comment_before_use_in_a_work_file() {
    assert_code(
        &work_err("/// before use\nuse a.b\nchange c {}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comment_before_base_or_integrate_in_a_change() {
    assert_code(
        &work_err("change c {\n    /// before base\n    base git(\"main\")\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &work_err("change c {\n    /// before integrate\n    integrate {}\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comment_dangling_at_the_end_of_a_work_file_or_change() {
    assert_code(
        &work_err("change c {}\n/// dangling\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &work_err("change c {\n    task t {}\n    /// dangling in change\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn doc_comment_before_expect_or_dangling_in_a_patch_file() {
    assert_code(
        &patch_err("patch f.body\n    /// before expect\n    expect base(f.body)\n{\n}\n"),
        codes::DOC_COMMENT_POSITION,
    );
    assert_code(
        &patch_err("patch f.body\n    expect base(f.body)\n{\n}\n/// dangling\n"),
        codes::DOC_COMMENT_POSITION,
    );
}

// ===========================================================================
// 3.5 Type declarations
// ===========================================================================

#[test]
fn type_wrapper_and_refinement_forms() {
    let d = module_ok(
        "module m\npub type PageId(Text)\ntype Boxed(List<List<Int>>)\ntype Q(a.b.T)\npub type Positive = Int where self > 0\ntype Small = Positive where self < 10 and self != 5\n",
    );
    assert_contains(&d, "(type PageId pub (wrapper (ty Text)))");
    assert_contains(&d, "(type Boxed (wrapper (ty List (ty List (ty Int)))))");
    assert_contains(&d, "(type Q (wrapper (ty a.b.T)))");
    assert_contains(
        &d,
        "(type Positive pub (refinement (ty Int) (where (> (name self) (int 0)))))",
    );
    assert_contains(
        &d,
        "(type Small (refinement (ty Positive) (where (and (< (name self) (int 10)) (!= (name self) (int 5))))))",
    );
}

#[test]
fn type_refinement_predicate_ends_at_the_line_unless_parenthesized() {
    // 3.5: "parenthesize to span lines".
    let d = module_ok("module m\n\ntype P = Int where (self > 0\n    and self < 10)\n");
    assert_contains(
        &d,
        "(where (paren (and (> (name self) (int 0)) (< (name self) (int 10)))))",
    );
    assert_code(
        &module_err("module m\n\ntype P = Int where self > 0\n    and self < 10\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype P = Int where self > 0 and\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn type_refinement_predicate_is_not_a_header_expression() {
    // D41 lists the header expressions (if/while conditions, match scrutinee,
    // for range, requires/ensures/invariant/decreases); the `where` predicate
    // is not among them, so a struct literal is legal there.
    let d = module_ok("module m\n\ntype P = Page where self == Page { id: x }\n");
    assert_contains(
        &d,
        "(where (== (name self) (struct-lit Page (field id (name x)))))",
    );
}

#[test]
fn type_alias_without_where_is_rejected_d64() {
    let e = module_err("module m\n\ntype A = Int\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert!(has_message(
        &e,
        "a type declaration is a wrapper, type A(T), or a refinement, type A = T where ..."
    ));
    assert_code(
        &module_err("module m\n\npub type A = List<Int>\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype A = Int where\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype A = where self > 0\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype A = 1 where self > 0\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype A = Int self > 0\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn type_wrapper_has_exactly_one_payload_d18() {
    assert_code(
        &module_err("module m\n\ntype Pair(Int, Int)\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype Empty()\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype A(Int,)\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype A(int)\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_err("module m\n\ntype A(Int\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\ntype A(Int) where self > 0\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype A(Int) {}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn type_name_is_pascal_case_and_takes_no_generics_on_wrappers() {
    assert_code(
        &module_err("module m\n\ntype page_id(Text)\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype (Text)\n"),
        codes::SYNTAX_ERROR,
    );
    // D19: wrappers and refinements take no generic parameters.
    assert_code(
        &module_err("module m\n\ntype A<T>(T)\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntype A<T> = T where self > 0\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn definition_less_type_parses_as_opaque_decl() {
    // 3.5 opaque_decl: `type A` and `type List<T>` parse; `missing_body` is a
    // check error of the semantics (spec/semantics.md 1.8), not a parse error.
    module_ok("module m\n\ntype Int\n");
    module_ok("module m\n\npub type List<T>\n");
    module_ok("module m\n\n@builtin(\"std.Int\")\npub type Int\n\nfn f() -> Int { 1 }\n");
    module_ok("module m\n\npub type Result<T, E>\n");
}

// ===========================================================================
// 3.6 Structs
// ===========================================================================

#[test]
fn struct_fields_one_per_line_zero_or_more() {
    let d = module_ok(
        "module m\npub struct Page {\n    id: PageId\n    text: Text\n    items: List<List<Int>>\n    q: a.b.T\n}\nstruct Empty {}\nstruct Empty2 {\n}\nstruct Empty3 {\n\n\n}\nstruct One { x: Int }\nstruct Gen<A, B>\n{\n    first: A\n    second: B\n}\n",
    );
    assert_contains(
        &d,
        "(struct Page pub (field id (ty PageId)) (field text (ty Text)) (field items (ty List (ty List (ty Int)))) (field q (ty a.b.T)))",
    );
    assert_contains(&d, "(struct Empty)");
    assert_contains(&d, "(struct Empty2)");
    assert_contains(&d, "(struct Empty3)");
    // `lines(X)` has optional NEWLINEs, so a one-line body is legal.
    assert_contains(&d, "(struct One (field x (ty Int)))");
    assert_contains(
        &d,
        "(struct Gen (generics A B) (field first (ty A)) (field second (ty B)))",
    );
}

#[test]
fn struct_field_separators_and_shapes_are_rejected() {
    assert_code(
        &module_err("module m\n\nstruct S {\n    x: Int, y: Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    x: Int y: Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    x: Int,\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    x Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    x:\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    X: Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    x: int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    x: Int = 1\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    let x: Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    _: Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // A generic argument list never spans lines where newlines are significant (2.2).
    assert_code(
        &module_err("module m\n\nstruct S {\n    x: List<\n        Int>\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {\n    x: List<Int,>\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn struct_header_shapes_are_rejected() {
    assert_code(
        &module_err("module m\n\nstruct s {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_err("module m\n\nstruct S\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nstruct S {\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nstruct S }\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nstruct {}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\nstruct S<> {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S<a> {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S<A,> {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S<A B> {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S(Int)\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S = Int\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nstruct S {} {}\n"),
        codes::SYNTAX_ERROR,
    );
    // A generic parameter list never spans lines (2.2).
    assert_code(
        &module_err("module m\n\nstruct S<A,\n    B> {}\n"),
        codes::SYNTAX_ERROR,
    );
}

// ===========================================================================
// 3.7 Enums
// ===========================================================================

#[test]
fn enum_variants_with_zero_to_many_payloads() {
    let d = module_ok(
        "module m\n\npub enum StoreError {\n    Missing(PageId)\n    Unavailable\n    Pair(Int, Text)\n    Triple(Int, Text, List<Int>)\n    Trailing(Int,)\n    Multi(\n        Int,\n        a.b.T,\n    )\n}\nenum Never {}\nenum One { A }\nenum Gen<T>\n{\n    Some(T)\n    None\n}\n",
    );
    assert_contains(&d, "(variant Missing (ty PageId))");
    assert_contains(&d, "(variant Unavailable)");
    assert_contains(&d, "(variant Pair (ty Int) (ty Text))");
    assert_contains(&d, "(variant Triple (ty Int) (ty Text) (ty List (ty Int)))");
    // 2.5: payload lists are parenthesized and accept a trailing comma.
    assert_contains(&d, "(variant Trailing (ty Int))");
    assert_contains(&d, "(variant Multi (ty Int) (ty a.b.T))");
    assert_contains(&d, "(enum Never)");
    assert_contains(&d, "(enum One (variant A))");
    assert_contains(
        &d,
        "(enum Gen (generics T) (variant Some (ty T)) (variant None))",
    );
}

#[test]
fn enum_variant_shapes_are_rejected() {
    // 3.7: `Variant()` is a syntax error.
    let e = module_err("module m\n\nenum E {\n    A()\n}\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\nenum E {\n    A(,)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Variant names are UPPER_IDENT.
    assert_code(
        &module_err("module m\n\nenum E {\n    a\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    _\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // D20: no record variants.
    assert_code(
        &module_err("module m\n\nenum E {\n    A { x: Int }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // One variant per line.
    assert_code(
        &module_err("module m\n\nenum E {\n    A B\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    A, B\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    A,\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Payloads are types, not names or literals.
    assert_code(
        &module_err("module m\n\nenum E {\n    A(x: Int)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    A(1)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    A(int)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    A(Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nenum E {\n    A = 1\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Header.
    assert_code(&module_err("module m\n\nenum e {}\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\nenum E\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\nenum E<T,> {}\n"),
        codes::SYNTAX_ERROR,
    );
}

// ===========================================================================
// 3.8 Capabilities
// ===========================================================================

#[test]
fn capability_operations_are_signatures_with_optional_contracts() {
    let d = module_ok(
        "module m\n\npub capability PageRead {\n    fn get(id: PageId) -> Result<Page, StoreError>\n    fn none() -> Unit\n    fn two(a: Int, b: Text,) -> Int\n        requires a > 0\n        ensures result > 0\n        requires b != \"\"\n    fn multi(\n        a: Int,\n        b: Int,\n    ) -> Int\n}\ncapability Nothing {}\ncapability One { fn get(id: Id) -> Id }\ncapability Next\n{\n    fn get(id: Id) -> Id\n}\n",
    );
    assert_contains(
        &d,
        "(op get (params (param id (ty PageId))) (ret (ty Result (ty Page) (ty StoreError))))",
    );
    assert_contains(&d, "(op none (params) (ret (ty Unit)))");
    // Clauses in source order (D66; the formatter reorders, the parser does not).
    assert_contains(
        &d,
        "(op two (params (param a (ty Int)) (param b (ty Text))) (ret (ty Int)) (requires (> (name a) (int 0))) (ensures (> (name result) (int 0))) (requires (!= (name b) (text \"\"))))",
    );
    assert_contains(
        &d,
        "(op multi (params (param a (ty Int)) (param b (ty Int))) (ret (ty Int)))",
    );
    assert_contains(&d, "(capability Nothing)");
    assert_contains(
        &d,
        "(capability One (op get (params (param id (ty Id))) (ret (ty Id))))",
    );
    assert_contains(&d, "(capability Next (op get");
}

#[test]
fn capability_operation_with_a_body_is_rejected() {
    // 3.8: operations are signatures only.
    assert_code(
        &module_err("module m\n\ncapability C {\n    fn get(id: Id) -> Int { 1 }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err(
            "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n    {\n        1\n    }\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        requires id != x\n    {\n        1\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability C {\n    fn get(id: Id) -> Int {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn capability_operation_restrictions() {
    // No generics, no `using`, no `effects`, no `decreases`, no `pub`, `->` mandatory.
    assert_code(
        &module_err("module m\n\ncapability C {\n    fn get<T>(x: T) -> T\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability C {\n    fn get(using db: Db) -> Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability C {\n    fn get(x: Int, using db: Db) -> Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err(
            "module m\n\ncapability C {\n    fn get(x: Int) -> Int\n        effects { x.y }\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err(
            "module m\n\ncapability C {\n    fn get(x: Int) -> Int\n        decreases x\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability C {\n    pub fn get(x: Int) -> Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    let e = module_err("module m\n\ncapability C {\n    fn get(x: Int)\n}\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\ncapability C {\n    fn get(x: Int) -> Int requires x > 0\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Two operations on one line.
    assert_code(
        &module_err("module m\n\ncapability C {\n    fn a() -> Int fn b() -> Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Only `fn` items.
    assert_code(
        &module_err("module m\n\ncapability C {\n    x: Int\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability C {\n    type T(Int)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // D19: capabilities take no generic parameters.
    assert_code(
        &module_err("module m\n\ncapability C<T> {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability c {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ncapability C\n"),
        codes::SYNTAX_ERROR,
    );
}

// ===========================================================================
// 3.9 Functions
// ===========================================================================

#[test]
fn fn_generics_params_and_return_type() {
    let d = module_ok(
        "module m\npub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>> { ?x }\nfn two<T, U>(a: T, b: U,) -> Unit {}\nfn none() -> Unit {}\nfn q(x: a.b.T) -> a.b.U { x }\n",
    );
    assert_contains(
        &d,
        "(fn paginate pub (generics T) (params (param items (ty List (ty T))) (param size (ty Positive))) (ret (ty List (ty List (ty T)))) (block (tail (hole x))))",
    );
    assert_contains(
        &d,
        "(fn two (generics T U) (params (param a (ty T)) (param b (ty U))) (ret (ty Unit)) (block))",
    );
    assert_contains(&d, "(fn none (params) (ret (ty Unit)) (block))");
    assert_contains(
        &d,
        "(fn q (params (param x (ty a.b.T))) (ret (ty a.b.U)) (block (tail (name x))))",
    );
}

#[test]
fn fn_header_shapes_are_rejected() {
    // Return type mandatory (`expected ->`).
    assert_code(
        &module_err("module m\n\nfn f() { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f() -> { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f() -> int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f() -> Int -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f() : Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // Names and generics.
    assert_code(
        &module_err("module m\n\nfn F() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn () -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f<>() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f<t>() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f<T,>() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f<T U>() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f<List<T>>() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // Parameters.
    assert_code(
        &module_err("module m\n\nfn f(x) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x:) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(X: Int) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(,) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int,, y: Int) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int y: Int) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(Int) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int = 1) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(_: Int) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn using_parameters_follow_positional_ones() {
    let d = module_ok(
        "module m\n\nfn a(using db: Db) -> Int { 1 }\nfn b(x: Int, using db: Db) -> Int { 1 }\nfn c(x: Int, y: Int, using db: Db, using log: Log,) -> Int { 1 }\nfn d(using db: Db, using log: Log) -> Int { 1 }\nfn e(\n    id: PageId,\n    using db: PageRead,\n) -> Int { 1 }\n",
    );
    assert_contains(&d, "(fn a (params (using db (ty Db))) (ret (ty Int))");
    assert_contains(&d, "(fn b (params (param x (ty Int)) (using db (ty Db)))");
    assert_contains(
        &d,
        "(fn c (params (param x (ty Int)) (param y (ty Int)) (using db (ty Db)) (using log (ty Log)))",
    );
    assert_contains(&d, "(fn d (params (using db (ty Db)) (using log (ty Log)))");
    assert_contains(
        &d,
        "(fn e (params (param id (ty PageId)) (using db (ty PageRead)))",
    );
    let m = module_ast("module m\n\nfn c(x: Int, using db: Db, using log: Log) -> Int { 1 }\n");
    let DeclKind::Fn(f) = &decl(&m, 0).kind else {
        panic!()
    };
    assert_eq!(f.params.positional.len(), 1);
    assert_eq!(f.params.using.len(), 2);
    assert_eq!(f.params.using[1].name.text, "log");
}

#[test]
fn positional_after_using_is_its_own_code() {
    let e = module_err("module m\n\nfn f(using db: Db, x: Int) -> Int { 1 }\n");
    assert_code(&e, codes::POSITIONAL_AFTER_USING);
    let e = module_err("module m\n\nfn f(x: Int, using db: Db, y: Int) -> Int { 1 }\n");
    assert_code(&e, codes::POSITIONAL_AFTER_USING);
    // Call sites follow the same rule (3.12 `args`).
    let e = module_err("module m\n\nfn f() -> Int {\n    g(using db, x)\n}\n");
    assert_code(&e, codes::POSITIONAL_AFTER_USING);
    let e = module_err("module m\n\nfn f() -> Int {\n    g(a, using db, b)\n}\n");
    assert_code(&e, codes::POSITIONAL_AFTER_USING);
}

#[test]
fn using_parameter_shapes_are_rejected() {
    assert_code(
        &module_err("module m\n\nfn f(using db) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(using: Db) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(using Db) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(using using db: Db) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(using Db: Db) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(using) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // `using` is reserved (D3), so it is not a parameter name.
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int {\n    g(using)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int {\n    g(using using db)\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn call_site_using_arguments_d31() {
    let d = module_ok(
        "module m\n\nfn f() -> Int {\n    let a = load_page(id, using db)?\n    let b = g(using db)\n    let c = g(using db, using ctx.log,)\n    let d = g(x, y, using ctx.reader)\n    g(\n        x,\n        using db,\n    )\n}\n",
    );
    assert_contains(
        &d,
        "(let a (propagate (call (name load_page) (name id) (using (name db)))))",
    );
    assert_contains(&d, "(let b (call (name g) (using (name db))))");
    assert_contains(
        &d,
        "(let c (call (name g) (using (name db)) (using (member (name ctx) log))))",
    );
    assert_contains(
        &d,
        "(let d (call (name g) (name x) (name y) (using (member (name ctx) reader))))",
    );
    assert_contains(&d, "(tail (call (name g) (name x) (using (name db))))");
}

#[test]
fn effects_sets_empty_trailing_comma_and_dotted_entries() {
    let d = module_ok(
        "module m\nfn a(using db: Db) -> Int\n    effects { db.get }\n{\n    1\n}\nfn b(using db: Db) -> Int\n    effects {}\n{\n    1\n}\nfn c(using db: Db) -> Int\n    effects { db.get, }\n{\n    1\n}\nfn d(using db: Db, using codec: Codec) -> Int\n    effects {\n        db.get,\n        db.put,\n        codec.dimensions,\n    }\n{\n    1\n}\n",
    );
    assert_contains(
        &d,
        "(fn a (params (using db (ty Db))) (ret (ty Int)) (effects db.get) (block (tail (int 1))))",
    );
    assert_contains(
        &d,
        "(fn b (params (using db (ty Db))) (ret (ty Int)) (effects) (block (tail (int 1))))",
    );
    assert_contains(
        &d,
        "(fn c (params (using db (ty Db))) (ret (ty Int)) (effects db.get) (block (tail (int 1))))",
    );
    assert_contains(&d, "(effects db.get db.put codec.dimensions)");
    let m = module_ast(
        "module m\n\nfn a(using db: Db) -> Int\n    effects { db.get, db.put }\n{\n    1\n}\n",
    );
    let DeclKind::Fn(f) = &decl(&m, 0).kind else {
        panic!()
    };
    let effects = f.effects().expect("effects clause");
    assert_eq!(effects.effects.len(), 2);
    assert_eq!(effects.effects[1].param.text, "db");
    assert_eq!(effects.effects[1].operation.text, "put");
}

#[test]
fn effects_entries_must_be_param_dot_operation_d23() {
    let wrap = |set: &str| {
        format!("module m\n\nfn f(using db: Db) -> Int\n    effects {set}\n{{\n    1\n}}\n")
    };
    assert_code(&module_err(&wrap("{ get }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ db }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ db.* }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ PageRead.get }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ db.Get }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ db.get.x }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ db.get() }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ db.get db.put }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ , }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ db. }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ .get }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("{ \"db.get\" }")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("db.get")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("( db.get )")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("[ db.get ]")), codes::SYNTAX_ERROR);
    assert_code(&module_err(&wrap("")), codes::SYNTAX_ERROR);
}

#[test]
fn duplicate_effects_and_decreases_clauses_carry_the_name_field() {
    let e = module_err("module m\n\nfn f(using db: Db) -> Int\n    effects { db.get }\n    effects {}\n{\n    1\n}\n");
    assert_code(&e, codes::DUPLICATE_CLAUSE);
    let d = e
        .iter()
        .find(|d| d.code == codes::DUPLICATE_CLAUSE)
        .unwrap();
    assert_eq!(d.data.get("name").and_then(|v| v.as_str()), Some("effects"));
    let e = module_err("module m\n\nfn f(x: Int) -> Int\n    decreases x\n    requires x > 0\n    decreases x - 1\n{\n    1\n}\n");
    assert_code(&e, codes::DUPLICATE_CLAUSE);
    let d = e
        .iter()
        .find(|d| d.code == codes::DUPLICATE_CLAUSE)
        .unwrap();
    assert_eq!(
        d.data.get("name").and_then(|v| v.as_str()),
        Some("decreases")
    );
}

#[test]
fn clauses_parse_in_any_order_and_keep_source_order_d66() {
    // D66: `requires` before `ensures` is a canonical-output rule, not a parse rule.
    let d = module_ok(
        "module m\n\nfn f(a: Int, b: Int, using db: Db) -> Int\n    ensures result > 0\n    requires b != 0\n    decreases a\n    effects { db.get }\n    requires a % b == 0\n    ensures result * b == a\n{\n    a / b\n}\n",
    );
    assert_contains(
        &d,
        "(ensures (> (name result) (int 0))) (requires (!= (name b) (int 0))) (decreases (name a)) (effects db.get) (requires (== (% (name a) (name b)) (int 0))) (ensures (== (* (name result) (name b)) (name a)))",
    );
    let m = module_ast(
        "module m\n\nfn f(a: Int, b: Int) -> Int\n    ensures result > 0\n    requires b != 0\n    requires a > 0\n{\n    a / b\n}\n",
    );
    let DeclKind::Fn(f) = &decl(&m, 0).kind else {
        panic!()
    };
    assert_eq!(f.clauses.len(), 3);
    assert!(matches!(&f.clauses[0], FnClause::Contract(c) if c.kind == ContractKind::Ensures));
    assert_eq!(f.requires().count(), 2);
    assert_eq!(f.ensures().count(), 1);
    // The relative order of `requires` clauses is preserved (3.9).
    let reqs: Vec<String> = f
        .requires()
        .map(|c| flat(&format!("{:?}", c.expr.span.start)))
        .collect();
    assert!(reqs[0] < reqs[1] || reqs[0].len() < reqs[1].len());
}

#[test]
fn clause_layout_rules_of_section_2_4() {
    // Rule 1: one clause per line.
    assert_code(
        &module_err(
            "module m\n\nfn f(x: Int) -> Int\n    requires x > 0 ensures result > 0\n{\n    x\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err(
            "module m\n\nfn f(x: Int) -> Int\n    effects {} requires x > 0\n{\n    x\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    // A clause on the header line.
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int requires x > 0\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int effects {}\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Rule 2: with a clause the `{` starts a new line.
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int\n    requires x > 0 {\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(using db: Db) -> Int\n    effects { db.get } {\n    1\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // A clause without an expression.
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int\n    requires\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int\n    decreases\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Unknown clause keywords.
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int\n    invariant x > 0\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int\n    where x > 0\n{\n    x\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Rule 2 positive: no clauses, `{` on the header line or the next (D63),
    // with blank lines and comments in between.
    let d = module_ok("module m\n\nfn f() -> Int\n\n    // note\n\n{\n    1\n}\n\nfn g(x: Int) -> Int\n\n    requires x > 0\n\n    // then\n    ensures result > 0\n\n{\n    x\n}\n");
    assert_contains(&d, "(fn f (params) (ret (ty Int)) (block (tail (int 1)");
    assert_contains(&d, "(requires (> (name x) (int 0))");
    assert_contains(&d, "(ensures (> (name result) (int 0))");
}

#[test]
fn bodiless_fn_parses_as_a_builtin_stub_form() {
    // 3.9 third `fn_tail` alternative: a header followed by neither a clause
    // nor `{` is complete without a body; `missing_body` is a check error of
    // spec/semantics.md 1.8, not a syntax error.
    module_ok("module m\n\nfn f(x: Int) -> Int\n\nfn g() -> Int { 1 }\n");
    module_ok("module m\n\nfn f(x: Int) -> Int\n");
    module_ok("module m\n\nfn f(x: Int) -> Int\n    requires x > 0\n    ensures result > 0\n");
    module_ok("module m\n\n@builtin(\"std.list.len\")\n@property\npub fn len<T>(self: List<T>) -> Int\n\n@builtin(\"std.list.push\")\n@method\npub fn push<T>(self: List<T>, value: T) -> List<T>\n");
    let m = module_ast("module m\n\nfn f(x: Int) -> Int\nfn g() -> Int { 1 }\n");
    assert_eq!(m.declarations.len(), 2);
}

// ===========================================================================
// 3.10 Tests and properties
// ===========================================================================

#[test]
fn test_declarations_take_a_text_name_and_a_block() {
    let d = module_ok(
        "module m\ntest \"pagination preserves a short tail\" {\n    assert paginate([1, 2, 3], 2) == [[1, 2], [3]]\n}\ntest \"one line\" {}\ntest \"next line\"\n{\n    assert true\n}\ntest \"\" {}\ntest \"esc\\n\\\"q\\\"\" {}\n",
    );
    assert_contains(
        &d,
        "(test \"pagination preserves a short tail\" (block (assert (== (call (name paginate) (list (int 1) (int 2) (int 3)) (int 2)) (list (list (int 1) (int 2)) (list (int 3)))))))",
    );
    assert_contains(&d, "(test \"one line\" (block))");
    assert_contains(&d, "(test \"next line\" (block (assert (bool true))))");
    // An empty name is a semantic error (3.10 "Semantics"), not a parse error.
    assert_contains(&d, "(test \"\" (block))");
    assert_contains(&d, "(test \"esc\\n\\\"q\\\"\" (block))");
}

#[test]
fn test_declaration_shapes_are_rejected() {
    assert_code(&module_err("module m\n\ntest t {}\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\ntest T {}\n"), codes::SYNTAX_ERROR);
    assert_code(&module_err("module m\n\ntest {}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\ntest \"a\" \"b\" {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&module_err("module m\n\ntest \"a\"\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\ntest \"a\"\n\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntest \"a\" -> Unit {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntest \"a\"() {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntest \"a\" {} {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntest \"a\" {} test \"b\" {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntest \"a\"\n    for x in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\ntest \"a\"\n    requires true\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn property_declarations_need_one_or_more_for_lines() {
    let d = module_ok(
        "module m\nproperty \"pagination preserves input\"\n    for items in gen.list(gen.int)\n    for size in gen.positive_int\n{\n    assert flatten(paginate(items, size)) == items\n}\nproperty \"one\"\n    for x in xs\n{\n}\n",
    );
    assert_contains(
        &d,
        "(property \"pagination preserves input\" (for items (call (member (name gen) list) (member (name gen) int))) (for size (member (name gen) positive_int)) (block (assert (== (call (name flatten) (call (name paginate) (name items) (name size))) (name items)))))",
    );
    assert_contains(&d, "(property \"one\" (for x (name xs)) (block))");
    // Blank lines and comments between the clauses collapse into one NEWLINE (2.4).
    let d = module_ok("module m\nproperty \"spaced\"\n\n    for x in xs\n\n    // c\n    for y in ys\n\n{\n    assert true\n}\n");
    assert_contains(&d, "(property \"spaced\" (for x (name xs) (trivia blank)) (for y (name ys) (trivia (lead-blank \"// c\"))) (block (assert (bool true)");
    let m = module_ast(
        "module m\n\nproperty \"p\"\n    for a in xs\n    for b in ys\n    for c in zs\n{\n}\n",
    );
    let DeclKind::Property(p) = &decl(&m, 0).kind else {
        panic!()
    };
    assert_eq!(p.for_clauses.len(), 3);
    assert_eq!(p.for_clauses[2].binder.text, "c");
}

#[test]
fn property_with_zero_for_lines_is_rejected() {
    // property_decl = "property" TEXT ( NEWLINE for_clause )+ NEWLINE block
    assert_code(
        &module_err("module m\n\nproperty \"p\" {\n    assert true\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n{\n    assert true\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\" {}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn property_layout_and_for_clause_shapes_are_rejected() {
    // `for` on the header line (2.4 rule 1).
    assert_code(
        &module_err("module m\n\nproperty \"p\" for x in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Two `for` clauses on one line.
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x in xs for y in ys\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // `{` on the clause line (rule 2).
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x in xs {\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Header with nothing after it (`expected { or a clause`, rule 3).
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x in xs\n"),
        codes::SYNTAX_ERROR,
    );
    // Binder is LOWER_IDENT; `in` and a range are mandatory.
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for X in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for _ in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x in\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x: Int in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x, y in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Name must be TEXT; other clause kinds are not `for`.
    assert_code(
        &module_err("module m\n\nproperty p\n    for x in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    requires true\n    for x in xs\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // A `for` clause is not a statement.
    assert_code(
        &module_err("module m\n\nfn f() -> Int {\n    for x in xs\n    1\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn for_clause_range_is_a_header_expression_d41() {
    // Struct literals are disabled at the top level of a `for` range and
    // re-enabled inside brackets.
    let d = module_ok("module m\n\nproperty \"p\"\n    for x in gen.of((Gen { seed: 1 }))\n{\n}\n");
    assert_contains(
        &d,
        "(for x (call (member (name gen) of) (paren (struct-lit Gen (field seed (int 1))))))",
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x in Gen { seed: 1 }\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

// ===========================================================================
// 4.1-4.3 Work files
// ===========================================================================

#[test]
fn work_file_corpus_shape_dumps_in_source_order() {
    let d = work_ok(
        "use reader.pages.{paginate, Positive}\nuse std.text.trim\nchange pagination_update {\n    base git(\"main\")\n    target git(\"main\")\n    workers 3\n    task implementation {\n        write { paginate.body }\n        depend { paginate.contract, Positive.definition }\n        preserve {\n            paginate.signature,\n            paginate.contract,\n            paginate.effects,\n            paginate.verification_policy,\n        }\n        goal \"Implement pagination.\"\n    }\n    task examples {\n        add { tests.pagination.* }\n        goal \"Add boundary cases.\"\n    }\n    task audit after implementation, examples {\n        write {}\n        goal \"Review both patches.\"\n    }\n    integrate {\n        require typecheck\n        require prove(paginate)\n        require test(tests.pagination.*)\n        require review(audit)\n        require no_holes\n    }\n}\n",
    );
    assert_contains(&d, "(work (use reader.pages (items paginate Positive)) (use std.text trim) (change pagination_update (base \"main\") (target \"main\") (workers 3) (task implementation (write paginate.body) (depend paginate.contract Positive.definition) (preserve paginate.signature paginate.contract paginate.effects paginate.verification_policy) (goal \"Implement pagination.\")) (task examples (add tests.pagination.*) (goal \"Add boundary cases.\")) (task audit (after implementation examples) (write) (goal \"Review both patches.\")) (integrate (require typecheck) (require (prove paginate)) (require (test tests.pagination.*)) (require (review audit)) (require no_holes))))");
}

#[test]
fn work_change_items_in_any_order_kept_in_source_order() {
    // 4.1: items may appear in any order; the formatter reorders, the parser keeps source order.
    let d = work_ok(
        "change c {\n    integrate {\n        require no_holes\n    }\n    task b {\n        goal \"b\"\n    }\n    target git(\"v1\")\n    workers 0\n    task a {\n        goal \"a\"\n    }\n    base git(\"refs/tags/v0\")\n}\n",
    );
    assert_contains(
        &d,
        "(change c (integrate (require no_holes)) (task b (goal \"b\")) (target \"v1\") (workers 0) (task a (goal \"a\")) (base \"refs/tags/v0\"))",
    );
    let w = work_ast("change c {\n    target git(\"t\")\n    base git(\"b\")\n}\n");
    let c = &w.changes[0];
    assert_eq!(c.base().map(|g| g.reference.value.as_str()), Some("b"));
    assert_eq!(c.target().map(|g| g.reference.value.as_str()), Some("t"));
    assert!(c.workers().is_none());
    assert!(c.integrate().is_none());
    assert_eq!(c.tasks().count(), 0);
}

#[test]
fn work_task_items_in_any_order_kept_in_source_order() {
    let d = work_ok(
        "change c {\n    task t {\n        goal \"g\"\n        preserve { a.signature }\n        add { ns.* }\n        depend { a.contract }\n        write { a.body }\n    }\n}\n",
    );
    assert_contains(
        &d,
        "(task t (goal \"g\") (preserve a.signature) (add ns.*) (depend a.contract) (write a.body))",
    );
}

#[test]
fn work_minimal_and_empty_bodies() {
    // The grammar admits empty bodies; `base`/`target` being required is a
    // coordinator semantics rule (4.1), not a parse rule.
    assert_contains(&work_ok("change c {}\n"), "(work (change c))");
    assert_contains(&work_ok("change c {\n}\n"), "(work (change c))");
    assert_contains(&work_ok("change c {\n\n\n}"), "(work (change c))");
    assert_contains(
        &work_ok("change c {\n    task t {}\n    integrate {}\n}\n"),
        "(change c (task t) (integrate))",
    );
    // Several changes per file (D34).
    assert_contains(
        &work_ok("change a {}\nchange b {}\n\n\nchange c {}\n"),
        "(work (change a) (change b) (change c (trivia blank)))",
    );
    // `{` on the next line after any header (D63 / 2.4 table).
    let d = work_ok("change c\n{\n    task t after a\n    {\n        write {}\n    }\n    integrate\n    {\n        require typecheck\n    }\n}\n");
    assert_contains(
        &d,
        "(change c (task t (after a) (write)) (integrate (require typecheck)))",
    );
}

#[test]
fn work_nested_braces_on_one_line_follow_lines_x() {
    // lines(X) = NEWLINE? ( X ( NEWLINE X )* NEWLINE? )? admits a single item
    // with no newline on either side, so a fully one-line change is legal.
    assert_contains(
        &work_ok("change c { task t { write { a.body } } }\n"),
        "(change c (task t (write a.body)))",
    );
    assert_contains(
        &work_ok("change c { integrate { require typecheck } }"),
        "(change c (integrate (require typecheck)))",
    );
    // Mismatched or stray braces.
    assert_code(
        &work_err("change c { task t { write { a } }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c { task t { write { a } } } }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write {{ a }}\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    { task t {} }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        goal { \"g\" }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn work_duplicate_items_carry_the_name_field() {
    let dup = |src: &str, name: &str| {
        let e = work_err(src);
        assert_code(&e, codes::DUPLICATE_WORK_ITEM);
        let d = e
            .iter()
            .find(|d| d.code == codes::DUPLICATE_WORK_ITEM)
            .unwrap();
        assert_eq!(
            d.data.get("name").and_then(|v| v.as_str()),
            Some(name),
            "{src}"
        );
    };
    dup(
        "change c {\n    base git(\"a\")\n    base git(\"b\")\n}\n",
        "base",
    );
    dup(
        "change c {\n    target git(\"a\")\n    target git(\"b\")\n}\n",
        "target",
    );
    dup("change c {\n    workers 1\n    workers 1\n}\n", "workers");
    dup(
        "change c {\n    integrate {}\n    integrate {}\n}\n",
        "integrate",
    );
    dup("change c {\n    task t {}\n    task t {}\n}\n", "t");
    dup(
        "change c {\n    task t {\n        write {}\n        write {}\n    }\n}\n",
        "write",
    );
    dup(
        "change c {\n    task t {\n        add { a.* }\n        add { b.* }\n    }\n}\n",
        "add",
    );
    dup(
        "change c {\n    task t {\n        depend { a }\n        depend { a }\n    }\n}\n",
        "depend",
    );
    dup(
        "change c {\n    task t {\n        preserve { a }\n        preserve { b }\n    }\n}\n",
        "preserve",
    );
    dup(
        "change c {\n    task t {\n        goal \"a\"\n        goal \"b\"\n    }\n}\n",
        "goal",
    );
    let e = work_err("change c {\n    base git(\"a\")\n    base git(\"b\")\n}\n");
    assert!(has_message(&e, "duplicate base"));
    let e = work_err("change c {\n    task t {\n        write {}\n        write {}\n    }\n}\n");
    assert!(has_message(&e, "duplicate write set"));
    // The same task name in two different changes is fine (unique within a change).
    work_ok("change a {\n    task t {}\n}\nchange b {\n    task t {}\n}\n");
}

#[test]
fn work_workers_takes_an_int_literal() {
    assert_contains(&work_ok("change c {\n    workers 0\n}\n"), "(workers 0)");
    assert_contains(
        &work_ok("change c {\n    workers 1_000\n}\n"),
        "(workers 1000)",
    );
    assert_contains(
        &work_ok("change c {\n    workers 99999999999999999999999\n}\n"),
        "(workers 99999999999999999999999)",
    );
    assert_code(
        &work_err("change c {\n    workers\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    workers -1\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    workers \"3\"\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    workers three\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    workers 3 4\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    workers (3)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    workers 1 + 2\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Lexical rules still apply (1.5).
    assert_code(
        &work_err("change c {\n    workers 03\n}\n"),
        codes::INVALID_INT_LITERAL,
    );
    assert_code(
        &work_err("change c {\n    workers 1.5\n}\n"),
        codes::FLOAT_LITERAL_UNSUPPORTED,
    );
}

#[test]
fn work_git_ref_is_the_only_ref_form_d35() {
    let d = work_ok("change c {\n    base git(\"main\")\n    target git(\"refs/heads/feature/x\")\n}\nchange d {\n    base git(\"\")\n    target git(\n        \"v1\"\n    )\n}\n");
    assert_contains(
        &d,
        "(change c (base \"main\") (target \"refs/heads/feature/x\"))",
    );
    assert_contains(&d, "(change d (base \"\") (target \"v1\"))");
    assert_code(
        &work_err("change c {\n    base main\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base \"main\"\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base git(main)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base git()\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base git(\"a\", \"b\")\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base git(\"a\",)\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base git \"main\"\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base git(\"main\"\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base hg(\"main\")\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    base git(\"main\") git(\"x\")\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&work_err("change c {\n    base\n}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_err("change c {\n    base git(\"main\") target git(\"main\")\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn work_change_header_shapes() {
    assert_code(&work_err("change C {}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change {}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change c\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change c {\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change c }\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change \"c\" {}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change a.b {}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change c after d {}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change c {} change d {}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_err("change c {}\nbase git(\"main\")\n"),
        codes::SYNTAX_ERROR,
    );
    // Reserved words are never names (1.4), contextual words are (D5).
    assert_code(&work_err("change test {}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("change loop {}\n"), codes::RESERVED_WORD);
    assert_contains(&work_ok("change task {\n    task base after target, write {\n        write { base.body }\n    }\n}\n"),
        "(change task (task base (after target write) (write base.body)))");
    // A `.work` file holds no `.cpl` declarations and no module header.
    assert_code(&work_err("module m\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("fn f() -> Int { 1 }\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_err("change c {}\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    // At least one change (work_file = ... change_decl ...).
    assert_code(&work_err(""), codes::SYNTAX_ERROR);
    assert_code(&work_err("\n\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("use a.b.{c}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("// only a comment\n"), codes::SYNTAX_ERROR);
}

#[test]
fn work_unknown_change_and_task_items() {
    assert_code(
        &work_err("change c {\n    frobnicate\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    write { a }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    goal \"g\"\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    require typecheck\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    use a.b\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    change d {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        base git(\"a\")\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        task u {}\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        integrate {}\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        require typecheck\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        workers 1\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        Write { a }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        write { a }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        typecheck\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn work_task_after_lists() {
    let d = work_ok("change c {\n    task a {}\n    task b after a {}\n    task d after a, b {}\n    task e after a,b,d {\n        goal \"g\"\n    }\n    task f after a, b\n    {\n    }\n}\n");
    assert_contains(&d, "(task b (after a))");
    assert_contains(&d, "(task d (after a b))");
    assert_contains(&d, "(task e (after a b d) (goal \"g\"))");
    assert_contains(&d, "(task f (after a b))");
    let w = work_ast("change c {\n    task d after a, b {}\n}\n");
    let ChangeItemKind::Task(t) = &w.changes[0].items[0].kind else {
        panic!()
    };
    let after = t.after.as_ref().expect("after clause");
    assert_eq!(
        after
            .tasks
            .iter()
            .map(|n| n.text.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    // 2.5 / 4.2: no trailing comma; same line as `task`; LOWER_IDENT names.
    assert_code(
        &work_err("change c {\n    task d after a, {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after a, b, {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after A {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after a b {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after a.b {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after \"a\" {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after { a } {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after (a, b) {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d\n        after a {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d after a after b {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task d {\n        after a\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task D {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {} task u {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn work_selector_sets_including_empty_and_multiline() {
    let d = work_ok(
        "change c {\n    task t {\n        write {}\n        add {}\n        depend { }\n        preserve {\n        }\n    }\n    task u {\n        write { a }\n        add { a.*, }\n        depend { a, b.c, D.definition, e.f.G.body, }\n        preserve {\n            a.signature,\n\n            // comment\n            a.contract\n        }\n    }\n}\n",
    );
    assert_contains(&d, "(task t (write) (add) (depend) (preserve))");
    assert_contains(
        &d,
        "(write a) (add a.*) (depend a b.c D.definition e.f.G.body)",
    );
    assert_contains(&d, "a.signature");
    assert_contains(&d, "a.contract");
    let w = work_ast("change c {\n    task t {\n        write {}\n    }\n}\n");
    let ChangeItemKind::Task(t) = &w.changes[0].items[0].kind else {
        panic!()
    };
    let TaskItemKind::Write(set) = &t.items[0].kind else {
        panic!()
    };
    assert!(set.selectors.is_empty());
}

#[test]
fn work_selector_set_shapes_are_rejected() {
    assert_code(
        &work_err("change c {\n    task t {\n        write a\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write a.body\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write { a b }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write { , }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write { ,a }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write { a,, b }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write ( a )\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write [ a ]\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write { \"a\" }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write { a\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write {} {}\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        write { a } add { b.* }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn work_add_sets_take_namespace_wildcards_only() {
    // 4.2: a facet or declaration selector in an `add` set is a syntax error.
    assert_contains(
        &work_ok(
            "change c {\n    task t {\n        add { tests.pagination.*, other.* }\n    }\n}\n",
        ),
        "(add tests.pagination.* other.*)",
    );
    let e = work_err("change c {\n    task t {\n        add { paginate.body }\n    }\n}\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert!(has_message(&e, "add names a namespace; write paginate.*"));
    let e = work_err("change c {\n    task t {\n        add { tests.pagination }\n    }\n}\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert!(has_message(
        &e,
        "add names a namespace; write tests.pagination.*"
    ));
    assert_code(
        &work_err("change c {\n    task t {\n        add { Positive.definition }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        add { a.*, b }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // The other sets accept every selector form.
    work_ok("change c {\n    task t {\n        write { a.*, b, c.body }\n        depend { a.*, b, c.body }\n        preserve { a.*, b, c.body }\n    }\n}\n");
}

#[test]
fn work_goal_takes_one_text_literal() {
    assert_contains(
        &work_ok("change c {\n    task t {\n        goal \"Implement it.\"\n    }\n}\n"),
        "(goal \"Implement it.\")",
    );
    assert_contains(
        &work_ok("change c {\n    task t {\n        goal \"\"\n    }\n}\n"),
        "(goal \"\")",
    );
    // The token carries the decoded string (1.5).
    let w = work_ast(
        "change c {\n    task t {\n        goal \"line one\\nline two \\u{e9}\"\n    }\n}\n",
    );
    let ChangeItemKind::Task(t) = &w.changes[0].items[0].kind else {
        panic!()
    };
    let TaskItemKind::Goal(g) = &t.items[0].kind else {
        panic!()
    };
    assert_eq!(g.value, "line one\nline two \u{e9}");
    assert_code(
        &work_err("change c {\n    task t {\n        goal\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        goal x\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        goal \"a\" \"b\"\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        goal \"a\" ++ \"b\"\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        goal { \"a\" }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    task t {\n        goal 1\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // D7: a text literal is single-line.
    assert_code(
        &work_err("change c {\n    task t {\n        goal \"a\nb\"\n    }\n}\n"),
        codes::UNTERMINATED_TEXT_LITERAL,
    );
}

#[test]
fn work_integrate_requirements_closed_set_d36() {
    let d = work_ok(
        "change c {\n    integrate {\n        require typecheck\n        require no_holes\n        require no_unresolved_assumptions\n        require prove(paginate)\n        require prove(a.b.c.body)\n        require prove(\n            reader.pages.paginate\n        )\n        require test(tests.pagination.*)\n        require test(tests.*)\n        require review(audit)\n        require typecheck\n    }\n}\n",
    );
    assert_contains(
        &d,
        "(integrate (require typecheck) (require no_holes) (require no_unresolved_assumptions) (require (prove paginate)) (require (prove a.b.c.body)) (require (prove reader.pages.paginate)) (require (test tests.pagination.*)) (require (test tests.*)) (require (review audit)) (require typecheck))",
    );
    let w = work_ast("change c {\n    integrate {\n        require test(tests.*)\n        require review(audit)\n    }\n}\n");
    let i = w.changes[0].integrate().expect("integrate");
    assert_eq!(i.requirements.len(), 2);
    assert!(
        matches!(&i.requirements[0].requirement.kind, RequirementKind::Test(s) if s.kind == SelectorKind::Namespace)
    );
    assert!(
        matches!(&i.requirements[1].requirement.kind, RequirementKind::Review(n) if n.text == "audit")
    );
}

#[test]
fn work_unknown_requirement_carries_found() {
    let e = work_err("change c {\n    integrate {\n        require lint\n    }\n}\n");
    assert_code(&e, codes::UNKNOWN_REQUIREMENT);
    let d = e
        .iter()
        .find(|d| d.code == codes::UNKNOWN_REQUIREMENT)
        .unwrap();
    assert!(d.data.get("found").is_some());
    // D36: the message lists the six forms.
    for form in [
        "typecheck",
        "no_holes",
        "no_unresolved_assumptions",
        "prove",
        "test",
        "review",
    ] {
        assert!(
            d.message.contains(form),
            "message lacks `{form}`: {}",
            d.message
        );
    }
    assert_code(
        &work_err("change c {\n    integrate {\n        require Typecheck\n    }\n}\n"),
        codes::UNKNOWN_REQUIREMENT,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require no_holes_at_all\n    }\n}\n"),
        codes::UNKNOWN_REQUIREMENT,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require typechecks\n    }\n}\n"),
        codes::UNKNOWN_REQUIREMENT,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require prove_all(x)\n    }\n}\n"),
        codes::UNKNOWN_REQUIREMENT,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require \"typecheck\"\n    }\n}\n"),
        codes::UNKNOWN_REQUIREMENT,
    );
}

#[test]
fn work_requirement_argument_shapes_are_rejected() {
    // grammar.md names no code for a `require` with nothing after it.
    work_err("change c {\n    integrate {\n        require\n    }\n}\n");
    assert_code(
        &work_err("change c {\n    integrate {\n        require prove\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require prove()\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require prove(a, b)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require prove(a,)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require prove(a\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require prove { a }\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require test\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require test()\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // D69: no TEXT segment names a single test.
    assert_code(&work_err("change c {\n    integrate {\n        require test(\"pagination handles empty input\")\n    }\n}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_err(
            "change c {\n    integrate {\n        require test(tests.pagination.\"x\")\n    }\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require review\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require review()\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require review(Audit)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require review(a.b)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require review(a, b)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require review(\"audit\")\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require typecheck()\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require typecheck, no_holes\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err(
            "change c {\n    integrate {\n        require typecheck require no_holes\n    }\n}\n",
        ),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require typecheck\n    } {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate x {}\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn work_use_declarations() {
    let d = work_ok("use reader.pages.{paginate, Positive}\nuse reader.pages.paginate\nuse a.{B,}\nchange c {}\n");
    assert_contains(&d, "(use reader.pages (items paginate Positive)) (use reader.pages paginate) (use a (items B))");
    assert_code(&work_err("use a.b.{}\nchange c {}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("use a\nchange c {}\n"), codes::SYNTAX_ERROR);
    assert_code(&work_err("use a.b change c {}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &work_err("change c {}\nuse a.b\n"),
        codes::USE_AFTER_DECLARATION,
    );
}

#[test]
fn work_file_tolerates_crlf_and_comments_everywhere() {
    let d = work_ok("// top\r\nuse a.b.{c} // t\r\n\r\nchange c { // open\r\n    // before base\r\n    base git(\"main\") // after\r\n    target git(\"main\")\r\n    task t after x { // task\r\n        write { // set\r\n            c.body, // sel\r\n        }\r\n        goal \"g\"\r\n    }\r\n}\r\n// end\r\n");
    assert_contains(&d, "(change c (base \"main\"");
    assert_contains(&d, "(task t (after x)");
}

// ===========================================================================
// 4.4 Selectors
// ===========================================================================

fn selector_of(src_set: &str) -> Selector {
    let w = work_ast(&format!(
        "change c {{\n    task t {{\n        write {{ {src_set} }}\n    }}\n}}\n"
    ));
    let ChangeItemKind::Task(t) = &w.changes[0].items[0].kind else {
        panic!()
    };
    let TaskItemKind::Write(set) = &t.items[0].kind else {
        panic!()
    };
    assert_eq!(set.selectors.len(), 1, "{src_set}");
    set.selectors[0].clone()
}

#[test]
fn selector_forms_facet_namespace_and_declaration() {
    let s = selector_of("a.b.c.body");
    assert_eq!(s.path.dotted(), "a.b.c");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Body));

    let s = selector_of("tests.pagination.*");
    assert_eq!(s.path.dotted(), "tests.pagination");
    assert_eq!(s.kind, SelectorKind::Namespace);

    let s = selector_of("Positive.definition");
    assert_eq!(s.path.dotted(), "Positive");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Definition));

    let s = selector_of("paginate");
    assert_eq!(s.path.dotted(), "paginate");
    assert_eq!(s.kind, SelectorKind::Declaration);

    let s = selector_of("a.*");
    assert_eq!(s.path.dotted(), "a");
    assert_eq!(s.kind, SelectorKind::Namespace);

    // Mixed-case segments (1.3: selector segments are either class).
    let s = selector_of("reader.pages.StoreError");
    assert_eq!(s.path.dotted(), "reader.pages.StoreError");
    assert_eq!(s.kind, SelectorKind::Declaration);
    let s = selector_of("A.B.C");
    assert_eq!(s.path.dotted(), "A.B.C");
    assert_eq!(s.kind, SelectorKind::Declaration);
}

#[test]
fn selector_every_facet_name_including_effects_keyword() {
    for (spelling, kind) in [
        ("identity", FacetKind::Identity),
        ("name", FacetKind::Name),
        ("signature", FacetKind::Signature),
        ("contract", FacetKind::Contract),
        ("effects", FacetKind::Effects),
        ("body", FacetKind::Body),
        ("docs", FacetKind::Docs),
        ("verification_policy", FacetKind::VerificationPolicy),
        ("definition", FacetKind::Definition),
    ] {
        let s = selector_of(&format!("reader.pages.paginate.{spelling}"));
        assert_eq!(s.path.dotted(), "reader.pages.paginate", "{spelling}");
        assert!(
            matches!(&s.kind, SelectorKind::Facet(f) if f.kind == kind),
            "{spelling}: {:?}",
            s.kind
        );
    }
    // The dump spells facets back exactly.
    assert_contains(
        &work_ok("change c {\n    task t {\n        write { a.effects, a.verification_policy, a.name }\n    }\n}\n"),
        "(write a.effects a.verification_policy a.name)",
    );
}

#[test]
fn selector_trailing_facet_name_is_always_the_facet_d37() {
    // A declaration named `body` is selected as `module.body.definition`.
    let s = selector_of("m.body.definition");
    assert_eq!(s.path.dotted(), "m.body");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Definition));
    let s = selector_of("m.body.body");
    assert_eq!(s.path.dotted(), "m.body");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Body));
    let s = selector_of("m.contract");
    assert_eq!(s.path.dotted(), "m");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Contract));
    // `Name` (UPPER) is not the facet `name`.
    let s = selector_of("m.Name");
    assert_eq!(s.path.dotted(), "m.Name");
    assert_eq!(s.kind, SelectorKind::Declaration);
    // A facet-spelled single segment has no path before it, so by the EBNF
    // (`selector_path "." facet`) it is a declaration selector named `body`.
    // The 4.4 implementation note is ambiguous here; see spec issues.
    let s = selector_of("body");
    assert_eq!(s.path.dotted(), "body");
    assert_eq!(s.kind, SelectorKind::Declaration);
    // `a.body.contract` is the contract facet of the declaration `a.body`.
    let s = selector_of("a.body.contract");
    assert_eq!(s.path.dotted(), "a.body");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Contract));
}

#[test]
fn selector_malformed_shapes_are_rejected() {
    let bad = |sel: &str| {
        let e = work_err(&format!(
            "change c {{\n    task t {{\n        write {{ {sel} }}\n    }}\n}}\n"
        ));
        assert_code(&e, codes::SYNTAX_ERROR);
    };
    bad("a.*.b");
    bad("a.*.*");
    bad("a.");
    bad("a.b.");
    bad(".a");
    bad("a..b");
    bad("*");
    bad("*.a");
    bad("a.**");
    bad("a.*b");
    bad("a.1");
    bad("a.\"x\"");
    bad("a.effects.body");
    bad("a.effects.*");
    bad("effects");
    bad("a.b(c)");
    bad("a b");
    bad("a.body body");
    bad("test.a");
    // `loop` is reserved for a future edition: D4's own diagnostic, which
    // spec/semantics.md 9.4 codes `reserved_word` (as `change loop {}` and
    // `module a.loop` above).
    assert_code(
        &work_err("change c {\n    task t {\n        write { a.loop }\n    }\n}\n"),
        codes::RESERVED_WORD,
    );
    bad("_");
    bad("a._");
    bad("?a");
    bad("a?");
}

#[test]
fn selectors_in_prove_and_test_requirements_accept_every_form() {
    let d = work_ok("change c {\n    integrate {\n        require prove(a.b.c.body)\n        require prove(tests.*)\n        require prove(Positive.definition)\n        require prove(paginate)\n        require test(tests.pagination.*)\n        require test(a.body)\n        require test(x)\n    }\n}\n");
    assert_contains(&d, "(require (prove a.b.c.body)) (require (prove tests.*)) (require (prove Positive.definition)) (require (prove paginate)) (require (test tests.pagination.*)) (require (test a.body)) (require (test x))");
    assert_code(
        &work_err("change c {\n    integrate {\n        require prove(a.*.b)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    integrate {\n        require test(a.)\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

// ===========================================================================
// 4.5 Patch files
// ===========================================================================

#[test]
fn patch_form_header_expect_block() {
    let d = patch_ok("patch paginate.body\n    expect base(paginate.body)\n{\n    var pages: List<List<T>> = []\n    pages\n}\n");
    assert_contains(&d, "(patch-file (patch paginate.body (expect paginate.body) (block (var pages (ty List (ty List (ty T))) (list)) (tail (name pages)))))");
    // Indentation is never significant (1.1); blank lines collapse (2.1).
    let d = patch_ok("patch a.body\nexpect base(a.body)\n{\n}\n");
    assert_contains(&d, "(patch a.body (expect a.body) (block))");
    let d = patch_ok("patch a.body\n\n\n    expect base(a.body)\n\n// note\n{\n    1\n}");
    assert_contains(&d, "(patch a.body (expect a.body (trivia blank)) (block (tail (int 1) (trivia (lead-blank \"// note\")))))");
    // Fully qualified selectors on both sides, and different selectors (the
    // coordinator checks they denote the same facet, D39).
    let d = patch_ok("patch reader.pages.paginate.body\n    expect base(paginate.body)\n{\n}\n");
    assert_contains(
        &d,
        "(patch reader.pages.paginate.body (expect paginate.body) (block))",
    );
    let p = patch_ast("patch a.body\n    expect base(b.body)\n{\n}\n");
    assert_eq!(p.forms[0].target.path.dotted(), "a");
    assert_eq!(p.forms[0].expect.base.path.dotted(), "b");
}

#[test]
fn patch_selector_is_any_selector_syntactically() {
    // 4.5 production: `"patch" selector`; D39's body-only rule is the
    // coordinator's check, so other selector forms parse.
    patch_ok("patch paginate.contract\n    expect base(paginate.contract)\n{\n}\n");
    patch_ok("patch paginate\n    expect base(paginate)\n{\n}\n");
    patch_ok("patch tests.*\n    expect base(tests.*)\n{\n}\n");
}

#[test]
fn patch_block_is_a_full_cpl_block() {
    let d = patch_ok("patch f.body\n    expect base(f.body)\n{\n    let patch = expect + base\n    while patch > 0\n        invariant patch >= 0\n        decreases patch\n    {\n        patch = patch - 1\n    }\n    if c { return } else { return Unit }\n    match x {\n        Ok(v) => v\n        Err(_) => 0\n    }\n}\n");
    assert_contains(&d, "(let patch (+ (name expect) (name base)))");
    assert_contains(&d, "(while (> (name patch) (int 0)) (invariant (>= (name patch) (int 0))) (decreases (name patch)) (block (assign patch (- (name patch) (int 1)))))");
    assert_contains(
        &d,
        "(tail (match (name x) (arm (ctor Ok (bind v)) (name v)) (arm (ctor Err _) (int 0))))",
    );
    // Declarations and `use` never appear inside the block (3.11).
    assert_code(
        &patch_err("patch f.body\n    expect base(f.body)\n{\n    use a.b\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch f.body\n    expect base(f.body)\n{\n    fn g() -> Int { 1 }\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch f.body\n    expect base(f.body)\n{\n    patch g.body\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn two_patches_in_one_file() {
    let d = patch_ok("patch a.body\n    expect base(a.body)\n{\n    1\n}\npatch b.body\n    expect base(b.body)\n{\n    2\n}\n/// third\npatch c.body\n    expect base(c.body)\n{\n}\n");
    assert_contains(&d, "(patch-file (patch a.body (expect a.body) (block (tail (int 1)))) (patch b.body (expect b.body) (block (tail (int 2)))) (patch c.body (docs \"third\") (expect c.body) (block)))");
    let p = patch_ast("patch a.body\n    expect base(a.body)\n{\n}\npatch b.body\n    expect base(b.body)\n{\n}\n");
    assert_eq!(p.forms.len(), 2);
}

#[test]
fn patch_missing_expect_is_rejected() {
    assert_code(&patch_err("patch a.body\n{\n}\n"), codes::SYNTAX_ERROR);
    assert_code(&patch_err("patch a.body\n"), codes::SYNTAX_ERROR);
    assert_code(&patch_err("patch a.body\n\n"), codes::SYNTAX_ERROR);
    assert_code(
        &patch_err("patch a.body\n    base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base()\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body, b.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base a.body\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect head(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base(\"a\")\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body)\n    expect base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body)\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body)\n\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body)\n{\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn patch_expect_or_brace_on_the_wrong_line_is_rejected() {
    // 4.5: `expect` starts its own line and `{` the next one.
    let e = patch_err("patch a.body expect base(a.body)\n{\n}\n");
    assert_code(&e, codes::SYNTAX_ERROR);
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body) {\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body expect base(a.body) {\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body expect base(a.body) {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&patch_err("patch a.body {\n}\n"), codes::SYNTAX_ERROR);
}

#[test]
fn patch_file_shapes_are_rejected() {
    assert_code(&patch_err(""), codes::SYNTAX_ERROR);
    assert_code(&patch_err("\n\n"), codes::SYNTAX_ERROR);
    assert_code(&patch_err("// nothing\n"), codes::SYNTAX_ERROR);
    assert_code(&patch_err("patch\n"), codes::SYNTAX_ERROR);
    assert_code(
        &patch_err("patch \"a\"\n    expect base(a)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.*.b\n    expect base(a)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.\n    expect base(a)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("expect base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&patch_err("{\n}\n"), codes::SYNTAX_ERROR);
    // D40: no module header, imports, declarations, or work items in a .patch file.
    assert_code(&patch_err("module m\n"), codes::SYNTAX_ERROR);
    assert_code(
        &patch_err("use a.b\npatch a.body\n    expect base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&patch_err("fn f() -> Int { 1 }\n"), codes::SYNTAX_ERROR);
    assert_code(&patch_err("change c {}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body)\n{\n}\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("patch a.body\n    expect base(a.body)\n{\n}\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&patch_err("patch a.body\n    expect base(a.body)\n{\n} patch b.body\n    expect base(b.body)\n{\n}\n"), codes::SYNTAX_ERROR);
    assert_code(
        &patch_err("Patch a.body\n    expect base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &patch_err("pub patch a.body\n    expect base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn patch_forms_never_appear_in_cpl_or_work_files_d40() {
    assert_code(
        &module_err("module m\n\npatch a.body\n    expect base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {}\npatch a.body\n    expect base(a.body)\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &work_err("change c {\n    patch a.body\n        expect base(a.body)\n    {\n    }\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

// ===========================================================================
// D59: diagnostics and the one-error-per-file rule
// ===========================================================================

#[test]
fn generic_syntax_errors_carry_expected_and_found_fields() {
    // D59: a syntax error carries the set of expected tokens; 9.4 lists the
    // `expected` and `found` fields on `syntax_error`.
    let e = module_err("module m\n\nfn f( -> Int { 1 }\n");
    let d = e
        .iter()
        .find(|d| d.code == codes::SYNTAX_ERROR)
        .expect("syntax_error");
    assert!(d.data.get("expected").is_some(), "{d:?}");
    assert!(d.data.get("found").is_some(), "{d:?}");
    assert!(d.message.starts_with("expected "), "{}", d.message);
    assert!(d.span.is_some());
    let e = work_err("change c {\n    task t {\n        write { a b }\n    }\n}\n");
    let d = e
        .iter()
        .find(|d| d.code == codes::SYNTAX_ERROR)
        .expect("syntax_error");
    assert!(d.data.get("found").is_some(), "{d:?}");
}

#[test]
fn every_diagnostic_is_an_error_with_a_span_and_the_parse_category() {
    for (src, kind) in [
        ("module m\n\nfn f() -> Int { let x = )\n    1\n}\n", "cpl"),
        ("change c {\n    workers 1\n    workers 2\n}\n", "work"),
        ("patch a.body expect base(a.body)\n{\n}\n", "patch"),
    ] {
        let diags = match kind {
            "cpl" => parse_module(src, FileId(3)).diagnostics,
            "work" => parse_work(src, FileId(3)).diagnostics,
            _ => parse_patch(src, FileId(3)).diagnostics,
        };
        assert!(!diags.is_empty(), "{src}");
        for d in &diags {
            assert!(d.is_error(), "{d:?}");
            assert_eq!(d.category, Category::Parse);
            let span = d.span.expect("every parse diagnostic has a span");
            assert_eq!(span.file, FileId(3));
            assert!(span.start as usize <= src.len());
            assert!(span.end as usize <= src.len());
            assert!(span.start <= span.end);
        }
    }
}

#[test]
fn d59_an_cpl_file_stops_at_its_first_syntax_error() {
    // D59: "A file stops at its first lexical or syntax error: 0.1 performs no
    // error recovery or resynchronisation" (also spec/semantics.md 9.4, S52).
    // Three independent errors in three functions: exactly one diagnostic,
    // at the first offending token.
    let src = "module m\n\nfn a() -> Int {\n    let x = )\n    1\n}\n\nfn b() -> Int {\n    let y = )\n    1\n}\n\nfn c() -> Int {\n    let z = )\n    1\n}\n";
    let r = parse_module(src, FileId(0));
    assert!(r.value.is_none());
    assert_eq!(
        r.diagnostics.len(),
        1,
        "D59: one diagnostic per file, got {}:\n{}",
        r.diagnostics.len(),
        render(src, &r.diagnostics)
    );
    // The first error is the `)` after `let x =` (the `)` of `fn a()` closes
    // a well-formed parameter list).
    let first_bad_paren = (src.find("= )").unwrap() + 2) as u32;
    assert_eq!(r.diagnostics[0].span.unwrap().start, first_bad_paren);
}

#[test]
fn d59_a_work_file_stops_at_its_first_syntax_error() {
    let src = "change c {\n    base git(\"a\")\n    base git(\"b\")\n    workers 1\n    workers 2\n    task t {\n        write {}\n        write {}\n    }\n}\n";
    let r = parse_work(src, FileId(0));
    assert!(r.value.is_none());
    assert_eq!(
        r.diagnostics.len(),
        1,
        "D59: one diagnostic per file, got {}:\n{}",
        r.diagnostics.len(),
        render(src, &r.diagnostics)
    );
    assert_eq!(r.diagnostics[0].code, codes::DUPLICATE_WORK_ITEM);
    assert_eq!(
        r.diagnostics[0].data.get("name").and_then(|v| v.as_str()),
        Some("base")
    );
}

#[test]
fn d59_a_patch_file_stops_at_its_first_syntax_error() {
    let src = "patch a.body expect base(a.body)\n{\n}\n\npatch b.body expect base(b.body)\n{\n}\n";
    let r = parse_patch(src, FileId(0));
    assert!(r.value.is_none());
    assert_eq!(
        r.diagnostics.len(),
        1,
        "D59: one diagnostic per file, got {}:\n{}",
        r.diagnostics.len(),
        render(src, &r.diagnostics)
    );
    let first_expect = src.find("expect").unwrap() as u32;
    assert_eq!(r.diagnostics[0].span.unwrap().start, first_expect);
}

#[test]
fn d59_a_lexical_error_is_the_only_diagnostic_even_with_later_syntax_errors() {
    let src = "module m\n\nfn f() -> Int { 1; }\n\nfn g() -> Int { let x = ) }\n";
    let r = parse_module(src, FileId(0));
    assert!(r.value.is_none());
    assert_eq!(r.diagnostics[0].code, codes::INVALID_CHARACTER);
    assert_eq!(
        r.diagnostics.len(),
        1,
        "D59: one diagnostic per file, got {}:\n{}",
        r.diagnostics.len(),
        render(src, &r.diagnostics)
    );
}

// ===========================================================================
// No-panic and stack safety
// ===========================================================================

const GARBAGE: &[&str] = &[
    "@",
    "@@",
    "@\n@\n@",
    "{",
    "}",
    "{}",
    "{{{}}}",
    "module m\n@",
    "module m\n@id(",
    "module m\n@id(\"x\")",
    "module m\n@id(\"x\")\n@prove",
    "module m\n{",
    "module m\n}",
    "module m\npub",
    "module m\npub pub pub",
    "module m\ntype",
    "module m\ntype A",
    "module m\ntype A =",
    "module m\ntype A = Int where",
    "module m\ntype A(",
    "module m\nstruct",
    "module m\nstruct S {",
    "module m\nstruct S {\n    x",
    "module m\nstruct S {\n    x:",
    "module m\nstruct S {\n    x: List<",
    "module m\nenum E {\n    A(",
    "module m\nenum E {\n    A(Int,",
    "module m\ncapability",
    "module m\ncapability C {\n    fn",
    "module m\ncapability C {\n    fn get(",
    "module m\ncapability C {\n    fn get(id: Id) ->",
    "module m\ncapability C {\n    fn get(id: Id) -> Int\n        requires",
    "module m\nfn f(using",
    "module m\nfn f(using db",
    "module m\nfn f(using db:",
    "module m\nfn f() -> Int\n    effects",
    "module m\nfn f() -> Int\n    effects {",
    "module m\nfn f() -> Int\n    effects { db.",
    "module m\nfn f() -> Int\n    effects { db.get",
    "module m\nfn f() -> Int\n    requires",
    "module m\nfn f() -> Int\n    ensures result ==",
    "module m\nfn f() -> Int\n    decreases",
    "module m\ntest",
    "module m\ntest \"t\"",
    "module m\ntest \"t\" {",
    "module m\nproperty",
    "module m\nproperty \"p\"",
    "module m\nproperty \"p\"\n    for",
    "module m\nproperty \"p\"\n    for x",
    "module m\nproperty \"p\"\n    for x in",
    "module m\nproperty \"p\"\n    for x in xs",
    "module m\nproperty \"p\"\n    for x in xs\n{",
    "module m\n///",
    "module m\n/// a\n@",
    "module m\n/// a\n@id(\"x\")\n/// b",
    "module m\nuse",
    "module m\nuse a",
    "module m\nuse a.",
    "module m\nuse a.{",
    "module m\nuse a.{b",
    "module m\nuse a.{b,",
    "change",
    "change c",
    "change c {",
    "change c {\n    base",
    "change c {\n    base git",
    "change c {\n    base git(",
    "change c {\n    base git(\"main\"",
    "change c {\n    workers",
    "change c {\n    task",
    "change c {\n    task t after",
    "change c {\n    task t after a,",
    "change c {\n    task t {",
    "change c {\n    task t {\n        write",
    "change c {\n    task t {\n        write {",
    "change c {\n    task t {\n        write { a.",
    "change c {\n    task t {\n        write { a.*",
    "change c {\n    task t {\n        write { a.*.",
    "change c {\n    task t {\n        goal",
    "change c {\n    integrate",
    "change c {\n    integrate {",
    "change c {\n    integrate {\n        require",
    "change c {\n    integrate {\n        require prove",
    "change c {\n    integrate {\n        require prove(",
    "change c {\n    integrate {\n        require review(",
    "change c {\n    integrate {\n        require test(",
    "patch",
    "patch a",
    "patch a.",
    "patch a.body",
    "patch a.body\n    expect",
    "patch a.body\n    expect base",
    "patch a.body\n    expect base(",
    "patch a.body\n    expect base(a.body",
    "patch a.body\n    expect base(a.body)",
    "patch a.body\n    expect base(a.body)\n{",
    "patch a.body\n    expect base(a.body)\n{\n    let",
    "///\n///\n///",
    "/// a\nchange",
    "/// a\npatch",
    "@ @ @ @ @ @ @ @ @ @",
    "{ { { ( ( ( [ [ [ < < <",
    "> > > ] ] ] ) ) ) } } }",
    "module m\nfn f() -> Int { 1 } }",
    "module m\nfn f() -> Int { 1 } )",
    "module m\nfn f() -> Int { 1 } ]",
    "pub type struct enum capability fn test property use module",
    "write add depend preserve goal base target workers task integrate require change patch expect",
    "change c { base git(\"m\") target git(\"m\") }",
    "module m\n\n\n\n\n@\n\n\n\n",
    "module m\n\x00\x01\x02",
    "module m\n\u{feff}",
    "module m\nfn f() -> Int { 1 } /// doc after code",
    "module m\nfn f(x: Int) -> Int requires x > 0 ensures x > 1 decreases x effects {} {}",
    "change c {\n    task t after a b c d e f g {}\n}",
    "change c {\n    task t {\n        add { a, b, c, d.body, e.* }\n    }\n}",
];

fn exercise_all_parsers(src: &str) {
    let a = parse_module(src, FileId(0));
    let b = parse_work(src, FileId(0));
    let c = parse_patch(src, FileId(0));
    check_consistency(src, &a);
    check_consistency(src, &b);
    check_consistency(src, &c);
    if let Some(m) = &a.value {
        let _ = dump_module(m);
    }
    if let Some(w) = &b.value {
        let _ = dump_work(w);
    }
    if let Some(p) = &c.value {
        let _ = dump_patch(p);
    }
}

#[test]
fn no_panic_on_garbage_declarations_work_and_patch_fragments() {
    for src in GARBAGE {
        exercise_all_parsers(src);
    }
}

#[test]
fn no_panic_on_every_prefix_of_declaration_heavy_sources() {
    let sources = [
        "/// docs\nmodule a.b\n\nuse std.text.{trim, parse_int}\nuse a.b.c\n\n@id(\"x\")\n@prove\npub type P = Int where self > 0\npub type W(List<Int>)\n\npub struct S<A> {\n    /// f\n    f: A\n    g: List<List<Int>>\n}\n\npub enum E<T> {\n    A\n    B(T, Int,)\n}\n\npub capability C {\n    /// op\n    fn get(id: Id) -> Result<Page, E>\n        requires id != x\n        ensures result != y\n}\n\n@id(\"y\")\npub fn f<T>(items: List<T>, using db: C,) -> Int\n    effects { db.get, }\n    ensures result > 0\n    requires items.len > 0\n    decreases items.len\n{\n    let x = db.get(id, using db)?\n    x\n}\n\ntest \"t\" {\n    assert true\n}\n\nproperty \"p\"\n    for x in gen.list(gen.int)\n{\n    assert x == x\n}\n",
        "use reader.pages.{paginate, Positive}\n\n/// change\nchange c {\n    base git(\"main\")\n    target git(\"main\")\n    workers 0\n    /// task\n    task t after a, b {\n        write { a.body, b.* }\n        add { tests.* }\n        depend { a.contract, }\n        preserve {\n            a.signature,\n        }\n        goal \"g\"\n    }\n    integrate {\n        require typecheck\n        require prove(a)\n        require test(tests.*)\n        require review(t)\n        require no_holes\n        require no_unresolved_assumptions\n    }\n}\n",
        "/// p\npatch a.b.body\n    expect base(a.b.body)\n{\n    var pages: List<List<T>> = []\n    while rest.len > size\n        invariant flatten(pages) ++ rest == items\n        decreases rest.len\n    {\n        pages = pages.push(rest.take(size))\n    }\n    if not rest.is_empty() {\n        pages = pages.push(rest)\n    }\n    pages\n}\n",
    ];
    let mut checked = 0;
    for src in sources {
        let mut boundaries: Vec<usize> = src.char_indices().map(|(i, _)| i).collect();
        boundaries.push(src.len());
        for &end in &boundaries {
            exercise_all_parsers(&src[..end]);
            checked += 1;
        }
    }
    assert!(checked > 1000, "checked only {checked} prefixes");
}

#[test]
fn lone_at_and_lone_brace_are_rejected_without_panic() {
    for src in [
        "@",
        "{",
        "}",
        "module m\n@",
        "module m\n{",
        "module m\n}",
        "@\n",
        "{\n",
    ] {
        let r = parse_module(src, FileId(0));
        check_consistency(src, &r);
        assert!(r.value.is_none(), "{src:?} must not parse as a module");
        assert!(!r.diagnostics.is_empty());
        let r = parse_work(src, FileId(0));
        assert!(r.value.is_none(), "{src:?} must not parse as a work file");
        assert!(!r.diagnostics.is_empty());
        let r = parse_patch(src, FileId(0));
        assert!(r.value.is_none(), "{src:?} must not parse as a patch file");
        assert!(!r.diagnostics.is_empty());
    }
}

fn deep_inputs(depth: usize) -> Vec<String> {
    let open = "(".repeat(depth);
    let close = ")".repeat(depth);
    vec![
        // where predicate
        format!("module m\n\ntype P = Int where {open}self{close} > 0\n"),
        format!("module m\n\ntype P = Int where {open}\n"),
        // requires clause
        format!(
            "module m\n\nfn f(x: Int) -> Int\n    requires {open}x{close} > 0\n{{\n    x\n}}\n"
        ),
        format!("module m\n\nfn f(x: Int) -> Int\n    requires {open}"),
        // for clause
        format!("module m\n\nproperty \"p\"\n    for x in {open}xs{close}\n{{\n}}\n"),
        // annotation argument
        format!("module m\n\n@id({open}\"x\"{close})\nfn f() -> Int {{ 1 }}\n"),
        // wrapper payload
        format!("module m\n\ntype A({open}Int{close})\n"),
        // selector set and git ref
        format!("change c {{\n    task t {{\n        write {{ {open}a{close} }}\n    }}\n}}\n"),
        format!("change c {{\n    base git({open}\"m\"{close})\n}}\n"),
        format!(
            "change c {{\n    integrate {{\n        require prove({open}a{close})\n    }}\n}}\n"
        ),
        // patch header and body
        format!("patch {open}a.body{close}\n    expect base(a.body)\n{{\n}}\n"),
        format!("patch a.body\n    expect base({open}a.body{close})\n{{\n}}\n"),
        format!("patch a.body\n    expect base(a.body)\n{{\n    {open}1{close}\n}}\n"),
        // nested braces in work files
        format!(
            "change c {{\n    task t {{\n        write {}\n    }}\n}}\n",
            "{".repeat(depth)
        ),
        format!("change c {}", "{".repeat(depth)),
        "{".repeat(depth),
        "}".repeat(depth),
        format!("module m\n\nstruct S {}", "{".repeat(depth)),
        format!("module m\n\nenum E {{\n    A{open}Int{close}\n}}\n"),
        format!(
            "module m\n\nfn f() -> {}Int{} {{ 1 }}\n",
            "List<".repeat(depth),
            ">".repeat(depth)
        ),
        format!(
            "module m\n\nfn f(x: {}Int{}) -> Int {{ 1 }}\n",
            "List<".repeat(depth),
            ">".repeat(depth)
        ),
    ]
}

fn run_deep_inputs() {
    for src in deep_inputs(2000) {
        exercise_all_parsers(&src);
    }
    for src in deep_inputs(5000) {
        exercise_all_parsers(&src);
    }
}

#[test]
fn deeply_nested_parentheses_in_declarations_do_not_panic() {
    run_deep_inputs();
}

#[test]
fn deeply_nested_parentheses_fit_in_a_small_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(run_deep_inputs)
        .expect("spawn");
    handle
        .join()
        .expect("the parser must not overflow a 1 MiB stack");
}

#[test]
fn moderately_nested_parentheses_are_valid_grammar() {
    // grammar.md names no nesting limit, so well-formed parentheses of any
    // depth are legal `paren_expr`s. Fifty levels inside a `where` predicate,
    // a `requires` clause, and a patch body must parse with zero diagnostics.
    let open = "(".repeat(50);
    let close = ")".repeat(50);
    module_ok(&format!(
        "module m\n\ntype P = Int where {open}self{close} > 0\n"
    ));
    module_ok(&format!(
        "module m\n\nfn f(x: Int) -> Int\n    requires {open}x{close} > 0\n{{\n    x\n}}\n"
    ));
    patch_ok(&format!(
        "patch a.body\n    expect base(a.body)\n{{\n    {open}1{close}\n}}\n"
    ));
}
