//! The thread suite (toolchain 12.7, 12.8): `cboundary` built `--sanitize thread`, `host_threads.c` compiled with
//! `-std=c11 -O1 -g -fsanitize=thread`, linked against the shared and, separately, the static library, and run as
//! `setarch $(uname -m) -R ./host_threads` (the WSL2 kernel's address layout otherwise aborts a TSan binary, T78) with
//! `TSAN_OPTIONS=halt_on_error=1`: it must exit 0 with nothing on stderr. Skipped with the printed reason when TSan or
//! `setarch` is unavailable, unless `CPL_REQUIRE_TSAN=1`.

use cpl_emit_c::cc::{Compiler, Profile, Sanitizer};
use cpl_emit_c::package::{ArtifactKind, BuildConfig};
use cpl_project::{load_project, LoadOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
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

fn skip(reason: &str) {
    if std::env::var("CPL_REQUIRE_TSAN").is_ok_and(|v| v == "1") {
        panic!("CPL_REQUIRE_TSAN=1, but {reason}");
    }
    println!("skipped: {reason} (set CPL_REQUIRE_TSAN=1 to fail)");
}

#[test]
fn host_threads_under_tsan() {
    let Some(cc) = cpl_rt::testing::find_cc() else {
        if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
        }
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    };
    if !Compiler::discover(None, None).is_ok_and(|c| c.supports(&[Sanitizer::Thread])) {
        return skip("the compiler cannot link ThreadSanitizer");
    }
    let arch = Command::new("uname")
        .arg("-m")
        .output()
        .ok()
        .filter(|o| o.status.success());
    let Some(arch) = arch.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()) else {
        return skip("uname -m is unavailable");
    };
    if !Command::new("setarch")
        .args([arch.as_str(), "-R", "true"])
        .status()
        .is_ok_and(|s| s.success())
    {
        return skip("setarch -R is unavailable");
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("threads");
    let root = dir.join("cboundary");
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&tests_dir().join("projects/cboundary"), &root);
    copy_dir(
        &tests_dir().join("clib/imgtest"),
        &root.join("vendor/imgtest"),
    );
    let extra = root.join("vendor/libextra");
    std::fs::create_dir_all(&extra).unwrap();
    let o = extra.join("extra.o");
    assert!(Command::new(&cc)
        .args(["-fPIC", "-c"])
        .arg(tests_dir().join("clib/libextra/extra.c"))
        .arg("-o")
        .arg(&o)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("ar")
        .arg("rcs")
        .arg(extra.join("libextra.a"))
        .arg(&o)
        .status()
        .unwrap()
        .success());
    let project = load_project(&root, &LoadOptions::default()).unwrap();
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let plan = checked.interop.clone().unwrap();
    let out = root.join(".cpl/build/c");
    let cfg = BuildConfig {
        out_dir: out.clone(),
        cache_dir: dir.join("cache"),
        profile: Profile::Optimized,
        sanitize: vec![Sanitizer::Thread],
        artifacts: vec![ArtifactKind::Shared, ArtifactKind::Static],
        release: false,
        lock: false,
        cc: None,
        jobs: 4,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    if let Err(ds) = cpl_emit_c::package::build(&project, &checked.program, &plan, &cfg) {
        panic!("cboundary does not build with TSan: {ds:#?}");
    }
    let lib = out.join("lib");
    for link in ["shared", "static"] {
        let exe = dir.join(format!("host_threads-{link}"));
        let mut cmd = Command::new(&cc);
        cmd.args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-O1",
            "-g",
            "-fsanitize=thread",
        ])
        .arg(format!("-I{}", tests_dir().join("c").display()))
        .arg(format!("-I{}", out.join("include").display()))
        .arg(tests_dir().join("c/host_threads.c"));
        if link == "shared" {
            cmd.arg(format!("-L{}", lib.display()))
                .arg("-lcboundary")
                .arg(format!("-Wl,-rpath,{}", lib.display()));
        } else {
            cmd.arg(lib.join("libcboundary.a"))
                .arg(format!("-L{}", extra.display()))
                .arg("-lextra");
        }
        cmd.arg("-lpthread").arg("-o").arg(&exe);
        let built = cmd.output().unwrap();
        assert!(
            built.status.success(),
            "host_threads ({link}) does not compile:\n{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let run = Command::new("setarch")
            .args([arch.as_str(), "-R"])
            .arg(&exe)
            .env("TSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&run.stdout);
        println!("host_threads ({link}, TSan):\n{stdout}");
        assert!(
            run.status.success() && run.stderr.is_empty(),
            "host_threads ({link}) under TSan failed ({}):\n{stdout}{}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
    }
}
