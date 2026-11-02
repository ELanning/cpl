//! The C target's manifest tables (`spec/toolchain.md` 11.1, T74, T80):
//! `[interop.c]` and `[bindings.<name>.c]`, their defaults and domains
//! (T53), and `check_c_bindings` with every code of its rows.

mod common;

use common::*;
use cpl_diag::{Category, Diagnostic, Severity};
use cpl_project::cbind::CBindError;
use cpl_project::{
    cbind_invalid, check_bindings, check_c_bindings, codes, is_c_prefix, load_project,
    parse_manifest, AdapterForm, ArtifactKind, CBindingSection, CInteropSection, ExportEntry,
    IntMapping, LoadOptions, Manifest, ManifestError,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

fn parse(text: &str) -> (Manifest, Vec<Diagnostic>) {
    parse_manifest(text).unwrap_or_else(|e| panic!("{text}\n---\n{e:?}"))
}

fn fails(text: &str) -> ManifestError {
    match parse_manifest(text) {
        Ok((m, d)) => panic!("expected a ManifestError for\n{text}\n---\n{m:?}\n{d:?}"),
        Err(e) => e,
    }
}

/// `[project]` named `t` with `targets` and `extra` appended.
fn c_manifest(targets: &str, extra: &str) -> String {
    format!("{MANIFEST}\n[interop]\ntargets = {targets}\n{extra}")
}

fn codes_of(diagnostics: &[Diagnostic]) -> Vec<&str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

// ---- [interop.c] ----------------------------------------------------------------

#[test]
fn a_manifest_without_c_tables_has_none() {
    let (m, diagnostics) = parse(MANIFEST);
    assert!(diagnostics.is_empty());
    assert_eq!(m.interop_c, None);
    assert!(m.binding_c.is_empty());
    assert_eq!(m.c_prefix(), "t");
}

#[test]
fn an_empty_interop_c_table_takes_every_default() {
    let (m, diagnostics) = parse(&c_manifest("[\"c\"]", "\n[interop.c]\n"));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let c = m.interop_c.clone().expect("[interop.c]");
    assert_eq!(c, CInteropSection::default());
    assert_eq!(c.prefix, None);
    assert_eq!(c.abi, 0);
    assert_eq!(c.artifacts, [ArtifactKind::Shared, ArtifactKind::Static]);
    assert_eq!(c.compiler, None);
    assert!(c.cflags.is_empty() && c.ldflags.is_empty());
    assert_eq!(c.exports, None, "absent exports is the default export set");
    assert!(c.type_names.is_empty());
    assert_eq!(m.c_prefix(), "t");
}

#[test]
fn every_key_of_interop_c_is_read() {
    let text = c_manifest(
        "[\"c\"]",
        r#"
[interop.c]
prefix = "cboundary"
abi = 65535
artifacts = ["executable", "shared", "executable"]
compiler = "/usr/bin/gcc-11"
cflags = ["-DX=1"]
ldflags = ["-Wl,-z,now"]
exports = [
    "paginate_ints",
    { function = "reader.pages.paginate_ints", symbol = "cboundary_paginate_ints_i64", int = "int64" },
    { function = "clean" },
    { function = "label", int = "uint32" },
]
type_names = { "reader.pages.Page" = "page_record", "List<Int>" = "ints" }
"#,
    );
    let (m, diagnostics) = parse(&text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let c = m.interop_c.clone().unwrap();
    assert_eq!(c.prefix.as_deref(), Some("cboundary"));
    assert_eq!(m.c_prefix(), "cboundary");
    assert_eq!(c.abi, 65535);
    assert_eq!(
        c.artifacts,
        [ArtifactKind::Executable, ArtifactKind::Shared],
        "a repeated entry counts once"
    );
    assert_eq!(c.compiler.as_deref(), Some("/usr/bin/gcc-11"));
    assert_eq!(c.cflags, ["-DX=1"]);
    assert_eq!(c.ldflags, ["-Wl,-z,now"]);
    assert_eq!(
        c.exports.unwrap(),
        [
            ExportEntry {
                function: "paginate_ints".into(),
                symbol: None,
                int: IntMapping::Handle
            },
            ExportEntry {
                function: "reader.pages.paginate_ints".into(),
                symbol: Some("cboundary_paginate_ints_i64".into()),
                int: IntMapping::Int64
            },
            ExportEntry {
                function: "clean".into(),
                symbol: None,
                int: IntMapping::Handle
            },
            ExportEntry {
                function: "label".into(),
                symbol: None,
                int: IntMapping::Uint32
            },
        ]
    );
    assert_eq!(
        c.type_names,
        BTreeMap::from([
            ("List<Int>".to_string(), "ints".to_string()),
            ("reader.pages.Page".to_string(), "page_record".to_string()),
        ])
    );
}

#[test]
fn an_empty_exports_array_exports_nothing() {
    let (m, _) = parse(&c_manifest("[\"c\"]", "\n[interop.c]\nexports = []\n"));
    assert_eq!(m.interop_c.unwrap().exports, Some(vec![]));
    let (m, _) = parse(&c_manifest("[\"c\"]", "\n[interop.c]\nartifacts = []\n"));
    assert!(m.interop_c.unwrap().artifacts.is_empty());
}

#[test]
fn every_int_mapping_spelling_is_read() {
    for mapping in IntMapping::ALL {
        let text = c_manifest(
            "[\"c\"]",
            &format!(
                "\n[interop.c]\nexports = [{{ function = \"f\", int = \"{}\" }}]\n",
                mapping.as_str()
            ),
        );
        let (m, _) = parse(&text);
        assert_eq!(m.interop_c.unwrap().exports.unwrap()[0].int, mapping);
        assert_eq!(IntMapping::parse(mapping.as_str()), Some(mapping));
        assert_eq!(
            serde_json::to_value(mapping).unwrap(),
            json!(mapping.as_str())
        );
    }
    assert_eq!(IntMapping::default(), IntMapping::Handle);
    for kind in ArtifactKind::ALL {
        assert_eq!(ArtifactKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(AdapterForm::parse("wrapper"), Some(AdapterForm::Wrapper));
    assert_eq!(
        AdapterForm::parse("declarative"),
        Some(AdapterForm::Declarative)
    );
    assert_eq!(AdapterForm::of_adapter("a/b.c"), Some(AdapterForm::Wrapper));
    assert_eq!(
        AdapterForm::of_adapter("a/b.cbind.toml"),
        Some(AdapterForm::Declarative)
    );
    assert_eq!(AdapterForm::of_adapter("a/b.h"), None);
    assert_eq!(AdapterForm::of_adapter("reader_adapters.images"), None);
}

#[test]
fn the_c_prefix_domain() {
    for good in [
        "reader",
        "a",
        "cboundary",
        "a1_b2",
        "x_1_y",
        "c",
        "cp",
        "abcdefghijklmnopqrstuvwxyz01234",
    ] {
        assert!(is_c_prefix(good), "{good}");
    }
    for bad in [
        "",
        "Reader",
        "1a",
        "_a",
        "a_",
        "a__b",
        "a-b",
        "cpl",
        "cplx",
        "cpl_tool",
        "abcdefghijklmnopqrstuvwxyz012345",
        "ä",
    ] {
        assert!(!is_c_prefix(bad), "{bad}");
    }
}

#[test]
fn values_of_interop_c_outside_their_domains_are_fatal() {
    let cases: &[(&str, &str)] = &[
        ("prefix = \"cpl_tool\"", "interop.c.prefix"),
        ("prefix = \"a__b\"", "interop.c.prefix"),
        ("prefix = \"Reader\"", "interop.c.prefix"),
        ("prefix = 3", "interop.c.prefix"),
        ("abi = 65536", "interop.c.abi"),
        ("abi = -1", "interop.c.abi"),
        ("abi = \"1\"", "interop.c.abi"),
        ("artifacts = [\"dll\"]", "interop.c.artifacts[0]"),
        ("artifacts = \"shared\"", "interop.c.artifacts"),
        ("compiler = \"\"", "interop.c.compiler"),
        ("compiler = [\"gcc\"]", "interop.c.compiler"),
        ("cflags = \"-O2\"", "interop.c.cflags"),
        ("ldflags = [1]", "interop.c.ldflags[0]"),
        ("exports = \"f\"", "interop.c.exports"),
        ("exports = [1]", "interop.c.exports[0]"),
        (
            "exports = [{ symbol = \"t_f\" }]",
            "interop.c.exports[0].function",
        ),
        (
            "exports = [{ function = \"f\", int = \"int8\" }]",
            "interop.c.exports[0].int",
        ),
        (
            "exports = [{ function = \"f\", mode = \"x\" }]",
            "interop.c.exports[0]",
        ),
        (
            "exports = [{ function = 3 }]",
            "interop.c.exports[0].function",
        ),
        ("type_names = { \"a.B\" = \"Bad\" }", "interop.c.type_names"),
        ("type_names = { \"a.B\" = 1 }", "interop.c.type_names"),
        ("type_names = [\"x\"]", "interop.c.type_names"),
    ];
    for (line, path) in cases {
        let text = c_manifest("[\"c\"]", &format!("\n[interop.c]\n{line}\n"));
        let e = fails(&text);
        assert!(
            e.message.contains(path),
            "{line}: {:?} lacks {path}",
            e.message
        );
        let at = text.lines().position(|l| l == *line).unwrap() as u32 + 1;
        assert_eq!(e.line, Some(at), "{line}: {:?}", e.message);
    }
    // `interop.c` itself must be a table.
    let e = fails(&c_manifest("[\"c\"]", "c = 1\n"));
    assert!(e.message.contains("interop.c"), "{}", e.message);
}

#[test]
fn unknown_keys_of_the_c_tables_are_diagnostics() {
    let text = c_manifest(
        "[\"c\"]",
        "\n[interop.c]\nprefix = \"t\"\nsoname = \"x\"\n\n[bindings.b]\ncapability = \"a.B\"\nbackend = \"c\"\nadapter = \"a.c\"\nisolation = \"in_process\"\n\n[bindings.b.c]\nsources = []\nlinkage = \"static\"\n",
    );
    let (m, diagnostics) = parse(&text);
    assert_eq!(
        codes_of(&diagnostics),
        [
            "project_manifest_unknown_key",
            "project_manifest_unknown_key"
        ]
    );
    assert_eq!(diagnostics[0].data["key"], json!("interop.c.soname"));
    assert_eq!(diagnostics[1].data["key"], json!("bindings.b.c.linkage"));
    assert!(diagnostics
        .iter()
        .all(|d| d.span.is_none() && d.category == Category::Project));
    assert_eq!(m.binding_c["b"], CBindingSection::default());
}

#[test]
fn with_the_c_target_the_project_name_must_be_a_c_prefix_unless_prefix_is_set() {
    for name in ["cpl_tool", "a__b", "a_", "abcdefghijklmnopqrstuvwxyz012345"] {
        let base = format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n");
        // Without "c" in targets the name is fine.
        parse(&format!("{base}\n[interop]\ntargets = [\"python\"]\n"));
        let e = fails(&format!("{base}\n[interop]\ntargets = [\"c\"]\n"));
        assert!(e.message.contains("project.name"), "{}", e.message);
        assert!(e.message.contains("[interop.c] prefix"), "{}", e.message);
        assert_eq!(e.line, Some(2));
        let (m, _) = parse(&format!(
            "{base}\n[interop]\ntargets = [\"c\"]\n\n[interop.c]\nprefix = \"tool\"\n"
        ));
        assert_eq!(m.c_prefix(), "tool");
    }
}

#[test]
fn every_key_of_a_binding_c_subtable_is_read() {
    let text = c_manifest(
        "[\"c\"]",
        r#"
[bindings.image_codec]
capability = "reader.images.ImageCodec"
backend = "c"
adapter = "adapters/image_codec.c"
isolation = "in_process"

[bindings.image_codec.c]
sources = ["vendor/imgtest/imgtest.c"]
include_dirs = ["vendor/imgtest", "."]
defines = ["IMGTEST_STATIC=1", "_GNU", "X=a=b"]
libraries = ["extra"]
library_dirs = ["vendor/libextra"]
cflags = ["-Wno-unused"]
"#,
    );
    let (m, diagnostics) = parse(&text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        m.binding_c["image_codec"],
        CBindingSection {
            sources: vec!["vendor/imgtest/imgtest.c".into()],
            include_dirs: vec!["vendor/imgtest".into(), ".".into()],
            defines: vec!["IMGTEST_STATIC=1".into(), "_GNU".into(), "X=a=b".into()],
            libraries: vec!["extra".into()],
            library_dirs: vec!["vendor/libextra".into()],
            cflags: vec!["-Wno-unused".into()],
        }
    );
    // The binding table itself is unchanged (T80).
    assert_eq!(m.bindings["image_codec"].adapter, "adapters/image_codec.c");
}

#[test]
fn values_of_a_binding_c_subtable_outside_their_domains_are_fatal() {
    let cases: &[(&str, &str)] = &[
        ("sources = [\"/abs/x.c\"]", "bindings.b.c.sources[0]"),
        ("sources = [\"vendor/../x.c\"]", "bindings.b.c.sources[0]"),
        ("sources = [\"\"]", "bindings.b.c.sources[0]"),
        ("include_dirs = [\"..\"]", "bindings.b.c.include_dirs[0]"),
        (
            "library_dirs = [\"/usr/lib\"]",
            "bindings.b.c.library_dirs[0]",
        ),
        ("defines = [\"1X\"]", "bindings.b.c.defines[0]"),
        ("defines = [\"A-B=1\"]", "bindings.b.c.defines[0]"),
        ("defines = [\"=1\"]", "bindings.b.c.defines[0]"),
        ("libraries = [\"\"]", "bindings.b.c.libraries[0]"),
        ("libraries = \"m\"", "bindings.b.c.libraries"),
        ("cflags = [true]", "bindings.b.c.cflags[0]"),
    ];
    for (line, path) in cases {
        let text = c_manifest(
            "[\"c\"]",
            &format!(
                "\n[bindings.b]\ncapability = \"a.B\"\nbackend = \"c\"\nadapter = \"a.c\"\nisolation = \"in_process\"\n\n[bindings.b.c]\n{line}\n"
            ),
        );
        let e = fails(&text);
        assert!(
            e.message.contains(path),
            "{line}: {:?} lacks {path}",
            e.message
        );
        let at = text.lines().position(|l| l == *line).unwrap() as u32 + 1;
        assert_eq!(e.line, Some(at), "{line}");
    }
}

#[test]
fn the_manifest_serializes_with_its_c_tables_and_reads_a_milestone_1_shape() {
    let text = c_manifest(
        "[\"c\"]",
        "\n[interop.c]\nabi = 2\n\n[bindings.b]\ncapability = \"a.B\"\nbackend = \"c\"\nadapter = \"a.c\"\nisolation = \"in_process\"\n\n[bindings.b.c]\nlibraries = [\"m\"]\n",
    );
    let (m, _) = parse(&text);
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(v["interop_c"]["abi"], json!(2));
    assert_eq!(v["interop_c"]["artifacts"], json!(["shared", "static"]));
    assert_eq!(v["binding_c"]["b"]["libraries"], json!(["m"]));
    let back: Manifest = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(back, m);
    let mut m1 = v;
    let object = m1.as_object_mut().unwrap();
    object.remove("interop_c");
    object.remove("binding_c");
    let read: Manifest = serde_json::from_value(m1).unwrap();
    assert_eq!(read.interop_c, None);
    assert!(read.binding_c.is_empty());
}

// ---- check_c_bindings -------------------------------------------------------------

/// A backend-c binding table `[bindings.<name>]` with `keys` after `backend = "c"`.
fn c_binding(name: &str, keys: &str) -> String {
    format!("\n[bindings.{name}]\ncapability = \"a.b.Codec\"\nbackend = \"c\"\n{keys}")
}

fn check(text: &str, root: Option<&Path>) -> Vec<Diagnostic> {
    let (m, diagnostics) = parse(text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let out = check_c_bindings(&m, root);
    assert!(out.cbind.is_empty());
    for d in &out.diagnostics {
        assert_eq!(d.category, Category::Project);
        assert_eq!(d.severity, Severity::Error);
        assert!(d.span.is_none());
        assert!(codes::PROJECT.contains(&d.code), "{}", d.code);
    }
    out.diagnostics
}

#[test]
fn a_valid_backend_c_binding_has_no_diagnostics() {
    let root = temp_dir("interop_valid");
    write(&root, "adapters/codec.c", "/* adapter */\n");
    write(&root, "vendor/lib/lib.c", "int f(void) { return 0; }\n");
    std::fs::create_dir_all(root.join("vendor/inc")).unwrap();
    let text = c_manifest(
        "[\"c\"]",
        &format!(
            "\n[interop.c]\nprefix = \"t\"\n{}\n[bindings.codec.c]\nsources = [\"vendor/lib/lib.c\"]\ninclude_dirs = [\"vendor/inc\"]\nlibrary_dirs = [\"vendor/lib\"]\nlibraries = [\"m\"]\n",
            c_binding("codec", "adapter = \"adapters/codec.c\"\nisolation = \"in_process\"\n")
        ),
    );
    assert!(check(&text, Some(&root)).is_empty());
    assert!(check(&text, None).is_empty());
    // check_bindings (T48) is unchanged and accepts it too.
    let (m, _) = parse(&text);
    assert!(check_bindings(&m, |_| true).is_empty());
}

#[test]
fn project_c_target_not_enabled() {
    let diagnostics = check(&c_manifest("[]", "\n[interop.c]\nabi = 1\n"), None);
    assert_eq!(codes_of(&diagnostics), ["project_c_target_not_enabled"]);
    assert_eq!(diagnostics[0].data["key"], json!("interop.c"));
    assert_eq!(diagnostics[0].data["targets"], json!([]));
    // From Milestone 3 the Python and Node packages contain the C package (spec/python.md 1).
    for targets in ["[\"python\"]", "[\"node\"]"] {
        let diagnostics = check(&c_manifest(targets, "\n[interop.c]\nabi = 1\n"), None);
        assert!(diagnostics.is_empty(), "{targets}: {diagnostics:?}");
    }
    // A manifest without [interop.c] never reports it.
    assert!(check(&c_manifest("[]", ""), None).is_empty());
}

#[test]
fn project_binding_name_invalid() {
    let text = c_manifest(
        "[\"c\"]",
        &c_binding(
            "Image-Codec",
            "adapter = \"a.c\"\nisolation = \"in_process\"\n",
        ),
    );
    let diagnostics = check(&text, None);
    assert_eq!(codes_of(&diagnostics), ["project_binding_name_invalid"]);
    assert_eq!(diagnostics[0].data["binding"], json!("Image-Codec"));
    assert_eq!(diagnostics[0].data.len(), 1);
    // A name that is only invalid as a C identifier of a python binding is fine.
    let text = c_manifest(
        "[\"python\"]",
        "\n[bindings.Image-Codec]\ncapability = \"a.b.Codec\"\nbackend = \"python\"\nadapter = \"x\"\n",
    );
    assert!(check(&text, None).is_empty());
}

#[test]
fn c_bindings_may_run_in_workers_from_milestone_3() {
    // spec/python.md 6.6: an absent isolation (the default worker) and an explicit worker are legal for backend c
    for keys in [
        "adapter = \"a.c\"\n",
        "adapter = \"a.c\"\nisolation = \"worker\"\n",
    ] {
        let diagnostics = check(&c_manifest("[\"c\"]", &c_binding("codec", keys)), None);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
    // An isolation outside {worker, in_process} is check_bindings' alone (T48).
    let text = c_manifest(
        "[\"c\"]",
        &c_binding("codec", "adapter = \"a.c\"\nisolation = \"thread\"\n"),
    );
    assert!(check(&text, None).is_empty());
    let (m, _) = parse(&text);
    assert_eq!(
        codes_of(&check_bindings(&m, |_| true)),
        ["project_binding_bad_isolation"]
    );
}

#[test]
fn project_binding_adapter_kind() {
    for adapter in [
        "adapters/codec.h",
        "reader_adapters.images",
        "codec.cbind",
        "x.C",
    ] {
        let text = c_manifest(
            "[\"c\"]",
            &c_binding(
                "codec",
                &format!("adapter = \"{adapter}\"\nisolation = \"in_process\"\n"),
            ),
        );
        let root = temp_dir("interop_adapter_kind");
        let diagnostics = check(&text, Some(&root));
        assert_eq!(
            codes_of(&diagnostics),
            ["project_binding_adapter_kind"],
            "{adapter}"
        );
        assert_eq!(diagnostics[0].data["binding"], json!("codec"));
        assert_eq!(diagnostics[0].data["found"], json!(adapter));
    }
}

#[test]
fn project_binding_adapter_missing() {
    let root = temp_dir("interop_adapter_missing");
    write(&root, "adapters/present.c", "x");
    std::fs::create_dir_all(root.join("adapters/dir.c")).unwrap();
    let missing = [
        "/abs/codec.c",
        "../codec.c",
        "adapters/../../codec.c",
        "adapters/absent.c",
        "adapters/dir.c",
    ];
    for adapter in missing {
        let text = c_manifest(
            "[\"c\"]",
            &c_binding(
                "codec",
                &format!("adapter = \"{adapter}\"\nisolation = \"in_process\"\n"),
            ),
        );
        let diagnostics = check(&text, Some(&root));
        assert_eq!(
            codes_of(&diagnostics),
            ["project_binding_adapter_missing"],
            "{adapter}"
        );
        assert_eq!(diagnostics[0].data["binding"], json!("codec"));
        assert_eq!(diagnostics[0].data["path"], json!(adapter));
        // Without a root only the path rules apply.
        let without_root = check(&text, None);
        let syntactic = adapter.starts_with('/') || adapter.contains("..");
        assert_eq!(without_root.len(), usize::from(syntactic), "{adapter}");
    }
    let text = c_manifest(
        "[\"c\"]",
        &c_binding(
            "codec",
            "adapter = \"adapters/present.c\"\nisolation = \"in_process\"\n",
        ),
    );
    assert!(check(&text, Some(&root)).is_empty());
}

#[cfg(unix)]
#[test]
fn a_link_to_an_adapter_file_counts_as_that_file() {
    let root = temp_dir("interop_adapter_link");
    write(&root, "real/codec.c", "x");
    std::fs::create_dir_all(root.join("adapters")).unwrap();
    std::os::unix::fs::symlink(root.join("real/codec.c"), root.join("adapters/codec.c")).unwrap();
    std::os::unix::fs::symlink(root.join("nowhere.c"), root.join("adapters/dangling.c")).unwrap();
    let text = c_manifest(
        "[\"c\"]",
        &c_binding(
            "codec",
            "adapter = \"adapters/codec.c\"\nisolation = \"in_process\"\n",
        ),
    );
    assert!(check(&text, Some(&root)).is_empty());
    let text = c_manifest(
        "[\"c\"]",
        &c_binding(
            "codec",
            "adapter = \"adapters/dangling.c\"\nisolation = \"in_process\"\n",
        ),
    );
    assert_eq!(
        codes_of(&check(&text, Some(&root))),
        ["project_binding_adapter_missing"]
    );
}

#[test]
fn project_binding_path_missing() {
    let root = temp_dir("interop_path_missing");
    write(&root, "a.c", "x");
    write(&root, "vendor/present.c", "x");
    write(&root, "vendor/file_not_dir", "x");
    let text = c_manifest(
        "[\"c\"]",
        &format!(
            "{}\n[bindings.codec.c]\nsources = [\"vendor/present.c\", \"vendor/absent.c\", \"vendor\"]\ninclude_dirs = [\"vendor\", \"inc\"]\nlibrary_dirs = [\"vendor/file_not_dir\"]\n",
            c_binding("codec", "adapter = \"a.c\"\nisolation = \"in_process\"\n")
        ),
    );
    let diagnostics = check(&text, Some(&root));
    assert_eq!(
        codes_of(&diagnostics),
        [
            "project_binding_path_missing",
            "project_binding_path_missing",
            "project_binding_path_missing",
            "project_binding_path_missing",
        ]
    );
    let found: Vec<(serde_json::Value, serde_json::Value)> = diagnostics
        .iter()
        .map(|d| (d.data["key"].clone(), d.data["path"].clone()))
        .collect();
    assert_eq!(
        found,
        [
            (json!("bindings.codec.c.sources"), json!("vendor/absent.c")),
            (json!("bindings.codec.c.sources"), json!("vendor")),
            (json!("bindings.codec.c.include_dirs"), json!("inc")),
            (
                json!("bindings.codec.c.library_dirs"),
                json!("vendor/file_not_dir")
            ),
        ]
    );
    assert!(diagnostics
        .iter()
        .all(|d| d.data["binding"] == json!("codec")));
    // The file checks are skipped without a root.
    assert!(check(&text, None).is_empty());
}

#[test]
fn project_binding_c_options_misplaced() {
    let text = c_manifest(
        "[\"python\", \"c\"]",
        "\n[bindings.py]\ncapability = \"a.b.Codec\"\nbackend = \"python\"\nadapter = \"x\"\n\n[bindings.py.c]\nlibraries = [\"m\"]\n\n[bindings.ruby]\ncapability = \"a.b.R\"\nbackend = \"ruby\"\nadapter = \"x\"\n\n[bindings.ruby.c]\n",
    );
    let diagnostics = check(&text, None);
    assert_eq!(
        codes_of(&diagnostics),
        [
            "project_binding_c_options_misplaced",
            "project_binding_c_options_misplaced"
        ]
    );
    assert_eq!(diagnostics[0].data["binding"], json!("py"));
    assert_eq!(diagnostics[1].data["binding"], json!("ruby"));
    assert_eq!(diagnostics[0].data.len(), 1);
}

#[test]
fn project_binding_duplicate_capability() {
    let text = c_manifest(
        "[\"c\"]",
        &format!(
            "{}{}{}",
            c_binding("zeta", "adapter = \"z.c\"\nisolation = \"in_process\"\n"),
            c_binding("alpha", "adapter = \"a.c\"\nisolation = \"in_process\"\n"),
            "\n[bindings.other]\ncapability = \"a.b.Other\"\nbackend = \"c\"\nadapter = \"o.c\"\nisolation = \"in_process\"\n"
        ),
    );
    let diagnostics = check(&text, None);
    assert_eq!(
        codes_of(&diagnostics),
        ["project_binding_duplicate_capability"]
    );
    assert_eq!(diagnostics[0].data["capability"], json!("a.b.Codec"));
    assert_eq!(diagnostics[0].data["bindings"], json!(["alpha", "zeta"]));
    // A python binding of the same capability is no duplicate.
    let text = c_manifest(
        "[\"c\", \"python\"]",
        &format!(
            "{}\n[bindings.py]\ncapability = \"a.b.Codec\"\nbackend = \"python\"\nadapter = \"x\"\n",
            c_binding("codec", "adapter = \"a.c\"\nisolation = \"in_process\"\n")
        ),
    );
    assert!(check(&text, None).is_empty());
}

#[test]
fn diagnostics_come_in_rule_order_per_binding_in_name_order() {
    let root = temp_dir("interop_order");
    let text = c_manifest(
        "[\"python\"]",
        &format!(
            "\n[interop.c]\n{}{}\n[bindings.b.c]\nsources = [\"absent.c\"]\n",
            c_binding("B-1", "adapter = \"x.h\"\n"),
            c_binding("b", "adapter = \"absent.c\"\n"),
        ),
    );
    let diagnostics = check(&text, Some(&root));
    let pairs: Vec<(&str, serde_json::Value)> = diagnostics
        .iter()
        .map(|d| {
            (
                d.code.as_str(),
                d.data.get("binding").cloned().unwrap_or(json!(null)),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        [
            // Milestone 3: [interop.c] is legal with the python target, and a C binding may run in a worker
            ("project_binding_name_invalid", json!("B-1")),
            ("project_binding_adapter_kind", json!("B-1")),
            ("project_binding_adapter_missing", json!("b")),
            ("project_binding_path_missing", json!("b")),
            ("project_binding_duplicate_capability", json!(null)),
        ]
    );
}

#[test]
fn a_declarative_binding_that_is_no_utf8_file_is_project_cbind_invalid() {
    let root = temp_dir("interop_cbind_bytes");
    write(
        &root,
        "adapters/thumbs.cbind.toml",
        [0x5b_u8, 0xff, 0xfe, 0x5d],
    );
    let text = c_manifest(
        "[\"c\"]",
        &c_binding(
            "thumbs",
            "adapter = \"adapters/thumbs.cbind.toml\"\nisolation = \"in_process\"\n",
        ),
    );
    let diagnostics = check(&text, Some(&root));
    assert_eq!(codes_of(&diagnostics), ["project_cbind_invalid"]);
    let d = &diagnostics[0];
    assert_eq!(d.data["binding"], json!("thumbs"));
    assert_eq!(d.data["key"], json!(""));
    assert!(d.data.get("line").is_none());
    assert!(d.data["reason"].as_str().unwrap().contains("UTF-8"));
    // A missing declarative file is adapter_missing, and without a root
    // nothing is read (the parser, track F2's, is never reached).
    let text = c_manifest(
        "[\"c\"]",
        &c_binding(
            "thumbs",
            "adapter = \"adapters/none.cbind.toml\"\nisolation = \"in_process\"\n",
        ),
    );
    assert_eq!(
        codes_of(&check(&text, Some(&root))),
        ["project_binding_adapter_missing"]
    );
    assert!(check(&text, None).is_empty());
}

#[test]
fn cbind_invalid_carries_binding_key_line_and_reason() {
    let d = cbind_invalid(
        "thumbs",
        &CBindError {
            key: "ops.thumbnail.args".into(),
            line: Some(12),
            reason: "`max_side` appears twice".into(),
        },
    );
    assert_eq!(d.code, codes::PROJECT_CBIND_INVALID);
    assert_eq!(d.category, Category::Project);
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(
        serde_json::Value::Object(d.data.clone()),
        json!({ "binding": "thumbs", "key": "ops.thumbnail.args", "line": 12, "reason": "`max_side` appears twice" })
    );
    assert!(d.message.contains("ops.thumbnail.args"), "{}", d.message);
}

#[test]
fn loading_reports_no_c_target_code() {
    // The C rules are the check driver's: loading reports none (11.1).
    let root = temp_dir("interop_loading");
    write(
        &root,
        "cpl.toml",
        c_manifest(
            "[]",
            &format!("\n[interop.c]\n{}", c_binding("Bad", "adapter = \"x.h\"\n")),
        ),
    );
    write(
        &root,
        "src/a/b.cpl",
        "module a.b\n\npub capability Codec {\n    fn size(image: Bytes) -> Int\n}\n",
    );
    let project = load_project(&root, &LoadOptions::default()).unwrap();
    assert!(project.diagnostics.is_empty(), "{:?}", project.diagnostics);
    assert!(project.manifest.interop_c.is_some());
}

#[test]
fn the_project_code_table_lists_every_c_target_code_with_its_severity() {
    let c_codes = [
        ("project_c_target_not_enabled", Severity::Error),
        ("project_binding_name_invalid", Severity::Error),
        ("project_binding_isolation_unsupported", Severity::Error),
        ("project_binding_adapter_kind", Severity::Error),
        ("project_binding_adapter_missing", Severity::Error),
        ("project_binding_path_missing", Severity::Error),
        ("project_binding_c_options_misplaced", Severity::Error),
        ("project_binding_duplicate_capability", Severity::Error),
        ("project_cbind_invalid", Severity::Error),
        ("project_export_unknown", Severity::Error),
        ("project_export_ambiguous", Severity::Error),
        ("project_export_not_function", Severity::Error),
        ("project_export_not_public", Severity::Error),
        ("project_export_not_project", Severity::Error),
        ("project_export_generic", Severity::Error),
        ("project_export_symbol_invalid", Severity::Error),
        ("project_export_symbol_collision", Severity::Error),
        ("project_export_int_unused", Severity::Warning),
        ("project_type_name_unknown", Severity::Error),
        ("project_export_skipped_generic", Severity::Info),
    ];
    // then the 6 of the Python target and the fixtures (Milestone 3)
    assert_eq!(codes::PROJECT.len(), 13 + c_codes.len() + 6);
    let tail: Vec<&str> = codes::PROJECT[13..13 + c_codes.len()]
        .iter()
        .map(|c| c.as_str())
        .collect();
    let expected: Vec<&str> = c_codes.iter().map(|(c, _)| *c).collect();
    assert_eq!(tail, expected);
    for (code, severity) in c_codes {
        let constant = codes::PROJECT.iter().find(|c| c.as_str() == code).unwrap();
        assert_eq!(codes::severity(*constant), severity, "{code}");
    }
    assert_eq!(
        codes::severity(codes::PROJECT_LOCK_MISSING),
        Severity::Warning
    );
    assert_eq!(
        codes::severity(codes::PROJECT_MANIFEST_INVALID),
        Severity::Error
    );
}
