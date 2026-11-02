//! The semantic half of `project_cbind_invalid` (`spec/cabi.md` 6.3, `spec/toolchain.md` 11.1): a declarative
//! binding's operations and binding types against the capability it binds, checked after resolution.

mod common;

use common::*;
use cpl_sema::project::{load_project, AdapterForm};
use cpl_sema::{check_with, CheckConfig, CheckResult};
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST_TAIL: &str = r#"
[interop]
targets = ["c"]

[bindings.thumbs]
capability = "app.thumbs.Thumbnails"
backend = "c"
adapter = "adapters/thumbs.cbind.toml"
isolation = "in_process"
"#;

const CAPABILITY: &str = r#"module app.thumbs

use std.foreign.{ForeignError}

pub type Side = Int where self > 0

pub struct Size {
    width: Int
    flag: Bool
}

pub capability Thumbnails {
    fn thumbnail(image: Bytes, max_side: Side) -> Result<Bytes, ForeignError>
    fn decoder_version() -> Int
    fn resize(size: Size, label: Text, strict: Bool) -> Result<Size, ForeignError>
}

pub fn version(using thumbs: Thumbnails) -> Int
    effects { thumbs.decoder_version }
{
    thumbs.decoder_version()
}
"#;

const CBIND: &str = r#"[library]
headers = ["imgtest.h"]

[state]
type = "imgtest_decoder *"
create = "imgtest_decoder_new"
destroy = "imgtest_decoder_free"

[structs.size]
c_type = "struct img_size"
width = "w: int32"
flag = "f: bool"

[ops.thumbnail]
c = "imgtest_thumbnail"
args = ["state", "image: bytes_span", "max_side: int32"]
out = [{ name = "data", type = "owned_bytes", release = "imgtest_free" }]
returns = { type = "int", ok = [0] }
errors = { "-1" = "InvalidArgument", default = "Exception" }
result = "data"

[ops.decoder_version]
c = "imgtest_version"
args = []
returns = { type = "int32", result = true }

[ops.resize]
c = "img_resize"
args = ["size: struct:size", "label: cstr", "strict: bool"]
out = [{ name = "resized", type = "struct:size" }]
returns = { type = "int", ok = [0] }
errors = { default = "Exception" }
result = "resized"
"#;

/// Checks a project of the capability (with `cap_edit` applied) bound by the `.cbind.toml` (with `edit` applied).
fn check(name: &str, cap_edit: (&str, &str), edit: (&str, &str)) -> CheckResult {
    assert!(
        cap_edit.0.is_empty() || CAPABILITY.contains(cap_edit.0),
        "{}",
        cap_edit.0
    );
    assert!(edit.0.is_empty() || CBIND.contains(edit.0), "{}", edit.0);
    let capability = if cap_edit.0.is_empty() {
        CAPABILITY.to_string()
    } else {
        CAPABILITY.replacen(cap_edit.0, cap_edit.1, 1)
    };
    let cbind = if edit.0.is_empty() {
        CBIND.to_string()
    } else {
        CBIND.replacen(edit.0, edit.1, 1)
    };
    let dir: PathBuf = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cpl-sema-cbind")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    for (rel, text) in [
        ("cpl.toml", format!("{}{MANIFEST_TAIL}", manifest("t"))),
        ("src/app/thumbs.cpl", capability),
        ("adapters/thumbs.cbind.toml", cbind),
    ] {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let project = load_project(&dir, &check_options()).unwrap();
    check_with(&project, &CheckConfig::default())
}

/// The (key, reason) of each project_cbind_invalid diagnostic.
fn invalid(result: &CheckResult) -> Vec<(String, String)> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.code.as_str() == "project_cbind_invalid")
        .map(|d| {
            assert_eq!(d.data["binding"], "thumbs", "{d:?}");
            (
                d.data["key"].as_str().unwrap_or_default().to_string(),
                d.data["reason"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The keys of the project_cbind_invalid diagnostics, asserting that there is at least one.
fn keys(result: &CheckResult) -> Vec<String> {
    let found = invalid(result);
    assert!(
        !found.is_empty(),
        "no project_cbind_invalid:\n{}",
        render(result)
    );
    found.into_iter().map(|(k, _)| k).collect()
}

#[test]
fn a_binding_that_agrees_with_its_capability_is_planned_with_its_spec() {
    let result = check("agrees", ("", ""), ("", ""));
    assert!(invalid(&result).is_empty(), "{}", render(&result));
    assert!(!result.has_errors(), "{}", render(&result));
    let plan = result.interop.as_ref().unwrap();
    let b = &plan.bindings[0];
    assert_eq!(
        (b.name.as_str(), b.form),
        ("thumbs", AdapterForm::Declarative)
    );
    let spec = b.cbind.as_ref().expect("the parsed spec");
    assert_eq!(spec.ops.len(), 3);
}

#[test]
fn every_operation_has_exactly_one_table() {
    let result = check("missing_op", ("", ""), ("[ops.decoder_version]\nc = \"imgtest_version\"\nargs = []\nreturns = { type = \"int32\", result = true }\n", ""));
    assert_eq!(keys(&result), ["ops.decoder_version"]);
    let result = check("extra_op", ("", ""), ("[ops.decoder_version]", "[ops.close]\nc = \"f\"\nargs = []\nreturns = { type = \"void\" }\n\n[ops.decoder_version]"));
    assert_eq!(keys(&result), ["ops.close"]);
}

#[test]
fn every_parameter_appears_once_in_args() {
    let result = check(
        "param_missing",
        ("", ""),
        (
            "args = [\"state\", \"image: bytes_span\", \"max_side: int32\"]",
            "args = [\"state\", \"image: bytes_span\"]",
        ),
    );
    let found = invalid(&result);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "ops.thumbnail.args");
    assert!(found[0].1.contains("max_side"), "{found:?}");
    let result = check(
        "param_twice",
        ("", ""),
        (
            "\"max_side: int32\"]",
            "\"max_side: int32\", \"max_side: int64\"]",
        ),
    );
    assert_eq!(keys(&result), ["ops.thumbnail.args"]);
    let result = check(
        "param_unknown",
        ("", ""),
        (
            "\"max_side: int32\"]",
            "\"max_side: int32\", \"quality: int32\"]",
        ),
    );
    let found = invalid(&result);
    assert_eq!(found.len(), 1);
    assert!(found[0].1.contains("quality"), "{found:?}");
}

#[test]
fn binding_types_agree_with_the_cpl_types() {
    for (name, from, to) in [
        ("bytes_as_int", "\"image: bytes_span\"", "\"image: int32\""),
        (
            "side_as_span",
            "\"max_side: int32\"",
            "\"max_side: bytes_span\"",
        ),
        ("side_as_bool", "\"max_side: int32\"", "\"max_side: bool\""),
        ("text_as_bytes", "\"label: cstr\"", "\"label: bytes_span\""),
        ("bool_as_int", "\"strict: bool\"", "\"strict: int32\""),
        ("struct_as_int", "\"size: struct:size\"", "\"size: int64\""),
    ] {
        let result = check(name, ("", ""), (from, to));
        let found = invalid(&result);
        assert_eq!(found.len(), 1, "{name}: {found:?}");
        assert!(found[0].0.ends_with(".args"), "{name}: {found:?}");
    }
    // a refinement of Int is an Int, and a span of Text either form
    let result = check(
        "text_span",
        ("", ""),
        ("\"label: cstr\"", "\"label: text_span\""),
    );
    assert!(invalid(&result).is_empty(), "{}", render(&result));
}

#[test]
fn a_struct_mapping_maps_exactly_the_fields_with_agreeing_scalars() {
    let result = check(
        "struct_missing_field",
        ("", ""),
        ("flag = \"f: bool\"\n", ""),
    );
    let found = invalid(&result);
    assert!(found.iter().all(|(_, r)| r.contains("fields")), "{found:?}");
    assert_eq!(found.len(), 2, "{found:?}"); // the argument and the result
    let result = check(
        "struct_bool_as_int",
        ("", ""),
        ("flag = \"f: bool\"", "flag = \"f: int32\""),
    );
    let found = invalid(&result);
    assert!(
        found.iter().any(|(_, r)| r.contains("field `flag`")),
        "{found:?}"
    );
}

#[test]
fn a_fallible_operation_returns_a_foreign_error_arm() {
    let result = check(
        "no_arm",
        (
            "fn thumbnail(image: Bytes, max_side: Side) -> Result<Bytes, ForeignError>",
            "fn thumbnail(image: Bytes, max_side: Side) -> Bytes",
        ),
        ("", ""),
    );
    assert_eq!(keys(&result), ["ops.thumbnail.returns"]);
    let result = check(
        "other_err",
        (
            "fn thumbnail(image: Bytes, max_side: Side) -> Result<Bytes, ForeignError>",
            "fn thumbnail(image: Bytes, max_side: Side) -> Result<Bytes, Text>",
        ),
        ("", ""),
    );
    assert_eq!(keys(&result), ["ops.thumbnail.returns"]);
}

#[test]
fn the_ok_payload_is_the_result_out_parameter() {
    let result = check(
        "payload_type",
        ("", ""),
        ("type = \"owned_bytes\"", "type = \"owned_text\""),
    );
    assert_eq!(keys(&result), ["ops.thumbnail.result"]);
    let result = check("payload_missing", ("", ""), ("result = \"data\"\n", ""));
    let found = invalid(&result);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "ops.thumbnail.result");
    assert!(found[0].1.contains("Bytes"), "{found:?}");
    // Unit needs no result
    let result = check(
        "unit_payload",
        (
            "fn thumbnail(image: Bytes, max_side: Side) -> Result<Bytes, ForeignError>",
            "fn thumbnail(image: Bytes, max_side: Side) -> Result<Unit, ForeignError>",
        ),
        ("result = \"data\"\n", ""),
    );
    assert!(invalid(&result).is_empty(), "{}", render(&result));
}

#[test]
fn infallible_and_void_returns_agree_with_the_operation() {
    let result = check(
        "version_bool",
        ("", ""),
        (
            "returns = { type = \"int32\", result = true }",
            "returns = { type = \"bool\", result = true }",
        ),
    );
    assert_eq!(keys(&result), ["ops.decoder_version.returns"]);
    let result = check(
        "version_void",
        ("", ""),
        (
            "returns = { type = \"int32\", result = true }",
            "returns = { type = \"void\" }",
        ),
    );
    assert_eq!(keys(&result), ["ops.decoder_version.returns"]);
    // a ForeignError arm around an infallible scalar is allowed
    let result = check(
        "version_result",
        (
            "fn decoder_version() -> Int",
            "fn decoder_version() -> Result<Int, ForeignError>",
        ),
        ("", ""),
    );
    assert!(invalid(&result).is_empty(), "{}", render(&result));
}

#[test]
fn a_header_named_like_a_generated_header_is_invalid() {
    // the package is `t` (manifest("t")): `t.h` is its header; `cpl.h` and `cpl_adapter.h` are the API's
    for header in [
        "t.h",
        "cpl.h",
        "cpl_adapter.h",
        "cpl_rt.h",
        "t_adapter_thumbs.h",
    ] {
        let result = check(
            &format!("shadowed_{}", header.replace('.', "_")),
            ("", ""),
            (
                "headers = [\"imgtest.h\"]",
                &format!("headers = [\"imgtest.h\", \"{header}\"]"),
            ),
        );
        let found = invalid(&result);
        assert_eq!(found.len(), 1, "{header}: {found:?}");
        assert_eq!(found[0].0, "library.headers", "{header}");
        assert!(found[0].1.contains(header), "{found:?}");
    }
    // a name in a directory of its own is found through the include_dirs
    let result = check(
        "unshadowed",
        ("", ""),
        (
            "headers = [\"imgtest.h\"]",
            "headers = [\"imgtest.h\", \"t/t.h\"]",
        ),
    );
    assert!(invalid(&result).is_empty(), "{}", render(&result));
}
