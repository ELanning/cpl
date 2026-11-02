//! Suite D3 (toolchain 12.6, 12.9 items 4 and 7): every `cpl run` and `cpl test` row of Section 12.3, on fresh
//! committed copies of `examples/reader` and `examples/holes`, with `--engine eval`, `native`, and `both`: the `eval`
//! and `native` envelopes byte-identical after the S75 normalization, and no `engine_divergence` under `both`; `cpl
//! test --seed s` for s in 1..=32. Then `cpl run reader.images.image_width` in `cboundary` (`Ok(640)`, and the `NEGW`
//! input's `InvalidResult`) on all three engines, and the goldens `run_paginate_ints_native.json`,
//! `run_image_width_eval.json`, and `test_reader_seed1_native.json`.

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

fn fixture(name: &str) -> Fixture {
    Fixture::committed(name).unwrap_or_else(|| {
        println!("git is not installed: uncommitted copies");
        Fixture::new(name)
    })
}

/// An envelope after the S75 normalization (`cpl_cabi::compare::normalize`), as compact JSON; two separate runs also
/// differ in Section 12.4's volatile `produced_at`, which is replaced.
fn normalized(mut v: Value) -> String {
    cpl_cabi::compare::normalize(&mut v);
    let mut work = vec![&mut v];
    while let Some(x) = work.pop() {
        match x {
            Value::Object(m) => {
                if m.contains_key("produced_at") {
                    m.insert("produced_at".to_string(), Value::from("<produced_at>"));
                }

                work.extend(m.values_mut());
            }
            Value::Array(a) => work.extend(a.iter_mut()),
            _ => {}
        }
    }
    v.to_string()
}

/// `stdout` with the receipt's native configuration masked: `compiler_digest` and `runtime_digest` digest this
/// machine's compiler and the runtime source, which every runtime change moves (build.rs masks every digest alike).
fn native_digests(stdout: &str) -> String {
    let mut out = stdout.to_string();
    for key in ["\"compiler_digest\": \"", "\"runtime_digest\": \""] {
        let mut at = 0;
        while let Some(i) = out[at..].find(key) {
            let start = at + i + key.len();
            let end = start + out[start..].find('"').unwrap();
            out.replace_range(start..end, "<digest>");
            at = start;
        }
    }
    out
}

/// `cpl <args>` with `--engine eval`, `native`, and `both`: the eval and native envelopes agree after the S75
/// normalization, `both` reports no divergence, and all three exit alike. The native envelope and exit code.
fn three_engines(dir: &Path, args: &[&str]) -> (i32, Value, String) {
    let with = |engine: &str| {
        let mut all: Vec<&str> = args.to_vec();
        all.insert(1, "--json");
        all.insert(2, "--engine");
        all.insert(3, engine);
        let r = cpl(dir, &all);
        let json = r.json();
        (r.code, json, r.stdout)
    };
    let (ec, eval, _) = with("eval");
    let (nc, native, stdout) = with("native");
    let (bc, both, _) = with("both");
    let (a, b) = (normalized(eval.clone()), normalized(native.clone()));
    if a != b {
        let at = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        panic!("{args:?}: the engines diverge at byte {at}\n  eval:   {a}\n  native: {b}");
    }
    assert_eq!(
        (ec, nc),
        (bc, bc),
        "{args:?}: exit codes eval {ec}, native {nc}, both {bc}"
    );
    assert!(
        with_code(&both, "engine_divergence").is_empty(),
        "{args:?}: --engine both reports a divergence: {both}"
    );
    (nc, native, stdout)
}

#[test]
fn d3_run_rows() {
    if !require_cc() {
        return;
    }
    let fx = fixture("engines-run");
    let rows: Vec<(&PathBuf, Vec<&str>, i32)> = vec![
        (
            &fx.reader,
            vec!["run", "reader.pages.paginate_ints", "[1, 2, 3]", "2"],
            0,
        ),
        (
            &fx.reader,
            vec!["run", "reader.pages.paginate_ints", "[]", "2"],
            0,
        ),
        (
            &fx.reader,
            vec!["run", "reader.pages.paginate_ints", "[1]", "0"],
            0,
        ),
        (
            &fx.reader,
            vec!["run", "reader.pages.divide_exactly", "6", "3"],
            0,
        ),
        (
            &fx.reader,
            vec!["run", "reader.pages.divide_exactly", "7", "0"],
            1,
        ),
        (
            &fx.reader,
            vec!["run", "reader.pages.divide_exactly", "7", "2"],
            1,
        ),
        (
            &fx.reader,
            vec!["run", "reader.pages.positive_size", "\"  12 \""],
            0,
        ),
        (
            &fx.reader,
            vec!["run", "reader.pages.positive_size", "\"-3\""],
            0,
        ),
        (
            &fx.holes,
            vec![
                "run",
                "holes.pages.summarize",
                "Page { id: PageId(\"p\"), text: \"t\" }",
            ],
            1,
        ),
        (
            &fx.holes,
            vec!["run", "holes.pages.paginate", "[1]", "1"],
            2,
        ),
    ];
    for (dir, args, code) in rows {
        let (c, native, stdout) = three_engines(dir, &args);
        assert_eq!(c, code, "{args:?}: {native}");
        if args[1] == "reader.pages.paginate_ints" && args[2] == "[1, 2, 3]" {
            check_golden(
                "run_paginate_ints_native.json",
                &normalize(&stdout, &Volatile::of(&native, Some(&fx.root), false)),
            );
        }
    }
}

#[test]
fn d3_test_rows_seeds_1_to_32() {
    if !require_cc() {
        return;
    }
    let fx = fixture("engines-test");
    for seed in 1..=32u64 {
        let s = seed.to_string();
        let (code, native, stdout) = three_engines(&fx.reader, &["test", "--seed", &s]);
        assert_eq!(code, 0, "seed {seed}: {native}");
        if seed == 1 {
            assert_eq!(native["selected"], 3);
            check_golden(
                "test_reader_seed1_native.json",
                &normalize(
                    &native_digests(&stdout),
                    &Volatile::of(&native, Some(&fx.root), false),
                ),
            );
        }
    }
}

/// A copy of the `cboundary` fixture with its vendored files (toolchain 12.7).
fn cboundary() -> PathBuf {
    let dir = temp_dir("engines-cboundary").join("cboundary");
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
    dir
}

#[test]
fn image_width_through_the_c_library_on_every_engine() {
    if !require_cc() {
        return;
    }
    let dir = cboundary();
    let image = r#"{"$type": "Bytes", "hex": "494d475480020000e0010000"}"#;
    let (code, native, _) =
        three_engines(&dir, &["run", "reader.images.image_width", "--arg", image]);
    assert_eq!(code, 0);
    assert_eq!(native["result_text"], "Ok(640)", "{native}");
    let r = cpl(
        &dir,
        &[
            "run",
            "--json",
            "--engine",
            "eval",
            "reader.images.image_width",
            "--arg",
            image,
        ],
    );
    r.expect_code(0);
    let eval = r.json();
    assert_eq!(eval["bindings"][0]["binding"], "image_codec", "{eval}");
    check_golden(
        "run_image_width_eval.json",
        &normalize(
            &r.stdout,
            &Volatile::of(&eval, Some(dir.parent().unwrap()), false),
        ),
    );
    let negw = r#"{"$type": "Bytes", "hex": "4e454757"}"#;
    let (code, native, _) =
        three_engines(&dir, &["run", "reader.images.image_width", "--arg", negw]);
    assert_eq!(code, 0);
    let text = native["result_text"].as_str().unwrap();
    assert!(
        text.starts_with("Err(ForeignError { kind: InvalidResult"),
        "{text}"
    );
}
