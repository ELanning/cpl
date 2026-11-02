//! Declarative wrapper generation against the test libraries (toolchain 12, the emitter row; spec/cabi.md 6.3): the
//! wrappers that `cpl_emit_c::emit` generates for the fixtures `cthumbs` (over `imgtest`) and `cshapes` (over
//! `shapes`) define every function of the adapter contract and compile against the libraries' real headers with the
//! flags of a generated wrapper (cabi.md 1.5) without a diagnostic; every library call passes its arguments uncast, so
//! that the compiler checks each against the real prototype; a declared scalar that does not fit the real parameter,
//! result, or struct field, or a pointer type that differs from it, fails the compile; and a warning inside a library
//! header, which the binding does not control, is not reported.

use cpl_project::{load_project, LoadOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn cc() -> Option<PathBuf> {
    let cc = cpl_rt::testing::find_cc();
    if cc.is_none() {
        assert!(
            std::env::var("CPL_REQUIRE_CC").map_or(true, |v| v != "1"),
            "CPL_REQUIRE_CC=1, but there is no C compiler"
        );
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
    }
    cc
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), &target).unwrap();
        }
    }
}

/// A copy of a fixture of `crates/cpl-cabi/tests/projects` with its vendored library, named `case`.
fn fixture(name: &str, case: &str) -> PathBuf {
    let tests = workspace().join("crates/cpl-cabi/tests");
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("emit_cbind")
        .join(case);
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&tests.join("projects").join(name), &root);
    if name == "cthumbs" {
        copy_dir(&tests.join("clib/imgtest"), &root.join("vendor/imgtest"));
    }
    root
}

/// Replaces `from` with `to` in the fixture file `rel` (exactly once).
fn edit(root: &Path, rel: &str, from: &str, to: &str) {
    let path = root.join(rel);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches(from).count(), 1, "{rel}: {from:?}");
    std::fs::write(&path, text.replacen(from, to, 1)).unwrap();
}

struct Wrapper {
    /// The wrapper's text and its path relative to the package root.
    rel: String,
    text: String,
    /// The declarations of the adapter contract in `src/<prefix>_api.c`, through which the runtime calls the wrapper
    /// (a declarative binding has no adapter header, which serves reviewed wrappers: cabi.md 4.1).
    decls: String,
    /// The output root the package's files were written to.
    out: PathBuf,
    /// The binding's include directories, absolute.
    include_dirs: Vec<PathBuf>,
}

/// Emits the package of the fixture at `root` and writes its files below `<root>/emitted`: the wrapper of `binding`.
fn emit(root: &Path, binding: &str) -> Wrapper {
    let project = load_project(root, &LoadOptions::default()).unwrap();
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let plan = checked.interop.clone().expect("an interop plan");
    let pkg = cpl_emit_c::emit(
        &checked.program,
        &project,
        &plan,
        &cpl_emit_c::EmitConfig::default(),
    )
    .unwrap();
    let out = root.join("emitted");
    let prefix = cpl_emit_c::unit_prefix(&plan);
    for (rel, text) in cpl_rt::amalgamation(&prefix, &cpl_rt::RuntimeSource::Embedded).unwrap() {
        let path = out.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    for f in &pkg.files {
        let path = out.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &f.text).unwrap();
    }
    let rel = format!("src/{prefix}_bind_{binding}.c");
    let text = pkg
        .files
        .iter()
        .find(|f| f.path == rel)
        .unwrap_or_else(|| panic!("no {rel}"))
        .text
        .clone();
    let api_rel = format!("src/{prefix}_api.c");
    let api = &pkg
        .files
        .iter()
        .find(|f| f.path == api_rel)
        .unwrap_or_else(|| panic!("no {api_rel}"))
        .text;
    let stem = format!("{prefix}_adapter_{binding}_");
    let decls: String = api
        .lines()
        .filter(|l| {
            (l.starts_with("cpl_status ") || l.starts_with("void "))
                && l.contains(&stem)
                && l.ends_with(");")
        })
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(
        !decls.is_empty(),
        "{api_rel} declares the contract of {binding}"
    );
    let include_dirs = project
        .manifest
        .binding_c
        .get(binding)
        .map(|c| c.include_dirs.clone())
        .unwrap_or_default();
    Wrapper {
        rel,
        text,
        decls,
        out,
        include_dirs: include_dirs.iter().map(|d| root.join(d)).collect(),
    }
}

/// The wrapper compiled as `cpl build` compiles it (cabi.md 1.5): the adapter set (the base set without its `-Wno-*`
/// flags), the include directories as `-isystem` (or `-I` with `system` false), and the generated-wrapper flags.
fn compile(cc: &Path, w: &Wrapper, system: bool) -> Output {
    compile_unit(cc, w, &w.out.join(&w.rel), system)
}

fn compile_unit(cc: &Path, w: &Wrapper, unit: &Path, system: bool) -> Output {
    let mut cmd = Command::new(cc);
    cmd.args(
        cpl_rt::flags::BASE
            .iter()
            .filter(|f| !f.starts_with("-Wno-")),
    )
    .args(cpl_rt::flags::OPTIMIZED)
    .args(cpl_rt::flags::GENERATED_WRAPPER);
    for d in &w.include_dirs {
        if system {
            cmd.arg("-isystem").arg(d);
        } else {
            cmd.arg(format!("-I{}", d.display()));
        }
    }
    cmd.arg(format!("-I{}", w.out.join("src").display()))
        .arg(format!("-I{}", w.out.join("include").display()))
        .arg("-c")
        .arg(unit)
        .arg("-o")
        .arg(w.out.join("wrapper.o"))
        .output()
        .unwrap()
}

/// The functions the contract declares: `<prefix>_adapter_<binding>_<name>`.
fn contract(header: &str, prefix: &str, binding: &str) -> Vec<String> {
    let stem = format!("{prefix}_adapter_{binding}_");
    let mut out: Vec<String> = Vec::new();
    for piece in header.split(&stem).skip(1) {
        let name: String = piece
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if piece[name.len()..].starts_with('(') && !out.contains(&format!("{stem}{name}")) {
            out.push(format!("{stem}{name}"));
        }
    }
    out
}

/// The argument lists of every call of `function` in `text`.
fn calls<'a>(text: &'a str, function: &str) -> Vec<&'a str> {
    let needle = format!("{function}(");
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find(&needle) {
        let start = at + i + needle.len();
        let before = text[..at + i].chars().last();
        at = start;
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut depth = 1;
        let end = text[start..]
            .char_indices()
            .find(|(_, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .map(|(j, _)| start + j)
            .unwrap();
        out.push(&text[start..end]);
    }
    out
}

#[test]
fn the_fixture_wrappers_compile_against_the_real_headers_without_a_diagnostic() {
    let Some(cc) = cc() else { return };
    for (name, prefix, binding, library_calls, stateful) in [
        (
            "cthumbs",
            "cthumbs",
            "thumbs",
            &["imgtest_thumbnail", "imgtest_version"][..],
            true,
        ),
        (
            "cshapes",
            "geom",
            "shapes",
            &[
                "shp_scale",
                "shp_upper",
                "shp_is_even",
                "shp_touch",
                "shp_count",
                "shp_check",
            ][..],
            true,
        ),
    ] {
        let w = emit(&fixture(name, name), binding);
        let out = compile(&cc, &w, true);
        assert!(
            out.status.success() && out.stderr.is_empty(),
            "{name}: the wrapper does not compile cleanly:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // every contract function is defined, with the type through which the runtime calls it
        let functions = contract(&w.decls, prefix, binding);
        let unit = w.out.join("src/contract_check.c");
        std::fs::write(
            &unit,
            format!(
                "#include <stddef.h>\n#include <stdint.h>\n#include \"cpl.h\"\n{}#include \"{}\"\n",
                w.decls,
                w.rel.trim_start_matches("src/")
            ),
        )
        .unwrap();
        let both = compile_unit(&cc, &w, &unit, true);
        assert!(
            both.status.success(),
            "{name}: the wrapper's definitions conflict with the contract:\n{}",
            String::from_utf8_lossy(&both.stderr)
        );
        let out = compile(&cc, &w, true);
        assert!(out.status.success());
        assert!(
            functions.iter().any(|f| f.ends_with("_init"))
                && functions.iter().any(|f| f.ends_with("_fini"))
        );
        let nm = Command::new("nm")
            .arg(w.out.join("wrapper.o"))
            .output()
            .unwrap();
        let symbols = String::from_utf8_lossy(&nm.stdout);
        for f in &functions {
            assert!(
                symbols.lines().any(|l| l.ends_with(&format!(" T {f}"))),
                "{name}: {f} is declared by the adapter header but not defined by the wrapper"
            );
        }
        // the library is called with its arguments uncast, so the compiler checks each against the prototype
        for lib in library_calls {
            let args = calls(&w.text, lib);
            assert!(!args.is_empty(), "{name}: {lib} is never called");
            for a in args {
                assert!(
                    !a.contains("(int")
                        && !a.contains("(uint")
                        && !a.contains("(const")
                        && !a.contains("(char")
                        && !a.contains("(void")
                        && !a.contains("(struct")
                        && !a.contains("(size_t"),
                    "{name}: a cast at the call of {lib}({a})"
                );
            }
        }
        // a [state] comes with a mutex held around the library calls that receive it
        assert_eq!(
            w.text.contains("pthread_mutex_lock"),
            stateful,
            "{name}: the state's mutex"
        );
    }
}

#[test]
fn a_declaration_that_does_not_fit_the_real_prototype_fails_the_compile() {
    let Some(cc) = cc() else { return };
    // (case, fixture, binding, file, from, to, the warning that must fail)
    let cases = [
        (
            "wider_argument",
            "cthumbs",
            "thumbs",
            "adapters/thumbs.cbind.toml",
            "\"max_side: int32\"",
            "\"max_side: int64\"",
            "conversion",
        ),
        (
            "other_state_pointer",
            "cthumbs",
            "thumbs",
            "adapters/thumbs.cbind.toml",
            "type = \"imgtest_decoder *\"",
            "type = \"struct imgtest_other *\"",
            "incompatible-pointer-types",
        ),
        (
            "narrower_status",
            "cthumbs",
            "thumbs",
            "adapters/thumbs.cbind.toml",
            "returns = { type = \"int\", ok = [0] }",
            "returns = { type = \"int8\", ok = [0] }",
            "conversion",
        ),
        (
            "narrower_result",
            "cthumbs",
            "thumbs",
            "adapters/thumbs.cbind.toml",
            "returns = { type = \"int32\", result = true }",
            "returns = { type = \"int16\", result = true }",
            "conversion",
        ),
        (
            "wider_struct_field",
            "cshapes",
            "shapes",
            "adapters/shapes.cbind.toml",
            "h = \"h: int16\"",
            "h = \"h: int32\"",
            "conversion",
        ),
        (
            "signed_for_unsigned",
            "cshapes",
            "shapes",
            "adapters/shapes.cbind.toml",
            "\"v: uint16\"",
            "\"v: int16\"",
            "sign-conversion",
        ),
    ];
    let mut failures = Vec::new();
    for (case, name, binding, file, from, to, warning) in cases {
        let root = fixture(name, case);
        edit(&root, file, from, to);
        let w = emit(&root, binding);
        let out = compile(&cc, &w, true);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            failures.push(format!("{case}: compiles"));
        } else if !stderr.contains(&format!("-Werror={warning}")) {
            failures.push(format!(
                "{case}: fails, but not with -Werror={warning}:\n{stderr}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_warning_inside_a_library_header_is_not_reported() {
    let Some(cc) = cc() else { return };
    let root = fixture("cthumbs", "noisy_header");
    // a conversion inside the library's own header: an error under the wrapper's flags were the header checked
    edit(
        &root,
        "vendor/imgtest/imgtest.h",
        "int32_t imgtest_version(void);",
        "int32_t imgtest_version(void);\nstatic inline int imgtest_noisy(long v) { return v; }",
    );
    let w = emit(&root, "thumbs");
    let checked = compile(&cc, &w, false);
    assert!(
        !checked.status.success(),
        "the header is noisy: with -I its conversion is an error"
    );
    let out = compile(&cc, &w, true);
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "with -isystem the library header's warning is not reported:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
