//! Declarative bindings end to end (`spec/cabi.md` 6.3, 6.5; `spec/stdlib.md` S45; `spec/semantics.md` S80): the
//! fixture `cshapes` of `crates/cpl-cabi/tests/projects`, whose generated wrapper covers the vocabulary beyond
//! `cthumbs` (struct arguments and outputs, `cstr`, `text_span`, `owned_text`, `bool` arguments and results, `void`,
//! `uint64` and `uint8` statuses, a `message` function), checked, built, and run by `cpl run --engine both`, which
//! compares the evaluator with the native driver and fails with `engine_divergence` when they differ.

mod common;

use common::*;
use serde_json::Value;
use std::path::PathBuf;

fn require_cc() -> bool {
    if cpl_rt::testing::find_cc().is_some() {
        return true;
    }
    if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
        panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
    }
    println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
    false
}

fn cshapes() -> PathBuf {
    let dir = temp_dir("cshapes");
    copy_dir(
        &workspace().join("crates/cpl-cabi/tests/projects/cshapes"),
        &dir,
    );
    dir
}

/// The rendering of `cpl run --engine both --json` (its `result_text`, or the fault's code and message).
fn run(dir: &std::path::Path, args: &[&str]) -> (i32, String) {
    let mut all = vec!["run", "--engine", "both", "--json"];
    all.extend_from_slice(args);
    let r = cpl(dir, &all);
    let envelope: Value = r.json();
    let divergence = envelope["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|d| d["code"] == "engine_divergence");
    assert!(!divergence, "the engines diverge on {args:?}: {envelope:#}");
    let text = match envelope["result_text"].as_str() {
        Some(t) => t.to_string(),
        None => format!(
            "fault {} {}",
            envelope["fault"]["kind"].as_str().unwrap_or_default(),
            envelope["fault"]["message"].as_str().unwrap_or_default()
        ),
    };
    (r.code, text)
}

const DETAILS_SCALE: &str = r#"ForeignDetail { key: "adapter", value: "shapes" }, ForeignDetail { key: "capability", value: "app.shapes.Shapes" }, ForeignDetail { key: "operation", value: "app.shapes.Shapes.scale" }, ForeignDetail { key: "foreign_type", value: "shp_scale" }"#;

#[test]
fn the_declarative_vocabulary_agrees_on_both_engines() {
    if !require_cc() {
        return;
    }
    let dir = cshapes();
    cpl(&dir, &["check"]).expect_code(0);
    cpl(&dir, &["build"]).expect_code(0);
    assert!(dir.join(".cpl/build/c/src/geom_bind_shapes.c").is_file());
    let size = "Size { w: 3, h: 4, flag: true }";
    let cases: Vec<(Vec<&str>, i32, String)> = vec![
        // a struct in and out, a cstr, a bool argument; two ok statuses
        (vec!["app.shapes.scale_of", size, "\"ok\"", "true"], 0, "Ok(Size { w: 6, h: 4, flag: true })".into()),
        (vec!["app.shapes.scale_of", size, "\"clamp\"", "false"], 0, "Ok(Size { w: 3, h: 4, flag: true })".into()),
        // the errors map without a message function: "<function> returned <code>"
        (
            vec!["app.shapes.scale_of", size, "\"bad\"", "false"],
            0,
            format!(r#"Err(ForeignError {{ kind: InvalidArgument, message: "shp_scale returned 7", details: [{DETAILS_SCALE}, ForeignDetail {{ key: "foreign_code", value: "7" }}] }})"#),
        ),
        // a struct field that is no Bool: InvalidResult at its pointer, with pointer and expected_type last
        (
            vec!["app.shapes.scale_of", size, "\"flag2\"", "false"],
            0,
            format!(r#"Err(ForeignError {{ kind: InvalidResult, message: "the result of app.shapes.Shapes.scale is not a value of Bool at /$payload/0/flag", details: [{DETAILS_SCALE}, ForeignDetail {{ key: "pointer", value: "/$payload/0/flag" }}, ForeignDetail {{ key: "expected_type", value: "Bool" }}] }})"#),
        ),
        // the fields of a struct argument are range-checked with their path
        (
            vec!["app.shapes.scale_of", "Size { w: 3000000000, h: 4, flag: true }", "\"ok\"", "false"],
            0,
            format!(r#"Err(ForeignError {{ kind: InvalidArgument, message: "argument size.w of app.shapes.Shapes.scale does not fit int32", details: [{DETAILS_SCALE}] }})"#),
        ),
        (
            vec!["app.shapes.scale_of", "Size { w: 3, h: -32769, flag: true }", "\"ok\"", "false"],
            0,
            format!(r#"Err(ForeignError {{ kind: InvalidArgument, message: "argument size.h of app.shapes.Shapes.scale does not fit int16", details: [{DETAILS_SCALE}] }})"#),
        ),
        // a cstr with an interior U+0000
        (
            vec!["app.shapes.scale_of", size, "\"a\\u{0}b\"", "false"],
            0,
            format!(r#"Err(ForeignError {{ kind: InvalidArgument, message: "argument label of app.shapes.Shapes.scale contains U+0000", details: [{DETAILS_SCALE}] }})"#),
        ),
        // owned text: copied, then released; not UTF-8 is InvalidResult
        (vec!["app.shapes.upper_of", "\"hello\""], 0, "Ok(\"HELLO\")".into()),
        (vec!["app.shapes.upper_of", "\"badutf8\""], 0, r#"Err(ForeignError { kind: InvalidResult, message: "the result of app.shapes.Shapes.upper is not a value of Text at /$payload/0", details: [ForeignDetail { key: "adapter", value: "shapes" }, ForeignDetail { key: "capability", value: "app.shapes.Shapes" }, ForeignDetail { key: "operation", value: "app.shapes.Shapes.upper" }, ForeignDetail { key: "foreign_type", value: "shp_upper" }, ForeignDetail { key: "pointer", value: "/$payload/0" }, ForeignDetail { key: "expected_type", value: "Text" }] })"#.into()),
        // a bool result; one other than 0 or 1 cannot be carried by Bool: the fault of semantics S80
        (vec!["app.shapes.is_even_of", "4"], 0, "true".into()),
        (vec!["app.shapes.is_even_of", "5"], 0, "false".into()),
        (
            vec!["app.shapes.is_even_of", "42"],
            1,
            "fault binding_error_unrepresentable internal error (binding_error_unrepresentable): shapes reported InvalidResult for app.shapes.Shapes.is_even: \"the result of app.shapes.Shapes.is_even is not a value of Bool\"".into(),
        ),
        // void, then a uint64 result beyond int64
        (vec!["app.shapes.touched", "2"], 0, "2".into()),
        (vec!["app.shapes.touched", "0"], 0, "18446744073709551615".into()),
        // a uint8 status: the errors map; a message function giving NULL or text that is not UTF-8 falls back
        (vec!["app.shapes.check_of", "5"], 0, "Ok(Unit)".into()),
        (vec!["app.shapes.check_of", "1500"], 0, r#"Err(ForeignError { kind: Unavailable, message: "shp_check returned 3", details: [ForeignDetail { key: "adapter", value: "shapes" }, ForeignDetail { key: "capability", value: "app.shapes.Shapes" }, ForeignDetail { key: "operation", value: "app.shapes.Shapes.check" }, ForeignDetail { key: "foreign_type", value: "shp_check" }, ForeignDetail { key: "foreign_code", value: "3" }] })"#.into()),
        (vec!["app.shapes.check_of", "3000"], 0, r#"Err(ForeignError { kind: Exception, message: "shp_check returned 4", details: [ForeignDetail { key: "adapter", value: "shapes" }, ForeignDetail { key: "capability", value: "app.shapes.Shapes" }, ForeignDetail { key: "operation", value: "app.shapes.Shapes.check" }, ForeignDetail { key: "foreign_type", value: "shp_check" }, ForeignDetail { key: "foreign_code", value: "4" }] })"#.into()),
        (vec!["app.shapes.check_of", "-1"], 0, r#"Err(ForeignError { kind: InvalidArgument, message: "argument v of app.shapes.Shapes.check does not fit uint16", details: [ForeignDetail { key: "adapter", value: "shapes" }, ForeignDetail { key: "capability", value: "app.shapes.Shapes" }, ForeignDetail { key: "operation", value: "app.shapes.Shapes.check" }, ForeignDetail { key: "foreign_type", value: "shp_check" }] })"#.into()),
    ];
    let mut failures = Vec::new();
    for (args, code, expected) in &cases {
        let (got_code, got) = run(&dir, args);
        if got_code != *code || &got != expected {
            failures.push(format!(
                "{args:?}\n  expected ({code}) {expected}\n  got      ({got_code}) {got}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_declarative_binding_that_does_not_compile_names_its_wrapper() {
    if !require_cc() {
        return;
    }
    let dir = cshapes();
    // the declared state type differs from the library's: an incompatible pointer, an error in the wrapper only
    let path = dir.join("adapters/shapes.cbind.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("type = \"shp_ctx *\"", "type = \"struct shp_size *\"");
    std::fs::write(&path, text).unwrap();
    cpl(&dir, &["check"]).expect_code(0);
    let r = cpl(&dir, &["build", "--json"]);
    r.expect_code(1);
    let envelope = r.json();
    let d = &with_code(&envelope, "build_adapter_compile_failed")[0];
    assert_eq!(d["binding"], "shapes");
    assert_eq!(d["file"], "src/geom_bind_shapes.c");
    // a scalar narrower than the real parameter is a conversion error, not a silent truncation
    let dir = cshapes();
    let path = dir.join("adapters/shapes.cbind.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("args = [\"n: int64\"]", "args = [\"n: uint64\"]");
    std::fs::write(&path, text).unwrap();
    let r = cpl(&dir, &["build", "--json"]);
    r.expect_code(1);
    let envelope = r.json();
    let d = &with_code(&envelope, "build_adapter_compile_failed")[0];
    assert!(
        d["stderr"]
            .as_str()
            .unwrap_or_default()
            .contains("sign-conversion"),
        "{d:#}"
    );
}
