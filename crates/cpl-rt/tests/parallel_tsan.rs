//! Run the parallel runtime and its owned-list operations under ThreadSanitizer, including memory failures.
use std::{path::Path, process::Command};
#[test]
fn structured_parallelism_under_thread_sanitizer() {
    let Some(cc) = cpl_rt::testing::find_cc() else {
        assert_ne!(
            std::env::var("CPL_REQUIRE_TSAN").as_deref(),
            Ok("1"),
            "C compiler required"
        );
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("parallel-tsan");
    std::fs::create_dir_all(&dir).unwrap();
    for (path, bytes) in cpl_rt::amalgamation("rttest", &cpl_rt::RuntimeSource::Embedded).unwrap() {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let exe = dir.join("parallel-tsan");
    let result = Command::new(cc)
        .args(["-std=c11", "-O1", "-g", "-fsanitize=thread", "-pthread"])
        .arg(format!("-I{}", dir.join("include").display()))
        .arg(format!("-I{}", dir.join("src").display()))
        .arg(dir.join("src/cpl_rt.c"))
        .arg(root.join("tests/c/program_stub.c"))
        .arg(root.join("tests/c/test_parallel.c"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let arch = Command::new("uname").arg("-m").output().unwrap();
    let arch = String::from_utf8_lossy(&arch.stdout);
    let run = Command::new("setarch")
        .args([arch.trim(), "-R"])
        .arg(exe)
        .env("TSAN_OPTIONS", "halt_on_error=1")
        .output()
        .unwrap();
    assert!(
        run.status.success() && run.stderr.is_empty(),
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
