//! The failing-allocator sweep of `host_oom` (toolchain 12.7, 12.9 item 5): `cboundary` built with ASan and UBSan
//! when the compiler links them (else optimized), `host_oom.c` linked against its static library and run once to
//! count the allocations of its three calls, then once per allocation index with that allocation failing: every run
//! exits 0 with nothing on stderr (no crash, no sanitizer report, no leak, every allocation freed). Then
//! `max_heap_bytes` budgets from the inputs' size upwards, each run twice: the same output both times.

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

fn run(exe: &Path, args: &[&str]) -> String {
    let o = Command::new(exe)
        .args(args)
        .env("ASAN_OPTIONS", "detect_leaks=1:halt_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(
        o.status.success() && o.stderr.is_empty(),
        "host_oom {args:?} failed ({}):\n{stdout}{}",
        o.status,
        String::from_utf8_lossy(&o.stderr)
    );
    stdout
}

#[test]
fn host_oom_sweep() {
    let Some(cc) = cpl_rt::testing::find_cc() else {
        if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
        }
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    };
    let sanitized = Compiler::discover(None, None)
        .is_ok_and(|c| c.supports(&[Sanitizer::Address, Sanitizer::Undefined]));
    if !sanitized && std::env::var("CPL_REQUIRE_SANITIZERS").is_ok_and(|v| v == "1") {
        panic!("CPL_REQUIRE_SANITIZERS=1, but the compiler cannot link address,undefined");
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("oom");
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
    let sanitize = if sanitized {
        vec![Sanitizer::Address, Sanitizer::Undefined]
    } else {
        vec![]
    };
    let cfg = BuildConfig {
        out_dir: out.clone(),
        cache_dir: dir.join("cache"),
        profile: Profile::Optimized,
        sanitize: sanitize.clone(),
        artifacts: vec![ArtifactKind::Static],
        release: false,
        lock: false,
        cc: None,
        jobs: 4,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    if let Err(ds) = cpl_emit_c::package::build(&project, &checked.program, &plan, &cfg) {
        panic!("cboundary does not build: {ds:#?}");
    }
    let exe = dir.join("host_oom");
    let mut cmd = Command::new(&cc);
    cmd.args(["-std=c11", "-Wall", "-Wextra", "-Werror"]);
    if sanitized {
        cmd.args([
            "-O1",
            "-g",
            "-fno-omit-frame-pointer",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ]);
    } else {
        cmd.arg("-O2");
    }
    cmd.arg(format!("-I{}", tests_dir().join("c").display()))
        .arg(format!("-I{}", out.join("include").display()))
        .arg(tests_dir().join("c/host_oom.c"))
        .arg(out.join("lib/libcboundary.a"))
        .arg(format!("-L{}", extra.display()))
        .args(["-lextra", "-lpthread", "-o"])
        .arg(&exe);
    let built = cmd.output().unwrap();
    assert!(
        built.status.success(),
        "host_oom does not compile:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    // the count
    let counted = run(&exe, &["count"]);
    let summary: serde_json::Value = counted
        .lines()
        .find(|l| l.starts_with("{\"allocations\""))
        .map(|l| serde_json::from_str(l).unwrap())
        .expect("the count line");
    let n = summary["allocations"].as_u64().unwrap();
    let base = summary["base_bytes"].as_u64().unwrap();
    assert!(n > 0, "the three calls allocate");
    // every allocation index fails once
    for k in 0..n {
        let k = k.to_string();
        let stdout = run(&exe, &["fail", &k]);
        assert!(stdout.contains("ok: 0 failures"), "fail {k}:\n{stdout}");
    }
    // budgets: deterministic outcomes
    let mut seen_failure = false;
    for delta in [0u64, 64, 256, 1024, 4096, 16384, 1 << 20] {
        let budget = (base + delta).to_string();
        let a = run(&exe, &["budget", &budget]);
        let b = run(&exe, &["budget", &budget]);
        assert_eq!(a, b, "budget {budget}: two runs differ");
        assert!(a.contains("ok: 0 failures"), "budget {budget}:\n{a}");
        seen_failure |= a.contains("\":5}") || a.contains("\":2}");
    }
    assert!(seen_failure, "no budget made a call run out of memory");
    println!(
        "host_oom: {n} allocation indices swept ({}), budgets deterministic",
        if sanitized {
            "ASan, UBSan"
        } else {
            "optimized"
        }
    );
}
