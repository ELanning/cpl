//! The C entries of `cpl.lock` (`spec/toolchain.md` 11.2, T75, T80): the
//! typed views over the reserved tables, their canonical inline rendering,
//! the closed schemas `parse_lock` enforces, and `c_input_digests`.

mod common;

use common::*;
use cpl_diag::json::Digest;
use cpl_diag::StableId;
use cpl_project::{
    c_input_digests, parse_lock, parse_manifest, render_lock, AdapterC, AdapterForm, ForeignC,
    IntMapping, Lock, LockToolchain, LockedExport, LockedLibrary, TargetC, VerificationSection,
};
use std::collections::BTreeMap;

fn hex(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn d(c: char) -> Digest {
    Digest(hex(c))
}

fn base() -> Lock {
    Lock {
        version: 1,
        toolchain: LockToolchain {
            cpl: "0.1.0".to_string(),
            edition: "0.1".to_string(),
            std: d('a'),
        },
        sources: BTreeMap::from([("cpl.toml".to_string(), d('1'))]),
        verification: VerificationSection {
            proof_timeout_ms: 10_000,
            admit: vec![],
        },
        foreign: toml::Table::new(),
        adapters: toml::Table::new(),
        targets: toml::Table::new(),
    }
}

/// The `cboundary` entries of the 11.2 example.
fn target() -> TargetC {
    TargetC {
        api: "0.1".to_string(),
        runtime: "0.1.0".to_string(),
        runtime_digest: d('2'),
        target: "x86_64-linux-gnu".to_string(),
        compiler: "gcc 11.4.0".to_string(),
        compiler_path: "/usr/bin/x86_64-linux-gnu-gcc-11".to_string(),
        compiler_digest: d('3'),
        flags_digest: d('4'),
        system_digest: d('5'),
        prefix: "cboundary".to_string(),
        abi: 1,
        exports_digest: d('6'),
        exports: BTreeMap::from([(
            "cboundary_paginate_ints".to_string(),
            LockedExport {
                function: "reader.pages.paginate_ints".to_string(),
                id: StableId::parse("reader.pages.paginate_ints").unwrap(),
                signature: d('7'),
                int: IntMapping::Handle,
            },
        )]),
        symbols: vec![
            "cboundary_bind_image_codec".to_string(),
            "cboundary_clean".to_string(),
        ],
    }
}

fn adapter() -> AdapterC {
    AdapterC {
        form: AdapterForm::Wrapper,
        isolation: "in_process".to_string(),
        capability: "reader.images.ImageCodec".to_string(),
        files: BTreeMap::from([("adapters/image_codec.c".to_string(), d('8'))]),
    }
}

fn foreign() -> ForeignC {
    ForeignC {
        sources: BTreeMap::from([("vendor/imgtest/imgtest.c".to_string(), d('9'))]),
        headers: d('b'),
        libraries: BTreeMap::from([(
            "extra".to_string(),
            LockedLibrary {
                path: "vendor/libextra/libextra.a".to_string(),
                digest: d('c'),
            },
        )]),
    }
}

fn locked() -> Lock {
    let mut lock = base();
    lock.set_target_c(Some(target()));
    lock.set_c_adapters(BTreeMap::from([("image_codec".to_string(), adapter())]));
    lock.set_foreign_c(BTreeMap::from([("image_codec".to_string(), foreign())]));
    lock
}

#[test]
fn the_c_entries_render_inline_as_the_canonical_file_of_11_2() {
    let text = render_lock(&locked());
    let expected_tail = format!(
        "[foreign]\n\
         c = {{ image_codec = {{ headers = \"{b}\", libraries = {{ extra = {{ digest = \"{c}\", path = \"vendor/libextra/libextra.a\" }} }}, sources = {{ \"vendor/imgtest/imgtest.c\" = \"{nine}\" }} }} }}\n\
         \n\
         [adapters]\n\
         image_codec = {{ backend = \"c\", capability = \"reader.images.ImageCodec\", files = {{ \"adapters/image_codec.c\" = \"{eight}\" }}, form = \"wrapper\", isolation = \"in_process\" }}\n\
         \n\
         [targets]\n\
         c = {{ abi = 1, api = \"0.1\", compiler = \"gcc 11.4.0\", compiler_digest = \"{three}\", compiler_path = \"/usr/bin/x86_64-linux-gnu-gcc-11\", exports = {{ cboundary_paginate_ints = {{ function = \"reader.pages.paginate_ints\", id = \"reader.pages.paginate_ints\", int = \"handle\", signature = \"{seven}\" }} }}, exports_digest = \"{six}\", flags_digest = \"{four}\", prefix = \"cboundary\", runtime = \"0.1.0\", runtime_digest = \"{two}\", symbols = [\"cboundary_bind_image_codec\", \"cboundary_clean\"], system_digest = \"{five}\", target = \"x86_64-linux-gnu\" }}\n",
        b = hex('b'),
        c = hex('c'),
        nine = hex('9'),
        eight = hex('8'),
        three = hex('3'),
        seven = hex('7'),
        six = hex('6'),
        four = hex('4'),
        two = hex('2'),
        five = hex('5'),
    );
    assert!(
        text.ends_with(&format!("admit = []\n\n{expected_tail}")),
        "{text}"
    );
}

#[test]
fn the_views_read_back_what_the_setters_wrote_and_parse_round_trips() {
    let lock = locked();
    assert_eq!(lock.target_c(), Some(target()));
    assert_eq!(
        lock.c_adapters(),
        BTreeMap::from([("image_codec".to_string(), adapter())])
    );
    assert_eq!(
        lock.foreign_c(),
        BTreeMap::from([("image_codec".to_string(), foreign())])
    );
    assert!(lock.has_c_entry());
    let text = render_lock(&lock);
    let back = parse_lock(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    assert_eq!(back, lock);
    assert_eq!(back.target_c(), Some(target()));
    assert_eq!(back.c_adapters()["image_codec"], adapter());
    assert_eq!(back.foreign_c()["image_codec"], foreign());
    assert_eq!(render_lock(&back), text);
}

#[test]
fn a_lock_without_c_entries_has_empty_views() {
    let lock = base();
    assert_eq!(lock.target_c(), None);
    assert!(lock.c_adapters().is_empty());
    assert!(lock.foreign_c().is_empty());
    assert!(!lock.has_c_entry());
}

#[test]
fn every_other_key_of_the_reserved_tables_stays_opaque_and_is_kept() {
    // Python targets and adapters have closed schemas from Milestone 3
    // (python.md 9); an unknown backend still stays opaque.
    let text = format!(
        "{}[foreign]\npython = {{ wheel = \"x\" }}\n\n[adapters]\nimage_codec = {{ backend = \"future\", anything = [1, 2] }}\nother = 3\n\n[targets]\nfuture = {{ abi = \"v1\" }}\n",
        render_lock(&base())
            .split("[foreign]")
            .next()
            .unwrap()
    );
    let mut lock = parse_lock(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    assert!(lock.c_adapters().is_empty());
    assert!(!lock.has_c_entry());
    lock.set_target_c(Some(target()));
    lock.set_c_adapters(BTreeMap::from([("codec_c".to_string(), adapter())]));
    lock.set_foreign_c(BTreeMap::from([("codec_c".to_string(), foreign())]));
    assert!(lock.targets.contains_key("future"));
    assert!(lock.foreign.contains_key("python"));
    assert!(lock.adapters.contains_key("image_codec"));
    assert!(lock.adapters.contains_key("other"));
    assert_eq!(lock.c_adapters().len(), 1);
    // Replacing the backend-c entries keeps the others.
    lock.set_c_adapters(BTreeMap::new());
    assert!(lock.c_adapters().is_empty());
    assert!(lock.adapters.contains_key("image_codec"));
    assert!(lock.adapters.contains_key("other"));
    lock.set_target_c(None);
    lock.set_foreign_c(BTreeMap::new());
    assert!(!lock.targets.contains_key("c"));
    assert!(!lock.foreign.contains_key("c"));
    assert!(lock.targets.contains_key("future"));
    let again = parse_lock(&render_lock(&lock)).unwrap();
    assert_eq!(again, lock);
}

#[test]
fn a_binding_renamed_from_another_backend_to_c_replaces_its_entry() {
    let mut lock = base();
    lock.adapters.insert(
        "image_codec".to_string(),
        toml::Value::Table(toml::Table::from_iter([(
            "backend".to_string(),
            toml::Value::String("python".to_string()),
        )])),
    );
    lock.set_c_adapters(BTreeMap::from([("image_codec".to_string(), adapter())]));
    assert_eq!(lock.c_adapters()["image_codec"], adapter());
    assert_eq!(lock.adapters.len(), 1);
}

/// `render_lock(locked())` with one substring replaced, expected to be rejected.
fn rejects(from: &str, to: &str, message: &str) {
    let text = render_lock(&locked());
    assert!(text.contains(from), "{from} not in\n{text}");
    let broken = text.replacen(from, to, 1);
    match parse_lock(&broken) {
        Ok(_) => panic!("expected a LockError for {to}\n{broken}"),
        Err(e) => assert!(
            e.message.contains(message),
            "{:?} does not contain {message:?}",
            e.message
        ),
    }
}

#[test]
fn malformed_target_c_entries_are_lock_errors() {
    rejects("abi = 1, ", "", "missing required key `targets.c.abi`");
    rejects("abi = 1, ", "abi = 70000, ", "`targets.c.abi`");
    rejects("abi = 1, ", "abi = -1, ", "`targets.c.abi`");
    rejects("abi = 1, ", "abi = \"1\", ", "`targets.c.abi`");
    rejects(
        "abi = 1, ",
        "abi = 1, extra = 2, ",
        "unknown key `targets.c.extra`",
    );
    rejects(
        "api = \"0.1\"",
        "api = 1",
        "`targets.c.api` in cpl.lock must be a string",
    );
    rejects(
        &format!("runtime_digest = \"{}\"", hex('2')),
        "runtime_digest = \"ABC\"",
        "`targets.c.runtime_digest`",
    );
    rejects(
        "int = \"handle\"",
        "int = \"int8\"",
        "`targets.c.exports.cboundary_paginate_ints.int`",
    );
    rejects(
        "id = \"reader.pages.paginate_ints\"",
        "id = \"Bad Id\"",
        "`targets.c.exports.cboundary_paginate_ints.id`",
    );
    rejects(
        "int = \"handle\", ",
        "int = \"handle\", note = 1, ",
        "unknown key `targets.c.exports.cboundary_paginate_ints.note`",
    );
    rejects(
        "symbols = [\"cboundary_bind_image_codec\", \"cboundary_clean\"]",
        "symbols = [1]",
        "`targets.c.symbols`",
    );
    rejects(
        "c = { abi",
        "c = 7\nd = { abi",
        "`targets.c` in cpl.lock must be a table",
    );
}

#[test]
fn malformed_adapter_and_foreign_entries_are_lock_errors() {
    rejects(
        "form = \"wrapper\"",
        "form = \"generated\"",
        "`adapters.image_codec.form`",
    );
    rejects(
        "isolation = \"in_process\"",
        "isolation = \"thread\"",
        "`adapters.image_codec.isolation`",
    );
    rejects(
        "capability = \"reader.images.ImageCodec\", ",
        "",
        "missing required key `adapters.image_codec.capability`",
    );
    rejects(
        "form = \"wrapper\", ",
        "form = \"wrapper\", version = \"1\", ",
        "unknown key `adapters.image_codec.version`",
    );
    rejects(
        &format!("\"adapters/image_codec.c\" = \"{}\"", hex('8')),
        "\"adapters/image_codec.c\" = 8",
        "`adapters.image_codec.files.\"adapters/image_codec.c\"`",
    );
    rejects(
        &format!("headers = \"{}\", ", hex('b')),
        "",
        "missing required key `foreign.c.image_codec.headers`",
    );
    rejects(
        &format!(
            "digest = \"{}\", path = \"vendor/libextra/libextra.a\"",
            hex('c')
        ),
        &format!("digest = \"{}\"", hex('c')),
        "missing required key `foreign.c.image_codec.libraries.extra.path`",
    );
    rejects(
        "c = { image_codec = { headers",
        "c = { image_codec = 3, x = { headers",
        "`foreign.c.image_codec` in cpl.lock must be a table",
    );
    rejects(
        "c = { image_codec = { headers",
        "c = 3\nz = { image_codec = { headers",
        "`foreign.c` in cpl.lock must be a table",
    );
}

#[test]
fn c_input_digests_read_each_backend_c_binding_from_the_root() {
    let root = temp_dir("lock_c_inputs");
    write(&root, "adapters/image_codec.c", "int x;\n");
    write(&root, "vendor/imgtest/imgtest.c", "int y;\n");
    let manifest_text = r#"
[project]
name = "t"
edition = "0.1"

[interop]
targets = ["c", "python"]

[bindings.image_codec]
capability = "a.b.ImageCodec"
backend = "c"
adapter = "adapters/image_codec.c"
isolation = "in_process"

[bindings.image_codec.c]
sources = ["vendor/imgtest/imgtest.c", "vendor/missing.c"]

[bindings.missing]
capability = "a.b.Other"
backend = "c"
adapter = "adapters/missing.c"
isolation = "in_process"

[bindings.py]
capability = "a.b.Py"
backend = "python"
adapter = "x.y"
"#;
    let (manifest, diagnostics) = parse_manifest(manifest_text).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let digests = c_input_digests(&manifest, &root);
    let blake = |text: &str| Digest(blake3::hash(text.as_bytes()).to_hex().to_string());
    assert_eq!(
        digests.adapters,
        BTreeMap::from([
            (
                "image_codec".to_string(),
                BTreeMap::from([("adapters/image_codec.c".to_string(), blake("int x;\n"))])
            ),
            ("missing".to_string(), BTreeMap::new()),
        ])
    );
    assert_eq!(
        digests.sources,
        BTreeMap::from([
            (
                "image_codec".to_string(),
                BTreeMap::from([("vendor/imgtest/imgtest.c".to_string(), blake("int y;\n"))])
            ),
            ("missing".to_string(), BTreeMap::new()),
        ])
    );
}

#[test]
fn c_input_digests_never_read_outside_the_project() {
    let root = temp_dir("lock_c_outside");
    let outside = root.join("..").join("lock_c_outside_secret.c");
    std::fs::write(&outside, "secret").unwrap();
    let manifest_text = r#"
[project]
name = "t"
edition = "0.1"

[interop]
targets = ["c"]

[bindings.b]
capability = "a.b.C"
backend = "c"
adapter = "../lock_c_outside_secret.c"
isolation = "in_process"
"#;
    let (manifest, _) = parse_manifest(manifest_text).unwrap();
    let digests = c_input_digests(&manifest, &root);
    assert_eq!(digests.adapters["b"], BTreeMap::new());
    std::fs::remove_file(outside).unwrap();
}
