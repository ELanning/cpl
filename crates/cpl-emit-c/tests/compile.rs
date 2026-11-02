//! The `-Werror` compile test (toolchain 12, T78): every corpus program's emitted units compile with the runtime's
//! headers under the base and debug flags and `-Werror`, as a driver (with tests) and as a package.

use cpl_project::{load_project, LoadOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cc() -> Option<PathBuf> {
    match cpl_rt::testing::find_cc() {
        Some(cc) => Some(cc),
        None if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") => {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler")
        }
        None => {
            println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
            None
        }
    }
}

/// Emits `root`'s program (driver form when `with_tests`) into a fresh directory and compiles every unit.
fn compile_project(root: &Path, with_tests: bool) {
    let Some(cc) = cc() else { return };
    let project = load_project(root, &LoadOptions::default()).expect("the project loads");
    let checked = cpl_sema::check(&project);
    assert!(
        !checked.has_errors(),
        "{} checks clean: {:?}",
        root.display(),
        checked.diagnostics
    );
    let plan = checked
        .interop
        .clone()
        .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
    let cfg = cpl_emit_c::EmitConfig {
        with_tests,
        ..Default::default()
    };
    let pkg = cpl_emit_c::emit(&checked.program, &project, &plan, &cfg).expect("emits");
    let name = root.file_name().unwrap().to_string_lossy().to_string();
    let work = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("emit_compile")
        .join(format!(
            "{name}-{}",
            if with_tests { "driver" } else { "package" }
        ));
    let _ = std::fs::remove_dir_all(&work);
    let prefix = cpl_emit_c::unit_prefix(&plan);
    for (rel, text) in cpl_rt::amalgamation(&prefix, &cpl_rt::RuntimeSource::Embedded).unwrap() {
        let path = work.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    for f in &pkg.files {
        let path = work.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &f.text).unwrap();
    }
    let mut failures = Vec::new();
    for f in pkg.files.iter().filter(|f| f.path.ends_with(".c")) {
        let mut cmd = Command::new(&cc);
        cmd.args(cpl_rt::flags::BASE)
            .args(cpl_rt::flags::DEBUG)
            .arg(cpl_rt::flags::WERROR)
            .arg(format!("-I{}", work.join("src").display()))
            .arg(format!("-I{}", work.join("include").display()));
        if with_tests {
            cmd.arg("-DCPL_WITH_TESTS=1");
        }
        cmd.arg("-c")
            .arg(work.join(&f.path))
            .arg("-o")
            .arg(work.join(format!("{}.o", f.path.replace('/', "_"))));
        let out = cmd.output().expect("the compiler runs");
        if !out.status.success() {
            failures.push(format!(
                "{}:\n{}",
                f.path,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{}: emitted units do not compile:\n{}",
        root.display(),
        failures.join("\n")
    );
}

#[test]
fn reader_compiles_as_a_driver() {
    compile_project(&workspace().join("examples/reader"), true);
}

#[test]
fn reader_compiles_as_a_package() {
    compile_project(&workspace().join("examples/reader"), false);
}

#[test]
fn holes_compiles_as_a_driver() {
    compile_project(&workspace().join("examples/holes"), true);
}

/// Every emitted unit of `project`, and with `runtime` the runtime amalgamation, compiled with `cc` and `-Werror`:
/// the failures.
fn compile_with(
    cc: &Path,
    name: &str,
    project: &cpl_project::Project,
    with_tests: bool,
    runtime: bool,
) -> Vec<String> {
    let checked = cpl_sema::check(project);
    if checked.has_errors() {
        return Vec::new();
    }
    let plan = checked
        .interop
        .clone()
        .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
    let cfg = cpl_emit_c::EmitConfig {
        with_tests,
        ..Default::default()
    };
    let pkg = cpl_emit_c::emit(&checked.program, project, &plan, &cfg).expect("emits");
    let compiler = cc.file_name().unwrap().to_string_lossy().to_string();
    let work = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("emit_compile_all")
        .join(format!(
            "{compiler}-{name}-{}",
            if with_tests { "driver" } else { "package" }
        ));
    let _ = std::fs::remove_dir_all(&work);
    let prefix = cpl_emit_c::unit_prefix(&plan);
    let mut units = Vec::new();
    for (rel, text) in cpl_rt::amalgamation(&prefix, &cpl_rt::RuntimeSource::Embedded).unwrap() {
        let path = work.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        if runtime && rel.ends_with("cpl_rt.c") {
            units.push(rel.clone());
        }
    }
    for f in &pkg.files {
        let path = work.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &f.text).unwrap();
        if f.path.ends_with(".c") {
            units.push(f.path.clone());
        }
    }
    let mut failures = Vec::new();
    for unit in units {
        let mut cmd = Command::new(cc);
        cmd.args(cpl_rt::flags::BASE)
            .args(cpl_rt::flags::DEBUG)
            .arg(cpl_rt::flags::WERROR)
            .arg(format!("-I{}", work.join("src").display()))
            .arg(format!("-I{}", work.join("include").display()));
        if with_tests {
            cmd.arg("-DCPL_WITH_TESTS=1");
        }
        let out = cmd
            .arg("-c")
            .arg(work.join(&unit))
            .arg("-o")
            .arg(work.join(format!("{}.o", unit.replace('/', "_"))))
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => failures.push(format!(
                "{compiler} {name} {unit}:\n{}",
                String::from_utf8_lossy(&o.stderr)
            )),
            Err(e) => failures.push(format!("{compiler} {name} {unit}: {e}")),
        }
    }
    failures
}

/// Every corpus program (the interpreter corpus, the checker's `ok` corpus, the differential corpus, the examples)
/// as a package and as a driver, and the runtime once, compiled with `-Werror` by the discovered compiler and by
/// clang when it is present (toolchain 12, 12.9 item 14).
#[test]
fn every_corpus_program_compiles_with_werror() {
    let Some(cc) = cc() else { return };
    let mut compilers = vec![cc];
    if let Ok(o) = Command::new("clang").arg("--version").output() {
        if o.status.success() {
            compilers.push(PathBuf::from("clang"));
        } else {
            println!("clang is not installed: gcc only");
        }
    } else {
        println!("clang is not installed: gcc only");
    }
    let ws = workspace();
    let mut programs: Vec<(String, cpl_project::Project)> = Vec::new();
    for root in ["examples/reader", "examples/holes"] {
        programs.push((
            root.replace('/', "_"),
            load_project(&ws.join(root), &LoadOptions::default()).unwrap(),
        ));
    }
    for dir in [
        "crates/cpl-eval/tests/corpus",
        "crates/cpl-sema/tests/corpus/ok",
        "crates/cpl-cabi/tests/differential/corpus",
    ] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(ws.join(dir))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "cpl"))
            .collect();
        files.sort();
        for f in files {
            let source = std::fs::read_to_string(&f).unwrap();
            let module = source
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("module "))
                .unwrap_or("m")
                .trim()
                .to_string();
            let tree = cpl_project::VirtualTree {
                files: [
                    (
                        "cpl.toml".to_string(),
                        b"[project]\nname = \"corpus\"\nedition = \"0.1\"\n".to_vec(),
                    ),
                    (
                        format!("src/{}.cpl", module.replace('.', "/")),
                        source.into_bytes(),
                    ),
                ]
                .into_iter()
                .collect(),
            };
            let project = cpl_project::load_project_from_tree(
                &tree,
                cpl_diag::FileOrigin::Virtual,
                &LoadOptions::default(),
            )
            .unwrap();
            programs.push((
                f.file_stem().unwrap().to_string_lossy().to_string(),
                project,
            ));
        }
    }
    let mut failures = Vec::new();
    for cc in &compilers {
        for (i, (name, project)) in programs.iter().enumerate() {
            for with_tests in [false, true] {
                failures.extend(compile_with(
                    cc,
                    name,
                    project,
                    with_tests,
                    i == 0 && !with_tests,
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} units do not compile:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
