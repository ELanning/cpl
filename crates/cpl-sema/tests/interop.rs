//! The export plan of a checked program (`spec/toolchain.md` 1.6, 11.1, T74,
//! T80; `spec/cabi.md` Section 5): `CheckResult::interop`, the default
//! export set, the resolution of `[interop.c] exports` entries, symbols and
//! collisions, the generic and generator skips, the listed types and
//! capabilities, the bindings, and a negative case for every code the plan
//! raises; plus the C binding checks and the C lock staleness as the check
//! driver reports them.

mod common;

use common::*;
use cpl_diag::json::Digest;
use cpl_diag::{Category, Diagnostic, Severity};
use cpl_sema::hir::{DefId, Ty};
use cpl_sema::interop::{self, snake_case, type_text, InteropPlan};
use cpl_sema::project::{
    c_input_digests, codes as project_codes, load_project, parse_lock, render_lock, AdapterC,
    AdapterForm, ForeignC, IntMapping, Lock,
};
use cpl_sema::{check_with, CheckConfig, CheckResult};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A manifest with `"c"` in `[interop] targets` and `extra` appended.
fn c_manifest(extra: &str) -> String {
    format!("{}\n[interop]\ntargets = [\"c\"]\n{extra}", manifest("t"))
}

/// Check files with the C target enabled (`extra` appended to the manifest).
fn check_c(extra: &str, files: &[(&str, &str)]) -> CheckResult {
    let manifest = c_manifest(extra);
    let mut all: Vec<(&str, &str)> = vec![("cpl.toml", manifest.as_str())];
    all.extend_from_slice(files);
    check_files(&all)
}

fn plan(result: &CheckResult) -> &InteropPlan {
    result
        .interop
        .as_ref()
        .unwrap_or_else(|| panic!("no plan:\n{}", render(result)))
}

/// The diagnostics of the C target: every `project_*` code that is not one
/// of Milestone 1's.
fn c_diagnostics(result: &CheckResult) -> Vec<&Diagnostic> {
    result
        .diagnostics
        .iter()
        .filter(|d| project_codes::PROJECT[13..].contains(&d.code))
        .collect()
}

fn c_codes(result: &CheckResult) -> Vec<&str> {
    c_diagnostics(result)
        .iter()
        .map(|d| d.code.as_str())
        .collect()
}

fn symbols(result: &CheckResult) -> Vec<String> {
    plan(result)
        .exports
        .iter()
        .map(|e| e.symbol.clone())
        .collect()
}

fn names(result: &CheckResult, defs: &[DefId]) -> Vec<String> {
    defs.iter()
        .map(|&d| result.program.display_path(d))
        .collect()
}

/// Every C-target diagnostic is span-less, of category `project`, has a
/// code of the table, and the table's severity.
fn assert_well_formed(result: &CheckResult) {
    for d in c_diagnostics(result) {
        assert!(d.span.is_none(), "{d:?}");
        assert_eq!(d.category, Category::Project, "{d:?}");
        assert_eq!(d.severity, project_codes::severity(d.code), "{d:?}");
        assert_eq!(cpl_sema::codes::category(d.code), Category::Project);
    }
}

const LIB: &str = "module app.lib

pub struct Point {
    x: Int
    y: Int
}

pub enum Shape {
    Dot(Point)
    Empty
}

pub type Count = Int where self >= 0

pub fn origin() -> Point {
    Point { x: 0, y: 0 }
}

pub fn area(shape: Shape) -> Int {
    match shape {
        Dot(_) => 0
        Empty => 0
    }
}

pub fn first<T>(items: List<T>) -> Option<T> {
    None
}

fn hidden() -> Int {
    1
}

pub fn first_int(items: List<Int>) -> Option<Bool> {
    first([true])
}

test \"origin is zero\" {
    assert origin().x == 0
}
";

// ---- the corpus ------------------------------------------------------------------

#[test]
fn the_verbatim_reader_exports_its_four_functions_and_skips_paginate() {
    let project = load_dir(&example("reader"));
    let result = check_with(&project, &CheckConfig::default());
    assert!(!result.has_errors(), "{}", render(&result));
    assert!(c_diagnostics(&result).is_empty(), "{}", render(&result));
    let plan = plan(&result);
    assert_eq!(plan.prefix, "reader");
    assert_eq!(plan.abi, 0);
    let exports: Vec<(String, String, IntMapping, Option<usize>)> = plan
        .exports
        .iter()
        .map(|e| {
            (
                e.symbol.clone(),
                result.program.display_path(e.function),
                e.int,
                e.entry,
            )
        })
        .collect();
    assert_eq!(
        exports,
        [
            (
                "reader_image_width".into(),
                "reader.images.image_width".into(),
                IntMapping::Handle,
                None
            ),
            (
                "reader_load_page".into(),
                "reader.pages.load_page".into(),
                IntMapping::Handle,
                None
            ),
            (
                "reader_load_text".into(),
                "reader.pages.load_text".into(),
                IntMapping::Handle,
                None
            ),
            (
                "reader_paginate_ints".into(),
                "reader.pages.paginate_ints".into(),
                IntMapping::Handle,
                None
            ),
        ]
    );
    assert_eq!(names(&result, &plan.skipped), ["reader.pages.paginate"]);
    assert_eq!(
        names(&result, &plan.capabilities),
        ["reader.images.ImageCodec", "reader.pages.PageRead"]
    );
    // Its one binding has backend python.
    assert!(plan.bindings.is_empty());
    // The listed types of spec/cabi.md Appendix C.1, in that order.
    let types: Vec<(&str, &str)> = plan
        .types
        .iter()
        .map(|t| (t.text.as_str(), t.c_name.as_str()))
        .collect();
    assert_eq!(
        types,
        [
            ("List<Int>", "list_int"),
            ("List<List<Int>>", "list_list_int"),
            (
                "Result<reader.images.Dimensions, std.foreign.ForeignError>",
                "result_dimensions_foreign_error"
            ),
            (
                "Result<reader.pages.Page, reader.pages.StoreError>",
                "result_page_store_error"
            ),
            ("reader.images.Dimensions", "dimensions"),
            ("reader.images.ImageCodec", "image_codec"),
            ("reader.pages.Page", "page"),
            ("reader.pages.PageId", "page_id"),
            ("reader.pages.PageRead", "page_read"),
            ("reader.pages.Positive", "positive"),
            ("reader.pages.StoreError", "store_error"),
        ]
    );
    for t in &plan.types {
        assert_eq!(type_text(&result.program, &t.ty), t.text);
    }
    // project_export_skipped_generic is cpl build's, never cpl check's.
    assert!(!result
        .diagnostics
        .iter()
        .any(|d| d.code == project_codes::PROJECT_EXPORT_SKIPPED_GENERIC));
    let infos = plan.skipped_diagnostics(&result.program);
    assert_eq!(infos.len(), 1);
    let info = &infos[0];
    assert_eq!(info.code, project_codes::PROJECT_EXPORT_SKIPPED_GENERIC);
    assert_eq!(info.severity, Severity::Info);
    assert_eq!(info.category, Category::Project);
    assert!(info.span.is_none() && info.symbol.is_none());
    let v = serde_json::to_value(info.to_json(result.program.source_map())).unwrap();
    assert_eq!(v["function"], json!("reader.pages.paginate"));
    assert_eq!(
        v.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["category", "code", "function", "message", "severity"]
    );
}

#[test]
fn a_project_without_the_c_target_has_no_plan() {
    let project = load_dir(&example("holes"));
    let result = check_with(&project, &CheckConfig::default());
    assert!(!project.manifest.interop.targets.iter().any(|t| t == "c"));
    assert_eq!(result.interop, None);
    let result = check_files(&[("src/app/lib.cpl", LIB)]);
    assert_eq!(result.interop, None);
}

#[test]
fn a_project_that_does_not_parse_has_no_plan() {
    let result = check_c(
        "",
        &[("src/app/lib.cpl", "module app.lib\n\npub fn f( -> Int {\n")],
    );
    assert!(result.has_errors());
    assert_eq!(result.interop, None);
}

// ---- the default export set ------------------------------------------------------

#[test]
fn the_default_set_is_every_pub_non_generic_project_function_in_qualified_name_order() {
    let result = check_c(
        "",
        &[
            ("src/app/lib.cpl", LIB),
            (
                "src/app/aaa.cpl",
                "module app.aaa\n\npub fn zed() -> Int {\n    1\n}\n",
            ),
        ],
    );
    assert!(!result.has_errors(), "{}", render(&result));
    assert_well_formed(&result);
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
    let plan = plan(&result);
    assert_eq!(
        symbols(&result),
        ["t_zed", "t_area", "t_first_int", "t_origin"]
    );
    assert!(plan
        .exports
        .iter()
        .all(|e| e.entry.is_none() && e.int == IntMapping::Handle));
    // The private `hidden`, the test, and std functions are not exported; the
    // generic `first` is skipped.
    assert_eq!(names(&result, &plan.skipped), ["app.lib.first"]);
    assert!(plan.capabilities.is_empty());
    let texts: Vec<&str> = plan.types.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "List<Int>",
            "Option<Bool>",
            "app.lib.Point",
            "app.lib.Shape"
        ]
    );
    let c_names: Vec<&str> = plan.types.iter().map(|t| t.c_name.as_str()).collect();
    assert_eq!(c_names, ["list_int", "option_bool", "point", "shape"]);
    assert!(matches!(plan.types[2].ty, Ty::Adt(_, ref args) if args.is_empty()));
}

#[test]
fn a_definition_with_errors_is_left_out_without_a_diagnostic() {
    let result = check_c(
        "",
        &[(
            "src/app/lib.cpl",
            "module app.lib\n\npub fn good() -> Int {\n    1\n}\n\npub fn bad() -> Int {\n    \"text\"\n}\n",
        )],
    );
    assert!(result.has_errors());
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
    assert_eq!(symbols(&result), ["t_good"]);
}

#[test]
fn std_generators_and_generic_functions_are_never_exported() {
    let result = check_c(
        "\n[interop.c]\nexports = [\"std.gen.int\", \"std.text.trim\", \"app.lib.first\"]\n",
        &[("src/app/lib.cpl", LIB)],
    );
    assert_well_formed(&result);
    let found: Vec<(String, serde_json::Value)> = c_diagnostics(&result)
        .iter()
        .map(|d| {
            (
                d.code.as_str().to_string(),
                serde_json::Value::Object(d.data.clone()),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "project_export_not_function".to_string(),
                json!({ "entry": "std.gen.int", "kind": "generator" })
            ),
            (
                "project_export_not_project".to_string(),
                json!({ "entry": "std.text.trim" })
            ),
            (
                "project_export_generic".to_string(),
                json!({ "entry": "app.lib.first" })
            ),
        ]
    );
    let generic = c_diagnostics(&result)[2];
    assert_eq!(generic.notes.len(), 1);
    assert!(
        generic.notes[0]
            .message
            .contains("such as app.lib.first_int"),
        "{:?}",
        generic.notes
    );
    assert!(plan(&result).exports.is_empty());
    assert!(
        plan(&result).skipped.is_empty(),
        "skipped is the default set's"
    );
}

#[test]
fn a_generic_export_without_monomorphic_callers_suggests_writing_one() {
    let result = check_c(
        "\n[interop.c]\nexports = [\"app.lib.id\"]\n",
        &[(
            "src/app/lib.cpl",
            "module app.lib\n\npub fn id<T>(x: T) -> T {\n    x\n}\n\npub fn twice<T>(x: T) -> T {\n    id(x)\n}\n",
        )],
    );
    let d = c_diagnostics(&result);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, project_codes::PROJECT_EXPORT_GENERIC);
    assert!(
        d[0].notes[0]
            .message
            .contains("write a monomorphic wrapper"),
        "{:?}",
        d[0].notes
    );
}

// ---- explicit exports ---------------------------------------------------------------

#[test]
fn explicit_entries_resolve_by_every_target_form_in_manifest_order() {
    let lib = "module app.lib\n\n@id(\"fn_origin_id\")\npub fn origin() -> Int {\n    0\n}\n\npub fn area(n: Int) -> Int {\n    n\n}\n\npub fn scale(n: Int) -> Result<Int, ConstraintError> {\n    Ok(n)\n}\n";
    let result = check_c(
        r#"
[interop.c]
exports = [
    "app.lib.scale",
    "area",
    "fn_origin_id",
    { function = "app.lib.area", symbol = "t_area_i64", int = "int64" },
    { function = "scale", symbol = "t_scale_u32", int = "uint32" },
]
"#,
        &[("src/app/lib.cpl", lib)],
    );
    assert!(!result.has_errors(), "{}", render(&result));
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
    let plan = plan(&result);
    let got: Vec<(String, String, IntMapping, Option<usize>)> = plan
        .exports
        .iter()
        .map(|e| {
            (
                e.symbol.clone(),
                result.program.display_path(e.function),
                e.int,
                e.entry,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "t_scale".into(),
                "app.lib.scale".into(),
                IntMapping::Handle,
                Some(0)
            ),
            (
                "t_area".into(),
                "app.lib.area".into(),
                IntMapping::Handle,
                Some(1)
            ),
            (
                "t_origin".into(),
                "app.lib.origin".into(),
                IntMapping::Handle,
                Some(2)
            ),
            (
                "t_area_i64".into(),
                "app.lib.area".into(),
                IntMapping::Int64,
                Some(3)
            ),
            (
                "t_scale_u32".into(),
                "app.lib.scale".into(),
                IntMapping::Uint32,
                Some(4)
            ),
        ]
    );
    assert!(plan.skipped.is_empty());
}

#[test]
fn project_export_unknown_and_ambiguous() {
    let result = check_c(
        "\n[interop.c]\nexports = [\"app.lib.nothing\", \"nope\", \"size\"]\n",
        &[
            (
                "src/app/lib.cpl",
                "module app.lib\n\npub fn size() -> Int {\n    1\n}\n",
            ),
            (
                "src/app/other.cpl",
                "module app.other\n\npub fn size() -> Int {\n    2\n}\n",
            ),
        ],
    );
    assert_well_formed(&result);
    let found: Vec<(&str, serde_json::Value)> = c_diagnostics(&result)
        .iter()
        .map(|d| (d.code.as_str(), serde_json::Value::Object(d.data.clone())))
        .collect();
    assert_eq!(
        found,
        [
            (
                "project_export_unknown",
                json!({ "entry": "app.lib.nothing", "index": 0 })
            ),
            (
                "project_export_unknown",
                json!({ "entry": "nope", "index": 1 })
            ),
            (
                "project_export_ambiguous",
                json!({ "entry": "size", "candidates": ["app.lib.size", "app.other.size"] })
            ),
        ]
    );
}

#[test]
fn project_export_not_function_names_the_kind() {
    let result = check_c(
        "\n[interop.c]\nexports = [\"app.lib.Point\", \"app.lib.Count\", \"app.lib.Shape\", \"app.lib.\\\"origin is zero\\\"\"]\n",
        &[("src/app/lib.cpl", LIB)],
    );
    let found: Vec<serde_json::Value> = c_diagnostics(&result)
        .iter()
        .map(|d| {
            assert_eq!(d.code, project_codes::PROJECT_EXPORT_NOT_FUNCTION);
            d.data["kind"].clone()
        })
        .collect();
    assert_eq!(
        found,
        [json!("ty"), json!("ty"), json!("ty"), json!("test")]
    );
    let result = check_c(
        "\n[interop.c]\nexports = [\"app.lib.Codec\"]\n",
        &[(
            "src/app/lib.cpl",
            "module app.lib\n\npub capability Codec {\n    fn size(image: Bytes) -> Int\n}\n",
        )],
    );
    assert_eq!(c_diagnostics(&result)[0].data["kind"], json!("cap"));
}

#[test]
fn project_export_not_public() {
    let result = check_c(
        "\n[interop.c]\nexports = [\"app.lib.hidden\"]\n",
        &[("src/app/lib.cpl", LIB)],
    );
    let d = c_diagnostics(&result);
    assert_eq!(c_codes(&result), ["project_export_not_public"]);
    assert_eq!(
        serde_json::Value::Object(d[0].data.clone()),
        json!({ "entry": "app.lib.hidden" })
    );
}

#[test]
fn project_export_symbol_invalid_for_patterns_and_reserved_families() {
    let lib = "module app.lib\n\npub fn f() -> Int {\n    1\n}\n";
    let long = format!("t_{}", "a".repeat(57));
    let ok_long = format!("t_{}", "a".repeat(56));
    let entries = [
        ("other_f", "does not match ^t_[a-z0-9_]{1,56}$"),
        ("t_", "does not match"),
        ("t_F", "does not match"),
        (long.as_str(), "does not match"),
        ("t_runtime_open", "reserved family t_runtime_*"),
        ("t_library_x", "reserved family t_library_*"),
        ("t_type_f", "reserved family t_type_*"),
        ("t_bind_f", "reserved family t_bind_*"),
        ("t_adapter_f", "reserved family t_adapter_*"),
    ];
    for (symbol, reason) in entries {
        let result = check_c(
            &format!("\n[interop.c]\nexports = [{{ function = \"f\", symbol = \"{symbol}\" }}]\n"),
            &[("src/app/lib.cpl", lib)],
        );
        assert_eq!(
            c_codes(&result),
            ["project_export_symbol_invalid"],
            "{symbol}"
        );
        let d = c_diagnostics(&result)[0];
        assert_eq!(d.data["c_symbol"], json!(symbol));
        assert!(
            d.data["reason"].as_str().unwrap().contains(reason),
            "{symbol}: {:?}",
            d.data["reason"]
        );
        assert!(plan(&result).exports.is_empty());
    }
    let result = check_c(
        &format!("\n[interop.c]\nexports = [{{ function = \"f\", symbol = \"{ok_long}\" }}, {{ function = \"f\", symbol = \"t_types\" }}]\n"),
        &[("src/app/lib.cpl", lib)],
    );
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
    // The default symbol of the default set is checked too.
    let result = check_c(
        "",
        &[(
            "src/app/lib.cpl",
            "module app.lib\n\npub fn getValue() -> Int {\n    1\n}\n\npub fn type_of() -> Int {\n    1\n}\n",
        )],
    );
    let found: Vec<serde_json::Value> = c_diagnostics(&result)
        .iter()
        .map(|d| d.data["c_symbol"].clone())
        .collect();
    assert_eq!(found, [json!("t_getValue"), json!("t_type_of")]);
    assert!(plan(&result).exports.is_empty());
}

#[test]
fn project_export_symbol_collision_names_both_items() {
    // Two exports of one name in different modules.
    let result = check_c(
        "",
        &[
            (
                "src/a/x.cpl",
                "module a.x\n\npub fn size() -> Int {\n    1\n}\n",
            ),
            (
                "src/b/y.cpl",
                "module b.y\n\npub fn size() -> Int {\n    2\n}\n",
            ),
        ],
    );
    assert_well_formed(&result);
    assert_eq!(c_codes(&result), ["project_export_symbol_collision"]);
    let d = c_diagnostics(&result)[0];
    assert_eq!(d.data["c_symbol"], json!("t_size"));
    assert_eq!(
        d.data["items"],
        json!(["export a.x.size", "export b.y.size"])
    );
    // An explicit symbol resolves it.
    let result = check_c(
        "\n[interop.c]\nexports = [\"a.x.size\", { function = \"b.y.size\", symbol = \"t_size_b\" }]\n",
        &[
            ("src/a/x.cpl", "module a.x\n\npub fn size() -> Int {\n    1\n}\n"),
            ("src/b/y.cpl", "module b.y\n\npub fn size() -> Int {\n    2\n}\n"),
        ],
    );
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
    // One function listed twice without a symbol.
    let result = check_c(
        "\n[interop.c]\nexports = [\"a.x.size\", { function = \"a.x.size\", int = \"int64\" }]\n",
        &[(
            "src/a/x.cpl",
            "module a.x\n\npub fn size() -> Int {\n    1\n}\n",
        )],
    );
    assert_eq!(c_codes(&result), ["project_export_symbol_collision"]);
    assert_eq!(
        c_diagnostics(&result)[0].data["items"],
        json!([
            "export a.x.size (exports[0])",
            "export a.x.size (exports[1])"
        ])
    );
}

#[test]
fn an_export_named_like_a_capability_constructor_collides() {
    let lib = "module app.lib\n\npub capability Codec {\n    fn size(image: Bytes) -> Int\n}\n\npub fn measure(image: Bytes, using codec: Codec) -> Int\n    effects { codec.size }\n{\n    codec.size(image)\n}\n\npub fn codec_new() -> Int {\n    1\n}\n";
    let result = check_c("", &[("src/app/lib.cpl", lib)]);
    assert_eq!(c_codes(&result), ["project_export_symbol_collision"]);
    let d = c_diagnostics(&result)[0];
    assert_eq!(d.data["c_symbol"], json!("t_codec_new"));
    assert_eq!(
        d.data["items"],
        json!([
            "constructor of capability app.lib.Codec",
            "export app.lib.codec_new"
        ])
    );
    let plan = plan(&result);
    assert_eq!(names(&result, &plan.capabilities), ["app.lib.Codec"]);
    assert!(plan
        .types
        .iter()
        .any(|t| t.text == "app.lib.Codec" && t.c_name == "codec"));
}

#[test]
fn two_types_with_one_bare_name_collide_until_type_names_renames_one() {
    let files = [
        ("src/a/x.cpl", "module a.x\n\npub struct Page {\n    n: Int\n}\n\npub fn page_a() -> Page {\n    Page { n: 1 }\n}\n"),
        ("src/b/y.cpl", "module b.y\n\npub struct Page {\n    m: Int\n}\n\npub fn page_b() -> Page {\n    Page { m: 1 }\n}\n"),
    ];
    let result = check_c("", &files);
    assert_eq!(c_codes(&result), ["project_export_symbol_collision"]);
    let d = c_diagnostics(&result)[0];
    assert_eq!(d.data["c_symbol"], json!("t_type_page"));
    assert_eq!(
        d.data["items"],
        json!(["type accessor of a.x.Page", "type accessor of b.y.Page"])
    );
    let result = check_c(
        "\n[interop.c]\ntype_names = { \"b.y.Page\" = \"page_b\" }\n",
        &files,
    );
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
    let c_names: Vec<(&str, &str)> = plan(&result)
        .types
        .iter()
        .map(|t| (t.text.as_str(), t.c_name.as_str()))
        .collect();
    assert_eq!(c_names, [("a.x.Page", "page"), ("b.y.Page", "page_b")]);
}

#[test]
fn field_and_variant_constants_collide_by_their_upper_case_names() {
    let result = check_c(
        "",
        &[(
            "src/a/x.cpl",
            "module a.x\n\npub enum Status {\n    IoError\n    IOError\n}\n\npub fn status() -> Status {\n    IoError\n}\n",
        )],
    );
    assert_eq!(c_codes(&result), ["project_export_symbol_collision"]);
    let d = c_diagnostics(&result)[0];
    assert_eq!(d.data["c_symbol"], json!("T_STATUS_V_IO_ERROR"));
    assert_eq!(
        d.data["items"],
        json!([
            "variant IoError of a.x.Status",
            "variant IOError of a.x.Status"
        ])
    );
}

#[test]
fn project_export_int_unused_is_a_warning() {
    let lib = "module app.lib\n\npub type Count = Int where self >= 0\n\npub fn name(t: Text) -> Text {\n    t\n}\n\npub fn count(n: Count) -> Bool {\n    n > 0\n}\n\npub fn parse(t: Text) -> Result<Count, ConstraintError> {\n    check<Count>(0)\n}\n\npub fn flags(xs: List<Int>) -> Bool {\n    true\n}\n";
    let result = check_c(
        r#"
[interop.c]
exports = [
    { function = "name", int = "int64" },
    { function = "count", int = "int32" },
    { function = "parse", symbol = "t_parse_u64", int = "uint64" },
    { function = "flags", symbol = "t_flags_u32", int = "uint32" },
    { function = "flags", symbol = "t_flags_handle", int = "handle" },
]
"#,
        &[("src/app/lib.cpl", lib)],
    );
    assert!(!result.has_errors(), "{}", render(&result));
    assert_well_formed(&result);
    let found: Vec<(&str, Severity, serde_json::Value)> = c_diagnostics(&result)
        .iter()
        .map(|d| (d.code.as_str(), d.severity, d.data["entry"].clone()))
        .collect();
    assert_eq!(
        found,
        [
            (
                "project_export_int_unused",
                Severity::Warning,
                json!("name")
            ),
            (
                "project_export_int_unused",
                Severity::Warning,
                json!("flags")
            ),
        ]
    );
    // The entries are exported anyway.
    assert_eq!(plan(&result).exports.len(), 5);
}

#[test]
fn project_type_name_unknown() {
    let result = check_c(
        "\n[interop.c]\ntype_names = { \"app.lib.Nowhere\" = \"x\", \"List<Int>\" = \"ints\", \"app.lib.Point\" = \"pt\" }\n",
        &[("src/app/lib.cpl", LIB)],
    );
    assert_eq!(c_codes(&result), ["project_type_name_unknown"]);
    assert_eq!(
        serde_json::Value::Object(c_diagnostics(&result)[0].data.clone()),
        json!({ "key": "app.lib.Nowhere" })
    );
    let c_names: BTreeMap<&str, &str> = plan(&result)
        .types
        .iter()
        .map(|t| (t.text.as_str(), t.c_name.as_str()))
        .collect();
    assert_eq!(c_names["List<Int>"], "ints");
    assert_eq!(c_names["app.lib.Point"], "pt");
}

// ---- bindings ---------------------------------------------------------------------------

/// A project directory under `CARGO_TARGET_TMPDIR` with `files`.
fn project_dir(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cpl-sema-interop")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    for (rel, bytes) in files {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    dir
}

const CODEC: &str = "module app.media\n\npub capability Codec {\n    fn init(image: Bytes) -> Int\n    fn size(image: Bytes) -> Int\n}\n\npub fn measure(image: Bytes, using codec: Codec) -> Int\n    effects { codec.size }\n{\n    codec.size(image)\n}\n";

#[test]
fn a_backend_c_binding_is_planned_and_found_by_its_capability() {
    let manifest = c_manifest(
        "\n[bindings.codec]\ncapability = \"app.media.Codec\"\nbackend = \"c\"\nadapter = \"adapters/codec.c\"\nisolation = \"in_process\"\n",
    );
    let dir = project_dir(
        "binding",
        &[
            ("cpl.toml", manifest.as_bytes()),
            ("src/app/media.cpl", CODEC.as_bytes()),
            ("adapters/codec.c", b"/* adapter */\n"),
        ],
    );
    let result = check_with(&load_dir(&dir), &CheckConfig::default());
    assert!(!result.has_errors(), "{}", render(&result));
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
    let plan = plan(&result);
    assert_eq!(plan.bindings.len(), 1);
    let binding = &plan.bindings[0];
    assert_eq!(binding.name, "codec");
    assert_eq!(binding.form, AdapterForm::Wrapper);
    assert_eq!(binding.adapter, "adapters/codec.c");
    assert_eq!(binding.cbind, None);
    assert_eq!(
        result.program.display_path(binding.capability),
        "app.media.Codec"
    );
    assert_eq!(plan.binding_for(binding.capability), Some(binding));
    let measure = plan.exports[0].function;
    assert_eq!(plan.binding_for(measure), None);
    assert_eq!(names(&result, &plan.capabilities), ["app.media.Codec"]);
}

#[test]
fn a_capability_bound_but_not_exported_is_listed() {
    let lib =
        "module app.media\n\npub capability Codec {\n    fn size(image: Bytes) -> Option<Int>\n}\n";
    let manifest = c_manifest(
        "\n[bindings.codec]\ncapability = \"app.media.Codec\"\nbackend = \"c\"\nadapter = \"adapters/codec.c\"\nisolation = \"in_process\"\n",
    );
    let result = check_files(&[("cpl.toml", manifest.as_str()), ("src/app/media.cpl", lib)]);
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
    let plan = plan(&result);
    assert!(plan.exports.is_empty());
    assert_eq!(names(&result, &plan.capabilities), ["app.media.Codec"]);
    let texts: Vec<&str> = plan.types.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["Option<Int>", "app.media.Codec"]);
}

#[test]
fn adapter_contract_names_follow_the_underscore_rule_and_collide_across_bindings() {
    // Binding `x`'s operation `y_init` and binding `x_y`'s `init` (cabi.md 5.2).
    let lib = "module app.media\n\npub capability A {\n    fn y_init() -> Int\n}\n\npub capability B {\n    fn ping() -> Int\n}\n";
    let manifest = c_manifest(
        "\n[bindings.x]\ncapability = \"app.media.A\"\nbackend = \"c\"\nadapter = \"x.c\"\nisolation = \"in_process\"\n\n[bindings.x_y]\ncapability = \"app.media.B\"\nbackend = \"c\"\nadapter = \"y.c\"\nisolation = \"in_process\"\n",
    );
    let result = check_files(&[("cpl.toml", manifest.as_str()), ("src/app/media.cpl", lib)]);
    assert_eq!(
        c_codes(&result),
        ["project_export_symbol_collision"],
        "{}",
        render(&result)
    );
    let d = c_diagnostics(&result)[0];
    assert_eq!(d.data["c_symbol"], json!("t_adapter_x_y_init"));
    assert_eq!(
        d.data["items"],
        json!([
            "adapter operation y_init of binding x",
            "adapter init of binding x_y"
        ])
    );
    // An operation named `init` is suffixed and collides with nothing.
    let manifest = c_manifest(
        "\n[bindings.codec]\ncapability = \"app.media.Codec\"\nbackend = \"c\"\nadapter = \"codec.c\"\nisolation = \"in_process\"\n",
    );
    let result = check_files(&[
        ("cpl.toml", manifest.as_str()),
        ("src/app/media.cpl", CODEC),
    ]);
    assert!(c_codes(&result).is_empty(), "{}", render(&result));
}

#[test]
fn the_c_binding_checks_run_in_the_check_driver_after_the_binding_checks() {
    let manifest = format!(
        "{}\n[interop]\ntargets = []\n\n[interop.c]\nabi = 1\n\n[bindings.Codec]\ncapability = \"app.media.Nope\"\nbackend = \"c\"\nadapter = \"x.h\"\n",
        manifest("t")
    );
    let result = check_files(&[
        ("cpl.toml", manifest.as_str()),
        ("src/app/media.cpl", CODEC),
    ]);
    // from Milestone 3 the python target also plans the exports (spec/python.md 1), so none of them here
    assert_eq!(
        result.interop, None,
        "no plan without \"c\" or \"python\" in targets"
    );
    let project: Vec<&str> = result
        .diagnostics
        .iter()
        .filter(|d| d.category == Category::Project)
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(
        project,
        [
            "project_binding_unknown_capability",
            "project_binding_target_not_enabled",
            "project_c_target_not_enabled",
            "project_binding_name_invalid",
            "project_binding_adapter_kind",
        ]
    );
    assert_well_formed(&result);
}

// ---- the release lock check over the C build inputs (T75) -----------------------------------

fn d(c: char) -> Digest {
    Digest(std::iter::repeat_n(c, 64).collect())
}

/// A lock that matches the project's sources, verification, and std, with
/// `with_c` applied (built from text: `Lock`'s reserved tables are `toml`
/// tables, a crate this one does not depend on).
fn write_matching_lock(dir: &Path, with_c: impl FnOnce(&mut Lock)) {
    let project = load_dir(dir);
    let mut sources = BTreeMap::new();
    sources.insert("cpl.toml".to_string(), project.manifest_digest.clone());
    for entry in &project.files {
        if entry.origin != cpl_diag::FileOrigin::Std {
            sources.insert(
                project.source_map.file(entry.file).path.clone(),
                entry.digest.clone(),
            );
        }
    }
    let mut text = format!(
        "version = 1\n\n[toolchain]\ncpl = \"{}\"\nedition = \"{}\"\nstd = \"{}\"\n\n[sources]\n",
        cpl_diag::version::TOOLCHAIN,
        cpl_diag::version::EDITION,
        project.std_digest().0
    );
    for (path, digest) in &sources {
        text.push_str(&format!("\"{path}\" = \"{}\"\n", digest.0));
    }
    text.push_str(&format!(
        "\n[verification]\nproof_timeout_ms = {}\nadmit = []\n",
        project.manifest.verification.proof_timeout_ms
    ));
    let mut lock = parse_lock(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    with_c(&mut lock);
    let text = render_lock(&lock);
    parse_lock(&text).unwrap();
    fs::write(dir.join("cpl.lock"), text).unwrap();
}

fn release(dir: &Path) -> CheckResult {
    let project = load_project(dir, &check_options()).unwrap();
    check_with(
        &project,
        &CheckConfig {
            release: true,
            ..CheckConfig::default()
        },
    )
}

fn stale(result: &CheckResult) -> Option<&Diagnostic> {
    result
        .diagnostics
        .iter()
        .find(|d| d.code == project_codes::PROJECT_LOCK_STALE)
}

#[test]
fn a_lock_with_c_entries_is_stale_when_an_adapter_or_vendored_source_changes() {
    let manifest = c_manifest(
        "\n[bindings.codec]\ncapability = \"app.media.Codec\"\nbackend = \"c\"\nadapter = \"adapters/codec.c\"\nisolation = \"in_process\"\n\n[bindings.codec.c]\nsources = [\"vendor/lib.c\"]\n",
    );
    let dir = project_dir(
        "lock_c_stale",
        &[
            ("cpl.toml", manifest.as_bytes()),
            ("src/app/media.cpl", CODEC.as_bytes()),
            ("adapters/codec.c", b"/* v1 */\n"),
            ("vendor/lib.c", b"int lib;\n"),
        ],
    );
    let current = c_input_digests(&load_dir(&dir).manifest, &dir);
    let adapter_now = current.adapters["codec"]["adapters/codec.c"].clone();
    let source_now = current.sources["codec"]["vendor/lib.c"].clone();
    let locked = |adapter_digest: Digest, source_digest: Digest| {
        move |lock: &mut Lock| {
            lock.set_c_adapters(BTreeMap::from([(
                "codec".to_string(),
                AdapterC {
                    form: AdapterForm::Wrapper,
                    isolation: "in_process".to_string(),
                    capability: "app.media.Codec".to_string(),
                    files: BTreeMap::from([("adapters/codec.c".to_string(), adapter_digest)]),
                },
            )]));
            lock.set_foreign_c(BTreeMap::from([(
                "codec".to_string(),
                ForeignC {
                    sources: BTreeMap::from([("vendor/lib.c".to_string(), source_digest)]),
                    headers: d('e'),
                    libraries: BTreeMap::new(),
                },
            )]));
        }
    };
    // A lock that matches: no lock diagnostic.
    write_matching_lock(&dir, locked(adapter_now.clone(), source_now.clone()));
    let result = release(&dir);
    assert!(stale(&result).is_none(), "{}", render(&result));
    assert!(!result
        .diagnostics
        .iter()
        .any(|d| d.code == project_codes::PROJECT_LOCK_MISSING));
    // The adapter changed.
    write_matching_lock(&dir, locked(d('0'), source_now.clone()));
    let result = release(&dir);
    let d1 = stale(&result).expect("project_lock_stale");
    assert_eq!(d1.data["differences"], json!(["adapters"]));
    assert_eq!(d1.data["paths"], json!(["adapters/codec.c"]));
    assert!(d1.message.contains("cpl build --lock"), "{}", d1.message);
    // Both changed.
    write_matching_lock(&dir, locked(d('0'), d('1')));
    let result = release(&dir);
    let d2 = stale(&result).expect("project_lock_stale");
    assert_eq!(d2.data["differences"], json!(["adapters", "foreign.c"]));
    assert_eq!(
        d2.data["paths"],
        json!(["adapters/codec.c", "vendor/lib.c"])
    );
    // A binding the lock records but the manifest no longer has contributes
    // all of its files; development mode compares nothing.
    write_matching_lock(&dir, |lock| {
        locked(adapter_now.clone(), source_now.clone())(lock);
        let mut adapters = lock.c_adapters();
        adapters.insert(
            "gone".to_string(),
            AdapterC {
                form: AdapterForm::Declarative,
                isolation: "in_process".to_string(),
                capability: "app.media.Gone".to_string(),
                files: BTreeMap::from([("adapters/gone.cbind.toml".to_string(), d('f'))]),
            },
        );
        lock.set_c_adapters(adapters);
    });
    let result = release(&dir);
    let d3 = stale(&result).expect("project_lock_stale");
    assert_eq!(d3.data["differences"], json!(["adapters"]));
    assert_eq!(d3.data["paths"], json!(["adapters/gone.cbind.toml"]));
    let development = check_with(&load_dir(&dir), &CheckConfig::default());
    assert!(stale(&development).is_none());
}

#[test]
fn a_lock_without_c_entries_compares_no_c_input() {
    let manifest = c_manifest(
        "\n[bindings.codec]\ncapability = \"app.media.Codec\"\nbackend = \"c\"\nadapter = \"adapters/codec.c\"\nisolation = \"in_process\"\n",
    );
    let dir = project_dir(
        "lock_c_none",
        &[
            ("cpl.toml", manifest.as_bytes()),
            ("src/app/media.cpl", CODEC.as_bytes()),
            ("adapters/codec.c", b"/* v1 */\n"),
        ],
    );
    write_matching_lock(&dir, |_| {});
    let result = release(&dir);
    assert!(stale(&result).is_none(), "{}", render(&result));
}

// ---- helpers ----------------------------------------------------------------------------------

#[test]
fn snake_case_follows_cabi_5_2() {
    for (name, snake) in [
        ("PageId", "page_id"),
        ("IOError", "io_error"),
        ("Page", "page"),
        ("A", "a"),
        ("ABC", "abc"),
        ("HTTPServer", "http_server"),
        ("Page2Id", "page2_id"),
        ("X1Y", "x1_y"),
        ("ForeignError", "foreign_error"),
        ("already_snake", "already_snake"),
    ] {
        assert_eq!(snake_case(name), snake, "{name}");
    }
}

#[test]
fn the_empty_plan_of_a_driver() {
    let plan = InteropPlan::empty("reader");
    assert_eq!(plan.prefix, "reader");
    assert_eq!(plan.abi, 0);
    assert!(plan.exports.is_empty() && plan.skipped.is_empty() && plan.types.is_empty());
    assert!(plan.capabilities.is_empty() && plan.bindings.is_empty());
    for name in [
        "cpl",
        "cpl_tool",
        "a__b",
        "a_",
        "abcdefghijklmnopqrstuvwxyz012345",
    ] {
        assert_eq!(InteropPlan::empty(name).prefix, "driver", "{name}");
    }
    assert_eq!(plan.binding_for(DefId(0)), None);
}

#[test]
fn type_text_names_prelude_declarations_bare_and_every_other_qualified() {
    let result = check_files(&[("src/app/lib.cpl", LIB)]);
    let program = &result.program;
    let point = program
        .lookup(&cpl_diag::SymbolPath::parse("app.lib.Point").unwrap())
        .unwrap();
    let option = program.std_defs().option;
    let result_def = program.std_defs().result;
    let foreign = program.std_defs().foreign_error;
    assert_eq!(type_text(program, &Ty::Adt(point, vec![])), "app.lib.Point");
    assert_eq!(
        type_text(program, &Ty::Adt(option, vec![Ty::List(Box::new(Ty::Int))])),
        "Option<List<Int>>"
    );
    assert_eq!(
        type_text(
            program,
            &Ty::Adt(result_def, vec![Ty::Bytes, Ty::Adt(foreign, vec![])])
        ),
        "Result<Bytes, std.foreign.ForeignError>"
    );
    assert_eq!(type_text(program, &Ty::Param(0)), "$0");
    assert_eq!(type_text(program, &Ty::Error), "?");
    // The plan function is callable on a finished program.
    let (plan, diagnostics) = interop::plan(
        program,
        &load_tree(&tree(&[
            ("cpl.toml", c_manifest("").as_str()),
            ("src/app/lib.cpl", LIB),
        ]))
        .manifest,
        &BTreeMap::new(),
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(plan.prefix, "t");
}
