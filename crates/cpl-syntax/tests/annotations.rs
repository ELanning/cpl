//! Annotations (grammar 3.3): the name is a `LOWER_IDENT` or any reserved
//! keyword (D71), the arguments are literals (D61), and `@name()` keeps its
//! parentheses through the tree (`Annotation::parens`), the dump, and the
//! formatter, which never removes a token (`spec/toolchain.md` T14).

use cpl_diag::{Diagnostic, FileId};
use cpl_syntax::ast::*;
use cpl_syntax::lexer::RESERVED_FUTURE_WORDS;
use cpl_syntax::{codes, dump_module, format_source, parse_module, FileKind, TokenKind};

/// The 33 reserved keywords of grammar 1.4.
const KEYWORDS: [&str; 33] = [
    "and",
    "assert",
    "capability",
    "decreases",
    "effects",
    "else",
    "ensures",
    "enum",
    "false",
    "fn",
    "for",
    "forall",
    "if",
    "in",
    "invariant",
    "let",
    "match",
    "module",
    "not",
    "or",
    "property",
    "pub",
    "requires",
    "return",
    "struct",
    "test",
    "true",
    "type",
    "use",
    "using",
    "var",
    "where",
    "while",
];

#[track_caller]
fn module(src: &str) -> Module {
    let r = parse_module(src, FileId(0));
    match r.value {
        Some(m) if r.diagnostics.is_empty() => m,
        _ => panic!("expected a clean parse of:\n{src}\n{:#?}", r.diagnostics),
    }
}

#[track_caller]
fn module_error(src: &str) -> Diagnostic {
    let r = parse_module(src, FileId(0));
    assert!(r.value.is_none(), "expected an error for:\n{src}");
    assert_eq!(r.diagnostics.len(), 1, "D59: one diagnostic");
    r.diagnostics.into_iter().next().expect("one diagnostic")
}

#[track_caller]
fn fmt(src: &str) -> String {
    format_source(src, FileKind::Cpl, FileId(0)).expect("formats")
}

fn flat(dump: &str) -> String {
    dump.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn d71_every_reserved_keyword_is_an_annotation_name() {
    for kw in KEYWORDS {
        assert!(
            TokenKind::keyword(kw).is_some_and(TokenKind::is_keyword),
            "`{kw}` is a keyword"
        );
        let src = format!("module m\n\n@{kw}\n@{kw}(\"x\", 1)\nfn f() -> Int {{\n    1\n}}\n");
        let m = module(&src);
        let d = &m.declarations[0];
        assert_eq!(d.annotations.len(), 2, "{kw}");
        assert_eq!(d.annotations[0].name.text, kw);
        assert!(!d.annotations[0].parens);
        assert_eq!(d.annotations[1].name.text, kw);
        assert_eq!(d.annotations[1].args.len(), 2);
        assert!(d.has_annotation(kw));
        let dump = flat(&dump_module(&m));
        assert!(
            dump.contains(&format!(
                "(annotation {kw}) (annotation {kw} (text \"x\") (int 1))"
            )),
            "{dump}"
        );
        // The source is canonical and formats to itself.
        assert_eq!(fmt(&src), src, "{kw}");
    }
}

#[test]
fn d71_the_standard_library_writes_property_annotations() {
    let prelude = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std/prelude.cpl"),
    )
    .expect("std/prelude.cpl");
    let m = module(&prelude);
    let properties = m
        .declarations
        .iter()
        .filter(|d| d.has_annotation("property"))
        .count();
    assert!(properties >= 3, "the `len` stubs are @property");
}

#[test]
fn d71_reserved_words_and_other_tokens_are_not_annotation_names() {
    // D4: a reserved-future word is `reserved_word`, after `@` too.
    for word in RESERVED_FUTURE_WORDS {
        let d = module_error(&format!(
            "module m\n\n@{word}\nfn f() -> Int {{\n    1\n}}\n"
        ));
        assert_eq!(d.code, codes::RESERVED_WORD, "{word}");
    }
    for (name, found) in [
        ("Upper", "identifier `Upper`"),
        ("_", "`_`"),
        ("1", "integer literal `1`"),
    ] {
        let d = module_error(&format!(
            "module m\n\n@{name}\nfn f() -> Int {{\n    1\n}}\n"
        ));
        assert_eq!(d.code, codes::SYNTAX_ERROR, "{name}");
        assert_eq!(
            d.data.get("expected").and_then(|v| v.as_str()),
            Some("an annotation name (snake_case identifier or keyword)")
        );
        assert_eq!(d.data.get("found").and_then(|v| v.as_str()), Some(found));
    }
    // A keyword is still never a member name (D53) or an argument.
    assert_eq!(
        module_error("module m\n\n@id(fn)\nfn f() -> Int {\n    1\n}\n").code,
        codes::SYNTAX_ERROR
    );
}

#[test]
fn empty_argument_lists_are_recorded_dumped_and_formatted() {
    let src = "module m\n\n@a\n@b()\n@c(1)\n@d(\n)\n@id()\nfn f() -> Int {\n    1\n}\n";
    let m = module(src);
    let d = &m.declarations[0];
    let parens: Vec<bool> = d.annotations.iter().map(|a| a.parens).collect();
    assert_eq!(parens, [false, true, true, true, true]);
    let args: Vec<usize> = d.annotations.iter().map(|a| a.args.len()).collect();
    assert_eq!(args, [0, 0, 1, 0, 0]);
    // The span of `@b()` covers its parentheses.
    let b = &d.annotations[1];
    assert_eq!(&src[b.span.start as usize..b.span.end as usize], "@b()");
    // `@id()` names no identity.
    assert_eq!(d.id(), None);
    let dump = flat(&dump_module(&m));
    for part in [
        "(annotation a)",
        "(annotation b ())",
        "(annotation c (int 1))",
        "(annotation d ())",
        "(annotation id ())",
    ] {
        assert!(dump.contains(part), "{part} in {dump}");
    }
    // T14: no token is removed; the broken empty list joins.
    assert_eq!(
        fmt(src),
        "module m\n\n@a\n@b()\n@c(1)\n@d()\n@id()\nfn f() -> Int {\n    1\n}\n"
    );
}
