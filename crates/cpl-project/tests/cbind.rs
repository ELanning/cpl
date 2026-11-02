//! The closed schema of declarative bindings (`spec/cabi.md` 6.3): the syntax half of `project_cbind_invalid`.

use cpl_project::cbind::{parse_cbind, ArgSpec, BindType, ReturnsSpec, Scalar};

/// The `.cbind.toml` of cabi.md 6.3 (the fixture `cthumbs`).
const THUMBS: &str = r#"
[library]
headers = ["imgtest.h"]

[state]
type = "imgtest_decoder *"
create = "imgtest_decoder_new"
destroy = "imgtest_decoder_free"

[ops.thumbnail]
c = "imgtest_thumbnail"
args = ["state", "image: bytes_span", "max_side: int32"]
out = [{ name = "data", type = "owned_bytes", release = "imgtest_free" }]
returns = { type = "int", ok = [0] }
errors = { "-1" = "InvalidArgument", "-2" = "Unavailable", default = "Exception" }
message = "imgtest_strerror"
result = "data"

[ops.decoder_version]
c = "imgtest_version"
args = []
returns = { type = "int32", result = true }
"#;

/// The reason `text` is outside the schema, with its key.
fn invalid(text: &str) -> (String, String) {
    match parse_cbind(text) {
        Ok(spec) => panic!("accepted: {spec:?}\n{text}"),
        Err(e) => (e.key, e.reason),
    }
}

/// THUMBS with `from` replaced by `to` (exactly once).
fn edit(from: &str, to: &str) -> String {
    assert_eq!(THUMBS.matches(from).count(), 1, "{from}");
    THUMBS.replace(from, to)
}

#[test]
fn the_fixture_of_the_spec_parses() {
    let spec = parse_cbind(THUMBS).unwrap();
    assert_eq!(spec.headers, ["imgtest.h"]);
    let state = spec.state.as_ref().unwrap();
    assert_eq!(
        (
            state.c_type.as_str(),
            state.create.as_str(),
            state.destroy.as_str()
        ),
        (
            "imgtest_decoder *",
            "imgtest_decoder_new",
            "imgtest_decoder_free"
        )
    );
    let t = &spec.ops["thumbnail"];
    assert_eq!(t.c, "imgtest_thumbnail");
    assert_eq!(
        t.args,
        [
            ArgSpec::State,
            ArgSpec::Param {
                name: "image".into(),
                ty: BindType::BytesSpan
            },
            ArgSpec::Param {
                name: "max_side".into(),
                ty: BindType::Scalar(Scalar::I32)
            },
        ]
    );
    assert_eq!(t.out.len(), 1);
    assert_eq!(
        (t.out[0].ty.clone(), t.out[0].release.as_deref()),
        (BindType::OwnedBytes, Some("imgtest_free"))
    );
    assert_eq!(
        t.returns,
        ReturnsSpec::Status {
            ty: Scalar::Int,
            ok: vec![0]
        }
    );
    let mut errors = t.errors.clone();
    errors.sort();
    assert_eq!(
        errors,
        [
            (-2, "Unavailable".to_string()),
            (-1, "InvalidArgument".to_string())
        ]
    );
    assert_eq!(t.default_error.as_deref(), Some("Exception"));
    assert_eq!(t.message.as_deref(), Some("imgtest_strerror"));
    assert_eq!(t.result.as_deref(), Some("data"));
    let v = &spec.ops["decoder_version"];
    assert_eq!(v.returns, ReturnsSpec::Result { ty: Scalar::I32 });
    assert!(v.args.is_empty() && v.errors.is_empty() && v.default_error.is_none());
}

#[test]
fn every_scalar_and_span_type_is_in_the_vocabulary() {
    for (name, ty) in [
        ("int8", Scalar::I8),
        ("int16", Scalar::I16),
        ("int32", Scalar::I32),
        ("int64", Scalar::I64),
        ("uint8", Scalar::U8),
        ("uint16", Scalar::U16),
        ("uint32", Scalar::U32),
        ("uint64", Scalar::U64),
        ("int", Scalar::Int),
        ("bool", Scalar::Bool),
    ] {
        let spec = parse_cbind(&edit(
            "\"max_side: int32\"",
            &format!("\"max_side: {name}\""),
        ))
        .unwrap();
        assert_eq!(
            spec.ops["thumbnail"].args[2],
            ArgSpec::Param {
                name: "max_side".into(),
                ty: BindType::Scalar(ty)
            },
            "{name}"
        );
    }
    for (name, ty) in [("text_span", BindType::TextSpan), ("cstr", BindType::CStr)] {
        let spec = parse_cbind(&edit(
            "\"image: bytes_span\"",
            &format!("\"image: {name}\""),
        ))
        .unwrap();
        assert_eq!(
            spec.ops["thumbnail"].args[1],
            ArgSpec::Param {
                name: "image".into(),
                ty
            },
            "{name}"
        );
    }
}

#[test]
fn a_file_that_is_not_toml_is_invalid_with_its_line() {
    let e = parse_cbind("[library]\nheaders = [\n").unwrap_err();
    assert!(e.reason.starts_with("the file is not TOML"), "{e:?}");
    assert!(e.line.is_some());
}

#[test]
fn the_schema_is_closed() {
    let (key, reason) = invalid(&format!("{THUMBS}\n[extra]\nx = 1\n"));
    assert_eq!(key, "extra", "{reason}");
    let (key, _) = invalid(&edit(
        "c = \"imgtest_version\"",
        "c = \"imgtest_version\"\ncallback = \"f\"",
    ));
    assert_eq!(key, "ops.decoder_version.callback");
    let (key, _) = invalid(&edit(
        "headers = [\"imgtest.h\"]",
        "headers = [\"imgtest.h\"]\nlink = \"x\"",
    ));
    assert_eq!(key, "library.link");
    let (key, _) = invalid(&edit(
        "destroy = \"imgtest_decoder_free\"",
        "destroy = \"imgtest_decoder_free\"\nlock = true",
    ));
    assert_eq!(key, "state.lock");
    let (key, _) = invalid(&edit(
        "{ name = \"data\", type = \"owned_bytes\", release = \"imgtest_free\" }",
        "{ name = \"data\", type = \"owned_bytes\", release = \"imgtest_free\", align = 8 }",
    ));
    assert_eq!(key, "ops.thumbnail.out.align");
    let (key, _) = invalid(&edit(
        "returns = { type = \"int32\", result = true }",
        "returns = { type = \"int32\", result = true, ok = [0] }",
    ));
    assert_eq!(key, "ops.decoder_version.returns");
}

#[test]
fn library_headers_are_required_and_non_empty() {
    assert_eq!(
        invalid(&edit("[library]\nheaders = [\"imgtest.h\"]\n", "")).0,
        "library"
    );
    assert_eq!(
        invalid(&edit("headers = [\"imgtest.h\"]", "headers = []")).0,
        "library.headers"
    );
    assert_eq!(
        invalid(&edit(
            "headers = [\"imgtest.h\"]",
            "headers = \"imgtest.h\""
        ))
        .0,
        "library.headers"
    );
    assert_eq!(
        invalid(&edit("headers = [\"imgtest.h\"]", "headers = [\"\"]")).0,
        "library.headers"
    );
    assert_eq!(
        invalid(&edit(
            "headers = [\"imgtest.h\"]",
            "headers = [\"a\\\"b.h\"]"
        ))
        .0,
        "library.headers"
    );
}

#[test]
fn the_state_needs_a_pointer_type_and_both_functions() {
    for bad in [
        "imgtest_decoder",
        "imgtest_decoder*",
        "int **",
        "struct *",
        "const imgtest_decoder *",
    ] {
        let (key, reason) = invalid(&edit(
            "type = \"imgtest_decoder *\"",
            &format!("type = \"{bad}\""),
        ));
        assert_eq!(key, "state.type", "{bad}: {reason}");
    }
    assert!(parse_cbind(&edit(
        "type = \"imgtest_decoder *\"",
        "type = \"struct imgtest_decoder *\""
    ))
    .is_ok());
    assert_eq!(
        invalid(&edit("create = \"imgtest_decoder_new\"\n", "")).0,
        "state.create"
    );
    assert_eq!(
        invalid(&edit("destroy = \"imgtest_decoder_free\"\n", "")).0,
        "state.destroy"
    );
    assert_eq!(
        invalid(&edit(
            "create = \"imgtest_decoder_new\"",
            "create = \"new()\""
        ))
        .0,
        "state.create"
    );
    // an operation that receives the state needs [state]
    let no_state = edit(
        "[state]\ntype = \"imgtest_decoder *\"\ncreate = \"imgtest_decoder_new\"\ndestroy = \"imgtest_decoder_free\"\n",
        "",
    );
    assert_eq!(invalid(&no_state).0, "ops.thumbnail.args");
}

#[test]
fn arguments_are_typed_parameters_or_the_state() {
    for bad in [
        "\"image\"",
        "\"image: bytes\"",
        "\"image: owned_bytes\"",
        "\"2x: int32\"",
        "1",
    ] {
        let (key, reason) = invalid(&edit("\"image: bytes_span\"", bad));
        assert_eq!(key, "ops.thumbnail.args", "{bad}: {reason}");
    }
    assert_eq!(
        invalid(&edit("args = []", "args = \"state\"")).0,
        "ops.decoder_version.args"
    );
    assert_eq!(
        invalid(&edit("args = []\n", "")).0,
        "ops.decoder_version.args"
    );
    // a struct names a [structs] table
    let (key, reason) = invalid(&edit("\"max_side: int32\"", "\"max_side: struct:size\""));
    assert_eq!(key, "ops.thumbnail.args");
    assert!(reason.contains("[structs.size]"), "{reason}");
}

#[test]
fn out_parameters_name_a_release_exactly_when_owned() {
    let (key, _) = invalid(&edit(", release = \"imgtest_free\" }", " }"));
    assert_eq!(key, "ops.thumbnail.out");
    let (key, _) = invalid(&edit(
        "type = \"owned_bytes\", release = \"imgtest_free\"",
        "type = \"int32\", release = \"imgtest_free\"",
    ));
    assert_eq!(key, "ops.thumbnail.out");
    let (key, _) = invalid(&edit("type = \"owned_bytes\"", "type = \"bytes_span\""));
    assert_eq!(key, "ops.thumbnail.out");
    let (key, reason) = invalid(&edit(
        "out = [{ name = \"data\", type = \"owned_bytes\", release = \"imgtest_free\" }]",
        "out = [{ name = \"data\", type = \"owned_bytes\", release = \"imgtest_free\" }, { name = \"data\", type = \"int32\" }]",
    ));
    assert_eq!(key, "ops.thumbnail.out");
    assert!(reason.contains("two out-parameters"), "{reason}");
    assert!(parse_cbind(&edit(
        "out = [{ name = \"data\", type = \"owned_bytes\", release = \"imgtest_free\" }]",
        "out = [{ name = \"data\", type = \"owned_bytes\", release = \"imgtest_free\" }, { name = \"n\", type = \"uint64\" }]",
    ))
    .is_ok());
}

#[test]
fn returns_has_exactly_one_form() {
    for bad in [
        "returns = { type = \"int\" }",
        "returns = { type = \"int\", ok = [0], result = true }",
        "returns = { type = \"text\", ok = [0] }",
        "returns = { type = \"void\", ok = [0] }",
        "returns = { type = \"int\", ok = [] }",
        "returns = { type = \"int\", ok = [\"0\"] }",
    ] {
        let (key, reason) = invalid(&edit("returns = { type = \"int\", ok = [0] }", bad));
        assert!(
            key.starts_with("ops.thumbnail.returns"),
            "{bad}: {key}: {reason}"
        );
    }
    assert_eq!(
        invalid(&edit("returns = { type = \"int32\", result = true }\n", "")).0,
        "ops.decoder_version.returns"
    );
    assert!(parse_cbind(&edit(
        "returns = { type = \"int32\", result = true }",
        "returns = { type = \"void\" }"
    ))
    .is_ok());
}

#[test]
fn statuses_fit_the_status_type() {
    let (key, reason) = invalid(&edit(
        "returns = { type = \"int\", ok = [0] }",
        "returns = { type = \"int8\", ok = [128] }",
    ));
    assert_eq!(key, "ops.thumbnail.returns.ok", "{reason}");
    // a negative error status of an unsigned status type can never be returned
    let unsigned = edit(
        "returns = { type = \"int\", ok = [0] }",
        "returns = { type = \"uint32\", ok = [0] }",
    );
    let (key, reason) = invalid(&unsigned);
    assert_eq!(key, "ops.thumbnail.errors", "{reason}");
    // a status that is both ok and an error
    let (key, reason) = invalid(&edit(
        "\"-1\" = \"InvalidArgument\"",
        "\"0\" = \"InvalidArgument\"",
    ));
    assert_eq!(key, "ops.thumbnail.errors");
    assert!(reason.contains("also an `ok` status"), "{reason}");
}

#[test]
fn errors_go_with_ok_and_need_a_default() {
    let (key, _) = invalid(&edit(", default = \"Exception\"", ""));
    assert_eq!(key, "ops.thumbnail.errors");
    let (key, _) = invalid(&edit("errors = { \"-1\" = \"InvalidArgument\", \"-2\" = \"Unavailable\", default = \"Exception\" }\n", ""));
    assert_eq!(key, "ops.thumbnail.errors");
    let (key, _) = invalid(&edit(
        "returns = { type = \"int32\", result = true }",
        "returns = { type = \"int32\", result = true }\nerrors = { default = \"Exception\" }",
    ));
    assert_eq!(key, "ops.decoder_version.errors");
    let (key, reason) = invalid(&edit("\"-2\" = \"Unavailable\"", "\"-2\" = \"Missing\""));
    assert_eq!(key, "ops.thumbnail.errors");
    assert!(reason.contains("ForeignErrorKind"), "{reason}");
    let (key, _) = invalid(&edit(
        "\"-2\" = \"Unavailable\"",
        "\"two\" = \"Unavailable\"",
    ));
    assert_eq!(key, "ops.thumbnail.errors");
}

#[test]
fn message_and_result_go_with_ok() {
    let (key, _) = invalid(&edit(
        "returns = { type = \"int32\", result = true }",
        "returns = { type = \"int32\", result = true }\nmessage = \"imgtest_strerror\"",
    ));
    assert_eq!(key, "ops.decoder_version.message");
    let (key, _) = invalid(&edit(
        "returns = { type = \"int32\", result = true }",
        "returns = { type = \"int32\", result = true }\nresult = \"x\"",
    ));
    assert_eq!(key, "ops.decoder_version.result");
    let (key, reason) = invalid(&edit("result = \"data\"", "result = \"pixels\""));
    assert_eq!(key, "ops.thumbnail.result");
    assert!(reason.contains("pixels"), "{reason}");
}

#[test]
fn struct_mappings_are_scalar_fields_of_a_c_struct() {
    let with_struct = |table: &str| {
        format!("{THUMBS}\n[structs.size]\n{table}\n")
            .replace("\"max_side: int32\"", "\"max_side: struct:size\"")
    };
    let spec = parse_cbind(&with_struct(
        "c_type = \"struct img_size\"\nwidth = \"w: int32\"\nheight = \"h: uint16\"",
    ))
    .unwrap();
    let s = &spec.structs["size"];
    assert_eq!(s.c_type, "struct img_size");
    assert_eq!(
        s.fields,
        [
            ("height".to_string(), "h".to_string(), Scalar::U16),
            ("width".to_string(), "w".to_string(), Scalar::I32)
        ]
    );
    assert_eq!(
        invalid(&with_struct("width = \"w: int32\"")).0,
        "structs.size.c_type"
    );
    assert_eq!(
        invalid(&with_struct(
            "c_type = \"img_size *\"\nwidth = \"w: int32\""
        ))
        .0,
        "structs.size.c_type"
    );
    for bad in [
        "\"w\"",
        "\"w: bytes_span\"",
        "\"w: struct:size\"",
        "\"1w: int32\"",
        "3",
    ] {
        let (key, reason) = invalid(&with_struct(&format!(
            "c_type = \"img_size\"\nwidth = {bad}"
        )));
        assert_eq!(key, "structs.size.width", "{bad}: {reason}");
    }
}

#[test]
fn errors_carry_the_line_of_their_key() {
    let e = parse_cbind(&edit("\"-2\" = \"Unavailable\"", "\"-2\" = \"Missing\"")).unwrap_err();
    let line = THUMBS
        .lines()
        .position(|l| l.starts_with("errors"))
        .unwrap() as u32
        + 1;
    assert_eq!(e.line, Some(line), "{e:?}");
}

#[test]
fn the_line_is_that_of_the_operation_concerned() {
    // `args` of decoder_version, not the first `args` of the file
    let e = parse_cbind(&edit("args = []", "args = [\"x\"]")).unwrap_err();
    let line = THUMBS.lines().position(|l| l == "args = []").unwrap() as u32 + 1;
    assert_eq!(
        (e.key.as_str(), e.line),
        ("ops.decoder_version.args", Some(line))
    );
    // a whole table: its header
    let e = parse_cbind(&format!("{THUMBS}\n[extra]\nx = 1\n")).unwrap_err();
    assert_eq!(e.line, Some(THUMBS.lines().count() as u32 + 2));
}

#[test]
fn c_names_are_identifiers_that_are_no_keywords() {
    for (from, to, key) in [
        (
            "type = \"imgtest_decoder *\"",
            "type = \"int *\"",
            "state.type",
        ),
        (
            "create = \"imgtest_decoder_new\"",
            "create = \"return\"",
            "state.create",
        ),
        (
            "c = \"imgtest_version\"",
            "c = \"sizeof\"",
            "ops.decoder_version.c",
        ),
        (
            "release = \"imgtest_free\"",
            "release = \"free()\"",
            "ops.thumbnail.out.release",
        ),
    ] {
        assert_eq!(invalid(&edit(from, to)).0, key, "{to}");
    }
}
