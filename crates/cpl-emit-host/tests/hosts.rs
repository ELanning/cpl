//! The host backends (`spec/hosts.md`): every program compiles to every target; the example of
//! `examples/hosts` prints the same in Rust, Go, Java, and Kotlin; and generated programs agree
//! with the reference evaluator in every host whose toolchain is on this machine.
//!
//! Rust runs wherever the tests run. Go runs when `go` is on `PATH`; Java when `javac` and
//! `java` are, or `JAVAC` and `JAVA` name them; Kotlin when `kotlinc` is, or `KOTLINC` names it.
//! A missing toolchain skips its part with a note on stderr.

mod common;

use cpl_emit_host::{emit, Options, Target};
use cpl_eval::testing::programs::generate;
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGETS: [Target; 3] = [Target::Rust, Target::Go, Target::Java];

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A fresh directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("hosts")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn run(cmd: &mut Command) -> String {
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("{cmd:?} does not start: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{cmd:?} failed:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

#[test]
fn every_program_compiles_to_every_host() {
    let mut sources = Vec::new();
    for dir in [
        "crates/cpl-cabi/tests/differential",
        "crates/cpl-eval/tests/corpus",
    ] {
        for f in common::cpl_sources(&repo().join(dir)) {
            sources.push(std::fs::read_to_string(f).expect("a corpus file"));
        }
    }
    sources.extend((0..60).map(|seed| generate(seed, &[]).source));
    let mut compiled = 0;
    for source in &sources {
        let Ok(p) = common::check(source) else {
            continue;
        };
        for target in TARGETS {
            let package = emit(
                &p,
                target,
                &Options {
                    name: "p".into(),
                    driver: true,
                },
            )
            .unwrap_or_else(|e| panic!("{}: {e}\n{source}", target.name()));
            assert!(!package.files.is_empty());
        }
        compiled += 1;
    }
    assert!(compiled > 150, "only {compiled} programs check");
}

/// The example project, checked, and the lines every host prints for it.
fn example() -> (cpl_hir::Program, String) {
    let root = repo().join("examples/hosts");
    let project = cpl_project::load_project(&root, &cpl_project::LoadOptions::default())
        .expect("the example loads");
    let checked = cpl_sema::check(&project);
    let errors: Vec<_> = checked
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let expected = std::fs::read_to_string(root.join("expected.txt")).expect("expected.txt");
    (checked.program, expected)
}

/// The example's package for `target`, written where its host program expects it.
fn example_package(target: Target, dir: &Path) -> cpl_hir::Program {
    let (p, _) = example();
    let package = emit(
        &p,
        target,
        &Options {
            name: "pricing".into(),
            driver: false,
        },
    )
    .expect("the example compiles");
    package
        .write(&dir.join(".cpl/build").join(target.name()))
        .expect("the package is written");
    p
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.parent().expect("a parent")).expect("a directory");
    std::fs::copy(from, to).unwrap_or_else(|e| panic!("{from:?}: {e}"));
}

#[test]
fn the_example_runs_in_rust() {
    let (_, expected) = example();
    let dir = scratch("example-rust");
    example_package(Target::Rust, &dir);
    let host = repo().join("examples/hosts/rust");
    copy(&host.join("Cargo.toml"), &dir.join("rust/Cargo.toml"));
    copy(&host.join("src/main.rs"), &dir.join("rust/src/main.rs"));
    let out = run(Command::new("cargo")
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(dir.join("rust/Cargo.toml"))
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .env("RUSTFLAGS", "-Awarnings"));
    assert_eq!(out, expected);
}

#[test]
fn the_example_runs_in_go() {
    if !common::available(Target::Go) {
        eprintln!("skipped: go is not on PATH");
        return;
    }
    let (_, expected) = example();
    let dir = scratch("example-go");
    example_package(Target::Go, &dir);
    let host = repo().join("examples/hosts/go");
    copy(&host.join("go.mod"), &dir.join("go/go.mod"));
    copy(&host.join("main.go"), &dir.join("go/main.go"));
    let out = run(Command::new("go")
        .args(["run", "."])
        .current_dir(dir.join("go")));
    assert_eq!(out, expected);
}

/// `kotlinc` and the Kotlin standard library beside it, when Kotlin is on this machine.
fn kotlin() -> Option<(PathBuf, PathBuf)> {
    let kotlinc = match std::env::var_os("KOTLINC") {
        Some(k) => PathBuf::from(k),
        None => std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join("kotlinc"))
            .find(|p| p.is_file())?,
    };
    let kotlinc = kotlinc.canonicalize().ok()?;
    let stdlib = kotlinc.parent()?.parent()?.join("lib/kotlin-stdlib.jar");
    stdlib.is_file().then_some((kotlinc, stdlib))
}

#[test]
fn the_example_runs_in_java_and_kotlin() {
    if !common::available(Target::Java) {
        eprintln!("skipped: no JDK (javac and java on PATH, or JAVAC and JAVA)");
        return;
    }
    let (_, expected) = example();
    let dir = scratch("example-java");
    example_package(Target::Java, &dir);
    let classes = dir.join("classes");
    let host = repo().join("examples/hosts");
    let mut sources = common::java_sources(&dir.join(".cpl/build/java/src"));
    sources.push(host.join("java/Main.java"));
    run(Command::new(common::javac())
        .args(["-nowarn", "-encoding", "UTF-8", "-d"])
        .arg(&classes)
        .args(&sources));
    let out = run(Command::new(common::java())
        .arg("-cp")
        .arg(&classes)
        .arg("Main"));
    assert_eq!(out, expected);

    let Some((kotlinc, stdlib)) = kotlin() else {
        eprintln!("skipped: kotlinc is not on PATH (or KOTLINC)");
        return;
    };
    let jar = dir.join("main.jar");
    let mut compile = Command::new(&kotlinc);
    compile
        .arg(host.join("kotlin/Main.kt"))
        .arg("-cp")
        .arg(&classes)
        .arg("-d")
        .arg(&jar);
    if let Ok(java) = std::env::var("JAVA") {
        // kotlinc runs on the JDK of `JAVA`, two levels above its `bin/java`.
        if let Some(home) = Path::new(&java).parent().and_then(Path::parent) {
            compile.env("JAVA_HOME", home);
        }
    }
    run(&mut compile);
    let sep = if cfg!(windows) { ";" } else { ":" };
    let cp = format!(
        "{}{sep}{}{sep}{}",
        classes.display(),
        jar.display(),
        stdlib.display()
    );
    let out = run(Command::new(common::java())
        .arg("-cp")
        .arg(cp)
        .arg("MainKt"));
    let (shared, kotlin_only): (Vec<&str>, Vec<&str>) =
        out.lines().partition(|l| !l.starts_with("kotlin: "));
    assert_eq!(shared.join("\n") + "\n", expected);
    assert_eq!(
        kotlin_only,
        [
            "kotlin: total * 2 + 1 = 4935, over 50 dollars: false",
            "kotlin: FOOD-APPLE: buy 2 get one free",
            "kotlin: MUG: 10% off",
        ]
    );
}

/// Random programs, and corpus programs on swept inputs, agree with the evaluator.
fn differential(target: Target) {
    if !common::available(target) {
        eprintln!("skipped: no {} toolchain", target.name());
        return;
    }
    let root = scratch(&format!("diff-{}", target.name()));
    let mut compared = 0;
    for seed in 1..=6 {
        let source = generate(seed, &[]).source;
        let case = common::run_case(target, &root, seed, &source, false)
            .unwrap_or_else(|e| panic!("seed {seed} does not build:\n{e}\n{source}"));
        if let Some(d) = case.divergence {
            panic!("seed {seed}: {d}");
        }
        compared += case.compared;
    }
    let corpus = [
        "crates/cpl-cabi/tests/differential/harvested/sema_enums_and_matching.cpl",
        "crates/cpl-cabi/tests/differential/harvested/sema_constraint_error.cpl",
        "crates/cpl-cabi/tests/differential/harvested/eval_adversarial_pipeline_language_011.cpl",
    ];
    for (i, file) in corpus.iter().enumerate() {
        let source = std::fs::read_to_string(repo().join(file)).expect("a corpus file");
        let case = common::run_case(target, &root, 100 + i as u64, &source, true)
            .unwrap_or_else(|e| panic!("{file} does not build:\n{e}"));
        if let Some(d) = case.divergence {
            panic!("{file}: {d}");
        }
        compared += case.compared;
    }
    assert!(compared > 500, "only {compared} calls compared");
}

#[test]
fn rust_agrees_with_the_evaluator() {
    differential(Target::Rust);
}

#[test]
fn go_agrees_with_the_evaluator() {
    differential(Target::Go);
}

#[test]
fn java_agrees_with_the_evaluator() {
    differential(Target::Java);
}

/// The builtins on hand-picked edge cases the random inputs rarely reach: every `White_Space`
/// scalar and control characters around text, overlapping and empty separators, malformed UTF-8
/// of every kind, `i64` boundaries of division and negation, indices and counts past 64 bits
/// (`tests/edge/builtins.jsonl`, 119 requests).
#[test]
fn builtins_agree_on_edge_cases() {
    let source = include_str!("edge/builtins.cpl");
    let requests = include_str!("edge/builtins.jsonl");
    for target in TARGETS {
        if !common::available(target) {
            eprintln!("skipped: no {} toolchain", target.name());
            continue;
        }
        let root = scratch(&format!("edge-{}", target.name()));
        let case = common::run_requests(target, &root, 0, source, requests)
            .unwrap_or_else(|e| panic!("{} does not build:\n{e}", target.name()));
        if let Some(d) = case.divergence {
            panic!("{}: {d}", target.name());
        }
        assert_eq!(case.compared, 119, "{}", target.name());
    }
}

/// Names that are keywords or the generated code's own names in some host (`impl`, `func`,
/// `class`, `new`, `default`, `cx`, `cpl`, ...) as functions, parameters, locals, fields,
/// operations, and module segments (`tests/edge/keywords.cpl`); names that one host's
/// conventions would merge (`foo_bar` and `fooBar`, `tests/edge/collisions.cpl`); types
/// named like what the generated code uses (`String`, `Override`, `Box`, `Fault`, `Cx`, ...,
/// `tests/edge/shadowing.cpl`); and values whose ownership the Rust printer moves or borrows
/// instead of cloning: a parameter an `ensures` reads (borrowed by the body), last reads in
/// loops and branches, disjoint fields moved together, match arms that move out of a dead
/// scrutinee, payloads behind an `Arc` (`tests/edge/ownership.cpl`). Every host compiles the
/// programs and agrees with the evaluator.
#[test]
fn edge_programs_agree_in_every_host() {
    for (name, source, least) in [
        ("keywords", include_str!("edge/keywords.cpl"), 1000),
        ("collisions", include_str!("edge/collisions.cpl"), 50),
        ("shadowing", include_str!("edge/shadowing.cpl"), 500),
        ("ownership", include_str!("edge/ownership.cpl"), 400),
    ] {
        for target in TARGETS {
            if !common::available(target) {
                eprintln!("skipped: no {} toolchain", target.name());
                continue;
            }
            let root = scratch(&format!("{name}-{}", target.name()));
            let case = common::run_case(target, &root, 0, source, true)
                .unwrap_or_else(|e| panic!("{name}, {}, does not build:\n{e}", target.name()));
            if let Some(d) = case.divergence {
                panic!("{name}, {}: {d}", target.name());
            }
            assert!(
                case.compared >= least,
                "{name}, {}: {}",
                target.name(),
                case.compared
            );
        }
    }
}

/// A Go string may hold any bytes and a Java string unpaired surrogates, but a CPL `Text` holds
/// Unicode scalar values: an invalid one is refused wherever a host can pass it in, as an
/// argument, inside a list, a struct, or an enum, as what a capability returns, and inside the
/// `ForeignError` it returns (`tests/edge/texts`).
#[test]
fn invalid_texts_never_enter_cpl() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/edge/texts");
    let project = cpl_project::load_project(&root, &cpl_project::LoadOptions::default())
        .expect("the project loads");
    let checked = cpl_sema::check(&project);
    assert!(
        !checked.diagnostics.iter().any(|d| d.is_error()),
        "{:?}",
        checked.diagnostics
    );
    let expected = std::fs::read_to_string(root.join("expected.txt")).expect("expected.txt");
    let package = |target: Target, dir: &Path| {
        emit(
            &checked.program,
            target,
            &Options {
                name: "texts".into(),
                driver: false,
            },
        )
        .expect("the program compiles")
        .write(&dir.join(".cpl/build").join(target.name()))
        .expect("the package is written");
    };

    if common::available(Target::Go) {
        let dir = scratch("texts-go");
        package(Target::Go, &dir);
        copy(&root.join("go/go.mod"), &dir.join("go/go.mod"));
        copy(&root.join("go/main.go"), &dir.join("go/main.go"));
        let out = run(Command::new("go")
            .args(["run", "."])
            .current_dir(dir.join("go")));
        assert_eq!(out, expected, "go");
    } else {
        eprintln!("skipped: go is not on PATH");
    }

    if common::available(Target::Java) {
        let dir = scratch("texts-java");
        package(Target::Java, &dir);
        let classes = dir.join("classes");
        let mut sources = common::java_sources(&dir.join(".cpl/build/java/src"));
        sources.push(root.join("java/Main.java"));
        run(Command::new(common::javac())
            .args(["-nowarn", "-encoding", "UTF-8", "-d"])
            .arg(&classes)
            .args(&sources));
        let out = run(Command::new(common::java())
            .arg("-cp")
            .arg(&classes)
            .arg("Main"));
        assert_eq!(out, expected, "java");
    } else {
        eprintln!("skipped: no JDK (javac and java on PATH, or JAVAC and JAVA)");
    }
}
