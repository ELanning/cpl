//! Native executables (`spec/cabi.md` Section 8; toolchain 12.9 item 13): `bin/cboundary` gives, for each call of the
//! host program `host_basic`, the `result`, `result_text`, and `fault` that `cpl run --json` gives, with `cpl run`'s
//! exit status; and it refuses as `cpl run` refuses (exit 2): an unknown function, a `using` parameter without a grant,
//! a wrong argument count.

mod common;

use common::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// A built copy of `cboundary` (its artifacts include the executable).
fn cboundary() -> PathBuf {
    let fixture = Fixture::new("executables");
    let dir = fixture.root.join("cboundary");
    let cabi = workspace().join("crates/cpl-cabi/tests");
    copy_dir(&cabi.join("projects/cboundary"), &dir);
    copy_dir(&cabi.join("clib/imgtest"), &dir.join("vendor/imgtest"));
    let extra = dir.join("vendor/libextra");
    std::fs::create_dir_all(&extra).unwrap();
    let cc = cpl_rt::testing::find_cc().unwrap();
    let o = extra.join("extra.o");
    assert!(Command::new(&cc)
        .args(["-fPIC", "-c"])
        .arg(cabi.join("clib/libextra/extra.c"))
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
    // a fresh committed copy (toolchain 12.9), when git is installed
    if git_available() {
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.commit_all("cboundary");
    }
    cpl(&dir, &["build"]).expect_code(0);
    dir
}

fn exe(dir: &Path, function: &str, args: &[&str]) -> (i32, Value) {
    let mut c = Command::new(dir.join(".cpl/build/c/bin/cboundary"));
    c.arg(function);
    for a in args {
        c.arg("--arg").arg(a);
    }
    let o = c.output().unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.stderr.is_empty(),
        "{function}: stderr {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{function}: one JSON line:\n{stdout}");
    (
        o.status.code().unwrap_or(-1),
        serde_json::from_str(lines[0]).unwrap(),
    )
}

fn run(dir: &Path, function: &str, args: &[&str]) -> (i32, Value) {
    let mut all = vec!["run", "--json", function];
    for a in args {
        all.push("--arg");
        all.push(a);
    }
    let r = cpl(dir, &all);
    (r.code, r.json())
}

const PAGE: &str = r#"{"$type": "reader.pages.Page", "id": {"$type": "reader.pages.PageId", "$payload": ["p"]}, "text": "  hi  "}"#;

#[test]
fn the_executable_answers_as_cpl_run() {
    if !require_cc() {
        return;
    }
    let dir = cboundary();
    let ok_page = format!(r#"{{"$type": "Result", "$variant": "Ok", "$payload": [{PAGE}]}}"#);
    let missing = r#"{"$type": "Result", "$variant": "Err", "$payload": [{"$type": "reader.pages.StoreError", "$variant": "Missing", "$payload": [{"$type": "reader.pages.PageId", "$payload": ["x"]}]}]}"#;
    let unavailable = r#"{"$type": "Result", "$variant": "Err", "$payload": [{"$type": "reader.pages.StoreError", "$variant": "Unavailable"}]}"#;
    // the calls of host_basic (tests/c/host_basic.c), as value JSON
    let calls: Vec<(&str, Vec<String>)> = vec![
        (
            "reader.pages.paginate_ints",
            vec!["[1, 2, 3]".into(), "2".into()],
        ),
        ("reader.pages.paginate_ints", vec!["[]".into(), "2".into()]),
        ("reader.pages.paginate_ints", vec!["[1]".into(), "0".into()]),
        (
            "reader.pages.paginate_ints",
            vec!["[1, 2, 3, 4, 5]".into(), "2".into()],
        ),
        (
            "reader.pages.paginate_ints",
            vec!["[1, 2, 3, 4, 5]".into(), "-3".into()],
        ),
        ("reader.pages.divide_exactly", vec!["6".into(), "3".into()]),
        ("reader.pages.divide_exactly", vec!["-9".into(), "3".into()]),
        ("reader.pages.divide_exactly", vec!["7".into(), "2".into()]),
        ("reader.pages.divide_exactly", vec!["7".into(), "0".into()]),
        ("reader.pages.positive_size", vec!["\"  42 \"".into()]),
        ("reader.pages.positive_size", vec!["\"0\"".into()]),
        ("reader.pages.positive_size", vec!["\"abc\"".into()]),
        ("reader.pages.clean", vec![PAGE.into()]),
        ("reader.pages.label", vec![ok_page.clone()]),
        ("reader.pages.label", vec![missing.into()]),
        ("reader.pages.label", vec![unavailable.into()]),
        // a Bytes parameter and the package's grant of its capability
        (
            "reader.images.image_width",
            vec![r#"{"$type": "Bytes", "hex": "494d475480020000e0010000"}"#.into()],
        ),
        (
            "reader.images.image_width",
            vec![r#"{"$type": "Bytes", "hex": "4e454757"}"#.into()],
        ),
    ];
    for (function, args) in &calls {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let (xc, x) = exe(&dir, function, &args);
        let (rc, r) = run(&dir, function, &args);
        assert_eq!(
            xc, rc,
            "{function} {args:?}: exit status\n  exe: {x}\n  run: {r}"
        );
        for key in ["result", "result_text", "fault"] {
            assert_eq!(
                x[key], r[key],
                "{function} {args:?}: {key}\n  exe: {x}\n  run: {r}"
            );
        }
        assert_eq!(x["function"], *function);
    }
    // the refusals: exit 2 with an error object of cpl run's code
    let refused = |function: &str, args: &[&str], code: &str| {
        let (xc, x) = exe(&dir, function, args);
        assert_eq!(xc, 2, "{function}: {x}");
        assert_eq!(x["error"]["code"], code, "{function}: {x}");
        let (rc, r) = run(&dir, function, args);
        assert_eq!(rc, 2, "cpl run {function}: {r}");
        assert!(
            r["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == code),
            "cpl run {function}: {r}"
        );
    };
    refused("reader.pages.nothing", &[], "index_unknown_symbol");
    refused(
        "reader.pages.load_page",
        &[r#"{"$type": "reader.pages.PageId", "$payload": ["p"]}"#],
        "cannot_run_effectful",
    );
    refused("reader.pages.divide_exactly", &["1"], "usage_bad_argument");
}
