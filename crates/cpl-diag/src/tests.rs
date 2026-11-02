//! Unit tests of the cpl-diag public API.

use crate::json::*;
use crate::*;
use serde_json::json;
use std::collections::BTreeMap;

fn map_with(path: &str, text: &str) -> (SourceMap, FileId) {
    let mut sm = SourceMap::new();
    let id = sm.add(path.to_string(), FileOrigin::Virtual, text.to_string());
    (sm, id)
}

// ---- source ----------------------------------------------------------------

#[test]
fn span_join_len_is_empty() {
    let f = FileId(0);
    let a = Span::new(f, 4, 9);
    let b = Span::new(f, 2, 6);
    assert_eq!(a.join(b), Span::new(f, 2, 9));
    assert_eq!(b.join(a), Span::new(f, 2, 9));
    assert_eq!(a.len(), 5);
    assert!(!a.is_empty());
    assert!(Span::point(f, 3).is_empty());
    assert_eq!(Span::point(f, 3).len(), 0);
    assert_eq!(Span::new(f, 5, 2).len(), 0);
}

#[test]
fn source_map_registers_finds_and_slices() {
    let mut sm = SourceMap::new();
    let std_id = sm.add(
        "std/prelude.cpl".into(),
        FileOrigin::Std,
        "module std.prelude\n".into(),
    );
    let src_id = sm.add(
        "src/a.cpl".into(),
        FileOrigin::Revision("3f2e".into()),
        "module a\nfn x() -> Int { 1 }\n".into(),
    );
    assert_eq!(std_id, FileId(0));
    assert_eq!(src_id, FileId(1));
    assert_eq!(sm.len(), 2);
    assert_eq!(sm.find("src/a.cpl"), Some(src_id));
    assert_eq!(sm.find("src/b.cpl"), None);
    let f = sm.file(src_id);
    assert_eq!(f.id, src_id);
    assert_eq!(f.path, "src/a.cpl");
    assert_eq!(f.origin, FileOrigin::Revision("3f2e".into()));
    assert_eq!(sm.text(Span::new(src_id, 9, 11)), "fn");
    assert_eq!(
        sm.files().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["std/prelude.cpl", "src/a.cpl"]
    );
    assert!(sm.get(FileId(7)).is_none());
}

#[test]
#[should_panic(expected = "not registered")]
fn file_panics_on_a_foreign_id() {
    SourceMap::new().file(FileId(3));
}

#[test]
fn line_col_counts_scalar_values_and_every_terminator() {
    let (sm, f) = map_with("a.cpl", "ab\r\ncdé f\nlast\rx");
    assert_eq!(sm.line_col(f, 0), LineCol { line: 1, col: 1 });
    assert_eq!(sm.line_col(f, 1), LineCol { line: 1, col: 2 });
    assert_eq!(sm.line_col(f, 4), LineCol { line: 2, col: 1 });
    // 'é' is two bytes: the space after it is column 4, `f` column 5.
    assert_eq!(sm.line_col(f, 8), LineCol { line: 2, col: 4 });
    assert_eq!(sm.line_col(f, 9), LineCol { line: 2, col: 5 });
    // An offset inside 'é' maps to its start.
    assert_eq!(sm.line_col(f, 7), LineCol { line: 2, col: 3 });
    assert_eq!(sm.line_col(f, 11), LineCol { line: 3, col: 1 });
    // A lone CR ends a line too.
    assert_eq!(sm.line_col(f, 16), LineCol { line: 4, col: 1 });
    let file = sm.file(f);
    assert_eq!(file.line_text(1), "ab");
    assert_eq!(file.line_text(2), "cdé f");
    assert_eq!(file.line_text(3), "last");
    assert_eq!(file.line_text(4), "x");
    assert_eq!(file.line_text(5), "");
    assert_eq!(file.line_count(), 4);
    // Out-of-range offsets clamp instead of panicking.
    assert_eq!(sm.line_col(f, 1000), LineCol { line: 4, col: 2 });
}

#[test]
fn location_is_one_based_with_an_exclusive_end() {
    let (sm, f) = map_with("src/reader/pages.cpl", "fn x() -> Int {\n    1\n}\n");
    assert_eq!(
        sm.location(Span::new(f, 20, 21)),
        Location {
            file: "src/reader/pages.cpl".into(),
            line: 2,
            col: 5,
            end_line: 2,
            end_col: 6
        }
    );
    let point = sm.location(Span::point(f, 20));
    assert_eq!((point.end_line, point.end_col), (point.line, point.col));
    let multi = sm.location(Span::new(f, 14, 23));
    assert_eq!(
        (multi.line, multi.col, multi.end_line, multi.end_col),
        (1, 15, 3, 2)
    );
}

#[test]
fn a_leading_byte_order_mark_is_not_a_column() {
    let (sm, f) = map_with("a.cpl", "\u{feff}module a\nx");
    assert_eq!(sm.line_col(f, 0), LineCol { line: 1, col: 1 });
    assert_eq!(sm.line_col(f, 3), LineCol { line: 1, col: 1 });
    assert_eq!(sm.line_col(f, 10), LineCol { line: 1, col: 8 });
    assert_eq!(sm.file(f).line_text(1), "module a");
    assert_eq!(sm.line_col(f, 12), LineCol { line: 2, col: 1 });
}

#[test]
fn invalid_utf8_registers_the_file_and_reports_a_parse_diagnostic() {
    let mut sm = SourceMap::new();
    let (ok, none) = sm.add_bytes("src/ok.cpl".into(), FileOrigin::Project, b"module ok\n");
    assert!(none.is_none());
    assert_eq!(sm.file(ok).text(), "module ok\n");
    let (bad, diag) = sm.add_bytes(
        "src/bad.cpl".into(),
        FileOrigin::Project,
        &[b'a', b'\n', 0x66, 0xff, 0x6e],
    );
    let diag = diag.expect("invalid UTF-8 is reported");
    assert_eq!(diag.code, "invalid_utf8");
    assert_eq!(diag.category, Category::Parse);
    assert_eq!(diag.severity, Severity::Error);
    assert_eq!(diag.message, "invalid UTF-8 at byte 3");
    let span = diag.span.expect("a span at the first invalid byte");
    assert_eq!(span.file, bad);
    assert_eq!(sm.text(span), "\u{fffd}");
    assert_eq!(
        sm.location(span),
        Location {
            file: "src/bad.cpl".into(),
            line: 2,
            col: 2,
            end_line: 2,
            end_col: 3
        }
    );
    assert_eq!(sm.len(), 2);
}

// ---- diag: codes and categories --------------------------------------------

#[test]
fn diag_code_behaves_like_a_string() {
    const CODE: DiagCode = DiagCode("syntax_error");
    assert_eq!(CODE.to_string(), "syntax_error");
    assert_eq!(CODE.len(), 12);
    assert!(CODE.starts_with("syntax"));
    assert_eq!(CODE.as_str(), "syntax_error");
    assert!(CODE == "syntax_error");
    assert!(CODE == *"syntax_error");
    let owned = String::from("syntax_error");
    assert!(CODE == owned);
    assert!("syntax_error" == CODE);
    assert!(owned == CODE);
    assert!(CODE != "type_mismatch");
    assert_eq!(serde_json::to_value(CODE).unwrap(), json!("syntax_error"));
}

#[test]
fn categories_and_severities_spell_snake_case() {
    assert_eq!(Category::ReleasePolicy.as_str(), "release_policy");
    assert_eq!(Category::ReleasePolicy.to_string(), "release_policy");
    assert_eq!(
        serde_json::to_value(Category::ReleasePolicy).unwrap(),
        json!("release_policy")
    );
    for c in Category::ALL {
        assert_eq!(serde_json::to_value(c).unwrap(), json!(c.as_str()));
        assert_eq!(Category::parse(c.as_str()), Some(c));
    }
    assert_eq!(Category::parse("lexical"), None);
    assert_eq!(
        serde_json::to_value(Severity::Warning).unwrap(),
        json!("warning")
    );
    assert!(Severity::Error < Severity::Warning && Severity::Warning < Severity::Info);
}

// ---- diag: builder and JSON ------------------------------------------------

fn counterexample(sm: &SourceMap, f: FileId) -> Diagnostic {
    let _ = sm;
    Diagnostic::error(
        DiagCode("contract_counterexample"),
        Category::Contract,
        "postcondition failed",
    )
    .with_span(Span::new(f, 20, 21))
    .with_symbol(
        StableId::parse("pgn_7c2d").unwrap(),
        "reader.pages.paginate",
        Some(Facet::Body),
    )
    .with_data("clause", json!("flatten(result) == items"))
    .with_data("input", json!({ "items": [1, 2, 3], "size": 2 }))
    .with_data("actual", json!([[1, 2]]))
    .with_data("status", json!("failed"))
}

#[test]
fn to_json_is_the_s49_object() {
    let (sm, f) = map_with("src/reader/pages.cpl", "fn x() -> Int {\n    1\n}\n");
    let d = counterexample(&sm, f);
    let v = serde_json::to_value(d.to_json(&sm)).unwrap();
    assert_eq!(
        v,
        json!({
            "code": "contract_counterexample",
            "category": "contract",
            "severity": "error",
            "message": "postcondition failed",
            "span": { "file": "src/reader/pages.cpl", "line": 2, "col": 5, "end_line": 2, "end_col": 6 },
            "symbol": "pgn_7c2d",
            "qualified_name": "reader.pages.paginate",
            "facet": "body",
            "clause": "flatten(result) == items",
            "input": { "items": [1, 2, 3], "size": 2 },
            "actual": [[1, 2]],
            "status": "failed"
        })
    );
    // The fixed keys come first, in declaration order; the payload follows.
    let text = serde_json::to_string(&d.to_json(&sm)).unwrap();
    assert!(text.starts_with(r#"{"code":"contract_counterexample","category":"contract","severity":"error","message":"postcondition failed","span":"#));
    // Round trip.
    let back: DiagnosticJson = serde_json::from_str(&text).unwrap();
    assert_eq!(back, d.to_json(&sm));
}

#[test]
fn to_json_omits_absent_fields_and_resolves_notes_and_frames() {
    let (sm, f) = map_with("src/a.cpl", "let x = 1\nlet x = 2\n");
    let bare = Diagnostic::warning(
        DiagCode("project_no_sources"),
        Category::Project,
        "no .cpl files found",
    );
    assert_eq!(
        serde_json::to_value(bare.to_json(&sm)).unwrap(),
        json!({ "code": "project_no_sources", "category": "project", "severity": "warning", "message": "no .cpl files found" })
    );
    let d = Diagnostic::error(
        DiagCode("division_by_zero"),
        Category::Runtime,
        "division by zero",
    )
    .with_span(Span::new(f, 14, 19))
    .with_note("the earlier binding", Some(Span::new(f, 4, 5)))
    .with_note("no location", None)
    .with_backtrace(vec![
        BacktraceFrame {
            qualified_name: "a.inner".into(),
            span: Some(Span::new(f, 10, 13)),
        },
        BacktraceFrame {
            qualified_name: "a.outer".into(),
            span: None,
        },
    ]);
    let v = serde_json::to_value(d.to_json(&sm)).unwrap();
    assert_eq!(
        v["notes"],
        json!([
            { "message": "the earlier binding", "span": { "file": "src/a.cpl", "line": 1, "col": 5, "end_line": 1, "end_col": 6 } },
            { "message": "no location" }
        ])
    );
    assert_eq!(
        v["backtrace"],
        json!([
            { "qualified_name": "a.inner", "span": { "file": "src/a.cpl", "line": 2, "col": 1, "end_line": 2, "end_col": 4 } },
            { "qualified_name": "a.outer" }
        ])
    );
}

#[test]
fn reserved_payload_keys_never_reach_the_json() {
    let (sm, _) = map_with("a.cpl", "");
    let mut d = Diagnostic::info(DiagCode("hole"), Category::Hole, "unfinished expression ?x");
    d.data.insert("code".into(), json!("forged"));
    d.data.insert("hole".into(), json!("x"));
    let v = serde_json::to_value(d.to_json(&sm)).unwrap();
    assert_eq!(v["code"], "hole");
    assert_eq!(v["hole"], "x");
    assert_eq!(v.as_object().unwrap().len(), 5);
}

#[test]
#[should_panic(expected = "reserved diagnostic key")]
#[cfg(debug_assertions)]
fn with_data_rejects_reserved_keys_in_debug_builds() {
    let _ = Diagnostic::info(DiagCode("hole"), Category::Hole, "m").with_data("symbol", json!("x"));
}

#[test]
fn with_symbol_sets_symbol_and_qualified_name_together() {
    let d = Diagnostic::error(DiagCode("x_y"), Category::Type, "m")
        .with_facet(Facet::Signature)
        .with_symbol(StableId::parse("a.b").unwrap(), "a.b", None);
    assert_eq!(d.symbol.as_ref().map(StableId::as_str), Some("a.b"));
    assert_eq!(d.qualified_name.as_deref(), Some("a.b"));
    assert_eq!(d.facet, Some(Facet::Signature));
    assert!(d.is_error());
}

#[test]
fn arbitrary_precision_payloads_round_trip() {
    let (sm, _) = map_with("a.cpl", "");
    let big: serde_json::Value = serde_json::from_str("123456789012345678901234567890").unwrap();
    let d = Diagnostic::error(DiagCode("contract_violation"), Category::Contract, "m")
        .with_data("actual", big);
    let text = serde_json::to_string(&d.to_json(&sm)).unwrap();
    assert!(
        text.contains(r#""actual":123456789012345678901234567890"#),
        "{text}"
    );
    let back: DiagnosticJson = serde_json::from_str(&text).unwrap();
    assert_eq!(
        back.data["actual"].to_string(),
        "123456789012345678901234567890"
    );
}

// ---- diag: human rendering -------------------------------------------------

fn twelve_lines() -> String {
    let mut text = String::new();
    for i in 1..=11 {
        text.push_str(&format!("// line {i}\n"));
    }
    text.push_str("    page.len\n");
    text
}

#[test]
fn render_matches_the_section_1_4_1_layout() {
    let text = twelve_lines();
    let (sm, f) = map_with("src/reader/pages.cpl", &text);
    let start = text.find("page.len").unwrap() as u32;
    let d = Diagnostic::error(
        DiagCode("type_mismatch"),
        Category::Type,
        "expected `Text`, found `Int`",
    )
    .with_span(Span::new(f, start, start + 8))
    .with_note("the tail expression is the function's return value", None);
    assert_eq!(
        d.render(&sm),
        "error[type_mismatch]: expected `Text`, found `Int`\n\
         \x20 --> src/reader/pages.cpl:12:5\n\
         \x20  |\n\
         12 |     page.len\n\
         \x20  |     ^^^^^^^^\n\
         \x20  |\n\
         \x20  = note: the tail expression is the function's return value\n"
    );
}

#[test]
fn render_without_a_span_has_no_arrow_line() {
    let sm = SourceMap::new();
    let d = Diagnostic::info(
        DiagCode("hole"),
        Category::Hole,
        "unfinished expression ?summary",
    )
    .with_note("expected type `Text`", None);
    assert_eq!(
        d.render(&sm),
        "info[hole]: unfinished expression ?summary\n  = note: expected type `Text`\n"
    );
    let plain = Diagnostic::warning(
        DiagCode("project_no_sources"),
        Category::Project,
        "no .cpl files found",
    );
    assert_eq!(
        plain.render(&sm),
        "warning[project_no_sources]: no .cpl files found\n"
    );
    assert_eq!(
        plain.to_string(),
        "warning[project_no_sources]: no .cpl files found"
    );
}

#[test]
fn render_shows_backtraces_spanned_notes_and_first_line_carets() {
    let (sm, f) = map_with("src/a.cpl", "fn f() -> Int {\n\tlet é = 1 / 0\n    é\n}\n");
    let div = "fn f() -> Int {\n\tlet é = ".len() as u32;
    let d = Diagnostic::error(
        DiagCode("division_by_zero"),
        Category::Runtime,
        "division by zero",
    )
    .with_span(Span::new(f, div, div + 5))
    .with_backtrace(vec![
        BacktraceFrame {
            qualified_name: "a.f".into(),
            span: Some(Span::new(f, div, div + 5)),
        },
        BacktraceFrame {
            qualified_name: "a.main".into(),
            span: None,
        },
    ])
    .with_note("the function starts here", Some(Span::new(f, 0, 20)));
    assert_eq!(
        d.render(&sm),
        "error[division_by_zero]: division by zero\n\
         \x20 --> src/a.cpl:2:10\n\
         \x20 |\n\
         2 |     let é = 1 / 0\n\
         \x20 |             ^^^^^\n\
         \x20 |\n\
         \x20 = at a.f (src/a.cpl:2)\n\
         \x20 = at a.main\n\
         note: the function starts here\n\
         \x20 --> src/a.cpl:1:1\n\
         \x20 |\n\
         1 | fn f() -> Int {\n\
         \x20 | ^^^^^^^^^^^^^^^\n"
    );
}

#[test]
fn render_points_one_caret_at_an_empty_span_and_at_the_end_of_a_line() {
    let (sm, f) = map_with("t.cpl", "let total = a +\n");
    let d = Diagnostic::error(
        DiagCode("syntax_error"),
        Category::Parse,
        "expected expression, found end of line",
    )
    .with_span(Span::new(f, 15, 16));
    assert_eq!(
        d.render(&sm),
        "error[syntax_error]: expected expression, found end of line\n\
         \x20 --> t.cpl:1:16\n\
         \x20 |\n\
         1 | let total = a +\n\
         \x20 |                ^\n"
    );
    let eof = Diagnostic::error(
        DiagCode("syntax_error"),
        Category::Parse,
        "expected `}`, found end of file",
    )
    .with_span(Span::point(f, 16));
    assert!(
        eof.render(&sm)
            .ends_with("  --> t.cpl:2:1\n  |\n2 |\n  | ^\n"),
        "{}",
        eof.render(&sm)
    );
}

// ---- diag: ordering --------------------------------------------------------

#[test]
fn sorted_orders_by_path_offset_severity_code_with_spanless_last() {
    let mut sm = SourceMap::new();
    let b = sm.add("src/b.cpl".into(), FileOrigin::Project, "x".repeat(100));
    let a = sm.add("src/a.cpl".into(), FileOrigin::Project, "x".repeat(100));
    let mk = |code: &'static str, sev: Severity, span: Option<Span>| {
        let d = Diagnostic::new(DiagCode(code), Category::Type, sev, code);
        match span {
            Some(s) => d.with_span(s),
            None => d,
        }
    };
    let mut list = Diagnostics::new();
    list.push(mk("spanless_first", Severity::Info, None));
    list.push(mk("b_late", Severity::Error, Some(Span::new(b, 50, 51))));
    list.push(mk(
        "a_warning",
        Severity::Warning,
        Some(Span::new(a, 10, 11)),
    ));
    list.push(mk("spanless_second", Severity::Error, None));
    list.push(mk("a_error_z", Severity::Error, Some(Span::new(a, 10, 12))));
    list.push(mk("a_error_y", Severity::Error, Some(Span::new(a, 10, 12))));
    list.push(mk("b_early", Severity::Info, Some(Span::new(b, 5, 6))));
    assert_eq!(list.len(), 7);
    assert!(list.has_errors());
    assert_eq!(list.error_count(), 4);
    assert_eq!(list.warning_count(), 1);
    let order: Vec<String> = list
        .sorted(&sm)
        .into_iter()
        .map(|d| d.code.to_string())
        .collect();
    assert_eq!(
        order,
        [
            "a_error_y",
            "a_error_z",
            "a_warning",
            "b_early",
            "b_late",
            "spanless_first",
            "spanless_second"
        ]
    );
}

// ---- facet -----------------------------------------------------------------

#[test]
fn facets_have_one_spelling_and_a_canonical_order() {
    let names: Vec<&str> = Facet::ALL.iter().map(|f| f.as_str()).collect();
    assert_eq!(
        names,
        [
            "identity",
            "name",
            "signature",
            "contract",
            "effects",
            "body",
            "docs",
            "verification_policy"
        ]
    );
    let mut sorted = Facet::ALL;
    sorted.sort();
    assert_eq!(sorted, Facet::ALL);
    for f in Facet::ALL {
        assert_eq!(Facet::parse(f.as_str()), Some(f));
        assert_eq!(serde_json::to_value(f).unwrap(), json!(f.as_str()));
        assert_eq!(f.to_string(), f.as_str());
    }
    assert_eq!(Facet::parse("definition"), None);
    assert_eq!("body".parse::<Facet>(), Ok(Facet::Body));
}

#[test]
fn facet_selectors_serialize_as_plain_strings() {
    let body = FacetSelector::Facet(Facet::Body);
    assert_eq!(serde_json::to_value(body).unwrap(), json!("body"));
    assert_eq!(
        serde_json::to_value(FacetSelector::Definition).unwrap(),
        json!("definition")
    );
    assert_eq!(
        serde_json::from_value::<FacetSelector>(json!("definition")).unwrap(),
        FacetSelector::Definition
    );
    assert_eq!(
        serde_json::from_value::<FacetSelector>(json!("verification_policy")).unwrap(),
        FacetSelector::Facet(Facet::VerificationPolicy)
    );
    assert!(serde_json::from_value::<FacetSelector>(json!("bodies")).is_err());
    assert_eq!(FacetSelector::parse("docs"), Some(Facet::Docs.into()));
    assert_eq!(FacetSelector::Definition.facets().len(), 8);
    assert!(FacetSelector::Definition.includes(Facet::Name));
    assert!(!body.includes(Facet::Name));
}

// ---- id --------------------------------------------------------------------

#[test]
fn accepted_id_syntax() {
    for ok in [
        "pgn_7c2d",
        "fn_k7w3qz2mb9ptx4ve",
        "a_b1",
        "ty_2_x",
        &format!("a_{}", "b".repeat(62)),
    ] {
        let id = StableId::parse(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        assert_eq!(id.as_str(), ok);
        assert!(!id.is_provisional());
        assert_eq!(StableId::from_annotation(ok), Ok(id));
    }
    assert_eq!(StableId::parse("Foo"), Err(IdError::Length(3)));
    assert_eq!(StableId::parse(""), Err(IdError::Length(0)));
    assert_eq!(
        StableId::parse(&format!("a_{}", "b".repeat(63))),
        Err(IdError::Length(65))
    );
    for bad in [
        "Fooo", "abcd", "a__b", "_abc", "abc_", "1a_b", "a_B1", "a-b_c", "ab_ç",
    ] {
        assert!(
            matches!(
                StableId::parse(bad),
                Err(IdError::Syntax(_)) | Err(IdError::Length(_))
            ),
            "{bad} accepted"
        );
    }
    assert!(StableId::from_annotation("reader.pages.clean").is_err());
}

#[test]
fn provisional_ids_are_canonical_qualified_names() {
    let id = StableId::parse("tests.pagination.\"pagination \\u{41}nd more\"").unwrap();
    assert_eq!(id.as_str(), "tests.pagination.\"pagination And more\"");
    assert!(id.is_provisional());
    assert_eq!(id.kind_prefix(), "");
    let made = StableId::provisional(
        "tests.pagination",
        &DeclName::Text("pagination And more".into()),
    );
    assert_eq!(made, id);
    assert_eq!(
        StableId::provisional(
            "std.prelude",
            &DeclName::Member {
                receiver: "List".into(),
                name: "take".into()
            }
        )
        .as_str(),
        "std.prelude.List.take"
    );
    assert!(matches!(
        StableId::parse("Reader.pages.clean"),
        Err(IdError::Path(_))
    ));
    assert!(matches!(
        StableId::parse("reader.Pages.Clean"),
        Err(IdError::Path(_))
    ));
    assert!(matches!(StableId::parse("reader."), Err(IdError::Path(_))));
}

#[test]
fn kind_prefixes_are_informative() {
    let prefix = |s: &str| StableId::parse(s).unwrap().kind_prefix().to_string();
    assert_eq!(prefix("fn_k7w3qz2mb9ptx4ve"), "fn");
    assert_eq!(prefix("ty_abcd"), "ty");
    assert_eq!(prefix("cap_ab"), "cap");
    assert_eq!(prefix("test_ab"), "test");
    assert_eq!(prefix("prop_ab"), "prop");
    assert_eq!(prefix("pgn_7c2d"), "");
    assert_eq!(prefix("reader.pages.clean"), "");
}

#[test]
fn stable_ids_serialize_transparently_and_validate_on_deserialize() {
    let id = StableId::parse("pgn_7c2d").unwrap();
    assert_eq!(serde_json::to_value(&id).unwrap(), json!("pgn_7c2d"));
    assert_eq!(
        serde_json::from_value::<StableId>(json!("pgn_7c2d")).unwrap(),
        id
    );
    assert!(serde_json::from_value::<StableId>(json!("Not An Id")).is_err());
    assert!(id == "pgn_7c2d");
    let mut map = BTreeMap::new();
    map.insert(id.clone(), 1);
    assert_eq!(map.get("pgn_7c2d"), Some(&1));
    assert_eq!(
        "reader.pages.clean"
            .parse::<StableId>()
            .unwrap()
            .to_string(),
        "reader.pages.clean"
    );
}

#[test]
fn symbol_paths_parse_every_decl_name_form() {
    let p = SymbolPath::parse("reader.pages.paginate").unwrap();
    assert_eq!(p.module, ["reader", "pages"]);
    assert_eq!(p.name, DeclName::Ident("paginate".into()));
    assert_eq!(p.module_path(), "reader.pages");
    let t = SymbolPath::parse(r#"tests.pagination."pagination handles empty input""#).unwrap();
    assert_eq!(
        t.name,
        DeclName::Text("pagination handles empty input".into())
    );
    let m = SymbolPath::parse("std.prelude.List.take").unwrap();
    assert_eq!(m.module, ["std", "prelude"]);
    assert_eq!(
        m.name,
        DeclName::Member {
            receiver: "List".into(),
            name: "take".into()
        }
    );
    let ty = SymbolPath::parse("reader.pages.Page").unwrap();
    assert_eq!(ty.name, DeclName::Ident("Page".into()));
    for s in [
        "reader.pages.paginate",
        r#"tests.pagination."pagination handles empty input""#,
        "std.prelude.List.take",
        "a.B",
    ] {
        assert_eq!(SymbolPath::parse(s).unwrap().display(), s);
    }
    for bad in [
        "",
        "paginate",
        "List.take",
        "a..b",
        "a.b.",
        ".a",
        "a.\"x\".b",
        "a.\"x",
        "a.B.C",
        "a.B.c.d",
        "a b",
        "a._",
        r#"a."\q""#,
    ] {
        assert!(SymbolPath::parse(bad).is_err(), "{bad} parsed");
    }
}

#[test]
fn text_segments_display_canonically_escaped() {
    let p = SymbolPath::new(
        vec!["t".into()],
        DeclName::Text("a \"q\" \\ \n\t\u{1}\u{7f} é".into()),
    );
    let shown = p.display();
    assert_eq!(shown, r#"t."a \"q\" \\ \n\t\u{1}\u{7f} é""#);
    assert_eq!(SymbolPath::parse(&shown).unwrap(), p);
    assert_eq!(quote_text("x"), "\"x\"");
}

// ---- version ---------------------------------------------------------------

#[test]
fn version_constants() {
    assert_eq!(version::TOOLCHAIN, "0.1.0");
    assert_eq!(version::EDITION, "0.1");
    assert_eq!(version::JSON_SCHEMA, 1);
    assert_eq!(
        serde_json::to_value(ToolchainInfo::current()).unwrap(),
        json!({ "version": "0.1.0", "edition": "0.1", "json_schema": 1 })
    );
}

// ---- json ------------------------------------------------------------------

fn version_of(id: &str, path: &str, provisional: bool) -> SymbolVersion {
    let mut facets = BTreeMap::new();
    facets.insert(Facet::VerificationPolicy, FacetHash("v".into()));
    facets.insert(Facet::Identity, FacetHash("i".into()));
    facets.insert(Facet::Body, FacetHash("b".into()));
    SymbolVersion {
        id: StableId::parse(id).unwrap(),
        path: path.into(),
        provisional,
        facets,
        definition: FacetHash("d".into()),
    }
}

fn revision() -> Revision {
    Revision {
        commit: None,
        state: RevisionState::Worktree,
        digest: Digest("9a0b".into()),
    }
}

#[test]
fn symbol_versions_list_facets_in_canonical_order() {
    let text =
        serde_json::to_string(&version_of("pgn_7c2d", "reader.pages.paginate", false)).unwrap();
    assert_eq!(
        text,
        r#"{"id":"pgn_7c2d","path":"reader.pages.paginate","provisional":false,"facets":{"identity":"i","body":"b","verification_policy":"v"},"definition":"d"}"#
    );
    let back: SymbolVersion = serde_json::from_str(&text).unwrap();
    assert_eq!(back, version_of("pgn_7c2d", "reader.pages.paginate", false));
}

#[test]
fn the_envelope_flattens_the_command_result() {
    let env = Envelope {
        command: "check".into(),
        toolchain: ToolchainInfo::current(),
        revision: Some(revision()),
        status: Status::Ok,
        diagnostics: vec![],
        result: CheckOutput {
            release: false,
            summary: CheckSummary {
                errors: 0,
                warnings: 0,
                holes: 2,
                definitions: 5,
                modules: 1,
                files: 1,
            },
            holes: vec![],
            verification: vec![VerificationStatusJson {
                symbol: StableId::parse("pgn_7c2d").unwrap(),
                qualified_name: "holes.pages.paginate".into(),
                status: ProofStatus::Unknown,
                reason: Some("verifier_unavailable".into()),
            }],
        },
    };
    let text = serde_json::to_string(&env).unwrap();
    assert_eq!(
        text,
        concat!(
            r#"{"command":"check","toolchain":{"version":"0.1.0","edition":"0.1","json_schema":1},"#,
            r#""revision":{"commit":null,"state":"worktree","digest":"9a0b"},"status":"ok","diagnostics":[],"#,
            r#""release":false,"summary":{"errors":0,"warnings":0,"holes":2,"definitions":5,"modules":1,"files":1},"#,
            r#""holes":[],"verification":[{"symbol":"pgn_7c2d","qualified_name":"holes.pages.paginate","status":"unknown","reason":"verifier_unavailable"}]}"#
        )
    );
    let back: Envelope<CheckOutput> = serde_json::from_str(&text).unwrap();
    assert_eq!(back, env);
    let empty = Envelope {
        result: NoOutput {},
        revision: None,
        status: Status::Error,
        ..env.clone().map_into()
    };
    assert_eq!(
        serde_json::to_value(&empty).unwrap(),
        json!({ "command": "check", "toolchain": ToolchainInfo::current(), "revision": null, "status": "error", "diagnostics": [] })
    );
    assert_eq!(Status::Unsuccessful.exit_code(), 1);
}

trait MapInto {
    fn map_into(self) -> Envelope<NoOutput>;
}

impl MapInto for Envelope<CheckOutput> {
    fn map_into(self) -> Envelope<NoOutput> {
        Envelope {
            command: self.command,
            toolchain: self.toolchain,
            revision: self.revision,
            status: self.status,
            diagnostics: self.diagnostics,
            result: NoOutput {},
        }
    }
}

#[test]
fn hole_payloads_match_the_check_example() {
    let report = HoleReportJson {
        hole: "build_pages".into(),
        expected_type: "List<List<T>>".into(),
        bindings: vec![
            HoleBindingJson {
                name: "size".into(),
                ty: "Positive".into(),
                kind: BindingKind::Parameter,
                mutable: false,
            },
            HoleBindingJson {
                name: "acc".into(),
                ty: "Int".into(),
                kind: BindingKind::Var,
                mutable: true,
            },
            HoleBindingJson {
                name: "self".into(),
                ty: "Int".into(),
                kind: BindingKind::SelfValue,
                mutable: false,
            },
        ],
        capabilities: vec![],
        requires: vec![],
        ensures: vec!["flatten(result) == items".into()],
        invariants: vec![],
        dependencies: HoleDependenciesJson {
            source_revision: "unversioned".into(),
            dirty: None,
            toolchain: ReceiptToolchain::current(Digest("s".into())),
            modules: vec![ModuleHash {
                module: "holes.pages".into(),
                content_hash: Digest("c".into()),
            }],
            symbols: vec![],
        },
    };
    let v = serde_json::to_value(&report).unwrap();
    assert_eq!(
        v["bindings"][0],
        json!({ "name": "size", "type": "Positive", "kind": "parameter" })
    );
    assert_eq!(
        v["bindings"][1],
        json!({ "name": "acc", "type": "Int", "kind": "var", "mutable": true })
    );
    assert_eq!(v["bindings"][2]["kind"], "self");
    assert_eq!(
        v["dependencies"],
        json!({ "source_revision": "unversioned", "toolchain": { "cpl": "0.1.0", "edition": "0.1", "std": "s" },
                "modules": [{ "module": "holes.pages", "content_hash": "c" }], "symbols": [] })
    );
    let data = to_data(&report);
    assert_eq!(data["hole"], "build_pages");
    assert_eq!(serde_json::from_value::<HoleReportJson>(v).unwrap(), report);
}

#[test]
fn symbol_blocks_distinguish_absent_and_null_docs() {
    let block = SymbolBlock {
        version: version_of("reader.pages.Positive", "reader.pages.Positive", true),
        kind: "ty".into(),
        module: "reader.pages".into(),
        public: true,
        location: Location {
            file: "src/reader/pages.cpl".into(),
            line: 4,
            col: 1,
            end_line: 4,
            end_col: 40,
        },
        verification_policy: None,
        signature: Some("pub type Positive = Int".into()),
        contract: Some(vec!["where self > 0".into()]),
        effects: None,
        docs: Some(None),
        implementation: None,
        holes: None,
        relation: Some(Relation::TypeUse),
        interface: None,
        call_sites: None,
    };
    let v = serde_json::to_value(&block).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "contract",
            "definition",
            "docs",
            "facets",
            "id",
            "kind",
            "location",
            "module",
            "path",
            "provisional",
            "public",
            "relation",
            "signature"
        ]
    );
    assert_eq!(v["docs"], serde_json::Value::Null);
    assert_eq!(v["relation"], "type_use");
    let text = serde_json::to_string(&block).unwrap();
    assert!(
        text.starts_with(
            r#"{"id":"reader.pages.Positive","path":"reader.pages.Positive","provisional":true,"#
        ),
        "{text}"
    );
    let back: SymbolBlock = serde_json::from_str(&text).unwrap();
    assert_eq!(back, block);
    let without = SymbolBlock {
        docs: None,
        ..block
    };
    let back: SymbolBlock =
        serde_json::from_str(&serde_json::to_string(&without).unwrap()).unwrap();
    assert_eq!(back.docs, None);
}

#[test]
fn test_output_flattens_the_report_and_keeps_big_seeds() {
    let record = TestCaseRecord {
        symbol: StableId::parse("tests.pagination.\"pagination preserves input\"").unwrap(),
        qualified_name: "tests.pagination.\"pagination preserves input\"".into(),
        kind: TestKind::Property,
        status: TestStatus::Failed,
        duration_ms: 3,
        seed: Some(1446357982114401307),
        property_seed: Some("c0f6".into()),
        cases: Some(38),
        failing_case: Some(FailingCase {
            index: 37,
            input: to_data(&json!({ "items": [7, -3, 0, 12, 5], "size": 2 })),
        }),
        shrunk: Some(ShrunkCase {
            input: to_data(&json!({ "items": [0, 0, 0], "size": 2 })),
            shrink_steps: 12,
            shrink_exhausted: false,
        }),
        fault: None,
        counterexamples: vec![],
        receipt: None,
    };
    let report = TestReport {
        seed: 1446357982114401307,
        cases: 100,
        selected: 3,
        tested: 2,
        failed: 1,
        results: vec![record],
    };
    let receipt = TestReceipt {
        receipt: ReceiptKind::Test,
        schema: version::JSON_SCHEMA,
        toolchain: ToolchainInfo::current(),
        revision: revision(),
        inputs: ReceiptInputsJson {
            manifest: Digest("m".into()),
            lock: None,
            std: Digest("s".into()),
            sources: BTreeMap::new(),
        },
        core: ReceiptCore {
            source_revision: "unversioned".into(),
            dirty: None,
            toolchain: ReceiptToolchain::current(Digest("s".into())),
            modules: vec![],
            check_configuration: json!({ "cases": 100, "max_shrinks": 1000, "max_call_depth": 10000, "seed": 1446357982114401307u64 }),
            assumptions: vec![],
            fixtures: Some(vec![]),
            environment: Some(BTreeMap::from([
                ("arch".to_string(), "x86_64".to_string()),
                ("os".to_string(), "linux".to_string()),
            ])),
        },
        configuration: TestRunConfig {
            seed: 1446357982114401307,
            cases: 100,
            max_shrinks: 1000,
            timeout_ms: 0,
            selector: TestSelectorSpec {
                namespace: Some(vec!["tests".into(), "pagination".into()]),
                name: None,
                id: None,
            },
            check_contracts: true,
            fail_fast: false,
        },
        tested: vec![],
        tests: vec![],
        summary: TestSummary {
            selected: 3,
            tested: 2,
            failed: 1,
        },
        fixtures: vec![],
        assumptions: vec![],
        environment: BTreeMap::new(),
        status: TestStatus::Failed,
        produced_at: "2026-10-27T21:00:00Z".into(),
        duration_ms: 12,
    };
    let out = TestOutput { report, receipt };
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(v["seed"], json!(1446357982114401307u64));
    assert_eq!(v["results"][0]["kind"], "property");
    assert_eq!(
        v["results"][0]["failing_case"],
        json!({ "index": 37, "input": { "items": [7, -3, 0, 12, 5], "size": 2 } })
    );
    assert!(v["results"][0].get("fault").is_none());
    assert!(v["results"][0].get("counterexamples").is_none());
    assert_eq!(v["receipt"]["receipt"], "test");
    assert_eq!(
        v["receipt"]["configuration"]["selector"],
        json!({ "namespace": ["tests", "pagination"], "name": null, "id": null })
    );
    assert_eq!(v["receipt"]["inputs"]["lock"], serde_json::Value::Null);
    let text = serde_json::to_string(&out).unwrap();
    let back: TestOutput = serde_json::from_str(&text).unwrap();
    assert_eq!(back, out);
}

#[test]
fn run_output_spells_absent_results_as_null() {
    let out = RunOutput {
        function: version_of(
            "reader.pages.paginate_ints",
            "reader.pages.paginate_ints",
            true,
        ),
        arguments: vec![json!([1, 2, 3]), json!(2)],
        result: Some(json!({ "$type": "Result", "$variant": "Ok", "$payload": [[[1, 2], [3]]] })),
        result_text: Some("Ok([[1, 2], [3]])".into()),
        fault: None,
    };
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(v["fault"], serde_json::Value::Null);
    assert_eq!(serde_json::from_value::<RunOutput>(v).unwrap(), out);
}

#[test]
fn work_check_requirements_match_the_example_shapes() {
    let target = WorkTargetJson {
        id: StableId::parse("pgn_7c2d").unwrap(),
        facet: FacetSelector::Definition,
    };
    let reqs = vec![
        WorkRequirementJson::new(RequireKind::Typecheck),
        WorkRequirementJson {
            targets: Some(vec![target]),
            ..WorkRequirementJson::new(RequireKind::Prove)
        },
        WorkRequirementJson {
            namespace: Some("tests.pagination".into()),
            matched: Some(3),
            ..WorkRequirementJson::new(RequireKind::Test)
        },
        WorkRequirementJson {
            task: Some("audit".into()),
            ..WorkRequirementJson::new(RequireKind::Review)
        },
        WorkRequirementJson::new(RequireKind::NoHoles),
        WorkRequirementJson::new(RequireKind::NoUnresolvedAssumptions),
    ];
    assert_eq!(
        serde_json::to_string(&reqs).unwrap(),
        concat!(
            r#"[{"require":"typecheck"},{"require":"prove","targets":[{"id":"pgn_7c2d","facet":"definition"}]},"#,
            r#"{"require":"test","namespace":"tests.pagination","matched":3},{"require":"review","task":"audit"},"#,
            r#"{"require":"no_holes"},{"require":"no_unresolved_assumptions"}]"#
        )
    );
    let file = WorkSelectorJson {
        selector: "project.manifest".into(),
        targets: None,
        file: Some("cpl.toml".into()),
    };
    assert_eq!(
        serde_json::to_value(&file).unwrap(),
        json!({ "selector": "project.manifest", "file": "cpl.toml" })
    );
    let back: Vec<WorkRequirementJson> =
        serde_json::from_str(&serde_json::to_string(&reqs).unwrap()).unwrap();
    assert_eq!(back, reqs);
}

#[test]
fn diff_and_obligation_documents_keep_their_nulls() {
    let st = DiffStatement {
        statement: DiffStatementKind::IdentityAdded,
        id: StableId::parse("tests.pagination.\"pagination rejects zero\"").unwrap(),
        path: "tests.pagination.\"pagination rejects zero\"".into(),
        kind: "test".into(),
        facet: None,
        detail: json!({ "version": {} }),
    };
    let v = serde_json::to_value(&st).unwrap();
    assert_eq!(v["statement"], "identity_added");
    assert_eq!(v["facet"], serde_json::Value::Null);
    let ob = Obligation {
        id: "pgn_7c2d#ensures/1".into(),
        kind: "ensures".into(),
        clause: "flatten(result) == items".into(),
        location: Location {
            file: "src/reader/pages.cpl".into(),
            line: 46,
            col: 5,
            end_line: 46,
            end_col: 37,
        },
        status: ObligationStatus::Unknown,
        reason: Some("verifier_unavailable".into()),
        evidence: None,
        assumptions: vec![],
    };
    let v = serde_json::to_value(&ob).unwrap();
    assert_eq!(v["evidence"], serde_json::Value::Null);
    assert_eq!(v["status"], "unknown");
    let asm = Assumption {
        id: "asm_1".into(),
        kind: AssumptionKind::ForeignAdapter,
        symbol: StableId::parse("reader.images.ImageCodec").unwrap(),
        facet: None,
        description: "adapter contract".into(),
        admitted: false,
    };
    assert_eq!(
        serde_json::to_value(&asm).unwrap()["kind"],
        "foreign_adapter"
    );
    assert_eq!(
        serde_json::to_value(&asm).unwrap()["facet"],
        serde_json::Value::Null
    );
}

#[test]
fn shared_vocabularies() {
    assert_eq!(
        serde_json::to_value(ProofStatus::Conditional).unwrap(),
        json!("conditional")
    );
    assert_eq!(
        serde_json::to_value(Include::InterfaceDependencies).unwrap(),
        json!("interface_dependencies")
    );
    let mut include = vec![
        Include::Callers,
        Include::Effects,
        Include::Signature,
        Include::Contract,
    ];
    include.sort();
    assert_eq!(
        serde_json::to_value(&include).unwrap(),
        json!(["signature", "contract", "effects", "callers"])
    );
    let strict = ExecutionBoundary::default();
    assert_eq!(
        serde_json::to_value(strict).unwrap(),
        json!({ "filesystem": "worktree", "process": "none", "network": "none" })
    );
    let host = ExecutionBoundary {
        filesystem: FsAccess::Host,
        process: ProcessAccess::Any,
        network: NetworkAccess::Any,
    };
    let mixed = ExecutionBoundary {
        filesystem: FsAccess::WorktreeAndRepository,
        process: ProcessAccess::Any,
        network: NetworkAccess::None,
    };
    assert!(strict.within(&host));
    assert!(!host.within(&strict));
    assert_eq!(host.min(&mixed), mixed);
    assert_eq!(
        serde_json::to_value(mixed).unwrap()["filesystem"],
        "worktree_and_repository"
    );
    assert_eq!(BindingKind::SelfValue.as_str(), "self");
    assert_eq!(
        serde_json::from_value::<BindingKind>(json!("self")).unwrap(),
        BindingKind::SelfValue
    );
}
