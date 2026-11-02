//! Adversarial tests of the C target's project files (track E, Milestone 2):
//! the keys of `[interop.c]` and `[bindings.<name>.c]` with every value they
//! admit and refuse (`spec/toolchain.md` 11.1, T53's fatal-versus-diagnostic
//! split), `check_c_bindings` with the exact code and payload of every row
//! of 11.1's table (T48: one diagnostic per key, T54 for links), the
//! `c_input_digests` of 11.2, the closed C schemas of `cpl.lock` and the
//! canonical inline form of `render_lock` (11.2, T75), and the S49 JSON of
//! the C diagnostics (1.4.2, T5).
//!
//! Every expectation is the specification's answer; where it is silent the
//! comment says which reading of `design-doc.md` the test takes.

mod common;

use common::*;
use cpl_diag::json::Digest;
use cpl_diag::{Category, Diagnostic, Severity, SourceMap, StableId};
use cpl_project::{
    c_input_digests, check_bindings, check_c_bindings, codes, parse_lock, parse_manifest,
    render_lock, AdapterC, AdapterForm, AdapterPython, ArtifactKind, ForeignC, IntMapping, Lock,
    LockToolchain, LockedExport, LockedLibrary, Manifest, ManifestError, TargetC,
    VerificationSection,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

// ---- helpers ----------------------------------------------------------------------

/// The minimal manifest with `[interop] targets = <targets>` and `extra`.
fn c_toml(targets: &str, extra: &str) -> String {
    format!("{MANIFEST}\n[interop]\ntargets = {targets}\n{extra}")
}

fn ok(text: &str) -> (Manifest, Vec<Diagnostic>) {
    parse_manifest(text)
        .unwrap_or_else(|e| panic!("expected the manifest to parse:\n{text}\n---\n{e:?}"))
}

fn fatal(text: &str) -> ManifestError {
    match parse_manifest(text) {
        Ok((m, d)) => panic!(
            "expected the fatal project_manifest_invalid (T53) for\n{text}\n---\n{:?}\n{d:?}",
            m.interop_c
        ),
        Err(e) => e,
    }
}

/// The 1-based line of the first line of `text` equal to `line`.
fn line_of(text: &str, line: &str) -> u32 {
    text.lines()
        .position(|l| l == line)
        .unwrap_or_else(|| panic!("{line:?} is no line of\n{text}")) as u32
        + 1
}

fn data(d: &Diagnostic) -> Value {
    Value::Object(d.data.clone())
}

fn codes_of(diagnostics: &[Diagnostic]) -> Vec<&str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

/// `check_c_bindings` on a manifest that parses without diagnostics; every
/// diagnostic is checked to be a span-less `project` error of the code table
/// (11.1: "Every code of the table but the fatal `project_manifest_invalid`
/// is a span-less diagnostic of category `project`").
fn c_check(text: &str, root: Option<&Path>) -> Vec<Diagnostic> {
    let (m, diagnostics) = ok(text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let out = check_c_bindings(&m, root);
    for d in &out.diagnostics {
        assert!(d.span.is_none(), "{d:?}");
        assert_eq!(d.category, Category::Project, "{d:?}");
        assert_eq!(d.severity, Severity::Error, "{d:?}");
        assert!(d.symbol.is_none() && d.qualified_name.is_none(), "{d:?}");
        assert!(codes::PROJECT.contains(&d.code), "{}", d.code);
    }
    out.diagnostics
}

/// A backend-`c` binding table named `name` (written as a TOML key) on
/// capability `a.b.Codec`, with `keys` appended.
fn c_binding(name: &str, keys: &str) -> String {
    format!("\n[bindings.{name}]\ncapability = \"a.b.Codec\"\nbackend = \"c\"\n{keys}")
}

const IN_PROCESS: &str = "isolation = \"in_process\"\n";

// ---- [interop.c]: T53 applies to every value ---------------------------------------

#[test]
fn the_prefix_domain_through_parse_manifest_with_the_line_of_the_value() {
    // toolchain 11.1: ^[a-z][a-z0-9]*(_[a-z0-9]+)*$, at most 31 characters,
    // not starting with `cpl`.
    let at_limit = "a".repeat(31);
    let mixed_limit = format!("ab_{}", "9".repeat(28));
    for good in [
        "a",
        "a1",
        "a_1",
        "x1_y2_z3",
        "cp",
        "c_pl",
        "xcpl",
        "reader_v2",
        at_limit.as_str(),
        mixed_limit.as_str(),
    ] {
        let text = c_toml("[\"c\"]", &format!("\n[interop.c]\nprefix = \"{good}\"\n"));
        let (m, diagnostics) = ok(&text);
        assert!(diagnostics.is_empty(), "{good}: {diagnostics:?}");
        assert_eq!(m.c_prefix(), good);
        assert_eq!(m.interop_c.unwrap().prefix.as_deref(), Some(good));
    }
    let too_long = "a".repeat(32);
    for bad in [
        too_long.as_str(),
        "cpl",
        "cplx",
        "cpl_a",
        "cpl1",
        "_a",
        "a_",
        "a__b",
        "a_b_",
        "A",
        "aB",
        "a-b",
        "a.b",
        "",
        " a",
        "a b",
        "1a",
        "9",
        "\u{e9}t\u{e9}",
    ] {
        let line = format!("prefix = \"{bad}\"");
        let text = c_toml("[\"c\"]", &format!("\n[interop.c]\n{line}\n"));
        let e = fatal(&text);
        assert_eq!(e.line, Some(line_of(&text, &line)), "{bad:?}: {e:?}");
    }
}

#[test]
fn the_prefix_value_is_checked_even_when_c_is_not_a_target() {
    // T53: every value of [interop.c] is checked when the manifest is parsed;
    // the table without "c" in targets is the diagnostic
    // project_c_target_not_enabled, but a value outside its domain is fatal.
    for targets in ["[]", "[\"python\"]", "[\"node\", \"python\"]"] {
        for bad in ["a__b", "cpl", "Reader"] {
            fatal(&c_toml(
                targets,
                &format!("\n[interop.c]\nprefix = \"{bad}\"\n"),
            ));
        }
        for line in [
            "abi = 65536",
            "artifacts = [\"dll\"]",
            "type_names = { \"a.B\" = \"B\" }",
        ] {
            fatal(&c_toml(targets, &format!("\n[interop.c]\n{line}\n")));
        }
        // A well-formed table parses (and is then check_c_bindings' to report).
        let (m, diagnostics) = ok(&c_toml(targets, "\n[interop.c]\nprefix = \"ok\"\n"));
        assert!(diagnostics.is_empty());
        assert_eq!(m.interop_c.unwrap().prefix.as_deref(), Some("ok"));
    }
}

#[test]
fn with_c_targeted_the_project_name_must_be_a_prefix_unless_prefix_is_set() {
    // 11.1: "When "c" is in [interop] targets the effective prefix (prefix,
    // else [project] name) must match the prefix pattern, else
    // project_manifest_invalid". `cpl`, `cplreader`, `x__y`, and `proj_` are
    // valid project names (^[a-z][a-z0-9_]*$) but no C prefixes.
    for name in [
        "cpl",
        "cplreader",
        "x__y",
        "proj_",
        "a_b_c_d_e_f_g_h_i_j_k_l_m_n_o_p_q",
    ] {
        let base = format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n");
        for targets in ["[\"c\"]", "[\"python\", \"c\"]", "[\"c\", \"c\"]"] {
            // An [interop.c] table without `prefix` does not help.
            for extra in ["", "\n[interop.c]\nabi = 1\n"] {
                let e = fatal(&format!("{base}\n[interop]\ntargets = {targets}\n{extra}"));
                assert_eq!(e.line, Some(2), "{name} {targets}: {e:?}");
            }
            let (m, diagnostics) = ok(&format!(
                "{base}\n[interop]\ntargets = {targets}\n\n[interop.c]\nprefix = \"pkg\"\n"
            ));
            assert!(diagnostics.is_empty());
            assert_eq!(m.c_prefix(), "pkg");
            assert_eq!(m.project.name, name);
        }
        // Without "c" the name is never a C prefix and nothing is checked.
        let (m, _) = ok(&format!("{base}\n[interop]\ntargets = [\"python\"]\n"));
        assert_eq!(m.c_prefix(), name, "c_prefix falls back to the name");
    }
    // A name that is a prefix needs no [interop.c].
    let (m, diagnostics) = ok(&c_toml("[\"c\"]", ""));
    assert!(diagnostics.is_empty());
    assert_eq!(m.interop_c, None);
    assert_eq!(m.c_prefix(), "t");
}

#[test]
fn abi_is_every_integer_from_0_to_65535_in_any_toml_spelling() {
    for (spelling, value) in [
        ("0", 0u16),
        ("1", 1),
        ("65535", 65535),
        ("+7", 7),
        // TOML integers may carry `_` separators and a radix prefix
        // (TOML 1.0 "Integer"); the domain is the value, not the spelling.
        ("1_000", 1000),
        ("0x10", 16),
        ("0o17", 15),
        ("0b101", 5),
    ] {
        let text = c_toml("[\"c\"]", &format!("\n[interop.c]\nabi = {spelling}\n"));
        let (m, _) = ok(&text);
        assert_eq!(m.interop_c.unwrap().abi, value, "abi = {spelling}");
    }
    for bad in [
        "65536",
        "-1",
        "100_000",
        "1.0",
        "1.5",
        "true",
        "\"1\"",
        "[1]",
        "{ major = 1 }",
        "1979-05-27",
    ] {
        let line = format!("abi = {bad}");
        let text = c_toml("[\"c\"]", &format!("\n[interop.c]\n{line}\n"));
        let e = fatal(&text);
        assert_eq!(e.line, Some(line_of(&text, &line)), "{bad}: {e:?}");
    }
}

#[test]
fn artifacts_admit_the_three_kinds_and_count_a_repeat_once() {
    for kind in ArtifactKind::ALL {
        let text = c_toml(
            "[\"c\"]",
            &format!("\n[interop.c]\nartifacts = [\"{}\"]\n", kind.as_str()),
        );
        assert_eq!(ok(&text).0.interop_c.unwrap().artifacts, [kind]);
    }
    let text = c_toml(
        "[\"c\"]",
        "\n[interop.c]\nartifacts = [\"static\", \"static\", \"shared\", \"static\", \"shared\"]\n",
    );
    let artifacts = ok(&text).0.interop_c.unwrap().artifacts;
    let mut sorted = artifacts.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        artifacts.len(),
        2,
        "a repeated entry counts once: {artifacts:?}"
    );
    assert_eq!(sorted, [ArtifactKind::Shared, ArtifactKind::Static]);
    // An empty array builds the headers and sources alone.
    let text = c_toml("[\"c\"]", "\n[interop.c]\nartifacts = []\n");
    assert!(ok(&text).0.interop_c.unwrap().artifacts.is_empty());
    for bad in [
        "[\"Shared\"]",
        "[\"shared \"]",
        "[\"dylib\"]",
        "[\"header\"]",
        "[\"\"]",
        "[1]",
        "[[\"shared\"]]",
        "\"static\"",
        "{ shared = true }",
    ] {
        let line = format!("artifacts = {bad}");
        let text = c_toml("[\"c\"]", &format!("\n[interop.c]\n{line}\n"));
        let e = fatal(&text);
        assert_eq!(e.line, Some(line_of(&text, &line)), "{bad}: {e:?}");
    }
}

#[test]
fn compiler_and_the_flag_arrays() {
    for good in [
        "cc",
        "gcc-11",
        "/opt/gcc/bin/gcc",
        "../tools/cc",
        "C:/mingw/bin/gcc.exe",
    ] {
        let text = c_toml(
            "[\"c\"]",
            &format!("\n[interop.c]\ncompiler = \"{good}\"\n"),
        );
        assert_eq!(
            ok(&text).0.interop_c.unwrap().compiler.as_deref(),
            Some(good)
        );
    }
    // cflags and ldflags are arrays of strings, any strings (11.1).
    let text = c_toml(
        "[\"c\"]",
        "\n[interop.c]\ncflags = [\"\", \"-O0\", \"-DX=\\\"y z\\\"\"]\nldflags = []\n",
    );
    let c = ok(&text).0.interop_c.unwrap();
    assert_eq!(c.cflags, ["", "-O0", "-DX=\"y z\""]);
    assert!(c.ldflags.is_empty());
    for bad in [
        "compiler = 1",
        "compiler = [\"cc\"]",
        "compiler = { path = \"cc\" }",
        "cflags = [[]]",
        "cflags = [1, \"-O2\"]",
        "ldflags = {}",
        "ldflags = \"-lm\"",
    ] {
        let text = c_toml("[\"c\"]", &format!("\n[interop.c]\n{bad}\n"));
        let e = fatal(&text);
        assert_eq!(e.line, Some(line_of(&text, bad)), "{bad}: {e:?}");
    }
}

#[test]
fn type_names_values_follow_their_pattern_and_keys_are_kept_verbatim() {
    let text = c_toml(
        "[\"c\"]",
        "\n[interop.c]\ntype_names = { \"List<List<Int>>\" = \"pages\", \"Result<a.b.C, std.foreign.ForeignError>\" = \"c_result\", \"a.b.C\" = \"c_\", \"a.b.D\" = \"d1\", \"anything at all\" = \"x\" }\n",
    );
    let names = ok(&text).0.interop_c.unwrap().type_names;
    assert_eq!(
        names,
        BTreeMap::from([
            ("List<List<Int>>".to_string(), "pages".to_string()),
            (
                "Result<a.b.C, std.foreign.ForeignError>".to_string(),
                "c_result".to_string()
            ),
            ("a.b.C".to_string(), "c_".to_string()),
            ("a.b.D".to_string(), "d1".to_string()),
            ("anything at all".to_string(), "x".to_string()),
        ])
    );
    for bad in [
        "\"_page\"",
        "\"1page\"",
        "\"Page\"",
        "\"\"",
        "\"page-x\"",
        "\"page x\"",
        "\"p\u{e4}ge\"",
        "1",
        "true",
        "[\"page\"]",
        "{ name = \"page\" }",
    ] {
        let line = format!("type_names = {{ \"a.B\" = {bad} }}");
        let text = c_toml("[\"c\"]", &format!("\n[interop.c]\n{line}\n"));
        let e = fatal(&text);
        assert_eq!(e.line, Some(line_of(&text, &line)), "{bad}: {e:?}");
    }
    // The table form of type_names is read like the inline form.
    let text = c_toml(
        "[\"c\"]",
        "\n[interop.c]\n\n[interop.c.type_names]\n\"a.b.C\" = \"cee\"\n",
    );
    let (m, diagnostics) = ok(&text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(m.interop_c.unwrap().type_names["a.b.C"], "cee");
}

#[test]
fn export_entries_are_not_resolved_or_symbol_checked_when_parsing() {
    // T53's split: a target that names nothing and an explicit symbol outside
    // its pattern are interop::plan's diagnostics (project_export_unknown,
    // project_export_symbol_invalid), never project_manifest_invalid.
    let text = c_toml(
        "[\"c\"]",
        r#"
[interop.c]
exports = [
    "",
    "no.such.thing",
    "not even a path",
    { function = "f", symbol = "NOT A SYMBOL" },
    { function = "f", symbol = "" },
    { function = "f", symbol = "other_prefix_f", int = "uint64" },
    { function = "f" },
    "f",
    { int = "int32", function = "f" },
]
"#,
    );
    let (m, diagnostics) = ok(&text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let exports = m.interop_c.unwrap().exports.unwrap();
    let got: Vec<(&str, Option<&str>, IntMapping)> = exports
        .iter()
        .map(|e| (e.function.as_str(), e.symbol.as_deref(), e.int))
        .collect();
    assert_eq!(
        got,
        [
            ("", None, IntMapping::Handle),
            ("no.such.thing", None, IntMapping::Handle),
            ("not even a path", None, IntMapping::Handle),
            ("f", Some("NOT A SYMBOL"), IntMapping::Handle),
            ("f", Some(""), IntMapping::Handle),
            ("f", Some("other_prefix_f"), IntMapping::Uint64),
            ("f", None, IntMapping::Handle),
            ("f", None, IntMapping::Handle),
            ("f", None, IntMapping::Int32),
        ]
    );
}

#[test]
fn export_entries_of_another_shape_are_fatal() {
    // 11.1: "an exports entry that is neither a string nor a table of
    // function, symbol, and int; an int outside its five values".
    for bad in [
        "exports = {}",
        "exports = \"f\"",
        "exports = [[\"f\"]]",
        "exports = [{}]",
        "exports = [true]",
        "exports = [1.5]",
        "exports = [{ function = \"f\", int = 64 }]",
        "exports = [{ function = \"f\", int = \"Int64\" }]",
        "exports = [{ function = \"f\", int = \"\" }]",
        "exports = [{ function = \"f\", int = \"i64\" }]",
        "exports = [{ function = \"f\", int = \"usize\" }]",
        "exports = [{ function = \"f\", symbol = 1 }]",
        "exports = [{ function = [\"f\"] }]",
        "exports = [{ function = \"f\", symbol = \"t_f\", extra = 1 }]",
        "exports = [{ fn = \"f\" }]",
        "exports = [\"f\", { symbol = \"t_g\" }]",
    ] {
        let text = c_toml("[\"c\"]", &format!("\n[interop.c]\n{bad}\n"));
        let e = fatal(&text);
        assert_eq!(e.line, Some(line_of(&text, bad)), "{bad}: {e:?}");
    }
}

#[test]
fn unknown_tables_and_keys_around_the_c_tables_are_diagnostics_in_document_order() {
    // T35: an unknown table or key is project_manifest_unknown_key (exit 1),
    // never fatal; the manifest still loads with every known value.
    let text = c_toml(
        "[\"c\"]",
        r#"
[interop.c]
prefix = "pkg"
soname = "libx.so"

[interop.c.extra]
x = 1

[interop.python]
version = "3.12"
abi = "abi3"

[bindings.b]
capability = "a.b.Codec"
backend = "c"
adapter = "b.c"
isolation = "in_process"
mode = "fast"

[bindings.b.c]
sources = ["v/x.c"]
linkage = "static"

[bindings.b.c.more]
y = 2
"#,
    );
    let (m, diagnostics) = ok(&text);
    let keys: Vec<Value> = diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d.code, codes::PROJECT_MANIFEST_UNKNOWN_KEY);
            assert_eq!(d.category, Category::Project);
            assert_eq!(d.severity, Severity::Error);
            assert!(d.span.is_none());
            d.data["key"].clone()
        })
        .collect();
    assert_eq!(
        keys,
        [
            json!("interop.c.soname"),
            json!("interop.c.extra"),
            json!("interop.python.abi"),
            json!("bindings.b.mode"),
            json!("bindings.b.c.linkage"),
            json!("bindings.b.c.more"),
        ]
    );
    assert_eq!(m.c_prefix(), "pkg");
    assert_eq!(m.binding_c["b"].sources, ["v/x.c"]);
}

#[test]
fn a_binding_c_subtable_is_parsed_and_validated_on_every_backend() {
    // T53 applies "to every value of [interop.c] and of [bindings.<name>.c]",
    // whatever the binding's backend; the misplaced subtable itself is the
    // diagnostic project_binding_c_options_misplaced.
    for backend in ["python", "node", "ruby"] {
        let head = format!(
            "\n[bindings.p]\ncapability = \"a.b.Codec\"\nbackend = \"{backend}\"\nadapter = \"x\"\n\n[bindings.p.c]\n"
        );
        for bad in [
            "sources = [\"/abs.c\"]",
            "include_dirs = [\"..\"]",
            "defines = [\"1X\"]",
            "library_dirs = [\"a/../../b\"]",
            "cflags = [1]",
        ] {
            let text = c_toml("[\"python\", \"node\"]", &format!("{head}{bad}\n"));
            let e = fatal(&text);
            assert_eq!(e.line, Some(line_of(&text, bad)), "{backend} {bad}: {e:?}");
        }
        let (m, diagnostics) = ok(&c_toml(
            "[\"python\", \"node\"]",
            &format!("{head}libraries = [\"m\"]\n"),
        ));
        assert!(diagnostics.is_empty());
        assert_eq!(m.binding_c["p"].libraries, ["m"]);
    }
}

#[test]
fn binding_c_paths_refuse_only_absolute_paths_and_dotdot_segments() {
    // 11.1: "a path of [bindings.<name>.c] that is absolute or has a `..`
    // segment" is fatal; any other project-relative spelling is kept as
    // written (the file checks come later, in check_c_bindings).
    let good = [
        "./x.c",
        "a/./b.c",
        "..x/y.c",
        "x..c",
        "a/b..",
        "...",
        "vendor/",
        "v/.hidden/x.c",
    ];
    for field in ["sources", "include_dirs", "library_dirs"] {
        let list: Vec<String> = good.iter().map(|p| format!("\"{p}\"")).collect();
        let text = c_toml(
            "[\"c\"]",
            &format!(
                "{}\n[bindings.b.c]\n{field} = [{}]\n",
                c_binding("b", &format!("adapter = \"b.c\"\n{IN_PROCESS}")),
                list.join(", ")
            ),
        );
        let (m, diagnostics) = ok(&text);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let section = &m.binding_c["b"];
        let kept = match field {
            "sources" => &section.sources,
            "include_dirs" => &section.include_dirs,
            _ => &section.library_dirs,
        };
        assert_eq!(kept, &good, "{field}: kept as written");
        for bad in [
            "/x.c",
            "/",
            "..",
            "../x",
            "a/..",
            "a/../b",
            "a/b/../../c",
            "./../x",
        ] {
            let line = format!("{field} = [\"ok.c\", \"{bad}\"]");
            let text = c_toml(
                "[\"c\"]",
                &format!(
                    "{}\n[bindings.b.c]\n{line}\n",
                    c_binding("b", &format!("adapter = \"b.c\"\n{IN_PROCESS}"))
                ),
            );
            let e = fatal(&text);
            assert_eq!(e.line, Some(line_of(&text, &line)), "{field} {bad}: {e:?}");
        }
    }
}

#[test]
fn defines_are_a_c_identifier_optionally_followed_by_equals_and_a_value() {
    // 11.1: "NAME" or "NAME=VALUE", NAME a C identifier ([A-Za-z_][A-Za-z0-9_]*);
    // the value is free (gcc's `-DX=` defines X as empty, so `X=` is NAME=VALUE
    // with an empty VALUE: the specification puts no constraint on VALUE).
    let good = [
        "X",
        "_X",
        "x1",
        "__STDC_WANT_LIB_EXT1__=1",
        "X=",
        "X=1",
        "X=a=b",
        "X=a b",
        "X=\\\"q\\\"",
        "X=-1",
    ];
    let list: Vec<String> = good.iter().map(|p| format!("\"{p}\"")).collect();
    let text = c_toml(
        "[\"c\"]",
        &format!(
            "{}\n[bindings.b.c]\ndefines = [{}]\n",
            c_binding("b", &format!("adapter = \"b.c\"\n{IN_PROCESS}")),
            list.join(", ")
        ),
    );
    let (m, _) = ok(&text);
    assert_eq!(m.binding_c["b"].defines.len(), good.len());
    assert_eq!(m.binding_c["b"].defines[6], "X=a=b");
    for bad in [
        "", "=", "=1", "1X", "9=9", "X-Y", "X Y", " X", "X =1", "X.Y=1", "\u{c4}", "X\u{e9}", "$X",
    ] {
        let line = format!("defines = [\"{bad}\"]");
        let text = c_toml(
            "[\"c\"]",
            &format!(
                "{}\n[bindings.b.c]\n{line}\n",
                c_binding("b", &format!("adapter = \"b.c\"\n{IN_PROCESS}"))
            ),
        );
        let e = fatal(&text);
        assert_eq!(e.line, Some(line_of(&text, &line)), "{bad:?}: {e:?}");
    }
}

#[test]
fn a_c_subtable_that_is_no_table_is_fatal() {
    for (extra, line) in [
        (
            format!(
                "{}c = \"opts\"\n",
                c_binding("b", &format!("adapter = \"b.c\"\n{IN_PROCESS}"))
            ),
            "c = \"opts\"",
        ),
        (
            format!(
                "{}c = []\n",
                c_binding("b", &format!("adapter = \"b.c\"\n{IN_PROCESS}"))
            ),
            "c = []",
        ),
        ("c = [\"x\"]\n".to_string(), "c = [\"x\"]"),
        ("c = true\n".to_string(), "c = true"),
    ] {
        let text = c_toml("[\"c\"]", &extra);
        let e = fatal(&text);
        assert_eq!(e.line, Some(line_of(&text, line)), "{extra}: {e:?}");
    }
    // The inline form of the subtable is the table.
    let text = c_toml(
        "[\"c\"]",
        &c_binding(
            "b",
            &format!("adapter = \"b.c\"\n{IN_PROCESS}c = {{ libraries = [\"m\"] }}\n"),
        ),
    );
    let (m, diagnostics) = ok(&text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(m.binding_c["b"].libraries, ["m"]);
}

// ---- check_c_bindings: every row of 11.1's table, with its exact payload ------------

#[test]
fn project_c_target_not_enabled_carries_exactly_key_and_targets() {
    // From Milestone 3 the Python and Node packages contain the C package, so
    // `[interop.c]` is legal with either target (spec/python.md 1); only a
    // project without any of the three leaves it without one.
    for targets in ["[\"python\"]", "[\"node\"]", "[\"python\", \"node\"]"] {
        let diagnostics = c_check(&c_toml(targets, "\n[interop.c]\n"), None);
        assert!(diagnostics.is_empty(), "{targets}: {diagnostics:?}");
    }
    for (targets, expected) in [("[]", json!([]))] {
        // An [interop.c] without any key is present all the same.
        for table in [
            "\n[interop.c]\n",
            "\n[interop.c]\nprefix = \"pkg\"\n",
            "c = {}\n",
        ] {
            let diagnostics = c_check(&c_toml(targets, table), None);
            assert_eq!(
                codes_of(&diagnostics),
                ["project_c_target_not_enabled"],
                "{table}"
            );
            assert_eq!(
                data(&diagnostics[0]),
                json!({ "key": "interop.c", "targets": expected }),
                "{targets}"
            );
        }
    }
    for targets in ["[\"c\"]", "[\"python\", \"c\"]"] {
        assert!(c_check(&c_toml(targets, "\n[interop.c]\n"), None).is_empty());
    }
}

#[test]
fn binding_names_outside_the_c_identifier_pattern_are_project_binding_name_invalid() {
    // 11.1: ^[a-z][a-z0-9_]*$ so that the grant <prefix>_bind_<name> is a C
    // identifier; the payload is exactly `binding`.
    for (key, name) in [
        ("Codec", "Codec"),
        ("codeC", "codeC"),
        ("_codec", "_codec"),
        ("1codec", "1codec"),
        ("codec-x", "codec-x"),
        ("\"codec x\"", "codec x"),
        ("\"codec.x\"", "codec.x"),
        ("\"c\u{f3}dec\"", "c\u{f3}dec"),
        ("\"\"", ""),
    ] {
        let text = c_toml(
            "[\"c\"]",
            &c_binding(key, &format!("adapter = \"a.c\"\n{IN_PROCESS}")),
        );
        let diagnostics = c_check(&text, None);
        assert_eq!(
            codes_of(&diagnostics),
            ["project_binding_name_invalid"],
            "{name:?}"
        );
        assert_eq!(data(&diagnostics[0]), json!({ "binding": name }));
    }
    for name in ["codec_", "c0dec", "a", "a__b", "z9"] {
        let text = c_toml(
            "[\"c\"]",
            &c_binding(name, &format!("adapter = \"a.c\"\n{IN_PROCESS}")),
        );
        assert!(c_check(&text, None).is_empty(), "{name}");
    }
}

#[test]
fn c_bindings_may_run_in_workers_from_milestone_3() {
    // spec/python.md 6.6: from Milestone 3 an absent isolation (the default
    // "worker") and an explicit "worker" are legal for backend c, so
    // project_binding_isolation_unsupported is no longer raised.
    for keys in [
        "",
        "isolation = \"worker\"\n",
        "isolation = \"in_process\"\n",
    ] {
        let text = c_toml(
            "[\"c\"]",
            &c_binding("codec", &format!("adapter = \"a.c\"\n{keys}")),
        );
        let diagnostics = c_check(&text, None);
        assert!(diagnostics.is_empty(), "{keys:?}: {diagnostics:?}");
    }
}

#[test]
fn an_isolation_outside_its_domain_is_one_diagnostic_for_its_key() {
    // T48 (D-236): a [bindings.*] table reports at most one diagnostic per
    // key, so a backend-c isolation that is neither `worker` nor
    // `in_process` is check_bindings' project_binding_bad_isolation alone.
    for value in [
        "thread",
        "In_Process",
        "",
        "inprocess",
        "in-process",
        "WORKER",
    ] {
        let text = c_toml(
            "[\"c\"]",
            &c_binding(
                "codec",
                &format!("adapter = \"a.c\"\nisolation = \"{value}\"\n"),
            ),
        );
        let (m, _) = ok(&text);
        let mut all = check_bindings(&m, |_| true);
        all.extend(check_c_bindings(&m, None).diagnostics);
        let about_isolation: Vec<&str> = all
            .iter()
            .filter(|d| {
                d.code == codes::PROJECT_BINDING_BAD_ISOLATION
                    || d.code == codes::PROJECT_BINDING_ISOLATION_UNSUPPORTED
            })
            .map(|d| d.code.as_str())
            .collect();
        assert_eq!(
            about_isolation,
            ["project_binding_bad_isolation"],
            "{value:?}"
        );
    }
}

#[test]
fn project_binding_adapter_kind_is_decided_by_the_suffix_alone() {
    let root = temp_dir("adv_adapter_kind");
    for adapter in [
        "x.cpp",
        "x.c.bak",
        "x.cbind.toml.bak",
        "x.toml",
        "x.cbind",
        "x.h",
        "x.C",
        "x.CBIND.TOML",
        "x.c/",
        "x.c ",
        "x",
        "c",
        "",
        "/abs/codec.h",
        "../codec.toml",
    ] {
        let text = c_toml(
            "[\"c\"]",
            &c_binding("codec", &format!("adapter = \"{adapter}\"\n{IN_PROCESS}")),
        );
        // One diagnostic for the key `adapter` (T48): the kind, never also
        // project_binding_adapter_missing.
        for r in [None, Some(root.as_path())] {
            let diagnostics = c_check(&text, r);
            assert_eq!(
                codes_of(&diagnostics),
                ["project_binding_adapter_kind"],
                "{adapter:?}"
            );
            assert_eq!(
                data(&diagnostics[0]),
                json!({ "binding": "codec", "found": adapter })
            );
        }
    }
    assert_eq!(AdapterForm::of_adapter("x.y.c"), Some(AdapterForm::Wrapper));
    assert_eq!(
        AdapterForm::of_adapter("d.cbind.toml/x.c"),
        Some(AdapterForm::Wrapper)
    );
    assert_eq!(
        AdapterForm::of_adapter("a.c.cbind.toml"),
        Some(AdapterForm::Declarative)
    );
    assert_eq!(AdapterForm::of_adapter(".c"), Some(AdapterForm::Wrapper));
}

#[test]
fn project_binding_adapter_missing_payload_and_cases() {
    let root = temp_dir("adv_adapter_missing");
    write(&root, "adapters/codec.c", "/* adapter */\n");
    std::fs::create_dir_all(root.join("adapters/dir.c")).unwrap();
    std::fs::create_dir_all(root.join("adapters/dir.cbind.toml")).unwrap();
    for adapter in [
        "adapters/absent.c",
        "adapters/dir.c",
        "adapters/dir.cbind.toml",
        "adapters/Codec.c",
        "Adapters/codec.c",
        "../adapters/codec.c",
        "adapters/../../codec.c",
        "/adapters/codec.c",
    ] {
        let text = c_toml(
            "[\"c\"]",
            &c_binding("codec", &format!("adapter = \"{adapter}\"\n{IN_PROCESS}")),
        );
        let diagnostics = c_check(&text, Some(&root));
        assert_eq!(
            codes_of(&diagnostics),
            ["project_binding_adapter_missing"],
            "{adapter}"
        );
        assert_eq!(
            data(&diagnostics[0]),
            json!({ "binding": "codec", "path": adapter })
        );
    }
    // Spellings that name the file are fine.
    for adapter in [
        "adapters/codec.c",
        "./adapters/codec.c",
        "adapters/./codec.c",
        "adapters//codec.c",
    ] {
        let text = c_toml(
            "[\"c\"]",
            &c_binding("codec", &format!("adapter = \"{adapter}\"\n{IN_PROCESS}")),
        );
        assert!(c_check(&text, Some(&root)).is_empty(), "{adapter}");
    }
}

#[cfg(unix)]
#[test]
fn an_adapter_reached_through_a_directory_link_is_missing() {
    // 11.1: an adapter that "names no regular file under the project root (a
    // link to a file counts as that file, T54)"; T54 (D-244): "a link to a
    // directory is never followed, so nothing reached only through it is a
    // member", and a link to a directory "leads to no file". An adapter
    // reached only through a linked directory therefore names no file of the
    // project, and its digest cannot be pinned (c_input_digests has no entry,
    // "check_c_bindings reports it").
    let root = temp_dir("adv_adapter_dir_link");
    write(&root, "real/codec.c", "/* adapter */\n");
    std::os::unix::fs::symlink(root.join("real"), root.join("adapters")).unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("dir_link.c")).unwrap();
    for adapter in ["adapters/codec.c", "dir_link.c"] {
        let text = c_toml(
            "[\"c\"]",
            &c_binding("codec", &format!("adapter = \"{adapter}\"\n{IN_PROCESS}")),
        );
        let diagnostics = c_check(&text, Some(&root));
        assert_eq!(
            codes_of(&diagnostics),
            ["project_binding_adapter_missing"],
            "{adapter}: T54, a directory link is never followed"
        );
        let (m, _) = ok(&text);
        assert_eq!(
            c_input_digests(&m, &root).adapters["codec"],
            BTreeMap::new(),
            "{adapter}"
        );
    }
}

#[test]
fn project_binding_path_missing_payload_for_each_key() {
    let root = temp_dir("adv_path_missing");
    write(&root, "a.c", "x");
    write(&root, "vendor/x.c", "x");
    write(&root, "vendor/file", "x");
    std::fs::create_dir_all(root.join("inc")).unwrap();
    let text = c_toml(
        "[\"c\"]",
        &format!(
            "{}\n[bindings.codec.c]\nsources = [\"vendor/x.c\", \"./vendor/x.c\", \"vendor/none.c\", \"inc\"]\ninclude_dirs = [\"inc\", \"vendor/file\", \"missing\", \".\"]\nlibrary_dirs = [\"vendor\", \"nolib\"]\nlibraries = [\"definitely_not_a_library_anywhere\"]\n",
            c_binding("codec", &format!("adapter = \"a.c\"\n{IN_PROCESS}"))
        ),
    );
    let diagnostics = c_check(&text, Some(&root));
    let found: Vec<Value> = diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d.code, codes::PROJECT_BINDING_PATH_MISSING);
            data(d)
        })
        .collect();
    // `libraries` are found by the linker, never checked here (11.1, 11.2).
    assert_eq!(
        found,
        [
            json!({ "binding": "codec", "key": "bindings.codec.c.sources", "path": "vendor/none.c" }),
            json!({ "binding": "codec", "key": "bindings.codec.c.sources", "path": "inc" }),
            json!({ "binding": "codec", "key": "bindings.codec.c.include_dirs", "path": "vendor/file" }),
            json!({ "binding": "codec", "key": "bindings.codec.c.include_dirs", "path": "missing" }),
            json!({ "binding": "codec", "key": "bindings.codec.c.library_dirs", "path": "nolib" }),
        ]
    );
}

#[test]
fn project_binding_duplicate_capability_groups_by_capability() {
    let text = c_toml(
        "[\"c\", \"python\"]",
        r#"
[bindings.zeta]
capability = "a.b.X"
backend = "c"
adapter = "z.c"
isolation = "in_process"

[bindings.alpha]
capability = "a.b.X"
backend = "c"
adapter = "a.c"
isolation = "in_process"

[bindings.mid]
capability = "a.b.X"
backend = "c"
adapter = "m.c"
isolation = "in_process"

[bindings.beta]
capability = "a.b.W"
backend = "c"
adapter = "b.c"
isolation = "in_process"

[bindings.aaa]
capability = "a.b.W"
backend = "c"
adapter = "aaa.cbind.toml"
isolation = "in_process"

[bindings.py]
capability = "a.b.X"
backend = "python"
adapter = "p"

[bindings.single]
capability = "a.b.Y"
backend = "c"
adapter = "s.c"
isolation = "in_process"
"#,
    );
    let diagnostics = c_check(&text, None);
    let found: Vec<Value> = diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d.code, codes::PROJECT_BINDING_DUPLICATE_CAPABILITY);
            data(d)
        })
        .collect();
    // One per capability, in capability order; `bindings` in name order; the
    // python binding is no backend-c binding.
    assert_eq!(
        found,
        [
            json!({ "capability": "a.b.W", "bindings": ["aaa", "beta"] }),
            json!({ "capability": "a.b.X", "bindings": ["alpha", "mid", "zeta"] }),
        ]
    );
}

#[test]
fn project_binding_c_options_misplaced_for_every_other_backend() {
    for backend in ["python", "node", "C", "ruby", "", " c"] {
        let text = c_toml(
            "[\"python\", \"node\"]",
            &format!(
                "\n[bindings.p]\ncapability = \"a.b.Codec\"\nbackend = \"{backend}\"\nadapter = \"x\"\n\n[bindings.p.c]\n"
            ),
        );
        let diagnostics = c_check(&text, None);
        assert_eq!(
            codes_of(&diagnostics),
            ["project_binding_c_options_misplaced"],
            "{backend:?}"
        );
        assert_eq!(data(&diagnostics[0]), json!({ "binding": "p" }));
    }
    // A backend that is not exactly "c" gets none of the backend-c rules.
    let text = c_toml(
        "[\"python\"]",
        "\n[bindings.Bad-Name]\ncapability = \"a.b.Codec\"\nbackend = \"C\"\nadapter = \"x.h\"\n",
    );
    assert!(c_check(&text, None).is_empty());
}

#[test]
fn the_backend_c_rules_apply_whatever_the_targets() {
    // 11.1's rows name "a backend-c binding" without a condition on
    // `[interop] targets`; check_bindings reports the backend key
    // (project_binding_target_not_enabled) and check_c_bindings the others,
    // one diagnostic per key (T48).
    let text = c_toml(
        "[\"python\"]",
        &c_binding("Codec", "adapter = \"codec.h\"\n"),
    );
    let (m, _) = ok(&text);
    assert_eq!(
        codes_of(&check_bindings(&m, |_| true)),
        ["project_binding_target_not_enabled"]
    );
    assert_eq!(
        codes_of(&check_c_bindings(&m, None).diagnostics),
        [
            "project_binding_name_invalid",
            "project_binding_adapter_kind"
        ]
    );
}

// ---- c_input_digests (11.2) --------------------------------------------------------------

#[test]
fn c_input_digests_are_blake3_of_the_bytes_by_the_paths_as_written() {
    let root = temp_dir("adv_inputs");
    write(&root, "adapters/x.c", "int x;\n");
    write(&root, "adapters/d.cbind.toml", "");
    write(&root, "vendor/a.c", "a");
    write(&root, "vendor/b.c", [0u8, 0xff, 0x80]);
    let text = c_toml(
        "[\"python\"]",
        r#"
[bindings.wrapper]
capability = "a.b.X"
backend = "c"
adapter = "./adapters/x.c"
isolation = "in_process"

[bindings.wrapper.c]
sources = ["vendor/a.c", "vendor/./b.c", "vendor/none.c"]

[bindings.declarative]
capability = "a.b.Y"
backend = "c"
adapter = "adapters/d.cbind.toml"

[bindings.py]
capability = "a.b.Z"
backend = "python"
adapter = "adapters/x.c"

[bindings.py.c]
sources = ["vendor/a.c"]
"#,
    );
    let (m, _) = ok(&text);
    let blake = |bytes: &[u8]| Digest(blake3::hash(bytes).to_hex().to_string());
    let digests = c_input_digests(&m, &root);
    // Every backend-c binding (whatever its other mistakes or the targets),
    // never another backend's.
    assert_eq!(
        digests.adapters,
        BTreeMap::from([
            (
                "declarative".to_string(),
                BTreeMap::from([("adapters/d.cbind.toml".to_string(), blake(b""))])
            ),
            (
                "wrapper".to_string(),
                BTreeMap::from([("./adapters/x.c".to_string(), blake(b"int x;\n"))])
            ),
        ])
    );
    assert_eq!(
        digests.sources,
        BTreeMap::from([
            ("declarative".to_string(), BTreeMap::new()),
            (
                "wrapper".to_string(),
                BTreeMap::from([
                    ("vendor/./b.c".to_string(), blake(&[0u8, 0xff, 0x80])),
                    ("vendor/a.c".to_string(), blake(b"a")),
                ])
            ),
        ])
    );
    for d in digests.adapters.values().flat_map(|m| m.values()) {
        assert_eq!(d.0.len(), 64);
        assert!(d
            .0
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
}

// ---- cpl.lock: the canonical inline form and the closed C schemas (11.2, T75) ----------

/// A distinct 64-character lowercase digest with hex letters in it.
fn dig(i: u8) -> String {
    format!("{}{i:02x}", &"abcdef0123456789".repeat(4)[..62])
}

/// The canonical lock text of 11.2 for the `cboundary` fixture, with every
/// digest distinct: `render_lock` must write exactly this.
fn canonical() -> String {
    format!(
        r#"# Generated by cpl. Do not edit.
version = 1

[toolchain]
cpl = "0.1.0"
edition = "0.1"
std = "{std}"

[sources]
"cpl.toml" = "{s1}"
"src/reader/images.cpl" = "{s2}"

[verification]
proof_timeout_ms = 10000
admit = []

[foreign]
c = {{ image_codec = {{ headers = "{headers}", libraries = {{ extra = {{ digest = "{l1}", path = "vendor/libextra/libextra.a" }}, "stdc++" = {{ digest = "{l2}", path = "/usr/lib/gcc/x86_64-linux-gnu/11/libstdc++.so" }} }}, sources = {{ "vendor/imgtest/imgtest.c" = "{src}" }} }} }}

[adapters]
image_codec = {{ backend = "c", capability = "reader.images.ImageCodec", files = {{ "adapters/image_codec.c" = "{file}" }}, form = "wrapper", isolation = "in_process" }}

[targets]
c = {{ abi = 1, api = "0.1", compiler = "gcc 11.4.0", compiler_digest = "{cd}", compiler_path = "/usr/bin/x86_64-linux-gnu-gcc-11", exports = {{ cboundary_paginate_ints = {{ function = "reader.pages.paginate_ints", id = "reader.pages.paginate_ints", int = "handle", signature = "{sig1}" }}, cboundary_paginate_ints_i64 = {{ function = "reader.pages.paginate_ints", id = "reader.pages.paginate_ints", int = "int64", signature = "{sig2}" }} }}, exports_digest = "{ed}", flags_digest = "{fd}", prefix = "cboundary", runtime = "0.1.0", runtime_digest = "{rd}", symbols = ["cboundary_bind_image_codec", "cboundary_clean", "cboundary_paginate_ints", "cboundary_paginate_ints_i64"], system_digest = "{sd}", target = "x86_64-linux-gnu" }}
"#,
        std = dig(0),
        s1 = dig(1),
        s2 = dig(2),
        headers = dig(10),
        l1 = dig(11),
        l2 = dig(12),
        src = dig(13),
        file = dig(14),
        cd = dig(20),
        sig1 = dig(21),
        sig2 = dig(22),
        ed = dig(23),
        fd = dig(24),
        rd = dig(25),
        sd = dig(26),
    )
}

/// The twelve digests of the C entries of [`canonical`], by the tag of [`dig`].
const C_DIGESTS: [u8; 12] = [10, 11, 12, 13, 14, 20, 21, 22, 23, 24, 25, 26];

#[test]
fn the_canonical_lock_of_11_2_parses_into_the_views_and_renders_back_byte_for_byte() {
    let text = canonical();
    let lock = parse_lock(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    let d = |i| Digest(dig(i));
    assert_eq!(
        lock.target_c(),
        Some(TargetC {
            api: "0.1".into(),
            runtime: "0.1.0".into(),
            runtime_digest: d(25),
            target: "x86_64-linux-gnu".into(),
            compiler: "gcc 11.4.0".into(),
            compiler_path: "/usr/bin/x86_64-linux-gnu-gcc-11".into(),
            compiler_digest: d(20),
            flags_digest: d(24),
            system_digest: d(26),
            prefix: "cboundary".into(),
            abi: 1,
            exports_digest: d(23),
            exports: BTreeMap::from([
                (
                    "cboundary_paginate_ints".to_string(),
                    LockedExport {
                        function: "reader.pages.paginate_ints".into(),
                        id: StableId::parse("reader.pages.paginate_ints").unwrap(),
                        signature: d(21),
                        int: IntMapping::Handle,
                    }
                ),
                (
                    "cboundary_paginate_ints_i64".to_string(),
                    LockedExport {
                        function: "reader.pages.paginate_ints".into(),
                        id: StableId::parse("reader.pages.paginate_ints").unwrap(),
                        signature: d(22),
                        int: IntMapping::Int64,
                    }
                ),
            ]),
            symbols: vec![
                "cboundary_bind_image_codec".into(),
                "cboundary_clean".into(),
                "cboundary_paginate_ints".into(),
                "cboundary_paginate_ints_i64".into(),
            ],
        })
    );
    assert_eq!(
        lock.c_adapters(),
        BTreeMap::from([(
            "image_codec".to_string(),
            AdapterC {
                form: AdapterForm::Wrapper,
                isolation: "in_process".into(),
                capability: "reader.images.ImageCodec".into(),
                files: BTreeMap::from([("adapters/image_codec.c".to_string(), d(14))]),
            }
        )])
    );
    assert_eq!(
        lock.foreign_c(),
        BTreeMap::from([(
            "image_codec".to_string(),
            ForeignC {
                sources: BTreeMap::from([("vendor/imgtest/imgtest.c".to_string(), d(13))]),
                headers: d(10),
                libraries: BTreeMap::from([
                    (
                        "extra".to_string(),
                        LockedLibrary {
                            path: "vendor/libextra/libextra.a".into(),
                            digest: d(11)
                        }
                    ),
                    (
                        "stdc++".to_string(),
                        LockedLibrary {
                            path: "/usr/lib/gcc/x86_64-linux-gnu/11/libstdc++.so".into(),
                            digest: d(12)
                        }
                    ),
                ]),
            }
        )])
    );
    assert!(lock.has_c_entry());
    assert_eq!(render_lock(&lock), text);
    // Rebuilding the same lock through the setters writes the same text.
    let mut rebuilt = lock.clone();
    rebuilt.foreign.clear();
    rebuilt.adapters.clear();
    rebuilt.targets.clear();
    rebuilt.set_foreign_c(lock.foreign_c());
    rebuilt.set_c_adapters(lock.c_adapters());
    rebuilt.set_target_c(lock.target_c());
    assert_eq!(render_lock(&rebuilt), text);
    assert_eq!(rebuilt, lock);
}

/// A minimal lock with no reserved entries.
fn bare_lock() -> Lock {
    Lock {
        version: 1,
        toolchain: LockToolchain {
            cpl: "0.1.0".into(),
            edition: "0.1".into(),
            std: Digest(dig(0)),
        },
        sources: BTreeMap::from([("cpl.toml".to_string(), Digest(dig(1)))]),
        verification: VerificationSection {
            proof_timeout_ms: 10_000,
            admit: vec![],
        },
        foreign: toml::Table::new(),
        adapters: toml::Table::new(),
        targets: toml::Table::new(),
    }
}

#[test]
fn empty_reserved_tables_are_followed_directly_by_the_next_header() {
    // 11.2: "the reserved tables are always written, and an empty one is
    // followed directly by the next header, as in the example".
    let full = parse_lock(&canonical()).unwrap();
    let target_line = canonical()
        .lines()
        .find(|l| l.starts_with("c = { abi"))
        .unwrap()
        .to_string();
    let foreign_line = canonical()
        .lines()
        .find(|l| l.starts_with("c = { image_codec"))
        .unwrap()
        .to_string();
    let adapter_line = canonical()
        .lines()
        .find(|l| l.starts_with("image_codec = "))
        .unwrap()
        .to_string();

    let mut only_target = bare_lock();
    only_target.set_target_c(full.target_c());
    let text = render_lock(&only_target);
    assert!(
        text.ends_with(&format!(
            "admit = []\n\n[foreign]\n[adapters]\n[targets]\n{target_line}\n"
        )),
        "{text}"
    );
    assert_eq!(parse_lock(&text).unwrap(), only_target);

    let mut only_foreign = bare_lock();
    only_foreign.set_foreign_c(full.foreign_c());
    let text = render_lock(&only_foreign);
    assert!(
        text.ends_with(&format!(
            "admit = []\n\n[foreign]\n{foreign_line}\n\n[adapters]\n[targets]\n"
        )),
        "{text}"
    );
    assert_eq!(parse_lock(&text).unwrap(), only_foreign);

    let mut only_adapter = bare_lock();
    only_adapter.set_c_adapters(full.c_adapters());
    let text = render_lock(&only_adapter);
    assert!(
        text.ends_with(&format!(
            "admit = []\n\n[foreign]\n[adapters]\n{adapter_line}\n\n[targets]\n"
        )),
        "{text}"
    );
    assert_eq!(parse_lock(&text).unwrap(), only_adapter);
}

#[test]
fn every_digest_of_the_c_entries_is_64_lowercase_hex() {
    // 11.2: "Every digest of the C entries is a blake3 digest, as 64 lowercase
    // hex digits"; a malformed C entry is project_lock_invalid (T75).
    let text = canonical();
    for tag in C_DIGESTS {
        let good = dig(tag);
        assert_eq!(text.matches(&good).count(), 1, "{tag}");
        let bad_variants = [
            good.to_uppercase(),
            good[..63].to_string(),
            format!("{good}0"),
            format!("g{}", &good[1..]),
            String::new(),
            format!(" {}", &good[1..]),
        ];
        for bad in bad_variants {
            let broken = text.replace(&good, &bad);
            assert!(
                parse_lock(&broken).is_err(),
                "digest {tag} as {bad:?} was accepted"
            );
        }
    }
}

/// The canonical lock as a TOML table, edited by `edit`, as TOML text.
fn edited(edit: impl FnOnce(&mut toml::Table)) -> String {
    let mut table: toml::Table = canonical().parse().unwrap();
    edit(&mut table);
    toml::to_string(&table).unwrap()
}

fn at<'t>(table: &'t mut toml::Table, path: &[&str]) -> &'t mut toml::Table {
    let mut t = table;
    for p in path {
        t = t
            .get_mut(*p)
            .and_then(toml::Value::as_table_mut)
            .unwrap_or_else(|| panic!("no table {p}"));
    }
    t
}

/// The closed C tables of the canonical lock with their keys.
const CLOSED: [(&[&str], &[&str]); 5] = [
    (
        &["targets", "c"],
        &[
            "api",
            "runtime",
            "runtime_digest",
            "target",
            "compiler",
            "compiler_path",
            "compiler_digest",
            "flags_digest",
            "system_digest",
            "prefix",
            "abi",
            "exports_digest",
            "exports",
            "symbols",
        ],
    ),
    (
        &["targets", "c", "exports", "cboundary_paginate_ints"],
        &["function", "id", "signature", "int"],
    ),
    (
        &["adapters", "image_codec"],
        &["form", "isolation", "capability", "files"],
    ),
    (
        &["foreign", "c", "image_codec"],
        &["sources", "headers", "libraries"],
    ),
    (
        &["foreign", "c", "image_codec", "libraries", "extra"],
        &["path", "digest"],
    ),
];

#[test]
fn the_edited_canonical_lock_parses_before_any_edit() {
    let text = edited(|_| {});
    assert_eq!(
        parse_lock(&text).unwrap(),
        parse_lock(&canonical()).unwrap()
    );
}

#[test]
fn every_key_of_the_closed_c_schemas_is_required() {
    for (path, keys) in CLOSED {
        for key in keys {
            let text = edited(|t| {
                at(t, path).remove(*key).unwrap();
            });
            assert!(
                parse_lock(&text).is_err(),
                "{}.{key} removed was accepted",
                path.join(".")
            );
        }
    }
}

#[test]
fn an_unknown_key_in_any_closed_c_table_is_rejected() {
    for (path, _) in CLOSED {
        for (key, value) in [
            ("extra", toml::Value::Integer(1)),
            ("version", toml::Value::String("1".into())),
            ("Api", toml::Value::String("0.1".into())),
        ] {
            let text = edited(|t| {
                at(t, path).insert(key.to_string(), value.clone());
            });
            assert!(
                parse_lock(&text).is_err(),
                "{}.{key} was accepted",
                path.join(".")
            );
        }
    }
}

/// Paths of the closed C tables of [`canonical`].
const TC: &[&str] = &["targets", "c"];
const TC_EXPORTS: &[&str] = &["targets", "c", "exports"];
const TC_EXPORT: &[&str] = &["targets", "c", "exports", "cboundary_paginate_ints"];
const AD: &[&str] = &["adapters", "image_codec"];
const AD_FILES: &[&str] = &["adapters", "image_codec", "files"];
const FC: &[&str] = &["foreign", "c", "image_codec"];
const FC_LIBS: &[&str] = &["foreign", "c", "image_codec", "libraries"];
const FC_LIB: &[&str] = &["foreign", "c", "image_codec", "libraries", "extra"];
const FC_SOURCES: &[&str] = &["foreign", "c", "image_codec", "sources"];

#[test]
fn ill_typed_c_values_are_rejected_and_their_domains_admitted() {
    use toml::Value as V;
    let s = |x: &str| V::String(x.to_string());
    let rejected: Vec<(&[&str], &str, V)> = vec![
        (TC, "abi", s("1")),
        (TC, "abi", V::Float(1.0)),
        (TC, "abi", V::Integer(-1)),
        (TC, "abi", V::Integer(65536)),
        (TC, "api", V::Integer(1)),
        (TC, "prefix", V::Integer(1)),
        (TC, "target", V::Boolean(true)),
        (TC, "compiler_path", V::Array(vec![])),
        (TC, "symbols", s("cboundary_clean")),
        (TC, "symbols", V::Array(vec![V::Integer(1)])),
        (TC, "exports", V::Array(vec![])),
        (TC, "exports", s("x")),
        (TC_EXPORTS, "cboundary_paginate_ints", s("x")),
        (TC_EXPORT, "int", s("Int64")),
        (TC_EXPORT, "int", s("int8")),
        (TC_EXPORT, "int", V::Integer(64)),
        (TC_EXPORT, "id", s("Bad Id")),
        (TC_EXPORT, "id", s("")),
        (TC_EXPORT, "function", V::Integer(1)),
        (AD, "form", s("native")),
        (AD, "form", s("Wrapper")),
        (AD, "capability", V::Integer(1)),
        (AD, "isolation", V::Integer(1)),
        (AD, "files", V::Array(vec![])),
        (AD_FILES, "adapters/image_codec.c", V::Integer(1)),
        (FC, "headers", V::Integer(1)),
        (FC, "sources", s("x")),
        (FC, "libraries", V::Array(vec![])),
        (FC_LIBS, "extra", s("libextra.a")),
        (FC_LIB, "path", V::Integer(1)),
        (FC_SOURCES, "vendor/imgtest/imgtest.c", V::Integer(1)),
    ];
    for (path, key, value) in rejected {
        let text = edited(|t| {
            at(t, path).insert(key.to_string(), value.clone());
        });
        assert!(
            parse_lock(&text).is_err(),
            "{}.{key} = {value} was accepted",
            path.join(".")
        );
    }
    // The domains: abi from 0 to 65535, the five mappings, both forms, and
    // the stable ids of either form.
    let mut admitted: Vec<(&[&str], &str, V)> = vec![
        (TC, "abi", V::Integer(0)),
        (TC, "abi", V::Integer(65535)),
        (TC, "symbols", V::Array(vec![])),
        (TC, "exports", V::Table(toml::Table::new())),
        (AD, "form", s("declarative")),
        (AD, "files", V::Table(toml::Table::new())),
        (FC, "sources", V::Table(toml::Table::new())),
        (FC, "libraries", V::Table(toml::Table::new())),
        (TC_EXPORT, "id", s("fn_k7w3qz2mb6ptx4ve")),
        (TC_EXPORT, "id", s("pgn_7c2d")),
    ];
    for mapping in IntMapping::ALL {
        admitted.push((TC_EXPORT, "int", s(mapping.as_str())));
    }
    for (path, key, value) in admitted {
        let text = edited(|t| {
            at(t, path).insert(key.to_string(), value.clone());
        });
        let lock = parse_lock(&text)
            .unwrap_or_else(|e| panic!("{}.{key} = {value}: {e:?}", path.join(".")));
        // And the canonical text of what was read round-trips.
        assert_eq!(parse_lock(&render_lock(&lock)).unwrap(), lock);
    }
}

#[test]
fn reserved_entries_outside_the_c_and_python_schemas_stay_opaque() {
    // Toolchain 11.2 closes the C schemas; python.md 9 also closes
    // `targets.python` and backend-python adapters. Other entries remain
    // opaque, including `foreign.python`.
    let head: String = canonical().split("[foreign]").next().unwrap().to_string();
    let text = format!(
        "{head}[foreign]\npython = {{ wheel = 1 }}\nC = \"not the c key\"\n\n[adapters]\nno_backend = {{ form = \"wrapper\", isolation = 3 }}\nupper = {{ backend = \"C\", form = 1 }}\nfuture = {{ backend = \"future\", files = [1] }}\nplain = \"c\"\nnumber = 5\n\n[targets]\nfuture = \"v1\"\nnode = {{ abi = \"x\" }}\n"
    );
    let lock = parse_lock(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    assert!(lock.c_adapters().is_empty());
    assert!(lock.foreign_c().is_empty());
    assert_eq!(lock.target_c(), None);
    assert!(!lock.has_c_entry());
    assert_eq!(parse_lock(&render_lock(&lock)).unwrap(), lock);
    // An [adapters] table with backend = "c" is closed, however incomplete.
    for entry in [
        "x = { backend = \"c\" }",
        "x = { backend = \"c\", form = \"wrapper\" }",
        "x = { backend = \"c\", form = \"wrapper\", isolation = \"in_process\", capability = \"a.b.C\", files = {}, extra = 1 }",
    ] {
        let text = format!("{head}[foreign]\n[adapters]\n{entry}\n\n[targets]\n");
        assert!(parse_lock(&text).is_err(), "{entry}");
    }
    // `[foreign] c` and `[targets] c` must be tables (of binding tables).
    for (table, entry) in [
        ("foreign", "c = 1"),
        ("foreign", "c = []"),
        ("foreign", "c = \"x\""),
        ("foreign", "c = { b = [] }"),
        ("foreign", "c = { b = 1 }"),
        ("targets", "c = \"x\""),
        ("targets", "c = []"),
        ("targets", "c = {}"),
    ] {
        let reserved = match table {
            "foreign" => format!("[foreign]\n{entry}\n\n[adapters]\n[targets]\n"),
            _ => format!("[foreign]\n[adapters]\n[targets]\n{entry}\n"),
        };
        assert!(
            parse_lock(&format!("{head}{reserved}")).is_err(),
            "{table}: {entry}"
        );
    }
    // An empty `[foreign] c` table has no binding, which is no error.
    let text = format!("{head}[foreign]\nc = {{}}\n\n[adapters]\n[targets]\n");
    let lock = parse_lock(&text).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(lock.foreign_c().is_empty());
}

#[test]
fn the_setters_replace_their_backend_c_entries_wholesale() {
    let full = parse_lock(&canonical()).unwrap();
    let mut lock = bare_lock();
    let python_adapters = BTreeMap::from([(
        "py".to_string(),
        AdapterPython {
            module: "adapters.codec".to_string(),
            digest: Digest("a".repeat(64)),
        },
    )]);
    lock.set_python_adapters(python_adapters.clone());
    lock.foreign
        .insert("python".to_string(), toml::Value::Integer(1));
    let adapter = full.c_adapters()["image_codec"].clone();
    let foreign = full.foreign_c()["image_codec"].clone();
    lock.set_c_adapters(BTreeMap::from([("first".to_string(), adapter.clone())]));
    lock.set_c_adapters(BTreeMap::from([("second".to_string(), adapter.clone())]));
    assert_eq!(
        lock.c_adapters().keys().collect::<Vec<_>>(),
        ["second"],
        "the earlier backend-c entries are replaced"
    );
    assert_eq!(lock.python_adapters(), python_adapters);
    lock.set_foreign_c(BTreeMap::from([("first".to_string(), foreign.clone())]));
    lock.set_foreign_c(BTreeMap::from([("second".to_string(), foreign.clone())]));
    assert_eq!(lock.foreign_c().keys().collect::<Vec<_>>(), ["second"]);
    assert!(lock.foreign.contains_key("python"));
    let mut target = full.target_c().unwrap();
    lock.set_target_c(Some(target.clone()));
    target.abi = 2;
    lock.set_target_c(Some(target.clone()));
    assert_eq!(lock.target_c().unwrap().abi, 2);
    assert!(lock.has_c_entry());
    let text = render_lock(&lock);
    assert_eq!(parse_lock(&text).unwrap(), lock);
    lock.set_c_adapters(BTreeMap::new());
    lock.set_foreign_c(BTreeMap::new());
    lock.set_target_c(None);
    assert!(!lock.has_c_entry());
    assert_eq!(lock.python_adapters(), python_adapters);
    assert!(lock.foreign.contains_key("python"));
}

// ---- the S49 JSON of the C diagnostics (1.4.2, T5) -------------------------------------

#[test]
fn c_binding_diagnostics_serialize_as_the_s49_object_with_flattened_payloads() {
    // 1.4.2: the keys in the field order of DiagnosticJson (code, category,
    // severity, message, then the optional keys, which a span-less
    // diagnostic omits, notes when non-empty), then the flattened payload in
    // sorted order (T4); never `span` or `symbol`. From Milestone 3
    // `[interop.c]` is legal with the python target, so no target at all
    // leaves it without one (spec/python.md 1).
    let text = c_toml(
        "[]",
        &format!(
            "\n[interop.c]\n{}",
            c_binding("codec", "adapter = \"a.c\"\n")
        ),
    );
    let diagnostics = c_check(&text, None);
    assert_eq!(codes_of(&diagnostics), ["project_c_target_not_enabled"]);
    let sm = SourceMap::new();
    let first = serde_json::to_string(&diagnostics[0].to_json(&sm)).unwrap();
    assert_eq!(
        first,
        format!(
            "{{\"code\":\"project_c_target_not_enabled\",\"category\":\"project\",\"severity\":\"error\",\"message\":{},\"key\":\"interop.c\",\"targets\":[]}}",
            serde_json::to_string(&diagnostics[0].message).unwrap()
        )
    );
    // The human form's first line (1.4.1).
    assert!(
        diagnostics[0]
            .render(&sm)
            .starts_with("error[project_c_target_not_enabled]: "),
        "{}",
        diagnostics[0].render(&sm)
    );
    // [interop.c] with the python target alone is legal (the Python package contains the C package)
    let text = c_toml(
        "[\"python\"]",
        &format!(
            "\n[interop.c]\n{}",
            c_binding("codec", "adapter = \"a.c\"\n")
        ),
    );
    assert!(c_check(&text, None).is_empty());
}

#[test]
fn every_c_target_code_is_in_the_project_table_once() {
    let mut seen = std::collections::BTreeSet::new();
    for code in codes::PROJECT {
        assert!(seen.insert(code.as_str()), "{} twice", code.as_str());
        assert!(code.as_str().starts_with("project_"), "{}", code.as_str());
    }
    // 13 of Milestone 1, 20 of the C target, 6 of the Python target and the fixtures (Milestone 3)
    assert_eq!(codes::PROJECT.len(), 39);
}
