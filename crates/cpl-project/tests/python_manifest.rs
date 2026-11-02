//! The Milestone 3 manifest rules (`spec/python.md` Sections 1, 5, 6, 8; toolchain 11.1): `[interop.python]`,
//! `[bindings.<name>] timeout_ms`, `[test.fixtures]`, and `check_python`'s diagnostics with their payloads.

use cpl_project::{check_python, codes, parse_manifest, Manifest};
use serde_json::json;

const HEAD: &str = "[project]\nname = \"shop\"\nedition = \"0.1\"\n";

fn parse(text: &str) -> Manifest {
    let (m, diagnostics) =
        parse_manifest(text).unwrap_or_else(|e| panic!("the manifest parses:\n{text}\n{e:?}"));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    m
}

fn codes_of(text: &str) -> Vec<String> {
    check_python(&parse(text), None)
        .iter()
        .map(|d| d.code.as_str().to_string())
        .collect()
}

#[test]
fn defaults_without_the_table() {
    let m = parse(&format!("{HEAD}\n[interop]\ntargets = [\"python\"]\n"));
    assert_eq!(m.python_package(), "shop_cpl");
    assert_eq!(m.python_version(), "0.1.0");
    assert_eq!(m.python_paths(), ["."]);
    assert!(m.binding_timeouts.is_empty());
    assert!(m.test_fixtures.is_empty());
    assert!(check_python(&m, None).is_empty());
}

#[test]
fn every_key_is_read_and_paths_are_normalized() {
    let m = parse(&format!(
        "{HEAD}\n[interop]\ntargets = [\"python\"]\n\n[interop.python]\npackage = \"shop_py\"\nversion = \"2.0.13\"\npaths = [\"adapters/\", \".\", \"adapters\"]\n"
    ));
    assert_eq!(m.python_package(), "shop_py");
    assert_eq!(m.python_version(), "2.0.13");
    assert_eq!(m.python_paths(), ["adapters", "."]);
}

#[test]
fn malformed_values_are_fatal_and_unknown_keys_are_diagnostics() {
    for bad in [
        "[interop.python]\npaths = [\"/abs\"]\n",
        "[interop.python]\npaths = [\"../up\"]\n",
        "[interop.python]\npackage = 3\n",
        "[interop.python]\npaths = \"adapters\"\n",
    ] {
        let text = format!("{HEAD}\n[interop]\ntargets = [\"python\"]\n\n{bad}");
        assert!(parse_manifest(&text).is_err(), "{bad}");
    }
    let text =
        format!("{HEAD}\n[interop]\ntargets = [\"python\"]\n\n[interop.python]\nabi = \"abi3\"\n");
    let (_, diagnostics) = parse_manifest(&text).unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, codes::PROJECT_MANIFEST_UNKNOWN_KEY);
    assert_eq!(diagnostics[0].data["key"], json!("interop.python.abi"));
}

#[test]
fn the_python_table_needs_the_python_target() {
    let text = format!("{HEAD}\n[interop]\ntargets = [\"c\"]\n\n[interop.python]\n");
    let diagnostics = check_python(&parse(&text), None);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        codes::PROJECT_PYTHON_TARGET_NOT_ENABLED
    );
    assert_eq!(diagnostics[0].data["key"], json!("interop.python"));
    assert_eq!(diagnostics[0].data["targets"], json!(["c"]));
}

#[test]
fn package_and_version_domains() {
    for (package, ok) in [
        ("shop_cpl", true),
        ("s", true),
        ("json", false),
        ("asyncio", false),
        ("class", false),
        ("match", false),
        ("Shop", false),
        ("_shop", false),
        ("2shop", false),
        ("shop-cpl", false),
    ] {
        let text = format!(
            "{HEAD}\n[interop]\ntargets = [\"python\"]\n\n[interop.python]\npackage = \"{package}\"\n"
        );
        let diagnostics = check_python(&parse(&text), None);
        assert_eq!(diagnostics.is_empty(), ok, "{package}: {diagnostics:?}");
        if !ok {
            assert_eq!(diagnostics[0].code, codes::PROJECT_PYTHON_OPTION_INVALID);
            assert_eq!(diagnostics[0].data["key"], json!("interop.python.package"));
            assert_eq!(diagnostics[0].data["found"], json!(package));
            assert!(diagnostics[0].data["reason"].is_string());
        }
    }
    for (version, ok) in [
        ("1", true),
        ("0.1.0", true),
        ("10.20.30.40", true),
        ("1.0b1", false),
        ("v1", false),
        ("1.", false),
    ] {
        let text = format!(
            "{HEAD}\n[interop]\ntargets = [\"python\"]\n\n[interop.python]\nversion = \"{version}\"\n"
        );
        assert_eq!(codes_of(&text).is_empty(), ok, "{version}");
    }
}

#[test]
fn paths_must_exist_under_a_root() {
    let root = std::env::temp_dir().join(format!("cpl-python-manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("adapters")).unwrap();
    let text = format!(
        "{HEAD}\n[interop]\ntargets = [\"python\"]\n\n[interop.python]\npaths = [\"adapters\", \"missing\"]\n"
    );
    let diagnostics = check_python(&parse(&text), Some(&root));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, codes::PROJECT_BINDING_PATH_MISSING);
    assert_eq!(diagnostics[0].data["key"], json!("interop.python.paths"));
    assert_eq!(diagnostics[0].data["path"], json!("missing"));
    // without a root (a virtual tree) the file checks are skipped
    assert!(check_python(&parse(&text), None).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn python_adapters_are_dotted_module_paths() {
    for (adapter, ok) in [
        ("reader_adapters.images", true),
        ("x", true),
        ("adapters/codec.py", false),
        ("codec.py.", false),
        ("a.def.b", false),
        ("", false),
    ] {
        let text = format!(
            "{HEAD}\n[interop]\ntargets = [\"python\"]\n\n[bindings.codec]\ncapability = \"a.b.Codec\"\nbackend = \"python\"\nadapter = \"{adapter}\"\n"
        );
        let diagnostics = check_python(&parse(&text), None);
        assert_eq!(diagnostics.is_empty(), ok, "{adapter}: {diagnostics:?}");
        if !ok {
            assert_eq!(diagnostics[0].code, codes::PROJECT_BINDING_ADAPTER_INVALID);
            assert_eq!(diagnostics[0].data["binding"], json!("codec"));
            assert_eq!(diagnostics[0].data["found"], json!(adapter));
        }
    }
    // a C adapter's path is not a Python module path, and is check_c_bindings' to judge
    let text = format!(
        "{HEAD}\n[interop]\ntargets = [\"c\"]\n\n[bindings.codec]\ncapability = \"a.b.Codec\"\nbackend = \"c\"\nadapter = \"adapters/codec.c\"\n"
    );
    assert!(codes_of(&text).is_empty());
}

#[test]
fn timeouts_are_positive_and_need_a_worker() {
    let binding = |backend: &str, isolation: &str, timeout: &str| {
        format!(
            "{HEAD}\n[interop]\ntargets = [\"c\", \"python\"]\n\n[bindings.codec]\ncapability = \"a.b.Codec\"\nbackend = \"{backend}\"\nadapter = \"{}\"\n{isolation}timeout_ms = {timeout}\n",
            if backend == "c" { "a.c" } else { "a.b" }
        )
    };
    for (text, ok) in [
        (binding("python", "", "5000"), true),
        (
            binding("python", "isolation = \"in_process\"\n", "5000"),
            true,
        ),
        (binding("c", "", "1"), true),
        (binding("c", "isolation = \"worker\"\n", "1"), true),
        (binding("python", "", "0"), false),
        (binding("python", "", "-5"), false),
        (binding("c", "isolation = \"in_process\"\n", "100"), false),
    ] {
        let m = parse(&text);
        assert!(m.binding_timeouts.contains_key("codec"));
        let diagnostics = check_python(&m, None);
        assert_eq!(diagnostics.is_empty(), ok, "{text}: {diagnostics:?}");
        if !ok {
            assert_eq!(diagnostics[0].code, codes::PROJECT_BINDING_TIMEOUT_INVALID);
            assert_eq!(diagnostics[0].data["binding"], json!("codec"));
        }
    }
    // a value that is no integer is fatal (T53)
    let text = format!(
        "{HEAD}\n[interop]\ntargets = [\"python\"]\n\n[bindings.codec]\ncapability = \"a.b.Codec\"\nbackend = \"python\"\nadapter = \"a.b\"\ntimeout_ms = \"5s\"\n"
    );
    assert!(parse_manifest(&text).is_err());
}

#[test]
fn fixtures_name_a_binding_of_their_capability() {
    let base = format!(
        "{HEAD}\n[interop]\ntargets = [\"python\"]\n\n[bindings.store]\ncapability = \"shop.store.Store\"\nbackend = \"python\"\nadapter = \"adapters.store\"\n"
    );
    let m = parse(&format!(
        "{base}\n[test.fixtures]\n\"shop.store.Store\" = \"store\"\n"
    ));
    assert_eq!(m.test_fixtures["shop.store.Store"], "store");
    assert!(check_python(&m, None).is_empty());
    let diagnostics = check_python(
        &parse(&format!(
            "{base}\n[test.fixtures]\n\"shop.store.Store\" = \"nope\"\n"
        )),
        None,
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, codes::PROJECT_FIXTURE_BINDING_UNKNOWN);
    assert_eq!(diagnostics[0].data["capability"], json!("shop.store.Store"));
    assert_eq!(diagnostics[0].data["binding"], json!("nope"));
    let diagnostics = check_python(
        &parse(&format!(
            "{base}\n[test.fixtures]\n\"shop.store.Other\" = \"store\"\n"
        )),
        None,
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        codes::PROJECT_FIXTURE_CAPABILITY_MISMATCH
    );
    assert_eq!(diagnostics[0].data["found"], json!("shop.store.Store"));
    // a value that is no string is fatal (T53)
    assert!(parse_manifest(&format!(
        "{base}\n[test.fixtures]\n\"shop.store.Store\" = 1\n"
    ))
    .is_err());
}

#[test]
fn diagnostics_come_in_rule_order() {
    let text = format!(
        "{HEAD}\n[interop]\ntargets = [\"c\"]\n\n[interop.python]\npackage = \"json\"\n\n[bindings.a]\ncapability = \"x.y.A\"\nbackend = \"python\"\nadapter = \"bad/path\"\ntimeout_ms = 0\n\n[test.fixtures]\n\"x.y.B\" = \"a\"\n"
    );
    assert_eq!(
        codes_of(&text),
        [
            "project_python_target_not_enabled",
            "project_python_option_invalid",
            "project_binding_adapter_invalid",
            "project_binding_timeout_invalid",
            "project_fixture_capability_mismatch",
        ]
    );
}
