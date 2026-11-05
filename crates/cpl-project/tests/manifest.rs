//! `cpl.toml` (`spec/toolchain.md` 11.1): every key, every default, every
//! validation, and the binding checks (T48).

mod common;

use common::*;
use cpl_diag::json::{ExecutionBoundary, FsAccess, NetworkAccess, ProcessAccess};
use cpl_diag::{Category, Diagnostic, FileOrigin, Severity};
use cpl_project::{
    check_bindings, codes, load_project_from_tree, parse_manifest, BindingSection, BuildSection,
    EvidenceLocation, InteropSection, LoadError, LoadOptions, Manifest, ManifestError, RunSection,
    TestSection, VerificationSection, WorkPolicySection, WorkSection,
};
use serde_json::json;

fn parse(text: &str) -> (Manifest, Vec<Diagnostic>) {
    parse_manifest(text).unwrap_or_else(|e| panic!("{text}\n---\n{e:?}"))
}

fn fails(text: &str) -> ManifestError {
    match parse_manifest(text) {
        Ok((m, d)) => panic!("expected a ManifestError for\n{text}\n---\n{m:?}\n{d:?}"),
        Err(e) => e,
    }
}

/// `MANIFEST` plus `extra`.
fn with(extra: &str) -> String {
    format!("{MANIFEST}{extra}")
}

#[test]
fn a_minimal_manifest_takes_every_default() {
    let (m, diagnostics) = parse(MANIFEST);
    assert!(diagnostics.is_empty());
    assert_eq!(m.project.name, "t");
    assert_eq!(m.project.edition, "0.1");
    assert_eq!(m.project.description, None);
    assert_eq!(
        m.build,
        BuildSection {
            sources: vec!["src".to_string()]
        }
    );
    assert_eq!(m.interop, InteropSection { targets: vec![] });
    assert!(m.bindings.is_empty());
    assert_eq!(
        m.verification,
        VerificationSection {
            proof_timeout_ms: 10_000,
            admit: vec![]
        }
    );
    assert_eq!(
        m.run,
        RunSection {
            max_call_depth: 10_000
        }
    );
    assert_eq!(
        m.test,
        TestSection {
            cases: 100,
            max_shrinks: 1000,
            timeout_ms: 0
        }
    );
    assert_eq!(
        m.work,
        WorkSection {
            lease_seconds: 900,
            max_integration_retries: 5,
            evidence: EvidenceLocation::Refs,
            evidence_dir: "evidence".to_string(),
            policy: WorkPolicySection {
                write: vec!["*".to_string()],
                add: vec!["*".to_string()],
                files: vec![],
                boundary: ExecutionBoundary {
                    filesystem: FsAccess::Worktree,
                    process: ProcessAccess::None,
                    network: NetworkAccess::None,
                },
            },
        }
    );
}

#[test]
fn empty_sections_take_the_defaults_too() {
    let text = with(
        "[build]\n[interop]\n[bindings]\n[verification]\n[run]\n[test]\n[work]\n[work.policy]\n",
    );
    let (m, diagnostics) = parse(&text);
    assert!(diagnostics.is_empty());
    let (defaults, _) = parse(MANIFEST);
    assert_eq!(m, defaults);
}

#[test]
fn every_key_of_section_11_1_is_read() {
    let text = r#"
[project]
name = "reader_2"
edition = "0.1"
description = "pages"

[build]
sources = ["src", "lib/", "."]

[interop]
targets = ["python", "node", "c"]

[bindings.image_codec]
capability = "reader.images.ImageCodec"
backend = "python"
adapter = "reader_adapters.images"
isolation = "in_process"

[bindings.other]
capability = "reader.images.Other"
backend = "c"
adapter = "x"

[verification]
proof_timeout_ms = 5000
admit = ["fn_k7w3qz2mb9ptx4ve", "reader.pages.paginate"]

[run]
max_call_depth = 77

[test]
cases = 7
max_shrinks = 8
timeout_ms = 9

[work]
lease_seconds = 60
max_integration_retries = 2
evidence = "tree"
evidence_dir = "proofs/receipts"

[work.policy]
write = ["reader.*"]
add = ["reader"]
files = ["project.manifest", "project.lock", "work.pagination"]
filesystem = "worktree_and_repository"
process = "toolchain"
network = "adapters"
"#;
    let (m, diagnostics) = parse(text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(m.project.name, "reader_2");
    assert_eq!(m.project.description.as_deref(), Some("pages"));
    assert_eq!(m.build.sources, ["src", "lib", "."]);
    assert_eq!(m.interop.targets, ["python", "node", "c"]);
    assert_eq!(
        m.bindings["image_codec"],
        BindingSection {
            capability: "reader.images.ImageCodec".to_string(),
            backend: "python".to_string(),
            adapter: "reader_adapters.images".to_string(),
            isolation: Some("in_process".to_string()),
        }
    );
    assert_eq!(m.bindings["other"].isolation, None);
    assert_eq!(m.bindings.len(), 2);
    assert_eq!(m.verification.proof_timeout_ms, 5000);
    assert_eq!(
        m.verification.admit,
        ["fn_k7w3qz2mb9ptx4ve", "reader.pages.paginate"]
    );
    assert_eq!(m.run.max_call_depth, 77);
    assert_eq!(
        m.test,
        TestSection {
            cases: 7,
            max_shrinks: 8,
            timeout_ms: 9
        }
    );
    assert_eq!(m.work.lease_seconds, 60);
    assert_eq!(m.work.max_integration_retries, 2);
    assert_eq!(m.work.evidence, EvidenceLocation::Tree);
    assert_eq!(m.work.evidence_dir, "proofs/receipts");
    assert_eq!(m.work.policy.write, ["reader.*"]);
    assert_eq!(m.work.policy.add, ["reader"]);
    assert_eq!(
        m.work.policy.files,
        ["project.manifest", "project.lock", "work.pagination"]
    );
    assert_eq!(
        m.work.policy.boundary,
        ExecutionBoundary {
            filesystem: FsAccess::WorktreeAndRepository,
            process: ProcessAccess::Toolchain,
            network: NetworkAccess::Adapters,
        }
    );
}

#[test]
fn every_enumerated_value_is_accepted() {
    for (key, value) in [
        ("filesystem", "worktree"),
        ("filesystem", "worktree_and_repository"),
        ("filesystem", "host"),
        ("process", "none"),
        ("process", "toolchain"),
        ("process", "any"),
        ("network", "none"),
        ("network", "adapters"),
        ("network", "any"),
    ] {
        parse(&with(&format!("[work.policy]\n{key} = \"{value}\"\n")));
    }
    let (m, _) = parse(&with("[work]\nevidence = \"refs\"\n"));
    assert_eq!(m.work.evidence, EvidenceLocation::Refs);
    let (m, _) = parse(&with(
        "[work.policy]\nfilesystem = \"host\"\nprocess = \"any\"\nnetwork = \"any\"\n",
    ));
    assert_eq!(
        m.work.policy.boundary,
        ExecutionBoundary {
            filesystem: FsAccess::Host,
            process: ProcessAccess::Any,
            network: NetworkAccess::Any
        }
    );
    let (m, _) = parse(&with(
        "[test]\ncases = 4294967295\ntimeout_ms = 18446744073709551615\n",
    ));
    assert_eq!(m.test.cases, u32::MAX);
    assert_eq!(m.test.timeout_ms, u64::MAX);
    // The other end of each range: one case, and zero for every other count.
    let (m, _) = parse(&with(
        "[run]\nmax_call_depth = 0\n\n[test]\ncases = 1\nmax_shrinks = 0\ntimeout_ms = 0\n",
    ));
    assert_eq!(
        (m.run.max_call_depth, m.test.cases, m.test.max_shrinks),
        (0, 1, 0)
    );
}

#[test]
fn zero_test_cases_is_manifest_invalid() {
    // Reading of 11.1 (TestSection::cases): a property run with zero cases
    // would be `tested` without evaluating its body once (semantics S46),
    // vacuous evidence like the empty selection the design makes a
    // configuration failure; T53 makes a value out of range fatal.
    let text = with("[test]\ncases = 0\n");
    let e = fails(&text);
    assert_eq!(
        e.message,
        "`test.cases` must be an integer from 1 to 4294967295, found the integer 0"
    );
    assert_eq!(
        e.line,
        Some(text.lines().position(|l| l == "cases = 0").unwrap() as u32 + 1)
    );
}

// ---- project_manifest_invalid (fatal) ---------------------------------------

#[test]
fn text_that_is_not_toml_is_manifest_invalid_with_its_line() {
    let e = fails("[project]\nname = \"t\"\nedition = \n");
    assert!(
        e.message.starts_with("cpl.toml is not valid TOML: "),
        "{}",
        e.message
    );
    assert_eq!(e.line, Some(3));

    let e = fails("[project]\nname = \"t\"\nname = \"u\"\nedition = \"0.1\"\n");
    assert!(
        e.message.starts_with("cpl.toml is not valid TOML: "),
        "{}",
        e.message
    );
    assert_eq!(e.line, Some(3));
}

#[test]
fn missing_required_keys_are_manifest_invalid() {
    let e = fails("[build]\nsources = [\"src\"]\n");
    assert_eq!(e.message, "missing required table `[project]` in cpl.toml");
    assert_eq!(e.line, None);

    let e = fails("\n[project]\nedition = \"0.1\"\n");
    assert_eq!(e.message, "missing required key `project.name` in cpl.toml");
    assert_eq!(e.line, Some(2));

    let e = fails("[project]\nname = \"t\"\n");
    assert_eq!(
        e.message,
        "missing required key `project.edition` in cpl.toml"
    );

    for (missing, rest) in [
        ("capability", "backend = \"python\"\nadapter = \"a\"\n"),
        ("backend", "capability = \"a.B\"\nadapter = \"a\"\n"),
        ("adapter", "capability = \"a.B\"\nbackend = \"python\"\n"),
    ] {
        let e = fails(&with(&format!("[bindings.codec]\n{rest}")));
        assert_eq!(
            e.message,
            format!("missing required key `bindings.codec.{missing}` in cpl.toml")
        );
        assert_eq!(e.line, Some(4));
    }
}

#[test]
fn ill_typed_keys_are_manifest_invalid() {
    let cases: &[(&str, &str)] = &[
        (
            "project = 1\n",
            "`project` must be a table, found the integer 1",
        ),
        (
            "[project]\nname = 5\nedition = \"0.1\"\n",
            "`project.name` must be a string, found the integer 5",
        ),
        (
            "[project]\nname = \"t\"\nedition = 0.1\n",
            "`project.edition` must be a string, found the float 0.1",
        ),
        (
            "[project]\nname = \"t\"\nedition = \"0.1\"\ndescription = []\n",
            "`project.description` must be a string, found an array",
        ),
        (
            "[project]\nname = \"t\"\nedition = \"0.1\"\nbuild = 3\n",
            "",
        ),
    ];
    for (text, message) in cases {
        if message.is_empty() {
            // `project.build` is an unknown key, not an ill-typed one.
            parse(text);
            continue;
        }
        assert_eq!(fails(text).message, *message, "{text}");
    }

    let extra: &[(&str, &str)] = &[
        ("[build]\nsources = \"src\"\n", "`build.sources` must be an array of strings, found the string \"src\""),
        ("[build]\nsources = [\"src\", 2]\n", "`build.sources[1]` must be a string, found the integer 2"),
        ("[interop]\ntargets = \"python\"\n", "`interop.targets` must be an array of strings, found the string \"python\""),
        ("[bindings]\ncodec = 1\n", "`bindings.codec` must be a table, found the integer 1"),
        ("[bindings.codec]\ncapability = 1\nbackend = \"python\"\nadapter = \"a\"\n", "`bindings.codec.capability` must be a string, found the integer 1"),
        ("[bindings.codec]\ncapability = \"a.B\"\nbackend = \"python\"\nadapter = \"a\"\nisolation = true\n", "`bindings.codec.isolation` must be a string, found the boolean true"),
        ("[verification]\nproof_timeout_ms = \"10\"\n", "`verification.proof_timeout_ms` must be an integer from 0 to 18446744073709551615, found the string \"10\""),
        ("[verification]\nproof_timeout_ms = -1\n", "`verification.proof_timeout_ms` must be an integer from 0 to 18446744073709551615, found the integer -1"),
        ("[verification]\nadmit = \"x\"\n", "`verification.admit` must be an array of strings, found the string \"x\""),
        ("[run]\nmax_call_depth = 4294967296\n", "`run.max_call_depth` must be an integer from 0 to 4294967295, found the integer 4294967296"),
        ("[run]\nmax_call_depth = 1.5\n", "`run.max_call_depth` must be an integer from 0 to 4294967295, found the float 1.5"),
        ("[test]\ncases = -1\n", "`test.cases` must be an integer from 1 to 4294967295, found the integer -1"),
        ("[test]\nmax_shrinks = \"1000\"\n", "`test.max_shrinks` must be an integer from 0 to 4294967295, found the string \"1000\""),
        ("[test]\ntimeout_ms = true\n", "`test.timeout_ms` must be an integer from 0 to 18446744073709551615, found the boolean true"),
        ("[work]\nlease_seconds = []\n", "`work.lease_seconds` must be an integer from 0 to 18446744073709551615, found an array"),
        ("[work]\nmax_integration_retries = -5\n", "`work.max_integration_retries` must be an integer from 0 to 4294967295, found the integer -5"),
        ("[work]\nevidence = 1\n", "`work.evidence` must be a string, found the integer 1"),
        ("[work]\nevidence_dir = 1\n", "`work.evidence_dir` must be a string, found the integer 1"),
        ("[work]\npolicy = \"open\"\n", "`work.policy` must be a table, found the string \"open\""),
        ("[work.policy]\nwrite = \"*\"\n", "`work.policy.write` must be an array of strings, found the string \"*\""),
        ("[work.policy]\nadd = [1]\n", "`work.policy.add[0]` must be a string, found the integer 1"),
        ("[work.policy]\nfiles = {}\n", "`work.policy.files` must be an array of strings, found a table"),
        ("[work.policy]\nnetwork = 0\n", "`work.policy.network` must be a string, found the integer 0"),
    ];
    for (text, message) in extra {
        let text = with(text);
        let e = fails(&text);
        assert_eq!(e.message, *message, "{text}");
        assert!(e.line.is_some(), "{text}");
    }

    // A top-level key must come before the first table header.
    let e = fails(&format!("build = \"src\"\n{MANIFEST}"));
    assert_eq!(
        e.message,
        "`build` must be a table, found the string \"src\""
    );
    assert_eq!(e.line, Some(1));
}

#[test]
fn values_outside_their_domain_are_manifest_invalid() {
    let e = fails("[project]\nname = \"Reader\"\nedition = \"0.1\"\n");
    assert!(
        e.message
            .starts_with("`project.name` must match ^[a-z][a-z0-9_]*$"),
        "{}",
        e.message
    );
    assert_eq!(e.line, Some(2));
    for bad in ["", "1abc", "_a", "a-b", "a b", "é"] {
        fails(&format!("[project]\nname = {bad:?}\nedition = \"0.1\"\n"));
    }
    for good in ["a", "a1", "a_b_", "reader"] {
        parse(&format!("[project]\nname = {good:?}\nedition = \"0.1\"\n"));
    }

    for bad in [
        "",
        "/src",
        "src//lib",
        "./src",
        "src/../lib",
        "..",
        "src\\lib",
        "/",
    ] {
        let e = fails(&with(&format!("[build]\nsources = [{bad:?}]\n")));
        assert!(
            e.message
                .starts_with("`build.sources` entries are project-relative directories"),
            "{bad:?}: {}",
            e.message
        );
    }
    for (good, stored) in [("src/", "src"), ("./", "."), (".", "."), ("a/b/c", "a/b/c")] {
        let (m, _) = parse(&with(&format!("[build]\nsources = [{good:?}]\n")));
        assert_eq!(m.build.sources, [stored]);
    }

    let e = fails(&with("[interop]\ntargets = [\"python\", \"ruby\"]\n"));
    assert_eq!(
        e.message,
        "`interop.targets` entries must be \"python\", \"node\", \"c\", \"rust\", \"go\", \"java\", or \"kotlin\", found \"ruby\""
    );

    let e = fails(&with("[verification]\nadmit = [\"not an id\"]\n"));
    assert!(
        e.message
            .starts_with("`verification.admit[0]` must be a stable id"),
        "{}",
        e.message
    );

    let e = fails(&with("[work]\nevidence = \"disk\"\n"));
    assert_eq!(
        e.message,
        "`work.evidence` must be \"refs\" or \"tree\", found \"disk\""
    );
    let e = fails(&with("[work]\nevidence_dir = \"../outside\"\n"));
    assert!(
        e.message
            .starts_with("`work.evidence_dir` must be a project-relative directory"),
        "{}",
        e.message
    );

    let e = fails(&with("[work.policy]\nfilesystem = \"everything\"\n"));
    assert_eq!(
        e.message,
        "`work.policy.filesystem` must be \"worktree\", \"worktree_and_repository\", or \"host\", found \"everything\""
    );
    let e = fails(&with("[work.policy]\nprocess = \"some\"\n"));
    assert_eq!(
        e.message,
        "`work.policy.process` must be \"none\", \"toolchain\", or \"any\", found \"some\""
    );
    let e = fails(&with("[work.policy]\nnetwork = \"all\"\n"));
    assert_eq!(
        e.message,
        "`work.policy.network` must be \"none\", \"adapters\", or \"any\", found \"all\""
    );
    for bad in ["src/a.cpl", "work.", "work.a/b", "project"] {
        let e = fails(&with(&format!("[work.policy]\nfiles = [{bad:?}]\n")));
        assert!(
            e.message
                .starts_with("`work.policy.files` entries are file selectors"),
            "{bad}: {}",
            e.message
        );
    }
}

#[test]
fn an_unusable_manifest_is_load_error_manifest() {
    let tree = tree(&[
        ("cpl.toml", "[project]\nname = \"t\"\n"),
        ("src/a.cpl", "module a\n"),
    ]);
    match load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()) {
        Err(LoadError::Manifest {
            path,
            message,
            line,
        }) => {
            assert_eq!(path, "cpl.toml");
            assert_eq!(
                message,
                "missing required key `project.edition` in cpl.toml"
            );
            assert_eq!(line, Some(1));
        }
        other => panic!("expected LoadError::Manifest, found {other:?}"),
    }

    let mut tree = tree;
    tree.files.insert(
        "cpl.toml".to_string(),
        b"[project]\nname = \"\xff\"\n".to_vec(),
    );
    match load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()) {
        Err(LoadError::Manifest {
            path,
            message,
            line,
        }) => {
            assert_eq!(path, "cpl.toml");
            assert!(
                message.starts_with("cpl.toml is not valid UTF-8"),
                "{message}"
            );
            assert_eq!(line, Some(2));
        }
        other => panic!("expected LoadError::Manifest, found {other:?}"),
    }
}

// ---- project_manifest_unknown_key --------------------------------------------

#[test]
fn unknown_tables_and_keys_are_diagnostics_in_document_order() {
    let text = r#"[project]
name = "t"
edition = "0.1"
colour = "blue"

[extra]
x = 1

[build]
sources = ["src"]
output = "out"

[bindings.codec]
capability = "a.B"
backend = "python"
adapter = "a"
timeout = 5

[work.policy]
cpu = "all"

[work.extra]
"#;
    let text = format!("top = 1\n{text}");
    let (manifest, diagnostics) = parse(&text);
    let keys: Vec<serde_json::Value> = diagnostics.iter().map(|d| d.data["key"].clone()).collect();
    assert_eq!(
        keys,
        [
            json!("top"),
            json!("project.colour"),
            json!("extra"),
            json!("build.output"),
            json!("bindings.codec.timeout"),
            json!("work.policy.cpu"),
            json!("work.extra"),
        ]
    );
    for d in &diagnostics {
        assert_eq!(d.code, codes::PROJECT_MANIFEST_UNKNOWN_KEY);
        assert_eq!(d.category, Category::Project);
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.span, None);
    }
    assert_eq!(
        diagnostics[1].message,
        "unknown key `colour` in `[project]` of cpl.toml"
    );
    assert_eq!(
        diagnostics[2].message,
        "unknown table `[extra]` in cpl.toml"
    );
    assert_eq!(diagnostics[0].message, "unknown key `top` in cpl.toml");
    // The rest of the manifest is still read.
    assert_eq!(manifest.bindings["codec"].backend, "python");
}

#[test]
fn unknown_keys_with_odd_names_are_quoted() {
    let (_, diagnostics) = parse(&with("[test]\n\"my key\" = 1\n"));
    assert_eq!(diagnostics[0].data["key"], json!("test.\"my key\""));
}

// ---- unsupported_edition --------------------------------------------------------

#[test]
fn another_edition_is_unsupported_edition_and_the_manifest_still_loads() {
    let (m, diagnostics) = parse("[project]\nname = \"t\"\nedition = \"0.2\"\n");
    assert_eq!(m.project.edition, "0.2");
    assert_eq!(codes_of(&diagnostics), ["unsupported_edition"]);
    let d = &diagnostics[0];
    assert_eq!(d.category, Category::Resolve);
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.span, None);
    assert_eq!(d.data["found"], json!("0.2"));
}

#[test]
fn manifest_diagnostics_reach_the_project() {
    let tree = tree(&[
        (
            "cpl.toml",
            "[project]\nname = \"t\"\nedition = \"1.0\"\nfoo = 1\n",
        ),
        ("src/a.cpl", "module a\n"),
    ]);
    let project = load_tree(&tree, false);
    // Stage order: project diagnostics before resolve diagnostics.
    assert_eq!(
        codes(&project.diagnostics),
        ["project_manifest_unknown_key", "unsupported_edition"]
    );
}

fn codes_of(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    codes(diagnostics)
}

// ---- binding checks (T48) ---------------------------------------------------------

fn binding_manifest(targets: &str, binding: &str) -> Manifest {
    parse(&with(&format!(
        "[interop]\ntargets = {targets}\n\n[bindings.codec]\n{binding}"
    )))
    .0
}

fn known(path: &str) -> bool {
    path == "cases.lib.Codec"
}

#[test]
fn a_valid_binding_has_no_diagnostics() {
    let m = binding_manifest(
        "[\"python\"]",
        "capability = \"cases.lib.Codec\"\nbackend = \"python\"\nadapter = \"a\"\nisolation = \"worker\"\n",
    );
    assert!(check_bindings(&m, known).is_empty());
    let m = binding_manifest(
        "[\"c\"]",
        "capability = \"cases.lib.Codec\"\nbackend = \"c\"\nadapter = \"a\"\nisolation = \"in_process\"\n",
    );
    assert!(check_bindings(&m, known).is_empty());
}

#[test]
fn project_binding_unknown_capability() {
    let m = binding_manifest(
        "[\"python\"]",
        "capability = \"cases.lib.Nope\"\nbackend = \"python\"\nadapter = \"a\"\n",
    );
    let diagnostics = check_bindings(&m, known);
    assert_eq!(codes(&diagnostics), ["project_binding_unknown_capability"]);
    let d = &diagnostics[0];
    assert_eq!(
        (d.category, d.severity, d.span),
        (Category::Project, Severity::Error, None)
    );
    assert_eq!(d.data["binding"], json!("codec"));
    assert_eq!(d.data["key"], json!("bindings.codec.capability"));
    assert_eq!(d.data["found"], json!("cases.lib.Nope"));
}

#[test]
fn project_binding_target_not_enabled() {
    let m = binding_manifest(
        "[\"node\"]",
        "capability = \"cases.lib.Codec\"\nbackend = \"python\"\nadapter = \"a\"\n",
    );
    let diagnostics = check_bindings(&m, known);
    assert_eq!(codes(&diagnostics), ["project_binding_target_not_enabled"]);
    assert_eq!(diagnostics[0].data["targets"], json!(["node"]));
    assert_eq!(diagnostics[0].data["key"], json!("bindings.codec.backend"));
}

#[test]
fn project_binding_bad_backend_alone_never_with_target_not_enabled() {
    // T48: the backend is checked for membership first; an invalid backend
    // is never also reported as a target that is not enabled.
    for targets in ["[]", "[\"python\", \"node\", \"c\"]"] {
        let m = binding_manifest(
            targets,
            "capability = \"cases.lib.Codec\"\nbackend = \"ruby\"\nadapter = \"a\"\n",
        );
        let diagnostics = check_bindings(&m, known);
        assert_eq!(
            codes(&diagnostics),
            ["project_binding_bad_backend"],
            "{targets}"
        );
        assert_eq!(diagnostics[0].data["found"], json!("ruby"));
    }
}

#[test]
fn project_binding_bad_isolation() {
    let m = binding_manifest(
        "[\"python\"]",
        "capability = \"cases.lib.Codec\"\nbackend = \"python\"\nadapter = \"a\"\nisolation = \"thread\"\n",
    );
    let diagnostics = check_bindings(&m, known);
    assert_eq!(codes(&diagnostics), ["project_binding_bad_isolation"]);
    assert_eq!(
        diagnostics[0].data["key"],
        json!("bindings.codec.isolation")
    );
}

#[test]
fn one_binding_carries_at_most_one_diagnostic_per_key() {
    let m = binding_manifest(
        "[]",
        "capability = \"cases.lib.Nope\"\nbackend = \"python\"\nadapter = \"a\"\nisolation = \"thread\"\n",
    );
    assert_eq!(
        codes(&check_bindings(&m, known)),
        [
            "project_binding_unknown_capability",
            "project_binding_target_not_enabled",
            "project_binding_bad_isolation",
        ]
    );
    let m = binding_manifest(
        "[]",
        "capability = \"cases.lib.Nope\"\nbackend = \"go\"\nadapter = \"a\"\nisolation = \"thread\"\n",
    );
    assert_eq!(
        codes(&check_bindings(&m, known)),
        [
            "project_binding_unknown_capability",
            "project_binding_bad_backend",
            "project_binding_bad_isolation",
        ]
    );
}

#[test]
fn bindings_are_checked_in_name_order() {
    let text = with(
        "[bindings.zeta]\ncapability = \"x.Z\"\nbackend = \"python\"\nadapter = \"a\"\n\n[bindings.alpha]\ncapability = \"x.A\"\nbackend = \"python\"\nadapter = \"a\"\n",
    );
    let (m, _) = parse(&text);
    let diagnostics = check_bindings(&m, |_| true);
    let names: Vec<&serde_json::Value> = diagnostics.iter().map(|d| &d.data["binding"]).collect();
    assert_eq!(names, [&json!("alpha"), &json!("zeta")]);
}
