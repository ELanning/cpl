//! Hole reports (`spec/toolchain.md` 12 "hole reports" row, 1.6
//! `HoleReport`; `spec/semantics.md` 8.1-8.3, S47, S48, S64, Section 11):
//! the reports of `examples/holes`, the `hole` diagnostic's payload, the
//! release-mode replacement by `hole_in_release`, and which holes get no
//! report.

mod common;

use common::*;
use cpl_diag::Severity;
use cpl_hir::{BindingKind, Ty};
use cpl_sema::{check_with, CheckConfig, CheckResult};
use serde_json::json;

fn holes_example(release: bool) -> CheckResult {
    let project = load_dir(&example("holes"));
    check_with(
        &project,
        &CheckConfig {
            release,
            ..CheckConfig::default()
        },
    )
}

/// Semantics Section 11: `build_pages` (`List<List<T>>`, bindings `items:
/// List<T>`, `size: Positive`, `ensures` x 3) and `summary` (`Text`,
/// bindings `page: Page`).
#[test]
fn examples_holes_reports() {
    let result = holes_example(false);
    let p = &result.program;
    assert_eq!(result.holes.len(), 2);
    let build = &result.holes[0];
    assert_eq!(build.name, "build_pages");
    let paginate = p.def(build.def);
    assert_eq!(p.display_path(build.def), "holes.pages.paginate");
    let generics = paginate.kind.generics();
    assert_eq!(p.display_ty_in(&build.expected, generics), "List<List<T>>");
    // Innermost scope first, most recent binding first (toolchain 7.2).
    let bindings: Vec<(String, String, BindingKind, bool)> = build
        .bindings
        .iter()
        .map(|b| {
            (
                b.name.clone(),
                p.display_ty_in(&b.ty, generics),
                b.kind,
                b.mutable,
            )
        })
        .collect();
    assert_eq!(
        bindings,
        [
            (
                "size".to_string(),
                "Positive".to_string(),
                BindingKind::Parameter,
                false
            ),
            (
                "items".to_string(),
                "List<T>".to_string(),
                BindingKind::Parameter,
                false
            ),
        ]
    );
    assert!(build.requires.is_empty());
    assert_eq!(
        build.ensures,
        [
            "flatten(result) == items",
            "forall page in result: page.len > 0 and page.len <= size",
            "forall page in result.drop_last(): page.len == size",
        ]
    );
    assert!(build.invariants.is_empty());
    assert!(build.capabilities.is_empty());
    // The declaration's dependencies include the refinement its signature names.
    let deps: Vec<String> = build
        .dependencies
        .iter()
        .map(|d| p.display_path(*d))
        .collect();
    assert!(
        deps.contains(&"holes.pages.Positive".to_string()),
        "{deps:?}"
    );

    let summary = &result.holes[1];
    assert_eq!(summary.name, "summary");
    assert_eq!(summary.expected, Ty::Text);
    assert_eq!(summary.bindings.len(), 1);
    assert_eq!(summary.bindings[0].name, "page");
    assert_eq!(p.display_ty(&summary.bindings[0].ty), "Page");
    assert!(summary.ensures.is_empty());
}

/// The `hole` diagnostic carries the S48 payload and blames the body facet.
#[test]
fn hole_diagnostic_payload() {
    let result = holes_example(false);
    let j = json_values(&result);
    let build = j
        .iter()
        .find(|d| d["code"] == "hole" && d["hole"] == "build_pages")
        .expect("the build_pages hole");
    assert_eq!(build["severity"], "info");
    assert_eq!(build["category"], "hole");
    assert_eq!(build["symbol"], "pgn_7c2d");
    assert_eq!(build["qualified_name"], "holes.pages.paginate");
    assert_eq!(build["facet"], "body");
    assert_eq!(build["expected_type"], "List<List<T>>");
    assert_eq!(
        build["bindings"],
        json!([
            { "name": "size", "type": "Positive", "kind": "parameter" },
            { "name": "items", "type": "List<T>", "kind": "parameter" },
        ])
    );
    assert_eq!(build["capabilities"], json!([]));
    assert_eq!(build["requires"], json!([]));
    assert_eq!(build["ensures"].as_array().map(Vec::len), Some(3));
    assert_eq!(build["invariants"], json!([]));
    assert_eq!(build["message"], "unfinished expression ?build_pages");
}

/// Under `--release` each `fn`-body hole is `hole_in_release` in place of
/// `hole` (S64), and `CheckResult::holes` still lists both reports.
#[test]
fn release_replaces_fn_body_holes() {
    let result = holes_example(true);
    let holes: Vec<(Severity, String)> = result
        .diagnostics
        .iter()
        .filter(|d| d.category.as_str() == "hole" || d.code.0 == "hole_in_release")
        .map(|d| (d.severity, d.code.0.to_string()))
        .collect();
    assert_eq!(
        holes,
        [
            (Severity::Error, "hole_in_release".to_string()),
            (Severity::Error, "hole_in_release".to_string()),
        ]
    );
    assert_eq!(result.holes.len(), 2);
    let j = json_values(&result);
    let first = j
        .iter()
        .find(|d| d["code"] == "hole_in_release")
        .expect("one");
    assert_eq!(first["hole"], "build_pages");
    assert_eq!(first["expected_type"], "List<List<T>>");
    assert_eq!(first["symbol"], "pgn_7c2d");
    assert_eq!(first["category"], "release_policy");
}

/// Test and property bodies are not linked: their holes stay `hole`,
/// raised to a warning under `--release` (S47).
#[test]
fn test_body_holes_are_warnings_in_release() {
    let source = "module m.t\n\ntest \"t\" {\n    assert ?claim\n}\n\nproperty \"p\"\n    for n in gen.int\n{\n    assert n == ?other\n}\n";
    let dev = check_source(source);
    assert_eq!(
        triples(&dev),
        [
            (4, Severity::Info, "hole".to_string()),
            (10, Severity::Info, "hole".to_string()),
        ],
        "{}",
        render(&dev)
    );
    let release = check_source_with(
        source,
        &CheckConfig {
            release: true,
            ..CheckConfig::default()
        },
    );
    let holes: Vec<_> = triples(&release)
        .into_iter()
        .filter(|t| t.2 == "hole")
        .collect();
    assert_eq!(
        holes,
        [
            (4, Severity::Warning, "hole".to_string()),
            (10, Severity::Warning, "hole".to_string()),
        ]
    );
    assert_eq!(release.holes.len(), 2);
    assert_eq!(release.holes[0].expected, Ty::Bool);
    assert_eq!(release.holes[1].expected, Ty::Int);
}

/// A hole without an expected type is `hole_type_unknown` alone (S56,
/// S64): no `hole` diagnostic and no report. A hole whose expected type is
/// resolved later in its statement takes it (3.8).
#[test]
fn hole_types_are_resolved_at_the_end_of_the_statement() {
    let result = check_source(
        "module m.h\n\nfn id<T>(x: T) -> T {\n    x\n}\n\nfn f(n: Int) -> Int {\n    let _a = ?unknown\n    let b = id(?later) + n\n    b\n}\n\nfn g() -> Int {\n    let _ = id(?never)\n    1\n}\n",
    );
    assert_eq!(
        triples(&result),
        [
            (8, Severity::Error, "hole_type_unknown".to_string()),
            (9, Severity::Info, "hole".to_string()),
            (14, Severity::Error, "hole_type_unknown".to_string()),
        ],
        "{}",
        render(&result)
    );
    assert_eq!(result.holes.len(), 1);
    assert_eq!(result.holes[0].name, "later");
    assert_eq!(result.holes[0].expected, Ty::Int);
    let j = json_values(&result);
    assert_eq!(j[0]["hole"], "unknown");
    assert_eq!(j[0]["symbol"], "m.h.f");
}

/// Holes in contract expressions get no report and no `hole` diagnostic
/// (S64: the contract stage's `hole_in_contract` alone).
#[test]
fn contract_holes_are_not_reported() {
    let result = check_source(
        "module m.c\n\nfn f(n: Int) -> Int\n    requires ?pre\n{\n    var i = n\n    while i > 0\n        invariant ?inv\n    {\n        i = i - 1\n    }\n    i\n}\n",
    );
    assert!(result.holes.is_empty());
    assert!(
        result.diagnostics.iter().all(|d| d.code.0 != "hole"),
        "{}",
        render(&result)
    );
}

/// The capabilities in scope, `var` bindings, and the invariants of the
/// enclosing loops, innermost first (S48).
#[test]
fn capabilities_vars_and_invariants() {
    let result = check_source(
        "module m.k\n\npub capability Store {\n    fn get(key: Text) -> Int\n    fn put(key: Text) -> Int\n}\n\nfn f(key: Text, using db: Store) -> Int\n    requires key.len > 0\n    effects { db.get }\n{\n    var total = db.get(key)\n    while total > 0\n        invariant total >= 0\n    {\n        while total > 10\n            invariant total > 10\n            invariant total >= 0\n        {\n            total = ?step\n        }\n        total = total - 1\n    }\n    total\n}\n",
    );
    assert_eq!(result.holes.len(), 1, "{}", render(&result));
    let h = &result.holes[0];
    let p = &result.program;
    assert_eq!(h.requires, ["key.len > 0"]);
    assert_eq!(h.invariants, ["total > 10", "total >= 0", "total >= 0"]);
    let names: Vec<(&str, bool)> = h
        .bindings
        .iter()
        .map(|b| (b.name.as_str(), b.mutable))
        .collect();
    assert_eq!(names, [("total", true), ("db", false), ("key", false)]);
    assert_eq!(h.capabilities.len(), 1);
    let cap = &h.capabilities[0];
    assert_eq!(cap.parameter, "db");
    assert_eq!(p.display_path(cap.capability), "m.k.Store");
    let ops: Vec<(&str, &str, bool)> = cap
        .operations
        .iter()
        .map(|o| (o.name.as_str(), o.signature.as_str(), o.in_bound))
        .collect();
    assert_eq!(
        ops,
        [
            ("get", "fn get(key: Text) -> Int", true),
            ("put", "fn put(key: Text) -> Int", false),
        ]
    );
    let j = json_values(&result);
    let d = j.iter().find(|d| d["code"] == "hole").expect("the hole");
    assert_eq!(
        d["bindings"][0],
        json!({ "name": "total", "type": "Int", "kind": "var", "mutable": true })
    );
    assert_eq!(d["capabilities"][0]["capability"], "m.k.Store");
}

/// Holes are numbered per program and are distinct.
#[test]
fn hole_ids_are_distinct() {
    let result = check_source(
        "module m.i\n\nfn f(n: Int) -> Int {\n    let x: Int = ?todo\n    let y: Int = ?todo\n    x + y + n\n}\n",
    );
    let f = result
        .program
        .defs()
        .find(|d| result.program.display_path(d.id) == "m.i.f")
        .expect("f");
    let mut ids = Vec::new();
    if let cpl_hir::FnBody::Block(b) = &f.kind.as_fn().expect("a fn").body {
        b.walk(&mut |e| {
            if let cpl_hir::ExprKind::Hole { id, name } = &e.kind {
                ids.push((id.0, name.clone()));
            }
        });
    }
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0].0, ids[1].0);
    assert_eq!(result.holes.len(), 2);
}

/// Toolchain 12's hole-report row for `?summary` (semantics Section 11,
/// S48): `expected_type: Text`, the one parameter binding, no contracts,
/// no capabilities, blamed on `summarize`'s body (its `symbol` is the
/// provisional identity, `summarize` having no `@id`), spanning the hole.
#[test]
fn summary_hole_report_and_payload() {
    let result = holes_example(false);
    let p = &result.program;
    let summary = &result.holes[1];
    assert_eq!(summary.name, "summary");
    assert_eq!(p.display_path(summary.def), "holes.pages.summarize");
    assert_eq!(p.display_ty(&summary.expected), "Text");
    assert_eq!(summary.bindings.len(), 1);
    assert_eq!(summary.bindings[0].kind, BindingKind::Parameter);
    assert!(!summary.bindings[0].mutable);
    assert!(summary.requires.is_empty());
    assert!(summary.ensures.is_empty());
    assert!(summary.invariants.is_empty());
    assert!(summary.capabilities.is_empty());
    let deps: Vec<String> = summary
        .dependencies
        .iter()
        .map(|d| p.display_path(*d))
        .collect();
    assert!(deps.contains(&"holes.pages.Page".to_string()), "{deps:?}");

    let j = json_values(&result);
    let d = j
        .iter()
        .find(|d| d["code"] == "hole" && d["hole"] == "summary")
        .expect("the summary hole");
    assert_eq!(d["severity"], "info");
    assert_eq!(d["category"], "hole");
    assert_eq!(d["symbol"], "holes.pages.summarize");
    assert_eq!(d["qualified_name"], "holes.pages.summarize");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["expected_type"], "Text");
    assert_eq!(
        d["bindings"],
        json!([{ "name": "page", "type": "Page", "kind": "parameter" }])
    );
    assert_eq!(d["capabilities"], json!([]));
    assert_eq!(d["requires"], json!([]));
    assert_eq!(d["ensures"], json!([]));
    assert_eq!(d["invariants"], json!([]));
    assert_eq!(d["message"], "unfinished expression ?summary");
    let text =
        std::fs::read_to_string(example("holes").join("src/holes/pages.cpl")).expect("the example");
    let line = text
        .lines()
        .position(|l| l.contains("?summary"))
        .expect("the hole's line") as u32
        + 1;
    assert_eq!(d["span"]["file"], "src/holes/pages.cpl");
    assert_eq!(d["span"]["line"], json!(line));
    assert_eq!(d["span"]["col"], json!(5));
    assert_eq!(d["span"]["end_col"], json!(13));
}

/// S69 (D-247): a hole whose expected type is, or contains, the error type
/// is still reported once, as `hole` with its report, and the error type
/// renders as `?`, in `expected_type` and in `Program::display_ty`.
#[test]
fn erroneous_expected_types_render_as_question_marks() {
    let src = "module m.q

fn takes(x: Nope) -> Int {
    1
}

fn wraps(xs: List<Nope>) -> Int {
    xs.len
}

fn f() -> Int {
    takes(?h) + wraps(?g)
}
";
    let result = check_source(src);
    assert_eq!(
        triples(&result),
        [
            (3, Severity::Error, "unresolved_type".to_string()),
            (7, Severity::Error, "unresolved_type".to_string()),
            (12, Severity::Info, "hole".to_string()),
            (12, Severity::Info, "hole".to_string()),
        ],
        "{}",
        render(&result)
    );
    let p = &result.program;
    let shown: Vec<(&str, String)> = result
        .holes
        .iter()
        .map(|h| (h.name.as_str(), p.display_ty(&h.expected)))
        .collect();
    assert_eq!(
        shown,
        [("h", "?".to_string()), ("g", "List<?>".to_string())]
    );
    assert_eq!(result.holes[0].expected, Ty::Error);
    let types: Vec<serde_json::Value> = json_values(&result)
        .into_iter()
        .filter(|d| d["code"] == "hole")
        .map(|d| d["expected_type"].clone())
        .collect();
    assert_eq!(types, [json!("?"), json!("List<?>")]);
}
