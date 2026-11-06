//! Parser-decision units (`spec/toolchain.md` Section 12, row "lexer and
//! parser units"): every grammar decision `D1`..`D73` of `spec/grammar.md`
//! has at least one positive and one negative case named by number.
//!
//! A test here is named `d<n>_positive_...` or `d<n>_negative_...`; the last
//! test, `every_decision_has_a_positive_and_a_negative_case_named_by_number`,
//! checks the row over the test names of this file and `lexer.rs`, the two
//! files the row names. Some decisions also have cases named by number in
//! the disambiguation, annotation, and adversarial test files of this crate;
//! those stay where they are, and this file repeats the side each one
//! contributed so that the row holds here alone.
//!
//! Reading of the row for the decisions that reject no input: D50 makes
//! `result` and `self` ordinary `LOWER_IDENT`s, so its negative case is the
//! positions that reject a snake_case name (a type name, a variant, a typed
//! parameter); D57 and D58 constrain the example corpus rather than the
//! grammar, so their negative cases check that the corpus does not take the
//! alternative each decision rejects (glue imports for prelude names; a
//! `?parse_size` hole or an edited design `use` line).

use cpl_diag::{DiagCode, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_syntax::ast::*;
use cpl_syntax::parser::{MAX_DEPTH, MAX_EXPR_OPS};
use cpl_syntax::{
    codes, dump_module, dump_patch, dump_work, lex, parse_module, parse_patch, parse_work,
    FileKind, TokenKind as T,
};
use std::path::Path;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn render(src: &str, diagnostics: &[Diagnostic]) -> String {
    let mut map = SourceMap::new();
    map.add("test.cpl".to_string(), FileOrigin::Virtual, src.to_string());
    cpl_diag::render_all(diagnostics, &map)
}

/// The dump on one line, so shapes can be asserted as substrings.
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

fn module_ok(src: &str) -> String {
    flat(&dump_module(&module_ast(src)))
}

fn wrap(stmts: &str) -> String {
    let indented: Vec<String> = stmts.lines().map(|l| format!("    {l}")).collect();
    format!(
        "module m\n\nfn f() -> Int {{\n{}\n}}\n",
        indented.join("\n")
    )
}

/// Statements inside `fn f() -> Int { ... }`, parsed cleanly.
fn body_ok(stmts: &str) -> String {
    module_ok(&wrap(stmts))
}

fn work_ok(src: &str) -> String {
    let r = parse_work(src, FileId(0));
    match r.value {
        Some(w) if r.diagnostics.is_empty() => flat(&dump_work(&w)),
        _ => panic!(
            "expected a clean .work parse of:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

fn patch_ok(src: &str) -> String {
    let r = parse_patch(src, FileId(0));
    match r.value {
        Some(p) if r.diagnostics.is_empty() => flat(&dump_patch(&p)),
        _ => panic!(
            "expected a clean .patch parse of:\n{src}\n{}",
            render(src, &r.diagnostics)
        ),
    }
}

/// The one diagnostic of a failed parse (grammar D59): no tree, exactly one
/// error, a known code of category `parse`.
#[track_caller]
fn single(src: &str, value_is_none: bool, diagnostics: Vec<Diagnostic>) -> Diagnostic {
    assert!(value_is_none, "expected no tree for:\n{src}");
    assert_eq!(
        diagnostics.len(),
        1,
        "D59: exactly one diagnostic for:\n{src}\n{diagnostics:#?}"
    );
    let d = diagnostics.into_iter().next().expect("one diagnostic");
    assert!(d.is_error(), "{d:#?}");
    assert_eq!(d.category, codes::CATEGORY, "{d:#?}");
    assert!(codes::ALL.contains(&d.code), "unknown code {}", d.code);
    d
}

#[track_caller]
fn module_err(src: &str) -> Diagnostic {
    let r = parse_module(src, FileId(0));
    single(src, r.value.is_none(), r.diagnostics)
}

#[track_caller]
fn body_err(stmts: &str) -> Diagnostic {
    module_err(&wrap(stmts))
}

#[track_caller]
fn work_err(src: &str) -> Diagnostic {
    let r = parse_work(src, FileId(0));
    single(src, r.value.is_none(), r.diagnostics)
}

#[track_caller]
fn patch_err(src: &str) -> Diagnostic {
    let r = parse_patch(src, FileId(0));
    single(src, r.value.is_none(), r.diagnostics)
}

/// Token kinds of a source that lexes without error.
fn kinds(src: &str) -> Vec<T> {
    let (tokens, diagnostics) = lex(src, FileId(0));
    assert!(diagnostics.is_empty(), "{src:?}: {diagnostics:#?}");
    tokens.into_iter().map(|t| t.kind).collect()
}

#[track_caller]
fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected to find\n  {needle}\nin\n{haystack}"
    );
}

#[track_caller]
fn assert_code(d: &Diagnostic, code: DiagCode) {
    assert_eq!(d.code, code, "{d:#?}");
}

#[track_caller]
fn assert_syntax(d: &Diagnostic, message_start: &str) {
    assert_code(d, codes::SYNTAX_ERROR);
    assert!(
        d.message.starts_with(message_start),
        "expected a message starting with {message_start:?}: {d:#?}"
    );
}

fn field<'a>(d: &'a Diagnostic, key: &str) -> Option<&'a str> {
    d.data.get(key).and_then(|v| v.as_str())
}

fn start(d: &Diagnostic) -> usize {
    d.span.expect("a span").start as usize
}

// ---------------------------------------------------------------------------
// Section 1: lexical structure
// ---------------------------------------------------------------------------

#[test]
fn d1_positive_comments_may_contain_non_ascii() {
    // D1: prose for human readers is not constrained to ASCII, in every file
    // form and in ordinary and doc comments alike.
    let m = module_ast("// Größe ∀ 漢字\nmodule m // café\n\n/// Ünïcode docs.\nfn f() -> Int {\n    1 // naïve\n}\n");
    assert_eq!(m.trivia.leading[0].text, "// Größe ∀ 漢字");
    assert_eq!(
        m.declarations[0].docs.as_ref().unwrap().text(),
        "Ünïcode docs."
    );
    work_ok("// über\nchange c {} // ü\n");
    patch_ok("patch f.body // é\n    expect base(f.body)\n{\n}\n");
}

#[test]
fn d1_negative_non_ascii_outside_comments_and_text_is_an_error() {
    // Grammar 1.1: non-ASCII is permitted only inside text literals and
    // comments; an identifier is ASCII (1.3).
    let d = body_err("let café = 1\ncafé");
    assert_code(&d, codes::NON_ASCII_OUTSIDE_TEXT);
    assert_eq!(field(&d, "found"), Some("é"));
}

#[test]
fn d2_positive_the_case_class_decides_the_reading() {
    // D2: `UPPER_IDENT` and `LOWER_IDENT` are distinct tokens; a bare
    // upper-case name in a pattern is a nullary constructor, a lower-case one
    // a binding, and `UPPER_IDENT {` in an expression a struct literal.
    assert_eq!(kinds("Page page"), [T::UpperIdent, T::LowerIdent, T::Eof]);
    let d = body_ok("match r {\n    Err(Unavailable) => 1\n    Err(e) => 2\n}");
    assert_contains(&d, "(arm (ctor Err (ctor Unavailable)) (int 1))");
    assert_contains(&d, "(arm (ctor Err (bind e)) (int 2))");
    assert_contains(
        &module_ok("module m\n\npub struct Page {\n    id: PageId\n}\n\nfn paginate() -> Page {\n    Page { id: x }\n}\n"),
        "(tail (struct-lit Page (field id (name x))))",
    );
}

#[test]
fn d2_negative_a_name_of_the_wrong_class_is_a_syntax_error() {
    // D2: every grammar position names the class it accepts.
    let d = module_err("module m\n\nstruct page {}\n");
    assert_syntax(&d, "expected a struct name (PascalCase identifier)");
    let d = module_err("module m\n\nfn Paginate() -> Int { 1 }\n");
    assert_syntax(&d, "expected a function name (snake_case identifier)");
    assert_code(&module_err("module Reader\n"), codes::SYNTAX_ERROR);
    assert_code(&body_err("let X = 1\n1"), codes::SYNTAX_ERROR);
}

#[test]
fn d3_positive_the_nine_contract_words_are_keyword_tokens() {
    // D3: `requires ensures effects invariant decreases using where forall
    // assert` are reserved keywords, lexed as keyword tokens in every file.
    assert_eq!(
        kinds("requires ensures effects invariant decreases using where forall assert"),
        [
            T::KwRequires,
            T::KwEnsures,
            T::KwEffects,
            T::KwInvariant,
            T::KwDecreases,
            T::KwUsing,
            T::KwWhere,
            T::KwForall,
            T::KwAssert,
            T::Eof
        ]
    );
    assert_contains(
        &module_ok("module m\n\nfn f(x: Int, using db: Db) -> Int\n    effects { db.get }\n    requires x > 0\n    ensures result > 0\n{\n    assert forall y in [x]: y > 0\n    x\n}\n"),
        "(assert (forall y (list (name x)) (> (name y) (int 0))))",
    );
}

#[test]
fn d3_negative_a_contract_word_is_never_an_identifier() {
    // D3: "an agent reading `ensures = 3` should never have to decide whether
    // `ensures` is a variable".
    let d = body_err("let ensures = 3\n1");
    assert_syntax(&d, "expected a binding name or `_`");
    assert_eq!(field(&d, "found"), Some("`ensures`"));
    assert_code(
        &module_err("module m\n\nfn where() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&body_err("assert = 1\n1"), codes::SYNTAX_ERROR);
}

#[test]
fn d4_positive_words_that_contain_a_reserved_word_are_identifiers() {
    // D4 reserves exactly eight spellings; longer identifiers that contain
    // one, and the words inside text literals and comments, are unaffected.
    assert_eq!(
        kinds("asx loops breakfast as_of ghostly await_all"),
        [
            T::LowerIdent,
            T::LowerIdent,
            T::LowerIdent,
            T::LowerIdent,
            T::LowerIdent,
            T::LowerIdent,
            T::Eof
        ]
    );
    assert_contains(
        &body_ok("let loops = \"break continue\" // as async\nloops"),
        "(let loops (text \"break continue\")",
    );
}

#[test]
fn d5_positive_the_work_vocabulary_names_ordinary_values_in_cpl_files() {
    // D5: `add`, `base`, `target`, `write`, `task`, ... are contextual: in a
    // `.cpl` file they are ordinary identifiers.
    let d = module_ok("module m\n\nfn add(write: Int, base: Int) -> Int {\n    let target = write + base\n    let task = target\n    task\n}\n");
    assert_contains(
        &d,
        "(fn add (params (param write (ty Int)) (param base (ty Int)))",
    );
    assert_contains(&d, "(let task (name target))");
    // In a `.work` file the same spellings are recognised by position.
    assert_contains(
        &work_ok("change base {\n    base git(\"main\")\n    task write {\n        write { add.body }\n    }\n}\n"),
        "(change base (base \"main\") (task write (write add.body)))",
    );
}

#[test]
fn d5_negative_the_work_vocabulary_has_no_meaning_in_cpl_files() {
    // D5: the words are `LOWER_IDENT` tokens the `.work`/`.patch` parsers
    // compare by spelling; in a `.cpl` file `change` is just an identifier,
    // which cannot begin a declaration.
    let d = module_err("module m\n\nchange c {}\n");
    assert_syntax(&d, "expected a declaration");
    assert_eq!(field(&d, "found"), Some("identifier `change`"));
    // And in a `.work` file only the exact spellings count.
    assert_syntax(
        &work_err("change c {\n    bases git(\"x\")\n}\n"),
        "expected `base`, `target`, `workers`, `task`, or `integrate`",
    );
}

#[test]
fn d6_positive_integer_literals_are_decimal_with_separators() {
    // D6 and grammar 1.5: decimal digits, `_` between two digits; the token
    // carries the digits without separators.
    let (tokens, diags) = lex("1_000_000 0 42", FileId(0));
    assert!(diags.is_empty(), "{diags:#?}");
    let texts: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["1000000", "0", "42", ""]);
    assert_contains(&body_ok("let n = 1_000\nn"), "(let n (int 1000))");
}

#[test]
fn d6_negative_hexadecimal_and_binary_forms_do_not_exist() {
    // D6: `0x1F` is the literal `0` followed by an identifier character, the
    // adjacency error of grammar 1.5.
    for src in ["let n = 0x1F\nn", "let n = 0b101\nn"] {
        let d = body_err(src);
        assert_code(&d, codes::LITERAL_ADJACENT_IDENTIFIER);
        assert!(d
            .message
            .starts_with("a literal must be separated from an identifier"));
    }
}

#[test]
fn d7_positive_line_breaks_in_text_are_written_with_an_escape() {
    // D7: a text literal is single-line; `\n` is the newline.
    let m = module_ast(&wrap("let t = \"one\\ntwo\"\nt"));
    let DeclKind::Fn(f) = &m.declarations[0].kind else {
        panic!()
    };
    let Statement::Let(l) = &f.body.as_ref().unwrap().statements[0] else {
        panic!()
    };
    assert!(
        matches!(&l.value.kind, ExprKind::Literal(Literal { kind: LiteralKind::Text(t), .. }) if t == "one\ntwo")
    );
}

#[test]
fn d7_negative_a_raw_line_break_ends_the_literal() {
    // D7 and grammar 1.5: a raw line terminator inside a literal is
    // `unterminated text literal`.
    let d = body_err("let t = \"one\ntwo\"\nt");
    assert_code(&d, codes::UNTERMINATED_TEXT_LITERAL);
    assert!(d.message.starts_with("unterminated text literal"));
}

#[test]
fn d8_positive_a_text_literal_separated_from_a_word_is_an_ordinary_literal() {
    // D8 rejects only a word *immediately* followed by `"`; with a space or
    // punctuation between them they are two tokens.
    assert_eq!(kinds("b \"x\""), [T::LowerIdent, T::Text, T::Eof]);
    assert_eq!(kinds("return \"x\""), [T::KwReturn, T::Text, T::Eof]);
    assert_contains(
        &body_ok("let b = f(b, \"x\")\nb"),
        "(let b (call (name f) (name b) (text \"x\")))",
    );
}

#[test]
fn d8_negative_a_prefixed_string_literal_is_reserved() {
    // D8 and grammar 1.8 step 4: identifier, keyword, or `_` directly before
    // `"` is `prefixed_literal_reserved`.
    for src in ["b\"x\"", "test\"x\"", "_\"x\"", "x\"y\""] {
        let d = body_err(src);
        assert_code(&d, codes::PREFIXED_LITERAL_RESERVED);
        assert!(d
            .message
            .starts_with("prefixed string literals are reserved"));
    }
}

#[test]
fn d9_positive_the_unit_value_is_the_constructor_unit() {
    // D9: `Unit` is an ordinary upper-case name, a constructor reference.
    let d = body_ok("let u = Unit\nOk(Unit)");
    assert_contains(&d, "(let u (name Unit))");
    assert_contains(&d, "(tail (call (name Ok) (name Unit)))");
}

#[test]
fn d9_negative_there_is_no_empty_parenthesis_value() {
    // D9: no `()` form exists; an empty paren is the generic syntax error.
    let d = body_err("let u = ()\nu");
    assert_syntax(&d, "expected an expression");
    assert_eq!(field(&d, "found"), Some("`)`"));
}

#[test]
fn d10_positive_an_adjacent_snake_case_name_after_question_is_a_hole() {
    // D10 and grammar 1.7: `?build_pages` is one HOLE token; `x?` then a line
    // break is propagation; `?self` and `?result` are legal holes (D50).
    let (tokens, _) = lex("?build_pages x? ?self ?result", FileId(0));
    let ks: Vec<T> = tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        ks,
        [
            T::Hole,
            T::LowerIdent,
            T::Question,
            T::Hole,
            T::Hole,
            T::Eof
        ]
    );
    assert_eq!(tokens[0].text, "build_pages");
    let d = body_ok("let x = f()?\n?build_pages");
    assert_contains(&d, "(let x (propagate (call (name f))))");
    assert_contains(&d, "(tail (hole build_pages))");
}

#[test]
fn d10_negative_hole_names_are_snake_case_and_not_keywords() {
    for (src, code) in [
        ("?Foo", codes::HOLE_NAME_CASE),
        ("?_", codes::HOLE_NAME_CASE),
        ("?if", codes::HOLE_NAME_KEYWORD),
        ("?loop", codes::HOLE_NAME_KEYWORD),
        ("?test", codes::HOLE_NAME_KEYWORD),
    ] {
        let d = body_err(src);
        assert_code(&d, code);
        assert_eq!(field(&d, "name"), Some(&src[1..]), "{src}");
    }
}

#[test]
fn d60_positive_four_or_more_slashes_are_an_ordinary_comment() {
    // D60: `////` is not documentation, so it may stand where a doc comment
    // may not, before a statement, and becomes ordinary trivia.
    assert_eq!(kinds("//// banner\nx"), [T::LowerIdent, T::Eof]);
    assert_contains(
        &body_ok("//// banner ////\n1"),
        "(tail (int 1) (trivia (lead \"//// banner ////\")))",
    );
    // Before a declaration it is a leading comment, not part of the docs.
    let m = module_ast("module m\n\n//// section\nfn f() -> Int { 1 }\n");
    assert!(m.declarations[0].docs.is_none());
    assert_eq!(m.declarations[0].trivia.leading[0].text, "//// section");
}

#[test]
fn d60_negative_exactly_three_slashes_are_a_doc_comment() {
    // D60's boundary: `///` is a DOC_COMMENT token, misplaced before a
    // statement (grammar 1.2).
    assert_eq!(kinds("/// doc\nx"), [T::DocComment, T::LowerIdent, T::Eof]);
    let d = body_err("/// doc\n1");
    assert_code(&d, codes::DOC_COMMENT_POSITION);
    assert!(d
        .message
        .starts_with("doc comment must precede a declaration"));
}

#[test]
fn d62_positive_tab_and_non_ascii_are_permitted_raw_in_text() {
    // D62: the exclusion is ASCII C0 other than tab, and DEL; the C1 range
    // and every other non-ASCII scalar value are permitted raw.
    let (tokens, diags) = lex("\"a\tb\u{80}\u{9f}é∀😀\"", FileId(0));
    assert!(diags.is_empty(), "{diags:#?}");
    assert_eq!(tokens[0].text, "a\tb\u{80}\u{9f}é∀😀");
}

#[test]
fn d62_negative_other_control_characters_are_errors() {
    // D62: U+0000-U+001F other than tab and U+000A/U+000D, and U+007F, are
    // `control character in text literal` (code invalid_character).
    for (c, found) in [
        ('\u{1}', "U+0001"),
        ('\u{1f}', "U+001F"),
        ('\u{7f}', "U+007F"),
    ] {
        let d = body_err(&format!("let t = \"a{c}b\"\nt"));
        assert_code(&d, codes::INVALID_CHARACTER);
        assert!(d.message.starts_with("control character in text literal"));
        assert_eq!(field(&d, "found"), Some(found));
    }
}

// ---------------------------------------------------------------------------
// Section 3: `.cpl` declarations
// ---------------------------------------------------------------------------

#[test]
fn d14_positive_a_module_holds_only_declarations() {
    // D14: `use` lines, then type, struct, enum, capability, fn, test, and
    // property declarations; nothing else.
    let m = module_ast("module m\n\nuse a.b.{c}\n\npub type A(Int)\n\nstruct S {}\n\nenum E {}\n\ncapability C {}\n\nfn f() -> Int { 1 }\n\ntest \"t\" {}\n\nproperty \"p\"\n    for x in xs\n{\n}\n");
    assert_eq!(m.uses.len(), 1);
    assert_eq!(m.declarations.len(), 7);
}

#[test]
fn d14_negative_there_are_no_module_level_values() {
    for src in [
        "module m\n\nlet x = 1\n",
        "module m\n\nvar x = 1\n",
        "module m\n\nx = 1\n",
    ] {
        let d = module_err(src);
        assert_syntax(&d, "expected a declaration");
    }
}

#[test]
fn d15_negative_a_use_names_a_module_and_an_item() {
    // D15's single form is `use a.b.c`: a path of at least one module
    // segment, then the item; a bare item is not a use declaration.
    for src in ["module m\n\nuse trim\n", "module m\n\nuse Trim\n"] {
        assert_code(&module_err(src), codes::SYNTAX_ERROR);
    }
    let d = module_err("module m\n\nuse trim\n");
    assert!(d.message.starts_with("expected `.`"), "{d:#?}");
}

#[test]
fn d16_positive_a_use_list_names_one_or_more_items() {
    // D16: the list form is non-empty; it may span lines and end in a comma
    // (grammar 2.5).
    let d = module_ok("module m\n\nuse a.b.{c, D}\nuse e.{f,}\nuse g.h.{\n    i,\n    J,\n}\n");
    assert_contains(&d, "(use a.b (items c D)");
    assert_contains(&d, "(use e (items f))");
    assert_contains(&d, "(use g.h (items i J))");
}

#[test]
fn d17_positive_the_annotation_grammar_is_open() {
    // D17: any `LOWER_IDENT` (or keyword, D71) name with literal arguments
    // parses; `cpl check` decides which annotations exist.
    let d = module_ok(
        "module m\n\n@whatever(\"x\", 1, true, false)\n@flag()\n@bare\nfn f() -> Int { 1 }\n",
    );
    assert_contains(
        &d,
        "(annotation whatever (text \"x\") (int 1) (bool true) (bool false)) (annotation flag ()) (annotation bare)",
    );
}

#[test]
fn d17_negative_an_annotation_has_one_name_and_its_own_line() {
    // D17: `annotation = "@" ( LOWER_IDENT | keyword ) ( "(" ... ")" )? NEWLINE`.
    let d = module_err("module m\n\n@prove fn f() -> Int { 1 }\n");
    assert_syntax(&d, "expected end of line after the annotation");
    let d = module_err("module m\n\n@a.b\nfn f() -> Int { 1 }\n");
    assert_syntax(&d, "expected end of line after the annotation");
    assert_code(
        &module_err("module m\n\n@\nfn f() -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn d18_positive_a_wrapper_has_one_payload_type() {
    let d = module_ok("module m\n\npub type PageId(Text)\ntype Pages(List<List<PageId>>)\n");
    // The first declaration follows a blank line, which it records
    // (toolchain 2.3; the dump's `(trivia blank)`, 12.1).
    assert_contains(&d, "(type PageId pub (wrapper (ty Text)) (trivia blank))");
    assert_contains(&d, "(type Pages (wrapper (ty List (ty List (ty PageId)))))");
}

#[test]
fn d19_positive_structs_enums_and_definition_less_types_take_generics() {
    let d = module_ok("module m\n\npub struct Pair<A, B> {\n    a: A\n    b: B\n}\n\nenum Tree<T> {\n    Leaf(T)\n}\n\npub type List<T>\n");
    assert_contains(&d, "(struct Pair pub (generics A B)");
    assert_contains(&d, "(enum Tree (generics T) (variant Leaf (ty T))");
    assert_contains(&d, "(type List pub (opaque (generics T))");
}

#[test]
fn d19_negative_wrappers_refinements_and_capabilities_take_no_generics() {
    // Grammar 3.5: after `type A<T>` the declaration ends.
    for src in [
        "module m\n\ntype A<T>(T)\n",
        "module m\n\ntype A<T> = Int where self > 0\n",
    ] {
        assert_syntax(
            &module_err(src),
            "expected end of line after the declaration",
        );
    }
    assert_code(
        &module_err("module m\n\ncapability C<T> {\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn d20_positive_variants_carry_positional_payloads() {
    let d = module_ok("module m\n\nenum StoreError {\n    Missing(PageId)\n    Unavailable\n    Pair(Int, Text,)\n}\n");
    assert_contains(
        &d,
        "(variant Missing (ty PageId)) (variant Unavailable) (variant Pair (ty Int) (ty Text))",
    );
}

#[test]
fn d20_negative_there_are_no_record_variants() {
    let d = module_err("module m\n\nenum E {\n    Missing { id: PageId }\n}\n");
    assert_syntax(&d, "expected end of line after the variant");
    // `Variant()` with empty parentheses is a syntax error too (3.7).
    let d = module_err("module m\n\nenum E {\n    V()\n}\n");
    assert_syntax(&d, "expected a payload type");
}

#[test]
fn d21_positive_capability_operations_carry_contracts() {
    let d = module_ok("module m\n\npub capability PageRead {\n    fn get(id: PageId) -> Result<Page, StoreError>\n        requires id != x\n        ensures result != y\n}\n");
    assert_contains(
        &d,
        "(requires (!= (name id) (name x))) (ensures (!= (name result) (name y))))",
    );
}

#[test]
fn d21_negative_operations_take_no_body_and_no_effects() {
    // Grammar 3.8: operations are signatures with contracts only.
    let d = module_err(
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int {\n        1\n    }\n}\n",
    );
    assert_syntax(&d, "expected end of line after the operation");
    let d = module_err(
        "module m\n\ncapability C {\n    fn get(id: Id) -> Int\n        effects { a.b }\n}\n",
    );
    assert_syntax(&d, "expected `requires` or `ensures`");
}

#[test]
fn d22_positive_a_function_header_may_carry_decreases() {
    let d = module_ok(
        "module m\n\nfn count(n: Int) -> Int\n    requires n >= 0\n    decreases n\n{\n    n\n}\n",
    );
    assert_contains(&d, "(requires (>= (name n) (int 0))) (decreases (name n))");
}

#[test]
fn d22_negative_at_most_one_decreases_clause() {
    // Grammar 3.9: `duplicate decreases clause`, at the second keyword.
    let src =
        "module m\n\nfn count(n: Int) -> Int\n    decreases n\n    decreases n - 1\n{\n    n\n}\n";
    let d = module_err(src);
    assert_code(&d, codes::DUPLICATE_CLAUSE);
    assert!(d.message.starts_with("duplicate decreases clause"));
    assert_eq!(field(&d, "name"), Some("decreases"));
    assert_eq!(start(&d), src.find("decreases n -").unwrap());
}

#[test]
fn d23_positive_effect_references_are_parameter_dot_operation() {
    let d = module_ok("module m\n\nfn f(using db: Db, using codec: ImageCodec) -> Int\n    effects { db.get, codec.dimensions, }\n{\n    1\n}\n\nfn g() -> Int\n    effects {}\n{\n    1\n}\n");
    assert_contains(&d, "(effects db.get codec.dimensions)");
    assert_contains(&d, "(effects)");
}

#[test]
fn d61_positive_annotation_change_task_and_after_names_are_snake_case() {
    // D61: these names are `LOWER_IDENT`; annotation arguments are literals.
    let d = module_ok("module m\n\n@id(\"pgn_7c2d\")\n@prove\n@note(\"t\", 3, true)\nfn f() -> Int {\n    ?build_pages\n}\n");
    assert_contains(&d, "(annotation id (text \"pgn_7c2d\")) (annotation prove)");
    assert_contains(&d, "(tail (hole build_pages))");
    assert_contains(
        &work_ok(
            "change pagination_update {\n    task audit after implementation, examples {}\n}\n",
        ),
        "(change pagination_update (task audit (after implementation examples)))",
    );
}

#[test]
fn d64_positive_a_refinement_carries_its_where_predicate() {
    let d = module_ok("module m\n\npub type Positive = Int where self > 0\ntype Small = Int where (self > 0 and self < 10)\n");
    assert_contains(
        &d,
        "(type Positive pub (refinement (ty Int) (where (> (name self) (int 0)))) (trivia blank))",
    );
    assert_contains(&d, "(type Small (refinement (ty Int) (where (paren (and");
}

#[test]
fn d65_positive_fields_and_variants_have_no_visibility_of_their_own() {
    let d = module_ok("module m\n\npub struct Page {\n    id: PageId\n    text: Text\n}\n\npub enum E {\n    A\n    B(Int)\n}\n");
    assert_contains(&d, "(field id (ty PageId)) (field text (ty Text))");
    assert_contains(&d, "(variant A) (variant B (ty Int))");
}

#[test]
fn d65_negative_pub_on_a_field_or_variant_is_a_syntax_error() {
    assert_syntax(
        &module_err("module m\n\nstruct S {\n    pub x: Int\n}\n"),
        "expected a field name",
    );
    assert_syntax(
        &module_err("module m\n\nenum E {\n    pub A\n}\n"),
        "expected a variant name",
    );
}

#[test]
fn d66_negative_clauses_come_before_the_body_and_once_each() {
    // D66 accepts the clauses in any order, but they are still header
    // continuation lines (grammar 2.4), and `effects` stays unique (3.9)
    // whatever lies between the two.
    let d = module_err("module m\n\nfn f(x: Int) -> Int { x }\n    requires x > 0\n");
    assert_code(&d, codes::SYNTAX_ERROR);
    let d = module_err("module m\n\nfn f(using db: Db) -> Int\n    effects { db.get }\n    requires true\n    ensures true\n    effects {}\n{\n    1\n}\n");
    assert_code(&d, codes::DUPLICATE_CLAUSE);
    assert_eq!(field(&d, "name"), Some("effects"));
}

#[test]
fn d67_positive_tests_and_properties_are_declared_without_pub() {
    let d = module_ok("module m\n\ntest \"pagination handles empty input\" {\n    assert true\n}\n\nproperty \"pagination preserves input\"\n    for items in gen.list(gen.int)\n{\n    assert true\n}\n");
    assert_contains(&d, "(test \"pagination handles empty input\"");
    assert_contains(&d, "(property \"pagination preserves input\"");
}

#[test]
fn d67_negative_pub_test_and_pub_property_are_syntax_errors() {
    for src in [
        "module m\n\npub test \"t\" {}\n",
        "module m\n\npub property \"p\"\n    for x in xs\n{\n}\n",
    ] {
        let d = module_err(src);
        assert_code(&d, codes::SYNTAX_ERROR);
        assert!(d
            .message
            .contains("test and property declarations take no `pub`"));
    }
}

// ---------------------------------------------------------------------------
// Section 3.11: statements
// ---------------------------------------------------------------------------

#[test]
fn d24_positive_let_underscore_discards_a_value() {
    assert_contains(
        &body_ok("let _ = f()\nlet _: Int = 1\n1"),
        "(let _ (call (name f))) (let _ (ty Int) (int 1))",
    );
}

#[test]
fn d24_negative_the_wildcard_is_not_an_assignable_name() {
    // D24: the discard is spelled `let _ =`; `_` alone cannot begin an
    // assignment (D52: it is the wildcard token, not an identifier).
    let d = body_err("_ = f()\n1");
    assert_syntax(&d, "expected an expression");
    assert_eq!(field(&d, "found"), Some("`_`"));
}

#[test]
fn d25_positive_let_and_var_bind_a_single_name() {
    let d = body_ok("let x: Int = 1\nvar y = 2\ny = x\ny");
    assert_contains(
        &d,
        "(let x (ty Int) (int 1)) (var y (int 2)) (assign y (name x))",
    );
}

#[test]
fn d25_negative_no_destructuring_compound_or_field_assignment() {
    assert_syntax(
        &body_err("let Page { id } = p\n1"),
        "expected a binding name or `_`",
    );
    assert_syntax(&body_err("x += 1\nx"), "expected an expression");
    assert_syntax(
        &body_err("p.text = t\n1"),
        "expected end of line after the statement",
    );
}

#[test]
fn d26_positive_return_is_a_statement_inside_a_block() {
    let d = body_ok("match r {\n    Err(e) => {\n        return Err(e)\n    }\n    Ok(v) => v\n}");
    assert_contains(
        &d,
        "(arm (ctor Err (bind e)) (block (return (call (name Err) (name e)))))",
    );
}

#[test]
fn d26_negative_return_is_not_an_expression() {
    let d = body_err("match r {\n    Err(e) => return Err(e)\n}");
    assert_syntax(&d, "expected an expression");
    assert_eq!(field(&d, "found"), Some("`return`"));
    assert_syntax(&body_err("let x = return 1\nx"), "expected an expression");
}

#[test]
fn d27_positive_assert_takes_one_expression() {
    assert_contains(
        &body_ok("assert paginate(xs, 2) == [[1, 2], [3]]\n1"),
        "(assert (== (call (name paginate) (name xs) (int 2)) (list (list (int 1) (int 2)) (list (int 3)))))",
    );
}

#[test]
fn d27_negative_assert_has_no_message_argument() {
    let d = body_err("assert a, \"a must hold\"\n1");
    assert_code(&d, codes::SYNTAX_ERROR);
    assert_code(&body_err("assert\n1"), codes::SYNTAX_ERROR);
}

#[test]
fn d28_positive_blocks_occur_as_the_bodies_the_grammar_names() {
    let d = body_ok("if c {\n    1\n}\nwhile c {\n}\nmatch x {\n    A => {\n        2\n    }\n}");
    assert_contains(&d, "(if (name c) (block (tail (int 1))))");
    assert_contains(&d, "(while (name c) (block))");
    assert_contains(&d, "(arm (ctor A) (block (tail (int 2))))");
}

#[test]
fn d28_negative_a_bare_block_is_neither_a_statement_nor_an_expression() {
    assert_syntax(&body_err("{\n    1\n}"), "expected an expression");
    assert_syntax(&body_err("let x = { 1 }\nx"), "expected an expression");
}

#[test]
fn d51_positive_a_block_may_be_empty() {
    let d = module_ok("module m\n\nfn a() -> Unit {}\n\nfn b() -> Unit {\n}\n\nfn c() -> Unit {\n\n    // nothing\n\n}\n");
    assert_contains(&d, "(fn a (params) (ret (ty Unit)) (block)");
    assert_contains(&d, "(fn b (params) (ret (ty Unit)) (block)");
    assert_contains(&d, "(block (trailing-comments \"// nothing\"))");
}

#[test]
fn d51_negative_an_empty_block_holds_no_stray_separator() {
    // D51: `{}` or `{` NEWLINE `}`; a comma is not a statement.
    let d = module_err("module m\n\nfn a() -> Unit { , }\n");
    assert_syntax(&d, "expected an expression");
    assert_eq!(field(&d, "found"), Some("`,`"));
}

// ---------------------------------------------------------------------------
// Section 3.12: expressions
// ---------------------------------------------------------------------------

#[test]
fn d29_positive_the_spread_comes_first_and_fields_override() {
    assert_contains(
        &body_ok("Page { ..page, text: trim(page.text) }"),
        "(struct-lit Page (spread (name page)) (field text (call (name trim) (member (name page) text))))",
    );
}

#[test]
fn d29_negative_a_spread_after_a_field_or_twice() {
    let d = body_err("Page { text: t, ..p }");
    assert_syntax(
        &d,
        "expected a field initializer (a spread `..base` must be the first entry",
    );
    assert_code(&body_err("Page { ..p, ..q }"), codes::SYNTAX_ERROR);
}

#[test]
fn d30_positive_every_field_is_written_name_colon_value() {
    assert_contains(
        &body_ok("Pair { a: a, b: 2 }"),
        "(struct-lit Pair (field a (name a)) (field b (int 2)))",
    );
}

#[test]
fn d30_negative_no_punning_and_no_type_arguments() {
    assert_syntax(&body_err("Page { id, text }"), "expected `:`");
    assert_code(&body_err("Pair<Int, Text> { a: 1 }"), codes::SYNTAX_ERROR);
}

#[test]
fn d31_negative_a_using_argument_needs_an_expression() {
    // D31: `using_arg = "using" expr`.
    for src in ["f(using)", "f(using using db)"] {
        assert_syntax(&body_err(src), "expected an expression");
    }
}

#[test]
fn d32_positive_callees_are_names_and_members() {
    let d = body_ok("let a = f(x)\nlet b = page.take(2)\nlet c = check<Positive>(n)\nc");
    assert_contains(&d, "(let a (call (name f) (name x)))");
    assert_contains(&d, "(let b (call (member (name page) take) (int 2)))");
    assert_contains(
        &d,
        "(let c (call (name check) (generics (ty Positive)) (name n)))",
    );
}

#[test]
fn d32_negative_there_are_no_closures_or_function_values() {
    assert_syntax(
        &body_err("let f = fn(x: Int) -> Int { x }\n1"),
        "expected an expression",
    );
    let d = body_err("let g = |x| x\n1");
    assert_code(&d, codes::INVALID_CHARACTER);
    assert!(d.message.starts_with("use and / or"));
}

#[test]
fn d42_positive_a_generic_call_after_a_name_chain() {
    let d = body_ok("let a = check<Positive>(n)\nlet b = std.check<Positive>(n)\nlet c = x.convert<Int>()\nlet d = a < b\nf(a < B, C > (d))");
    assert_contains(
        &d,
        "(let a (call (name check) (generics (ty Positive)) (name n)))",
    );
    assert_contains(
        &d,
        "(let b (call (member (name std) check) (generics (ty Positive)) (name n)))",
    );
    assert_contains(
        &d,
        "(let c (call (member (name x) convert) (generics (ty Int))))",
    );
    assert_contains(&d, "(let d (< (name a) (name b)))");
    // Authoritative when it parses: one argument, the call `a<B, C>(d)`.
    assert_contains(
        &d,
        "(tail (call (name f) (call (name a) (generics (ty B) (ty C)) (name d))))",
    );
}

#[test]
fn d42_negative_generic_arguments_only_after_a_name_chain() {
    // `f(x)<T>(y)` is not a pure name chain, so `<` is a comparison and the
    // `>` then chains (grammar 3.12 rule 3).
    let d = body_err("f(x)<T>(y)");
    assert_syntax(
        &d,
        "expected `and`, `or`, or the end of the expression (comparisons do not chain",
    );
    // A failed speculation leaves `<` to the comparison: `check < Positive`
    // and then a stray `,`.
    assert_code(&body_err("check<Positive,>(n)"), codes::SYNTAX_ERROR);
}

#[test]
fn d43_positive_one_member_access_form_for_properties_and_methods() {
    let d = body_ok("let a = page.len\nlet b = page.is_empty()\nlet c = result.drop_last()\nlet d = page.len()\nd");
    assert_contains(&d, "(let a (member (name page) len))");
    assert_contains(&d, "(let b (call (member (name page) is_empty)))");
    assert_contains(&d, "(let c (call (member (name result) drop_last)))");
    // Whether `len` is a property is the typechecker's business (D43).
    assert_contains(&d, "(let d (call (member (name page) len)))");
}

#[test]
fn d43_negative_there_is_no_other_member_access_spelling() {
    assert_syntax(
        &body_err("page::len"),
        "expected end of line after the statement",
    );
    assert_syntax(
        &body_err("page->len"),
        "expected end of line after the statement",
    );
}

#[test]
fn d44_positive_constructors_parse_unqualified_and_qualified() {
    // D44 is resolution; the grammar gives every form one reading: a
    // constructor call or pattern by name chain, whatever it resolves to.
    let d = body_ok("let a = Missing(id)\nlet b = StoreError.Missing(id)\nlet c = reader.pages.PageId(\"p\")\nmatch r {\n    Err(Missing(_)) => 1\n    reader.pages.StoreError.Missing(x) => 2\n    None => 3\n}");
    assert_contains(&d, "(let a (call (name Missing) (name id)))");
    assert_contains(
        &d,
        "(let b (call (member (name StoreError) Missing) (name id)))",
    );
    assert_contains(
        &d,
        "(let c (call (member (member (name reader) pages) PageId) (text \"p\")))",
    );
    assert_contains(&d, "(arm (ctor Err (ctor Missing _)) (int 1))");
    assert_contains(
        &d,
        "(arm (ctor reader.pages.StoreError.Missing (bind x)) (int 2))",
    );
    assert_contains(&d, "(arm (ctor None) (int 3))");
}

#[test]
fn d44_negative_a_constructor_path_ends_in_an_upper_case_name() {
    // Grammar 3.13: `ctor_path = ( LOWER_IDENT "." )* UPPER_IDENT ( "." UPPER_IDENT )*`.
    let d = body_err("match x {\n    reader.pages.missing(y) => 1\n}");
    assert_syntax(&d, "expected a constructor name (PascalCase identifier)");
    // A qualified variant is not a struct literal type (3.12 rule 2, D20).
    let d = body_err("let s = StoreError.Missing { id: x }\ns");
    assert_syntax(
        &d,
        "a struct literal names a type; variants have positional payloads",
    );
}

#[test]
fn d45_positive_the_forall_predicate_extends_over_the_whole_expression() {
    let d = module_ok("module m\n\nfn f(result: List<List<Int>>, size: Int) -> Int\n    ensures forall page in result: page.len > 0 and page.len <= size\n{\n    1\n}\n");
    assert_contains(
        &d,
        "(ensures (forall page (name result) (and (> (member (name page) len) (int 0)) (<= (member (name page) len) (name size)))))",
    );
    let d = body_ok(
        "let a = c and (forall x in xs: x > 0)\nlet b = forall x in xs: forall y in x: y > 0\na",
    );
    assert_contains(
        &d,
        "(and (name c) (paren (forall x (name xs) (> (name x) (int 0)))))",
    );
    assert_contains(
        &d,
        "(forall x (name xs) (forall y (name x) (> (name y) (int 0))))",
    );
}

#[test]
fn d45_negative_forall_is_never_an_operand() {
    let d = body_err("a and forall x in c: p");
    assert_syntax(
        &d,
        "expected an expression (`forall` is the lowest-precedence form",
    );
}

#[test]
fn d46_positive_if_is_one_expression_form() {
    let d = body_ok("if c {\n}\nlet x = if c { 1 } else { 2 }\n1 + if c { 1 } else { 2 }");
    assert_contains(&d, "(expr (if (name c) (block)))");
    assert_contains(
        &d,
        "(let x (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
    assert_contains(&d, "(tail (+ (int 1) (if (name c)");
}

#[test]
fn d46_negative_if_branches_are_blocks() {
    let d = body_err("if c 1 else 2");
    assert_syntax(&d, "expected `{`");
    assert_eq!(field(&d, "found"), Some("integer literal `1`"));
}

#[test]
fn d47_positive_return_with_and_without_a_value() {
    let d = module_ok("module m\n\nfn a() -> Unit {\n    return\n}\n\nfn b() -> Int {\n    return 1\n}\n\nfn c() -> Unit { return }\n");
    assert_contains(&d, "(fn a (params) (ret (ty Unit)) (block (return))");
    assert_contains(&d, "(block (return (int 1)))");
    assert_contains(&d, "(fn c (params) (ret (ty Unit)) (block (return))");
}

#[test]
fn d47_negative_what_follows_return_on_its_line_is_an_expression() {
    assert_syntax(&body_err("return )"), "expected an expression");
    assert_syntax(
        &body_err("return 1 2"),
        "expected end of line after the statement",
    );
}

#[test]
fn d48_positive_negative_numbers_are_unary_minus_except_in_patterns() {
    let d = body_ok("let a = -5\nlet b = x -1\nlet c = 2 * -3\nlet d = -x.len\nlet e = - 5\nmatch a {\n    -1 => 1\n    _ => 2\n}");
    assert_contains(&d, "(let a (neg (int 5)))");
    assert_contains(&d, "(let b (- (name x) (int 1)))");
    assert_contains(&d, "(let c (* (int 2) (neg (int 3))))");
    assert_contains(&d, "(let d (neg (member (name x) len)))");
    assert_contains(&d, "(let e (neg (int 5)))");
    assert_contains(&d, "(arm (pat-int -1) (int 1))");
}

#[test]
fn d48_negative_only_minus_integer_is_a_literal_pattern() {
    for src in ["match x {\n    - y => 1\n}", "match x {\n    --1 => 1\n}"] {
        assert_syntax(&body_err(src), "expected an integer literal after `-`");
    }
}

#[test]
fn d49_positive_else_if_chains_end_in_one_else() {
    assert_contains(
        &body_ok("if a {\n    1\n} else if b {\n    2\n} else if c {\n    3\n} else {\n    4\n}"),
        "(if (name a) (block (tail (int 1))) (else (if (name b) (block (tail (int 2))) (else (if (name c) (block (tail (int 3))) (else (block (tail (int 4)))))))))",
    );
}

#[test]
fn d49_negative_there_is_no_elif_and_one_else_ends_the_chain() {
    let d = body_err("if a {} elif b {}");
    assert_syntax(&d, "expected end of line after the statement");
    assert_syntax(&body_err("if a {} else else {}"), "expected `{`");
    assert_syntax(
        &body_err("if a {} else {} else {}"),
        "expected end of line after the statement",
    );
}

#[test]
fn d50_positive_result_and_self_are_ordinary_identifiers() {
    // D50: contextual meaning only; they are `LOWER_IDENT` tokens that any
    // name position admits (semantics S9 restricts their use, not the parser).
    assert_eq!(kinds("result self"), [T::LowerIdent, T::LowerIdent, T::Eof]);
    let d = module_ok("module m\n\npub type Positive = Int where self > 0\n\nfn label(result: Result<Page, StoreError>) -> Text {\n    let self = result\n    ?result\n}\n\nfn f(x: Int) -> Int\n    ensures result > x\n{\n    x\n}\n");
    assert_contains(&d, "(where (> (name self) (int 0)))");
    assert_contains(
        &d,
        "(params (param result (ty Result (ty Page) (ty StoreError))))",
    );
    assert_contains(&d, "(let self (name result)) (tail (hole result))");
    assert_contains(&d, "(ensures (> (name result) (name x)))");
}

#[test]
fn d50_negative_result_and_self_are_not_keywords_or_type_names() {
    // D50: the two are `LOWER_IDENT`s, so a position that requires an
    // `UPPER_IDENT` rejects them like any other snake_case name, and `self`
    // is no receiver form (`fn f(self)` is a parameter without a type).
    for src in [
        "module m\n\ntype self(Int)\n",
        "module m\n\nfn f(x: self) -> Int {\n    1\n}\n",
        "module m\n\nfn f() -> result {\n    1\n}\n",
        "module m\n\nenum E {\n    result\n}\n",
        "module m\n\nfn f(self) -> Int {\n    1\n}\n",
    ] {
        assert_code(&module_err(src), codes::SYNTAX_ERROR);
    }
}

// ---------------------------------------------------------------------------
// Section 3.13: patterns
// ---------------------------------------------------------------------------

#[test]
fn d33_positive_constructors_literals_bindings_and_the_wildcard() {
    let d = body_ok("match x {\n    _ => 1\n    -1 => 2\n    0 => 3\n    \"t\" => 4\n    true => 5\n    y => 6\n    Err(Missing(_)) => 7\n    Pair(a, Unavailable,) => 8\n}");
    assert_contains(
        &d,
        "(arm _ (int 1)) (arm (pat-int -1) (int 2)) (arm (pat-int 0) (int 3)) (arm (pat-text \"t\") (int 4)) (arm (pat-bool true) (int 5)) (arm (bind y) (int 6)) (arm (ctor Err (ctor Missing _)) (int 7)) (arm (ctor Pair (bind a) (ctor Unavailable)) (int 8))",
    );
}

#[test]
fn d33_negative_no_struct_list_or_patterns_bindings_or_guards() {
    assert_syntax(&body_err("match x {\n    [] => 1\n}"), "expected a pattern");
    assert_syntax(
        &body_err("match x {\n    Page { .. } => 1\n}"),
        "expected `=>`",
    );
    assert_syntax(&body_err("match x {\n    y @ A => 1\n}"), "expected `=>`");
    assert_syntax(&body_err("match x {\n    _ if c => 1\n}"), "expected `=>`");
    // `|` is not even a token (grammar 1.6).
    assert_code(
        &body_err("match x {\n    A | B => 1\n}"),
        codes::INVALID_CHARACTER,
    );
}

// ---------------------------------------------------------------------------
// Section 4: `.work` and `.patch`
// ---------------------------------------------------------------------------

#[test]
fn d34_positive_a_work_file_holds_one_or_more_changes() {
    assert_contains(
        &work_ok("use a.b.{f}\n\nchange first {}\n\nchange second {\n    base git(\"main\")\n}\n"),
        "(change first (trivia blank)) (change second (base \"main\") (trivia blank))",
    );
}

#[test]
fn d34_negative_a_work_file_without_a_change() {
    for src in ["", "// only a comment\n", "use a.b.{f}\n"] {
        let d = work_err(src);
        assert_syntax(
            &d,
            "expected `change` (a .work file declares at least one change)",
        );
        assert_eq!(field(&d, "found"), Some("end of file"));
    }
}

#[test]
fn d36_negative_anything_else_after_require() {
    let src = "change c {\n    integrate {\n        require compile\n    }\n}\n";
    let d = work_err(src);
    assert_code(&d, codes::UNKNOWN_REQUIREMENT);
    assert!(d.message.starts_with("unknown requirement identifier `compile`; expected typecheck, no_holes, no_unresolved_assumptions, prove(selector), test(selector), or review(task)"));
    assert_eq!(field(&d, "found"), Some("identifier `compile`"));
    // A reserved word is `reserved_word` (D4); nothing at all is the generic
    // syntax error.
    let d = work_err("change c {\n    integrate {\n        require async\n    }\n}\n");
    assert_code(&d, codes::RESERVED_WORD);
    let d = work_err("change c {\n    integrate {\n        require\n    }\n}\n");
    assert_syntax(&d, "expected a requirement (typecheck, no_holes");
}

#[test]
fn d37_negative_a_facet_is_the_last_segment() {
    // D37: a facet-spelled final segment is the facet; the keyword facet
    // `effects` cannot be anything else, so it must end the selector (4.4).
    let d = work_err("change c {\n    task t {\n        preserve { f.effects.x }\n    }\n}\n");
    assert_syntax(
        &d,
        "expected the end of the selector (a facet must be its last segment",
    );
}

#[test]
fn d38_positive_bare_declaration_and_namespace_selectors() {
    let d = work_ok("change c {\n    task t {\n        write { Positive, paginate.definition }\n        add { tests.pagination.*, tests.* }\n    }\n    integrate {\n        require prove(paginate)\n        require test(tests.*)\n    }\n}\n");
    assert_contains(&d, "(write Positive paginate.definition)");
    assert_contains(&d, "(add tests.pagination.* tests.*)");
    assert_contains(&d, "(require (prove paginate)) (require (test tests.*))");
    let w = parse_work(
        "change c {\n    task t {\n        write { paginate }\n    }\n}\n",
        FileId(0),
    )
    .value
    .unwrap();
    let ChangeItemKind::Task(t) = &w.changes[0].items[0].kind else {
        panic!()
    };
    let TaskItemKind::Write(set) = &t.items[0].kind else {
        panic!()
    };
    assert_eq!(set.selectors[0].kind, SelectorKind::Declaration);
}

#[test]
fn d38_negative_add_takes_namespaces_and_star_ends_a_selector() {
    let d = work_err("change c {\n    task t {\n        add { tests.pagination }\n    }\n}\n");
    assert_syntax(&d, "add names a namespace; write tests.pagination.*");
    let d = work_err("change c {\n    task t {\n        write { paginate.*.body }\n    }\n}\n");
    assert_syntax(
        &d,
        "expected the end of the selector (`*` must be its last segment",
    );
}

#[test]
fn d39_positive_a_patch_replaces_a_body_against_an_expected_base() {
    let d = patch_ok("patch paginate.body\n    expect base(paginate.body)\n{\n    var pages: List<List<T>> = []\n    pages\n}\n");
    assert_contains(
        &d,
        "(patch paginate.body (expect paginate.body) (block (var pages (ty List (ty List (ty T))) (list)) (tail (name pages))))",
    );
}

#[test]
fn d39_negative_the_expect_base_clause_is_mandatory_and_the_only_form() {
    let d = patch_err("patch f.body\n{\n    1\n}\n");
    assert_syntax(&d, "expected `expect` on its own line");
    let d = patch_err("patch f.body\n    expect other(f.body)\n{\n    1\n}\n");
    assert_syntax(&d, "expected `base`");
    assert_code(
        &patch_err("patch f.body expect base(f.body) {\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn d40_positive_patch_forms_have_a_file_form_of_their_own() {
    assert_eq!(
        FileKind::from_path(Path::new("work/pagination.patch")),
        Some(FileKind::Patch)
    );
    let d = patch_ok("/// One.\npatch f.body\n    expect base(f.body)\n{\n    1\n}\n\npatch g.body\n    expect base(g.body)\n{}\n");
    assert_contains(&d, "(patch f.body (docs \"One.\") (expect f.body)");
    assert_contains(&d, "(patch g.body (expect g.body) (block) (trivia blank))");
}

#[test]
fn d68_positive_a_task_requests_edit_scopes() {
    assert_contains(
        &work_ok("change c {\n    task t {\n        write { f.body }\n        add { tests.* }\n        depend { f.contract }\n        preserve { f.signature }\n        goal \"g\"\n    }\n}\n"),
        "(task t (write f.body) (add tests.*) (depend f.contract) (preserve f.signature) (goal \"g\"))",
    );
}

#[test]
fn d68_negative_there_is_no_host_resource_request_item() {
    for item in ["request { network }", "filesystem {}", "network {}"] {
        let d = work_err(&format!(
            "change c {{\n    task t {{\n        {item}\n    }}\n}}\n"
        ));
        assert_syntax(
            &d,
            "expected `write`, `add`, `depend`, `preserve`, or `goal`",
        );
    }
}

#[test]
fn d69_positive_tests_are_selected_through_their_namespace() {
    assert_contains(
        &work_ok(
            "change c {\n    integrate {\n        require test(tests.pagination.*)\n    }\n}\n",
        ),
        "(require (test tests.pagination.*))",
    );
}

#[test]
fn d69_negative_a_selector_has_no_text_segment() {
    let d = work_err("change c {\n    integrate {\n        require test(tests.pagination.\"pagination preserves input\")\n    }\n}\n");
    assert_syntax(
        &d,
        "expected a selector segment, `*`, or a facet name after `.`",
    );
}

// ---------------------------------------------------------------------------
// Corpus decisions
// ---------------------------------------------------------------------------

fn example(path: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(path),
    )
    .unwrap_or_else(|e| panic!("examples/{path}: {e}"))
}

#[test]
fn d57_positive_prelude_names_parse_without_imports() {
    // D57: the corpus uses `flatten`, `check`, `ConstraintError`, and `gen`
    // with no `use` line for them; to the grammar they are ordinary names.
    let tests = example("reader/src/tests/pagination.cpl");
    let m = module_ast(&tests);
    assert!(m.uses.iter().all(|u| u.path.dotted() == "reader.pages"));
    let d = flat(&dump_module(&m));
    assert_contains(
        &d,
        "(call (member (name gen) list) (member (name gen) int))",
    );
    assert_contains(&d, "(call (name flatten)");
    let pages = flat(&dump_module(&module_ast(&example(
        "reader/src/reader/pages.cpl",
    ))));
    assert_contains(
        &pages,
        "(call (name check) (generics (ty Positive)) (name n))",
    );
    assert_contains(&pages, "(ty ConstraintError)");
}

#[test]
fn d58_positive_the_parse_size_glue_delegates_to_std_text() {
    // D58: a second `use std.text.{...}` line, marked `// glue:`, and a body
    // that calls `parse_int`.
    let src = example("reader/src/reader/pages.cpl");
    let m = module_ast(&src);
    let glue = m
        .uses
        .iter()
        .find(|u| u.items.iter().any(|i| i.name.text == "parse_int"))
        .expect("the glue import");
    assert_eq!(glue.path.dotted(), "std.text");
    assert!(glue
        .trivia
        .leading
        .iter()
        .any(|c| c.text.starts_with("// glue:")));
    let d = flat(&dump_module(&m));
    assert_contains(&d, "(fn parse_size");
    assert_contains(&d, "(call (name parse_int)");
}

/// Every `.cpl` file of the acceptance corpus, parsed.
fn example_modules() -> Vec<(String, Module)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut out = Vec::new();
    let mut pending = vec![dir.clone()];
    while let Some(d) = pending.pop() {
        for entry in std::fs::read_dir(&d).expect("read examples") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "cpl") {
                let relative = path.strip_prefix(&dir).unwrap().display().to_string();
                out.push((relative.clone(), module_ast(&example(&relative))));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn d57_negative_the_corpus_has_no_glue_import_for_a_prelude_name() {
    // D57 and corpus notes, which constrain the corpus and no input of the
    // grammar: the corpus imports none of `flatten`, `check`,
    // `ConstraintError`, and `gen`, and none of the rejected alternative's
    // glue paths (`std.list`, `std.check`, `std.gen`).
    let modules = example_modules();
    assert!(modules.len() >= 4, "{modules:?}");
    for (path, m) in &modules {
        for u in &m.uses {
            let dotted = u.path.dotted();
            assert!(
                !["std.list", "std.check", "std.gen"].contains(&dotted.as_str()),
                "{path}: use {dotted}"
            );
            for item in &u.items {
                assert!(
                    !["flatten", "check", "ConstraintError", "gen"]
                        .contains(&item.name.text.as_str()),
                    "{path}: use {dotted}.{}",
                    item.name.text
                );
            }
        }
    }
}

#[test]
fn d58_negative_the_design_use_line_is_untouched_and_parse_size_is_no_hole() {
    // D58 rejects `?parse_size` as a hole in `reader` and keeps the design's
    // own `use std.text.{trim}` line verbatim: the glue import is a line of
    // its own, and no `reader` function body holds a hole.
    let src = example("reader/src/reader/pages.cpl");
    assert!(src.contains("\nuse std.text.{trim}\n"));
    let m = module_ast(&src);
    let design = &m.uses[0];
    assert_eq!(design.path.dotted(), "std.text");
    assert_eq!(
        design
            .items
            .iter()
            .map(|i| i.name.text.as_str())
            .collect::<Vec<_>>(),
        ["trim"]
    );
    for (path, m) in example_modules() {
        if path.starts_with("reader") {
            let d = flat(&dump_module(&m));
            assert!(!d.contains("(hole "), "{path} has a hole:\n{d}");
        }
    }
}

// ---------------------------------------------------------------------------
// Diagnostics and limits
// ---------------------------------------------------------------------------

#[test]
fn d59_positive_a_clean_file_yields_its_tree_and_no_diagnostic() {
    for path in [
        "reader/src/reader/pages.cpl",
        "reader/src/reader/images.cpl",
        "reader/src/tests/pagination.cpl",
        "holes/src/holes/pages.cpl",
    ] {
        let src = example(path);
        let r = parse_module(&src, FileId(0));
        assert!(r.diagnostics.is_empty() && !r.has_errors(), "{path}");
        assert!(r.into_result().is_ok(), "{path}");
    }
    let r = parse_work(&example("reader/work/pagination.work"), FileId(0));
    assert!(r.value.is_some() && r.diagnostics.is_empty());
    let r = parse_patch(&example("reader/work/pagination.patch"), FileId(0));
    assert!(r.value.is_some() && r.diagnostics.is_empty());
}

#[test]
fn d70_positive_parsing_is_total_over_any_input() {
    // D70: resynchronisation keeps the lexer and parser total; whatever the
    // input, every parser returns either a tree and no error or no tree and
    // exactly one error, without panicking.
    let src = example("reader/src/reader/pages.cpl");
    let mut ends: Vec<usize> = src.char_indices().map(|(i, _)| i).collect();
    ends.push(src.len());
    for end in ends {
        let text = &src[..end];
        for (has_value, diagnostics) in [
            {
                let r = parse_module(text, FileId(0));
                (r.value.is_some(), r.diagnostics)
            },
            {
                let r = parse_work(text, FileId(0));
                (r.value.is_some(), r.diagnostics)
            },
            {
                let r = parse_patch(text, FileId(0));
                (r.value.is_some(), r.diagnostics)
            },
        ] {
            let errors = diagnostics.iter().filter(|d| d.is_error()).count();
            assert!(
                (has_value && errors == 0) || (!has_value && errors == 1 && diagnostics.len() == 1),
                "prefix of {end} bytes: {diagnostics:#?}"
            );
        }
    }
    assert!(parse_module(&src, FileId(0)).value.is_some());
}

#[test]
fn d70_negative_the_earliest_error_is_the_one_reported() {
    // A syntax error before a lexical one: the syntax error.
    let src = "module m\n\nfn f() -> Int { let x = ) }\n\nfn g() -> Int { 1; }\n";
    let d = module_err(src);
    assert_code(&d, codes::SYNTAX_ERROR);
    assert_eq!(start(&d), src.find("= )").unwrap() + 2);
    // A lexical error before a syntax one: the lexical error.
    let src = "module m\n\nfn g() -> Int { 1; }\n\nfn f() -> Int { let x = ) }\n";
    assert_code(&module_err(src), codes::INVALID_CHARACTER);
    // Both at one offset: the lexical one (`1x` is a literal adjacent to an
    // identifier, and an integer where a function name must be).
    let src = "module m\n\nfn 1x() -> Int { 1 }\n";
    let d = module_err(src);
    assert_code(&d, codes::LITERAL_ADJACENT_IDENTIFIER);
    assert_eq!(start(&d), src.find("1x").unwrap());
}

#[test]
fn d72_positive_generic_messages_name_what_was_expected_and_found() {
    // D72: `expected <description>, found <token>` with the `expected` and
    // `found` fields; a quoted message is the fixed beginning of the text.
    let d = body_err("let x = 1 let y = 2\nx");
    assert_eq!(
        d.message,
        "expected end of line after the statement, found `let`"
    );
    assert_eq!(
        field(&d, "expected"),
        Some("end of line after the statement")
    );
    assert_eq!(field(&d, "found"), Some("`let`"));
    let d = module_err("module m\n\nfn f() Int { 1 }\n");
    assert!(d.message.starts_with("expected `->`"), "{d:#?}");
    assert!(d.message.ends_with(", found identifier `Int`"), "{d:#?}");
    let d = module_err("module m\n\nfn f() -> Int { 1 }\n\nuse a.b\n");
    assert_code(&d, codes::USE_AFTER_DECLARATION);
    assert!(d
        .message
        .starts_with("use declarations must precede all other declarations"));
}

#[test]
fn d72_negative_other_messages_carry_only_their_own_fields() {
    // D72: a message not of the generic shape carries the fields semantics
    // 9.4 lists for its code, never `expected` and `found` together.
    let d = body_err("1;");
    assert_code(&d, codes::INVALID_CHARACTER);
    assert!(d.message.starts_with("semicolons are not part of CPL"));
    assert_eq!(field(&d, "found"), Some(";"));
    assert_eq!(field(&d, "expected"), None);
    let d = module_err("module m\n\nfn f(using db: Db) -> Int\n    effects { db.get }\n    effects {}\n{\n    1\n}\n");
    assert_code(&d, codes::DUPLICATE_CLAUSE);
    assert_eq!(field(&d, "name"), Some("effects"));
    assert_eq!(field(&d, "expected"), None);
    assert_eq!(field(&d, "found"), None);
}

/// `fn f() -> Int` whose body (one nesting level) holds `parens` nested
/// parenthesis pairs around `1`.
fn nested_parens(parens: usize) -> String {
    wrap(&format!("{}1{}", "(".repeat(parens), ")".repeat(parens)))
}

#[test]
fn d73_positive_64_levels_and_1000_operators_are_accepted() {
    // D73: the body's braces are the first level, so 63 parentheses inside
    // make 64 levels.
    assert_eq!(MAX_DEPTH, 64);
    assert!(parse_module(&nested_parens(63), FileId(0)).value.is_some());
    assert_eq!(MAX_EXPR_OPS, 1000);
    let src = wrap(&format!("1{}", " + 1".repeat(1000)));
    let r = parse_module(&src, FileId(0));
    assert!(r.value.is_some(), "{:#?}", r.diagnostics);
    // Operators are counted along a path of nested expressions: the next
    // statement has a budget of its own.
    let src = wrap(&format!(
        "let a = 1{}\nlet b = 1{}\na",
        " + 1".repeat(1000),
        " * 1".repeat(1000)
    ));
    assert!(parse_module(&src, FileId(0)).value.is_some());
}

#[test]
fn d73_negative_beyond_the_limits_is_a_syntax_error_at_the_excess_token() {
    let src = nested_parens(64);
    let d = module_err(&src);
    assert_syntax(&d, "nesting is too deep (more than 64 levels)");
    // At the first token after the bracket that opens the excess level.
    assert_eq!(start(&d), src.find('1').unwrap());
    // An `if` or `match` that would open the excess level is reported at
    // its keyword: 63 parentheses inside the body make 64 levels, and the
    // `if` would be the 65th.
    for expr in ["if c { 1 } else { 2 }", "match x {\n_ => 1\n}"] {
        let src = wrap(&format!("{}{expr}{}", "(".repeat(63), ")".repeat(63)));
        let d = module_err(&src);
        assert_syntax(&d, "nesting is too deep");
        assert_eq!(start(&d), src.find(&expr[..2]).unwrap(), "{expr}");
    }
    let src = wrap(&format!("1{}", " + 1".repeat(1001)));
    let d = module_err(&src);
    assert_syntax(
        &d,
        "an expression has too many operators (more than 1000); split it into bindings",
    );
    // At the operator that exceeds the budget.
    let excess = src.match_indices('+').nth(1000).unwrap().0;
    assert_eq!(start(&d), excess);
}

// ---------------------------------------------------------------------------
// Decisions whose other side first lived only in the adversarial and
// disambiguation files (those tests stay where they are; the cases here make
// this file and `lexer.rs` hold both sides of every decision, as the row of
// toolchain 12 asks)
// ---------------------------------------------------------------------------

#[test]
fn d4_negative_a_reserved_future_word_is_reserved_word() {
    // D4: the eight words lex as `ReservedWord` and are a syntax error
    // wherever they are written outside comments and text.
    assert_eq!(
        kinds("as async await break continue ghost lemma loop"),
        [T::ReservedWord; 8]
            .into_iter()
            .chain([T::Eof])
            .collect::<Vec<_>>()
    );
    let d = body_err("let loop = 1\n1");
    assert_code(&d, codes::RESERVED_WORD);
    assert!(
        d.message
            .starts_with("`loop` is reserved for a future edition"),
        "{d:#?}"
    );
    for src in ["let x = as\nx", "ghost"] {
        assert_code(&body_err(src), codes::RESERVED_WORD);
    }
    assert_code(
        &module_err("module m\n\nfn async() -> Unit {}\n"),
        codes::RESERVED_WORD,
    );
    assert_code(&work_err("change break {}\n"), codes::RESERVED_WORD);
}

#[test]
fn d11_positive_brackets_are_the_continuation_mechanism() {
    // D11: inside `(`, `[`, and a struct literal's `{` newlines are
    // insignificant, and a block inside parentheses is significant again.
    assert_contains(
        &body_ok("let total = (a\n    + b)\ntotal"),
        "(let total (paren (+ (name a) (name b))))",
    );
    assert_contains(
        &body_ok("let pages = paginate(\n    items,\n    size,\n)\npages"),
        "(let pages (call (name paginate) (name items) (name size)))",
    );
    assert_contains(
        &body_ok("let labels = [\n    \"a\",\n    \"b\",\n]\nlabels"),
        "(let labels (list (text \"a\") (text \"b\")))",
    );
    assert_contains(
        &body_ok("let page = Page {\n    ..base,\n    text: trim(base.text),\n}\npage"),
        "(let page (struct-lit Page (spread (name base)) (field text (call (name trim) (member (name base) text)))))",
    );
    assert_contains(
        &body_ok("foo(if c {\n    x = 1\n    2\n} else { 3 })"),
        "(block (assign x (int 1)) (tail (int 2)))",
    );
}

#[test]
fn d11_negative_an_operator_or_a_dot_never_continues_a_line() {
    // D11: a binary operator at the end of a line, or a leading `.` on the
    // next, does not continue the expression.
    let d = body_err("let total = a +\n    b");
    assert_syntax(&d, "expected expression, found end of line");
    assert_code(&body_err("let text = page\n    .text"), codes::SYNTAX_ERROR);
    assert_syntax(
        &body_err("let x =\n    compute()"),
        "expected expression, found end of line",
    );
}

#[test]
fn d12_positive_else_after_a_line_break_inside_brackets() {
    // D12 follows from the mode stack: inside an insignificant-newline
    // context the NEWLINE before `else` is discarded.
    assert_contains(
        &body_ok("let x = (if c { 1 }\nelse if d { 2 }\nelse { 3 })\nx"),
        "(paren (if (name c) (block (tail (int 1))) (else (if (name d) (block (tail (int 2))) (else (block (tail (int 3))))))))",
    );
    assert_contains(
        &body_ok("if c { 1 } else { 2 }"),
        "(tail (if (name c) (block (tail (int 1))) (else (block (tail (int 2))))))",
    );
}

#[test]
fn d12_negative_a_line_beginning_with_else_is_misplaced_else() {
    for src in [
        "if c { 1 }\nelse { 2 }",
        "if c { 1 } // then\nelse { 2 }",
        "if c {\n    1\n}\nelse if d {\n    2\n}",
        "let x = if c { 1 }\nelse { 2 }\nx",
    ] {
        let d = body_err(src);
        assert_code(&d, codes::MISPLACED_ELSE);
        assert!(
            d.message.starts_with(
                "`else` must follow the closing `}` of the preceding block on the same line"
            ),
            "{src}: {d:#?}"
        );
    }
}

#[test]
fn d13_positive_use_declarations_come_between_the_header_and_the_declarations() {
    let d = module_ok("module m\nuse a.b\nuse c.d.{e}\nfn f() -> Int { 1 }\n");
    assert_contains(&d, "(use a b) (use c.d (items e)) (fn f");
    // In a `.work` file they precede the changes.
    assert_contains(
        &work_ok("use a.b.{c}\n\nchange x {}\n"),
        "(work (use a.b (items c)) (change x",
    );
}

#[test]
fn d13_negative_use_after_a_declaration_is_use_after_declaration() {
    let d = module_err("module m\n\nstruct S {}\n\nuse a.b\n");
    assert_code(&d, codes::USE_AFTER_DECLARATION);
    assert!(
        d.message
            .starts_with("use declarations must precede all other declarations"),
        "{d:#?}"
    );
    assert_code(
        &module_err("module m\n\ntest \"t\" {}\nuse a.b.{c}\n"),
        codes::USE_AFTER_DECLARATION,
    );
    let d = work_err("change x {}\nuse a.b\n");
    assert_code(&d, codes::USE_AFTER_DECLARATION);
    assert!(
        d.message
            .starts_with("use declarations must precede all change declarations"),
        "{d:#?}"
    );
}

#[test]
fn d15_positive_the_single_item_form_imports_its_last_segment() {
    let d = module_ok("module m\nuse a.b.c\nuse a.B\nuse a.b.c.d\nuse a.b\n");
    assert_contains(&d, "(use a.b c)");
    assert_contains(&d, "(use a B)");
    assert_contains(&d, "(use a.b.c d)");
    assert_contains(&d, "(use a b)");
}

#[test]
fn d16_negative_no_renaming_no_glob_and_no_empty_list() {
    for src in [
        "module m\n\nuse a.b.{}\n",
        "module m\n\nuse a.b.{,}\n",
        "module m\n\nuse a.*\n",
        "module m\n\nuse a.{*}\n",
    ] {
        assert_code(&module_err(src), codes::SYNTAX_ERROR);
    }
    assert_code(
        &module_err("module m\n\nuse a.b as c\n"),
        codes::RESERVED_WORD,
    );
}

#[test]
fn d18_negative_a_wrapper_has_exactly_one_payload() {
    for src in [
        "module m\n\ntype Pair(Int, Int)\n",
        "module m\n\ntype Empty()\n",
        "module m\n\ntype A(Int,)\n",
    ] {
        assert_code(&module_err(src), codes::SYNTAX_ERROR);
    }
}

#[test]
fn d23_negative_effects_name_a_parameter_and_an_operation() {
    let wrap = |set: &str| {
        format!("module m\n\nfn f(using db: Db) -> Int\n    effects {set}\n{{\n    1\n}}\n")
    };
    for set in [
        "{ get }",
        "{ db }",
        "{ db.* }",
        "{ PageRead.get }",
        "{ db.get.x }",
        "{ db.get() }",
    ] {
        assert_code(&module_err(&wrap(set)), codes::SYNTAX_ERROR);
    }
}

#[test]
fn d31_positive_a_using_argument_takes_any_expression() {
    let d = module_ok(
        "module m\n\nfn f() -> Int {\n    let a = load_page(id, using db)?\n    let b = g(using ctx.reader)\n    g(x, using db, using ctx.log)\n}\n",
    );
    assert_contains(
        &d,
        "(let a (propagate (call (name load_page) (name id) (using (name db)))))",
    );
    assert_contains(
        &d,
        "(let b (call (name g) (using (member (name ctx) reader))))",
    );
    assert_contains(
        &d,
        "(tail (call (name g) (name x) (using (name db)) (using (member (name ctx) log))))",
    );
}

#[test]
fn d35_positive_git_refs_pass_their_text_through() {
    let d = work_ok(
        "change c {\n    base git(\"main\")\n    target git(\"refs/heads/feature/x\")\n}\nchange d {\n    base git(\"3f2e9a1\")\n    target git(\n        \"v1\"\n    )\n}\n",
    );
    assert_contains(
        &d,
        "(change c (base \"main\") (target \"refs/heads/feature/x\"))",
    );
    assert_contains(&d, "(change d (base \"3f2e9a1\") (target \"v1\")");
}

#[test]
fn d35_negative_git_paren_text_is_the_only_ref_form() {
    for item in [
        "base main",
        "base \"main\"",
        "base git(main)",
        "base git()",
        "base git(\"a\", \"b\")",
        "base hg(\"main\")",
    ] {
        assert_code(
            &work_err(&format!("change c {{\n    {item}\n}}\n")),
            codes::SYNTAX_ERROR,
        );
    }
}

#[test]
fn d36_positive_the_six_requirements() {
    let d = work_ok(
        "change c {\n    integrate {\n        require typecheck\n        require no_holes\n        require no_unresolved_assumptions\n        require prove(paginate)\n        require test(tests.pagination.*)\n        require review(audit)\n    }\n}\n",
    );
    assert_contains(
        &d,
        "(integrate (require typecheck) (require no_holes) (require no_unresolved_assumptions) (require (prove paginate)) (require (test tests.pagination.*)) (require (review audit)))",
    );
}

/// The one selector of `write { <text> }`.
fn selector(text: &str) -> Selector {
    let src = format!("change c {{\n    task t {{\n        write {{ {text} }}\n    }}\n}}\n");
    let r = parse_work(&src, FileId(0));
    let w = match r.value {
        Some(w) if r.diagnostics.is_empty() => w,
        _ => panic!(
            "expected a clean .work parse of:\n{src}\n{}",
            render(&src, &r.diagnostics)
        ),
    };
    let ChangeItemKind::Task(task) = &w.changes[0].items[0].kind else {
        panic!("a task")
    };
    let TaskItemKind::Write(set) = &task.items[0].kind else {
        panic!("a write set")
    };
    assert_eq!(set.selectors.len(), 1, "{text}");
    set.selectors[0].clone()
}

#[test]
fn d37_positive_a_final_facet_spelling_is_the_facet() {
    // D37: `m.body.definition` selects the declaration `m.body`, and
    // `m.contract` the contract facet of `m`.
    let s = selector("m.body.definition");
    assert_eq!(s.path.dotted(), "m.body");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Definition));
    let s = selector("m.contract");
    assert_eq!(s.path.dotted(), "m");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Contract));
    let s = selector("a.body.body");
    assert_eq!(s.path.dotted(), "a.body");
    assert!(matches!(&s.kind, SelectorKind::Facet(f) if f.kind == FacetKind::Body));
    // `Name` is a segment, not the facet `name`.
    let s = selector("m.Name");
    assert_eq!(s.path.dotted(), "m.Name");
    assert_eq!(s.kind, SelectorKind::Declaration);
}

#[test]
fn d40_negative_patch_forms_appear_only_in_patch_files() {
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
    // And a `.patch` file holds nothing else.
    assert_code(&patch_err("module m\n"), codes::SYNTAX_ERROR);
}

#[test]
fn d41_positive_brackets_reenable_struct_literals_in_header_expressions() {
    assert_contains(
        &body_ok("if page == (Page { id: x }) {\n    1\n} else {\n    2\n}"),
        "(if (== (name page) (paren (struct-lit Page (field id (name x))))) (block (tail (int 1)))",
    );
    assert_contains(
        &module_ok("module m\n\nproperty \"p\"\n    for x in gen.of((Gen { seed: 1 }))\n{\n}\n"),
        "(for x (call (member (name gen) of) (paren (struct-lit Gen (field seed (int 1))))))",
    );
    // The body and every later statement see struct literals again.
    assert_contains(
        &body_ok("if c {\n    let _ = 1\n}\nPage { text: \"x\" }"),
        "(tail (struct-lit Page (field text (text \"x\"))))",
    );
}

#[test]
fn d41_negative_a_struct_literal_at_the_top_of_a_header_expression() {
    // `{` after `Page` opens the body, so `..` cannot begin a statement.
    assert_code(
        &body_err("if page == Page { ..p } {\n    1\n}"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &module_err("module m\n\nproperty \"p\"\n    for x in Gen { seed: 1 }\n{\n}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(
        &body_err("while x == P { a: 1 } {\n    1\n}"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn d52_positive_the_wildcard_and_underscore_names() {
    assert_eq!(
        kinds("_ _x __"),
        [T::Underscore, T::LowerIdent, T::LowerIdent, T::Eof]
    );
    assert_contains(&body_ok("let _ = f()\n1"), "(let _ (call (name f)))");
    assert_contains(&body_ok("let _x = 1\n_x"), "(let _x (int 1))");
    assert_contains(
        &body_ok("match x {\n    _ => 1\n}"),
        "(match (name x) (arm _ (int 1)))",
    );
}

#[test]
fn d52_negative_the_wildcard_binds_nothing_else() {
    for src in ["var _ = 1\n1", "_ = 1\n1", "let y = _\ny"] {
        assert_code(&body_err(src), codes::SYNTAX_ERROR);
    }
    assert_code(
        &module_err("module m\n\nfn f(_: Int) -> Int { 1 }\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn d53_positive_member_names_are_identifiers_of_either_class() {
    assert_contains(&body_ok("x.Type"), "(member (name x) Type)");
    assert_contains(&body_ok("x.result"), "(member (name x) result)");
    // The facet `effects` in a selector is the one keyword after `.`.
    assert_contains(
        &work_ok("change c {\n    task t {\n        preserve { f.effects, f.body }\n    }\n}\n"),
        "(preserve f.effects f.body)",
    );
}

#[test]
fn d53_negative_a_keyword_is_never_a_member_name() {
    let d = body_err("x.type");
    assert_syntax(&d, "expected a member name");
    for src in ["x.if", "x.fn", "x.test"] {
        assert_code(&body_err(src), codes::SYNTAX_ERROR);
    }
}

#[test]
fn d54_positive_dotdot_is_the_spread() {
    assert_contains(
        &body_ok("Page { ..p, }"),
        "(struct-lit Page (spread (name p)))",
    );
    assert_contains(
        &body_ok("Page { ..p, text: t }"),
        "(struct-lit Page (spread (name p)) (field text (name t)))",
    );
}

#[test]
fn d54_negative_there_are_no_ranges() {
    assert_code(&body_err("a..b"), codes::SYNTAX_ERROR);
    assert_code(
        &body_err("match x {\n    1..5 => 1\n}"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&body_err("let r = [0..3]\nr"), codes::SYNTAX_ERROR);
}

#[test]
fn d55_positive_match_arms_are_newline_separated() {
    assert_contains(
        &body_ok("match x\n{\n    A => 1\n    B => 2\n}"),
        "(match (name x) (arm (ctor A) (int 1)) (arm (ctor B) (int 2)))",
    );
}

#[test]
fn d55_negative_a_comma_after_an_arm() {
    let d = body_err("match x {\n    A => 1,\n    B => 2\n}");
    assert_syntax(&d, "match arms are separated by newlines");
    assert_code(&body_err("match x { A => 1 B => 2 }"), codes::SYNTAX_ERROR);
}

#[test]
fn d56_positive_tests_is_an_ordinary_identifier() {
    assert_contains(
        &module_ok("module tests.pagination\n"),
        "(module tests.pagination",
    );
    assert_contains(&body_ok("tests.run()"), "(call (member (name tests) run))");
    assert_eq!(kinds("test tests"), [T::KwTest, T::LowerIdent, T::Eof]);
}

#[test]
fn d56_negative_test_is_reserved() {
    assert_code(&module_err("module test.x\n"), codes::SYNTAX_ERROR);
    assert_code(
        &module_err("module m\n\nfn test() -> Unit {}\n"),
        codes::SYNTAX_ERROR,
    );
    assert_code(&body_err("let test = 1\ntest"), codes::SYNTAX_ERROR);
}

#[test]
fn d59_negative_a_file_with_several_errors_reports_the_first_only() {
    // Three independent errors in three functions: exactly one
    // diagnostic, at the first offending token (`module_err` asserts the
    // count, D59 and D70).
    let src = "module m\n\nfn a() -> Int {\n    let x = )\n    1\n}\n\nfn b() -> Int {\n    let y = )\n    1\n}\n\nfn c() -> Int {\n    let z = )\n    1\n}\n";
    let d = module_err(src);
    assert_eq!(start(&d), src.find("= )").unwrap() + 2);
}

#[test]
fn d61_negative_annotation_arguments_and_coordination_names() {
    // Annotation arguments are literals, never expressions.
    for args in ["(x)", "(1 + 2)", "(-1)", "([1])"] {
        assert_code(
            &module_err(&format!("module m\n\n@id{args}\nfn f() -> Int {{ 1 }}\n")),
            codes::SYNTAX_ERROR,
        );
    }
    // Change, task, and `after` names are snake_case.
    for src in [
        "change Big {}\n",
        "change c {\n    task T {}\n}\n",
        "change c {\n    task t after A {}\n}\n",
    ] {
        assert_code(&work_err(src), codes::SYNTAX_ERROR);
    }
}

#[test]
fn d63_positive_the_body_brace_may_start_the_next_line() {
    let d = module_ok("module m\n\nfn f() -> Int\n{\n    1\n}\n\nstruct S\n{\n    x: Int\n}\n\ntest \"t\"\n{\n    assert true\n}\n");
    assert_contains(&d, "(fn f (params) (ret (ty Int)) (block (tail (int 1)))");
    assert_contains(&d, "(struct S (field x (ty Int))");
    assert_contains(&d, "(test \"t\" (block (assert (bool true)))");
    // The design's paginate_ints shape: a broken parameter list, then `{`.
    assert_contains(
        &module_ok("module m\n\nfn f(\n    a: Int,\n) -> Int\n{\n    a\n}\n"),
        "(fn f (params (param a (ty Int))) (ret (ty Int)) (block (tail (name a)))",
    );
}

#[test]
fn d63_negative_with_clauses_the_brace_starts_its_own_line() {
    assert_code(
        &module_err("module m\n\nfn f(x: Int) -> Int\n    requires x > 0 {\n    1\n}\n"),
        codes::SYNTAX_ERROR,
    );
    // Grammar 2.4 rule 1, the generic message with its hint (D72).
    let d = module_err("module m\n\nfn f(x: Int) -> Int requires x > 0\n{\n    1\n}\n");
    assert_syntax(&d, "expected end of line");
    assert!(
        d.message.contains("a clause must start its own line"),
        "{d:#?}"
    );
    assert_eq!(field(&d, "found"), Some("`requires`"));
    assert_code(
        &module_err("module m\n\nproperty \"p\" for x in xs\n{\n    assert true\n}\n"),
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn d64_negative_a_type_without_a_definition_form() {
    let d = module_err("module m\n\ntype A = Int\n");
    assert_syntax(
        &d,
        "a type declaration is a wrapper, type A(T), or a refinement, type A = T where ...",
    );
    for src in [
        "module m\n\npub type A = List<Int>\n",
        "module m\n\ntype A = Int where\n",
        "module m\n\ntype A = where self > 0\n",
    ] {
        assert_code(&module_err(src), codes::SYNTAX_ERROR);
    }
}

#[test]
fn d66_positive_clauses_parse_in_any_order_and_keep_it() {
    // The tree keeps the source order; only the formatter reorders (T13).
    let d = module_ok(
        "module m\n\nfn f(a: Int, b: Int, using db: Db) -> Int\n    ensures result > 0\n    requires b != 0\n    decreases a\n    effects { db.get }\n    requires a % b == 0\n{\n    a / b\n}\n",
    );
    assert_contains(
        &d,
        "(ensures (> (name result) (int 0))) (requires (!= (name b) (int 0))) (decreases (name a)) (effects db.get) (requires (== (% (name a) (name b)) (int 0)))",
    );
    assert_contains(
        &body_ok("while x > 0\n    decreases x\n    invariant x >= 0\n{\n    x = x - 1\n}"),
        "(while (> (name x) (int 0)) (decreases (name x)) (invariant (>= (name x) (int 0)))",
    );
}

#[test]
fn d71_positive_a_keyword_is_an_annotation_name() {
    let d =
        module_ok("module m\n\n@property\n@fn(\"x\", 1)\n@test()\npub fn f() -> Int {\n    1\n}\n");
    assert_contains(
        &d,
        "(annotation property) (annotation fn (text \"x\") (int 1)) (annotation test ())",
    );
}

#[test]
fn d71_negative_reserved_words_and_upper_case_names_after_at() {
    assert_code(
        &module_err("module m\n\n@async\nfn f() -> Int {\n    1\n}\n"),
        codes::RESERVED_WORD,
    );
    let d = module_err("module m\n\n@Upper\nfn f() -> Int {\n    1\n}\n");
    assert_code(&d, codes::SYNTAX_ERROR);
    assert_eq!(
        field(&d, "expected"),
        Some("an annotation name (snake_case identifier or keyword)")
    );
    assert_eq!(field(&d, "found"), Some("identifier `Upper`"));
}

// ---------------------------------------------------------------------------
// The row of toolchain 12: every decision has both cases, named by number
// ---------------------------------------------------------------------------

/// The names of the `#[test]` functions of one test source.
fn test_names(src: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut lines = src.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if line == "#[test]" {
            if let Some(next) = lines.next() {
                if let Some(rest) = next.strip_prefix("fn ") {
                    let name: String = rest
                        .chars()
                        .take_while(|c| *c == '_' || c.is_ascii_alphanumeric())
                        .collect();
                    names.push(name);
                }
            }
        }
    }
    names
}

#[test]
fn every_decision_has_a_positive_and_a_negative_case_named_by_number() {
    // The row names `lexer.rs` and `parser.rs`; a case counts when its test
    // in one of them is named `d<n>_positive_...` or `d<n>_negative_...`.
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut names = Vec::new();
    for file in ["lexer.rs", "parser.rs"] {
        let src = std::fs::read_to_string(tests_dir.join(file))
            .unwrap_or_else(|e| panic!("tests/{file}: {e}"));
        names.extend(test_names(&src));
    }
    let mut missing = Vec::new();
    for name in &names {
        let mut parts = name.splitn(3, '_');
        if let (Some(d), Some(side)) = (parts.next(), parts.next()) {
            if let Some(n) = d.strip_prefix('d').and_then(|n| n.parse::<u32>().ok()) {
                assert!((1..=73).contains(&n), "`{name}`: no decision D{n}");
                assert!(matches!(side, "positive" | "negative"), "`{name}`");
            }
        }
    }
    for n in 1..=73u32 {
        for side in ["positive", "negative"] {
            let prefix = format!("d{n}_{side}_");
            if !names.iter().any(|t| t.starts_with(&prefix)) {
                missing.push(format!("D{n} has no {side} case"));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}
