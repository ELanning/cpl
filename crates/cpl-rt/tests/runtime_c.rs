//! The runtime's C unit tests (`spec/toolchain.md` 1.12, 12.8; `spec/cabi.md` 2.2): the shared harness.
//!
//! Every `crates/cpl-rt/tests/c/*.c` except `program_stub.c` is one C test program. For each of the profiles `debug`
//! (`flags::DEBUG`), `optimized` (`flags::OPTIMIZED`), and `sanitize` (`flags::SANITIZE_ADDRESS_UNDEFINED`, run with
//! LeakSanitizer and halt on error), the harness writes the runtime as a package receives it
//! (`cpl_rt::amalgamation("rttest", ...)`: `src/cpl_rt.c` and the runtime headers, `include/cpl.h`, ...) into a fresh
//! directory under `CARGO_TARGET_TMPDIR`, compiles the amalgamation and `program_stub.c` once with the base flags,
//! the profile's flags, and `-Werror`, then compiles, links, and runs each test program against them through
//! `cpl_rt::testing`, and reports every file separately; the test fails when any file fails to build or exits with a
//! status other than 0, and lists each such file with its output.
//!
//! The amalgamation holds exactly the runtime `.c` files that exist (the two mains excepted), so while the files of
//! tracks R2 and R3 are missing, `program_stub.c`'s weak stand-ins supply the functions the existing files call.
//!
//! Interface for later C test files: add `tests/c/<name>.c`, beginning with the runtime prologue of the prefix `rttest`
//! (`#define _GNU_SOURCE 1`, `#define CPLRT_NS rttest`, `#define CPLRT(name) cplrt_rttest__##name`,
//! `#define CPLPKG(name) cplpkg_rttest__##name`), including `cpl_rt_internal.h` (and `cpl_rt_gen.h` when it needs the
//! generated tables), defining `main`, and exiting 0 only when every check passed; it is linked with the runtime and
//! with `program_stub.c` (whose tables, `RTTEST_*` indices, and `rttest_runtime_init`/`rttest_runtime_fini` it may
//! use, declaring them itself) and needs no change here. Its standard output is printed when it fails.
//!
//! Without a C compiler (`CPL_CC`, else `cc` on `PATH`) every test prints the skip reason of toolchain 12.8 and
//! passes, unless `CPL_REQUIRE_CC=1`. The work directory is removed after a profile passes.

use std::path::{Path, PathBuf};

const NO_CC: &str = "skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)";

fn cc_or_skip() -> Option<PathBuf> {
    match cpl_rt::testing::find_cc() {
        Some(cc) => Some(cc),
        None if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") => {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler (CPL_CC, or cc on PATH)")
        }
        None => {
            println!("{NO_CC}");
            None
        }
    }
}

fn c_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("c")
}

/// Every C test program: `tests/c/*.c` but `program_stub.c`, in name order.
fn test_programs() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(c_dir())
        .expect("tests/c is readable")
        .map(|e| e.expect("an entry of tests/c").path())
        .filter(|p| p.extension().is_some_and(|e| e == "c"))
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .filter(|n| n != "program_stub.c")
        .collect();
    names.sort();
    names
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Builds the runtime and the stub once for `profile`, then builds and runs every test program; panics listing every
/// file that failed.
fn run_profile(profile: &str, profile_flags: &[&str]) {
    let Some(cc) = cc_or_skip() else { return };
    let work = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("runtime_c")
        .join(profile);
    if work.exists() {
        std::fs::remove_dir_all(&work).expect("the old work directory can be removed");
    }
    for (relative, contents) in cpl_rt::amalgamation("rttest", &cpl_rt::RuntimeSource::Embedded)
        .expect("the embedded runtime assembles")
    {
        let path = work.join(&relative);
        std::fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the work directory can be created");
        std::fs::write(&path, contents).expect("a runtime file can be written");
    }
    let src = work.join("src");
    let include = work.join("include");
    let mut flags: Vec<String> = cpl_rt::flags::BASE.iter().map(|s| s.to_string()).collect();
    flags.extend(profile_flags.iter().map(|s| s.to_string()));
    flags.push(cpl_rt::flags::WERROR.to_string());
    flags.push(format!("-I{}", src.display()));
    flags.push(format!("-I{}", include.display()));
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let mut compile_flags = flags.clone();
    compile_flags.push("-c");

    let object = |source: &Path, name: &str| -> PathBuf {
        let out = work.join(name);
        let built = cpl_rt::testing::compile(&cc, &[source.to_path_buf()], &compile_flags, &out)
            .unwrap_or_else(|e| panic!("cannot run {}: {e}", cc.display()));
        assert!(
            built.status.success(),
            "[{profile}] compiling {} failed ({}):\n{}{}",
            source.display(),
            built.status,
            text(&built.stdout),
            text(&built.stderr)
        );
        out
    };
    let runtime = object(&src.join("cpl_rt.c"), "cpl_rt.o");
    std::fs::copy(c_dir().join("program_stub.c"), work.join("program_stub.c"))
        .expect("program_stub.c can be copied");
    let stub = object(&work.join("program_stub.c"), "program_stub.o");

    let mut failures = Vec::new();
    let programs = test_programs();
    assert!(!programs.is_empty(), "tests/c holds C test programs");
    for name in &programs {
        let source = work.join(name);
        std::fs::copy(c_dir().join(name), &source).expect("a test program can be copied");
        let result = cpl_rt::testing::compile_and_run(
            &cc,
            &[source.clone(), runtime.clone(), stub.clone()],
            &flags,
        );
        match result {
            Ok(out) if out.status.success() => {
                let stdout = text(&out.stdout);
                let last = stdout.lines().last().unwrap_or("");
                println!("runtime_c [{profile}] {name}: ok ({last})");
            }
            Ok(out) => {
                println!("runtime_c [{profile}] {name}: FAILED ({})", out.status);
                failures.push(format!(
                    "{name} exited with {}:\n--- stdout\n{}--- stderr\n{}",
                    out.status,
                    text(&out.stdout),
                    text(&out.stderr)
                ));
            }
            Err(e) => {
                println!("runtime_c [{profile}] {name}: FAILED to build");
                failures.push(format!("{name} did not build: {e}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "[{profile}] {} of {} C test programs failed:\n\n{}",
        failures.len(),
        programs.len(),
        failures.join("\n\n")
    );
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn c_tests_pass_in_the_debug_profile() {
    run_profile("debug", cpl_rt::flags::DEBUG);
}

#[test]
fn c_tests_pass_in_the_optimized_profile() {
    run_profile("optimized", cpl_rt::flags::OPTIMIZED);
}

#[test]
fn c_tests_pass_under_address_undefined_and_leak_sanitizers() {
    run_profile("sanitize", cpl_rt::flags::SANITIZE_ADDRESS_UNDEFINED);
}
