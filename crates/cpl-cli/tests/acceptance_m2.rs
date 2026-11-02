//! The acceptance checklist of Milestone 2 (toolchain 12.9) on fresh committed copies of the corpus, through the
//! built `cpl` binary. Every item runs on such copies somewhere; this file runs the items whose evidence is a host
//! program over packages that `cpl build` built, and the two audits of the repository, and names the test of every
//! other item:
//!
//! | Item | Where it runs |
//! | - | - |
//! | 1 Gates and Milestone 1 | the four workspace gates themselves; the Milestone 1 test files are compared with their hashes by the acceptance audit |
//! | 2 The Export example | `build.rs` (`cpl build --json`: the info, `include/reader.h`, the symbols under `READER_0`); here `host_reader` against both libraries in both configurations |
//! | 3 A C host calls CPL | here (`host_basic`, `host_ownership`, `host_failures`, `host_version`, `host_caps`); `cpl-cabi/tests/c_hosts.rs` compares every line with the evaluator |
//! | 4 CPL calls a C library | `engines.rs` (`image_width` on the three engines); here `host_imports` and `host_declarative` |
//! | 5 Ownership and failure paths | here (`host_ownership`, `host_failures`, imgtest's counters at exit); `cpl-cabi/tests/oom.rs` (`host_oom`'s sweep) |
//! | 6 Versioning | here (`#error` on another API, `g++ -std=c++17 -fsyntax-only`; `CPL_E_VERSION` in `host_version`) |
//! | 7 Native execution equals the oracle | `engines.rs` (D3, `cpl test --engine both --seed 1..=32`); `cpl-cabi/tests/differential.rs` (D1, D2, D4-D8, the coverage gate) |
//! | 8 Threads | here (`host_threads`, TSan, `setarch -R`); `cpl-cabi/tests/threads.rs` |
//! | 9 Two packages | here (`host_two_packages`, two static copies) |
//! | 10 Release policy | `build.rs` (`release_refuses_the_corpus`, `the_lock_pins_the_build`) |
//! | 11 The lock pins the build | `build.rs` (`the_lock_pins_the_build`) |
//! | 12 The manifest is validated | here (the audit of the negative cases) |
//! | 13 Native executables | `executables.rs` |
//! | 14 Structure | `workspace_dag.rs`; `cpl-emit-c/tests/{tables,shape,compile}.rs` |
//! | 15 Specifications | here (the audit of the decision index and of the Milestone 2 row) |

mod common;

use common::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

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

fn cabi_tests() -> PathBuf {
    workspace().join("crates/cpl-cabi/tests")
}

/// Whether `flags` compile and run a trivial program; with `required` set, a panic when they do not.
fn runs_with(flags: &[&str], env: &[(&str, &str)], setarch: bool, required: &str) -> bool {
    let cc = cpl_rt::testing::find_cc().unwrap();
    let dir = temp_dir(&format!(
        "acceptance-probe-{}",
        flags.join("").replace(['-', '=', ','], "")
    ));
    let src = dir.join("probe.c");
    std::fs::write(&src, "int main(void) { return 0; }\n").unwrap();
    let exe = dir.join("probe");
    let ok = Command::new(&cc)
        .args(flags)
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .status()
        .is_ok_and(|s| s.success())
        && if setarch {
            let arch = String::from_utf8(Command::new("uname").arg("-m").output().unwrap().stdout)
                .unwrap();
            Command::new("setarch")
                .arg(arch.trim())
                .arg("-R")
                .arg(&exe)
                .envs(env.iter().copied())
                .status()
        } else {
            Command::new(&exe).envs(env.iter().copied()).status()
        }
        .is_ok_and(|s| s.success());
    if !ok {
        assert!(
            std::env::var(required).map_or(true, |v| v != "1"),
            "{required}=1, but {flags:?} do not build a running program"
        );
        println!("skipped a configuration: {flags:?} do not build a running program (set {required}=1 to fail)");
    }
    ok
}

/// The fixtures of `crates/cpl-cabi/tests/projects` (and `reader`, the verbatim `examples/reader`) that the hosts
/// name, with their vendored files, committed with the examples to one fresh repository (toolchain 12.7, 12.9).
fn corpus(name: &str) -> &'static Fixture {
    static CORPUS: OnceLock<Fixture> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let fx = Fixture::new(name);
        for project in ["cboundary", "cfaults", "cthumbs", "second"] {
            let dir = fx.root.join(project);
            copy_dir(&cabi_tests().join("projects").join(project), &dir);
            if project == "cboundary" || project == "cthumbs" {
                copy_dir(
                    &cabi_tests().join("clib/imgtest"),
                    &dir.join("vendor/imgtest"),
                );
            }
        }
        // vendor/libextra/libextra.a of cboundary
        let cc = cpl_rt::testing::find_cc().unwrap();
        let extra = fx.root.join("cboundary/vendor/libextra");
        std::fs::create_dir_all(&extra).unwrap();
        assert!(Command::new(&cc)
            .args(["-fPIC", "-c"])
            .arg(cabi_tests().join("clib/libextra/extra.c"))
            .arg("-o")
            .arg(extra.join("extra.o"))
            .status()
            .unwrap()
            .success());
        assert!(Command::new("ar")
            .arg("rcs")
            .arg(extra.join("libextra.a"))
            .arg(extra.join("extra.o"))
            .status()
            .unwrap()
            .success());
        let mut fx = fx;
        if git_available() {
            fx.git(&["init", "-q", "-b", "main"]);
            fx.commit = Some(fx.commit_all("the Milestone 2 corpus"));
        } else {
            println!("git is not installed: an uncommitted copy");
        }
        fx
    })
}

/// A configuration of the C host suites (toolchain 12.7).
#[derive(Clone, Copy, PartialEq)]
enum Config {
    Optimized,
    Sanitized,
    Thread,
}

impl Config {
    fn name(self) -> &'static str {
        match self {
            Config::Optimized => "optimized",
            Config::Sanitized => "sanitize",
            Config::Thread => "thread",
        }
    }
    fn build_args(self) -> &'static [&'static str] {
        match self {
            Config::Optimized => &[],
            Config::Sanitized => &["--sanitize", "address,undefined"],
            Config::Thread => &["--sanitize", "thread"],
        }
    }
    fn cflags(self) -> &'static [&'static str] {
        match self {
            Config::Optimized => &["-O2"],
            Config::Sanitized => &[
                "-O1",
                "-g",
                "-fno-omit-frame-pointer",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ],
            Config::Thread => &["-O1", "-g", "-fsanitize=thread"],
        }
    }
}

/// The two configurations of the host suites: optimized, and ASan with UBSan when the toolchain has them.
fn configs() -> Vec<Config> {
    static SANITIZED: OnceLock<bool> = OnceLock::new();
    let mut out = vec![Config::Optimized];
    if *SANITIZED.get_or_init(|| {
        runs_with(
            Config::Sanitized.cflags(),
            &[("ASAN_OPTIONS", "detect_leaks=1")],
            false,
            "CPL_REQUIRE_SANITIZERS",
        )
    }) {
        out.push(Config::Sanitized);
    }
    out
}

/// The C prefix of a project: `[interop.c] prefix`, else the project's name (`Manifest::c_prefix`).
fn prefix(project: &Path) -> String {
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(project.join("cpl.toml")).unwrap()).unwrap();
    manifest
        .get("interop")
        .and_then(|i| i.get("c"))
        .and_then(|c| c.get("prefix"))
        .or_else(|| manifest.get("project").and_then(|p| p.get("name")))
        .and_then(toml::Value::as_str)
        .unwrap()
        .to_string()
}

/// `cpl build --json` of a corpus project in a configuration, once per process: the output root.
fn built(project: &str, config: Config) -> PathBuf {
    static BUILT: OnceLock<std::sync::Mutex<std::collections::HashMap<String, PathBuf>>> =
        OnceLock::new();
    let key = format!("{project}-{}", config.name());
    let map = BUILT.get_or_init(Default::default);
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(out) = map.get(&key) {
        return out.clone();
    }
    let fx = corpus("acceptance-m2");
    let dir = fx.root.join(project);
    let out = fx.root.join("out").join(config.name()).join(project);
    let mut args = vec!["build", "--json", "--out", out.to_str().unwrap()];
    args.extend(config.build_args());
    let r = cpl(&dir, &args);
    assert_eq!(
        r.code,
        0,
        "cpl build of {project} ({}):\n{}",
        config.name(),
        r.stdout
    );
    let json = r.json();
    assert!(
        json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["severity"] != "error"),
        "{project}: {json}"
    );
    map.insert(key, out.clone());
    out
}

/// A host program of `crates/cpl-cabi/tests/c` compiled against the packages `cpl build` built for the fixtures its
/// first line names, linked against the shared and, separately, the static libraries, and run: it exits 0 with
/// nothing on stderr, and imgtest's counters at exit show no live allocation and no double free. Its stdout per link.
fn run_host(host: &str, config: Config) -> Vec<String> {
    let cc = cpl_rt::testing::find_cc().unwrap();
    let fx = corpus("acceptance-m2");
    let source = cabi_tests().join("c").join(host);
    let text = std::fs::read_to_string(&source).unwrap();
    let names: Vec<String> = text
        .lines()
        .next()
        .and_then(|l| l.trim().strip_prefix("/* fixture:"))
        .and_then(|s| s.strip_suffix("*/"))
        .unwrap_or_else(|| panic!("{host}: the first line names its fixtures"))
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let mut includes = vec![format!("-I{}", cabi_tests().join("c").display())];
    let mut packages = Vec::new();
    for name in &names {
        let dir = fx.root.join(name);
        let out = built(name, config);
        includes.push(format!("-I{}", out.join("include").display()));
        if dir.join("vendor/imgtest").is_dir() {
            includes.push(format!("-I{}", dir.join("vendor/imgtest").display()));
        }
        packages.push((out.join("lib"), prefix(&dir), dir));
    }
    let work = fx.root.join("hosts").join(config.name());
    std::fs::create_dir_all(&work).unwrap();
    let mut outputs = Vec::new();
    for link in ["shared", "static"] {
        let exe = work.join(format!("{}-{link}", host.trim_end_matches(".c")));
        let mut cmd = Command::new(&cc);
        cmd.args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
            .args(config.cflags())
            .args(&includes)
            .arg(&source);
        for (lib, prefix, dir) in &packages {
            if link == "shared" {
                cmd.arg(format!("-L{}", lib.display()))
                    .arg(format!("-l{prefix}"))
                    .arg(format!("-Wl,-rpath,{}", lib.display()));
            } else {
                cmd.arg(lib.join(format!("lib{prefix}.a")));
                if dir.join("vendor/libextra").is_dir() {
                    cmd.arg(format!("-L{}", dir.join("vendor/libextra").display()))
                        .arg("-lextra");
                }
            }
        }
        let built = cmd.arg("-lpthread").arg("-o").arg(&exe).output().unwrap();
        assert!(
            built.status.success(),
            "{host} ({}, {link}) does not compile:\n{}",
            config.name(),
            String::from_utf8_lossy(&built.stderr)
        );
        let report = work.join(format!("{}-{link}.imgtest", host.trim_end_matches(".c")));
        let _ = std::fs::remove_file(&report);
        let mut run = if config == Config::Thread {
            let arch = String::from_utf8(Command::new("uname").arg("-m").output().unwrap().stdout)
                .unwrap();
            let mut c = Command::new("setarch");
            c.arg(arch.trim()).arg("-R").arg(&exe);
            c
        } else {
            Command::new(&exe)
        };
        let run = run
            .env("ASAN_OPTIONS", "detect_leaks=1:halt_on_error=1")
            .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
            .env("TSAN_OPTIONS", "halt_on_error=1")
            .env("IMGTEST_REPORT", &report)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&run.stdout).to_string();
        assert!(
            run.status.success() && run.stderr.is_empty(),
            "{host} ({}, {link}) failed ({}):\n{stdout}{}",
            config.name(),
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
        let counts = std::fs::read_to_string(&report).unwrap_or_default();
        assert!(
            !counts.is_empty() || !stdout.contains("\"grant\""),
            "{host} ({}, {link}): imgtest wrote no report",
            config.name()
        );
        for l in counts.lines() {
            assert_eq!(
                l,
                "live 0 double_frees 0",
                "{host} ({}, {link}): imgtest at exit",
                config.name()
            );
        }
        outputs.push(stdout);
    }
    outputs
}

fn hosts(names: &[&str]) {
    if !require_cc() {
        return;
    }
    for config in configs() {
        for host in names {
            let outputs = run_host(host, config);
            for out in outputs {
                assert!(
                    out.lines().any(|l| l.starts_with('{')),
                    "{host}: no call line:\n{out}"
                );
            }
        }
    }
}

#[test]
fn item_02_host_reader_against_both_libraries() {
    hosts(&["host_reader.c"]);
}

#[test]
fn item_03_a_c_host_calls_cpl() {
    hosts(&[
        "host_basic.c",
        "host_ownership.c",
        "host_failures.c",
        "host_version.c",
        "host_caps.c",
    ]);
}

#[test]
fn item_04_cpl_calls_a_c_library() {
    hosts(&["host_imports.c", "host_declarative.c"]);
}

#[test]
fn item_06_versioning() {
    if !require_cc() {
        return;
    }
    let cc = cpl_rt::testing::find_cc().unwrap();
    let include = built("cboundary", Config::Optimized).join("include");
    let work = corpus("acceptance-m2").root.join("versioning");
    // another API's cpl.h beside the package header: the package header's guard stops the compile
    for (name, from, to) in [
        (
            "major",
            "#define CPL_API_VERSION_MAJOR 0u",
            "#define CPL_API_VERSION_MAJOR 1u",
        ),
        (
            "minor",
            "#define CPL_API_VERSION_MINOR 1u",
            "#define CPL_API_VERSION_MINOR 0u",
        ),
    ] {
        let bad = work.join(name);
        std::fs::create_dir_all(&bad).unwrap();
        let cpl_h = std::fs::read_to_string(include.join("cpl.h")).unwrap();
        assert!(cpl_h.contains(from), "cpl.h defines {from}");
        std::fs::write(bad.join("cpl.h"), cpl_h.replace(from, to)).unwrap();
        std::fs::copy(include.join("cboundary.h"), bad.join("cboundary.h")).unwrap();
        std::fs::write(
            bad.join("use.c"),
            "#include \"cboundary.h\"\nint main(void) { return 0; }\n",
        )
        .unwrap();
        let o = Command::new(&cc)
            .args(["-std=c11", "-fsyntax-only"])
            .arg(format!("-I{}", bad.display()))
            .arg(bad.join("use.c"))
            .output()
            .unwrap();
        assert!(!o.status.success(), "{name}: a mismatched cpl.h compiles");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("#error"),
            "{name}: not the header's #error"
        );
    }
    // every generated header is C++17
    let Some(cxx) = ["g++", "c++", "clang++"].into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }) else {
        println!("skipped the C++17 check: no C++ compiler");
        return;
    };
    let mut headers: Vec<PathBuf> = std::fs::read_dir(&include)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    headers.sort();
    assert!(headers.len() >= 4, "{headers:?}");
    for h in headers {
        let o = Command::new(cxx)
            .args([
                "-std=c++17",
                "-fsyntax-only",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-x",
                "c++",
            ])
            .arg(format!("-I{}", include.display()))
            .arg(&h)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{} is no C++17: {}",
            h.display(),
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn item_08_threads() {
    if !require_cc() {
        return;
    }
    let setarch = Command::new("setarch")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !setarch
        || !runs_with(
            Config::Thread.cflags(),
            &[("TSAN_OPTIONS", "halt_on_error=1")],
            true,
            "CPL_REQUIRE_TSAN",
        )
    {
        assert!(
            std::env::var("CPL_REQUIRE_TSAN").map_or(true, |v| v != "1"),
            "CPL_REQUIRE_TSAN=1, but TSan or setarch -R is unavailable"
        );
        return;
    }
    run_host("host_threads.c", Config::Thread);
}

#[test]
fn item_09_two_packages() {
    hosts(&["host_two_packages.c"]);
    if !require_cc() {
        return;
    }
    // two static copies of one package: a multiple definition under --whole-archive, one copy when linked plainly
    let cc = cpl_rt::testing::find_cc().unwrap();
    let fx = corpus("acceptance-m2");
    let first = built("cboundary", Config::Optimized);
    let second = fx.root.join("out/debug/cboundary");
    let r = cpl(
        &fx.root.join("cboundary"),
        &[
            "build",
            "--profile",
            "debug",
            "--out",
            second.to_str().unwrap(),
        ],
    );
    assert_eq!(r.code, 0, "{}", r.stdout);
    let work = fx.root.join("two-copies");
    std::fs::create_dir_all(&work).unwrap();
    let src = work.join("one_copy.c");
    std::fs::write(
        &src,
        "#include <stdio.h>\n#include \"cboundary.h\"\nint main(void) { printf(\"%s\\n\", cboundary_library_info()->profile); return 0; }\n",
    )
    .unwrap();
    let archives = [
        first.join("lib/libcboundary.a"),
        second.join("lib/libcboundary.a"),
    ];
    let include = format!("-I{}", first.join("include").display());
    let extra = format!("-L{}", fx.root.join("cboundary/vendor/libextra").display());
    let whole = Command::new(&cc)
        .args(["-std=c11", &include])
        .arg(&src)
        .arg("-Wl,--whole-archive")
        .args(&archives)
        .arg("-Wl,--no-whole-archive")
        .args([extra.as_str(), "-lextra", "-lpthread", "-o"])
        .arg(work.join("whole"))
        .output()
        .unwrap();
    assert!(
        !whole.status.success(),
        "two whole archives of one package link"
    );
    assert!(String::from_utf8_lossy(&whole.stderr).contains("multiple definition"));
    let plain = Command::new(&cc)
        .args(["-std=c11", &include])
        .arg(&src)
        .args(&archives)
        .args([extra.as_str(), "-lextra", "-lpthread", "-o"])
        .arg(work.join("plain"))
        .output()
        .unwrap();
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    let run = Command::new(work.join("plain")).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "optimized\n",
        "the first archive's copy"
    );
}

/// The negative cases of the repository: every `project_*` code that a test of `crates/cpl-project/tests/` expects,
/// and every code a marker of the `cpl-sema` corpus expects.
fn negative_cases() -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&workspace().join("crates/cpl-project/tests"), &mut files);
    walk(
        &workspace().join("crates/cpl-sema/tests/corpus"),
        &mut files,
    );
    let mut out = BTreeSet::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let corpus = f.extension().is_some_and(|x| x == "cpl");
        for (i, _) in text.match_indices("project_") {
            let code: String = text[i..]
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            let before = &text[..i];
            let expected = if corpus {
                // a corpus marker: `expect-error...: <code>` on the same line
                before
                    .rsplit('\n')
                    .next()
                    .is_some_and(|l| l.contains("expect-"))
            } else {
                true
            };
            if expected {
                out.insert(code);
            }
        }
    }
    out
}

#[test]
fn item_12_every_new_project_code_has_a_negative_case() {
    let cases = negative_cases();
    let codes =
        std::fs::read_to_string(workspace().join("crates/cpl-project/src/codes.rs")).unwrap();
    let m2 = &codes[codes
        .find("// ---- Milestone 2")
        .expect("codes.rs marks the Milestone 2 codes")..];
    let mut missing = Vec::new();
    let mut count = 0;
    for (i, _) in m2.match_indices("DiagCode(\"project_") {
        let code: String = m2[i + 10..].chars().take_while(|c| *c != '"').collect();
        count += 1;
        // the info of `cpl build`, which `cpl check` never reports, has its case in build.rs (item 2); and the
        // code that Milestone 3 retired (a C adapter may run in a worker, spec/python.md 6.6) is never raised
        if code == "project_export_skipped_generic"
            || code == "project_binding_isolation_unsupported"
        {
            continue;
        }
        let dir = workspace()
            .join("crates/cpl-sema/tests/corpus/errors/project/c_target")
            .join(&code);
        if !cases.contains(&code) && !dir.is_dir() {
            missing.push(code);
        }
    }
    assert!(count >= 20, "the Milestone 2 project codes: {count}");
    assert!(
        missing.is_empty(),
        "project codes without a negative case: {missing:?}"
    );
}

#[test]
fn item_15_the_decisions_are_indexed() {
    let spec = workspace().join("spec");
    let mut decisions: Vec<(u32, String)> = Vec::new();
    // every specification document: the five of Milestones 1 and 2 and those later milestones add
    let mut docs: Vec<String> = std::fs::read_dir(&spec)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".md") && n != "README.md" && n != "decisions.md")
        .collect();
    docs.sort();
    for doc in &docs {
        let text = std::fs::read_to_string(spec.join(doc)).unwrap();
        for marker in ["Decision: [D-", "Design conflict: [D-"] {
            for (i, _) in text.match_indices(marker) {
                let id: String = text[i + marker.len()..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                decisions.push((id.parse::<u32>().unwrap(), doc.clone()));
            }
        }
    }
    decisions.sort();
    let index = std::fs::read_to_string(spec.join("decisions.md")).unwrap();
    let mut rows = Vec::new();
    for line in index.lines() {
        if let Some(rest) = line.strip_prefix("| D-") {
            let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let doc = rest.split('`').nth(1).unwrap_or_default().to_string();
            rows.push((id.parse::<u32>().unwrap(), doc));
        }
    }
    rows.sort();
    assert_eq!(
        decisions, rows,
        "every decision has exactly one row of spec/decisions.md, in its document"
    );
    let ids: Vec<u32> = rows.iter().map(|(i, _)| *i).collect();
    assert_eq!(
        ids,
        (1..=ids.len() as u32).collect::<Vec<_>>(),
        "the stable ids are dense"
    );
    // the local series of Milestone 2 are recorded: T68-T82, semantics S75-S81, stdlib S45-S47, cabi C1-C39
    let has = |doc: &str, local: &str| {
        let text = std::fs::read_to_string(spec.join(doc)).unwrap();
        text.contains(&format!("] {local}. ")) || text.contains(&format!("] {local}.\n"))
    };
    for t in 68..=82 {
        assert!(has("toolchain.md", &format!("T{t}")), "toolchain T{t}");
    }
    for s in 75..=81 {
        assert!(has("semantics.md", &format!("S{s}")), "semantics S{s}");
    }
    for s in 45..=47 {
        assert!(has("stdlib.md", &format!("S{s}")), "stdlib S{s}");
    }
    for c in 1..=39 {
        assert!(has("cabi.md", &format!("C{c}")), "cabi C{c}");
    }
    // the Milestone 2 row of toolchain Section 13 cites the tests
    let toolchain = std::fs::read_to_string(spec.join("toolchain.md")).unwrap();
    let row = toolchain
        .lines()
        .rfind(|l| l.starts_with("| 2 C boundary |"))
        .expect("the Milestone 2 row");
    for test in [
        "crates/cpl-cabi/tests/c/",
        "Section 12.6",
        "Section 12.9",
        "acceptance_m2.rs",
    ] {
        assert!(
            row.contains(test),
            "the Milestone 2 row does not cite {test}"
        );
    }
}
