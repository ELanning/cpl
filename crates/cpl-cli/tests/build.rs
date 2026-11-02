//! `cpl build` through the built binary (toolchain 12.7, 12.9 items 2, 10, 11): the design's Export example on a fresh
//! committed copy of `examples/reader` (its `project_export_skipped_generic`, `include/reader.h` against the emitter's
//! golden, the dynamic symbols under `READER_0`), the release refusals of the corpus, a release build of a locked
//! `cboundary` (the library reports `release` 1 and `holes` 0), every way the lock pins the build, and the
//! `build_*.json` goldens, normalized as Section 12.4 normalizes plus every digest and the compiler's path and version.

mod common;

use common::*;
use serde_json::Value;
use std::collections::BTreeMap;
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

/// Every run of 64 lowercase hex digits as `<digest>`.
fn digests(text: &str) -> String {
    let b = text.as_bytes();
    let hex = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if i + 64 <= b.len()
            && b[i..i + 64].iter().all(|&c| hex(c))
            && (i == 0 || !hex(b[i - 1]))
            && (i + 64 == b.len() || !hex(b[i + 64]))
        {
            out.push_str("<digest>");
            i += 64;
            continue;
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// A build envelope for its golden: Section 12.4's normalization, every digest, the compiler's path and version.
fn normalize_build(stdout: &str, json: &Value, root: &Path) -> String {
    let mut text = normalize(stdout, &Volatile::of(json, Some(root), false));
    for key in ["path", "version"] {
        if let Some(v) = json["compiler"][key].as_str() {
            text = text.replace(&format!("\"{v}\""), &format!("\"<compiler_{key}>\""));
        }
    }
    // the compiler's target triple (x86_64-linux-gnu, x86_64-redhat-linux, ...: toolchain 12.4)
    if let Some(t) = json["compiler"]["target"].as_str() {
        text = text.replace(&format!("\"target\": \"{t}\""), "\"target\": \"<target>\"");
    }
    digests(&text)
}

/// A fresh copy of the `cboundary` fixture with its vendored files (toolchain 12.7).
fn cboundary(name: &str) -> PathBuf {
    let fixture = Fixture::new(name);
    let dir = fixture.root.join("cboundary");
    let cabi = workspace().join("crates/cpl-cabi/tests");
    copy_dir(&cabi.join("projects/cboundary"), &dir);
    copy_dir(&cabi.join("clib/imgtest"), &dir.join("vendor/imgtest"));
    libextra(&dir, "");
    // a fresh committed copy (toolchain 12.9), when git is installed
    if git_available() {
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.commit_all("cboundary");
    }
    dir
}

/// Builds `vendor/libextra/libextra.a` of a copy, with `more` as a second member when not empty.
fn libextra(dir: &Path, more: &str) {
    let extra = dir.join("vendor/libextra");
    std::fs::create_dir_all(&extra).unwrap();
    let cc = cpl_rt::testing::find_cc().expect("a C compiler");
    let mut members = vec![extra.join("extra.o")];
    assert!(Command::new(&cc)
        .args(["-fPIC", "-c"])
        .arg(workspace().join("crates/cpl-cabi/tests/clib/libextra/extra.c"))
        .arg("-o")
        .arg(&members[0])
        .status()
        .unwrap()
        .success());
    if !more.is_empty() {
        let src = extra.join("more.c");
        std::fs::write(&src, more).unwrap();
        members.push(extra.join("more.o"));
        assert!(Command::new(&cc)
            .args(["-fPIC", "-c"])
            .arg(&src)
            .arg("-o")
            .arg(&members[1])
            .status()
            .unwrap()
            .success());
    }
    let a = extra.join("libextra.a");
    let _ = std::fs::remove_file(&a);
    assert!(Command::new("ar")
        .arg("rcs")
        .arg(&a)
        .args(&members)
        .status()
        .unwrap()
        .success());
}

/// The codes of the error diagnostics of an envelope, each with its `differences` when it has them.
fn errors(json: &Value) -> Vec<(String, Vec<String>)> {
    json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == "error")
        .map(|d| {
            let diffs = d["differences"]
                .as_array()
                .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
                .unwrap_or_default();
            (d["code"].as_str().unwrap().to_string(), diffs)
        })
        .collect()
}

#[test]
fn the_export_example_on_the_verbatim_corpus() {
    if !require_cc() {
        return;
    }
    let fixture = Fixture::committed("build-reader").unwrap_or_else(|| {
        println!("git is not installed: an uncommitted copy");
        Fixture::new("build-reader")
    });
    let r = cpl(&fixture.reader, &["build", "--json"]);
    r.expect_code(0);
    let json = r.json();
    let infos: Vec<(&str, &str)> = json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["code"].as_str().unwrap(), d["severity"].as_str().unwrap()))
        .collect();
    assert_eq!(infos, [("project_export_skipped_generic", "info")]);
    // include/reader.h equals the emitter's golden
    let header =
        std::fs::read_to_string(fixture.reader.join(".cpl/build/c/include/reader.h")).unwrap();
    let mut header = digests(&header);
    if let Some(at) = header.find("; target ") {
        let end = header[at..].find(" */").map(|e| at + e).unwrap();
        header = format!("{}; target <target>{}", &header[..at], &header[end..]);
    }
    let golden = std::fs::read_to_string(
        workspace().join("crates/cpl-emit-c/tests/golden/reader/include/reader.h"),
    )
    .unwrap();
    assert_eq!(
        header, golden,
        "include/reader.h differs from crates/cpl-emit-c/tests/golden/reader/include/reader.h"
    );
    // exactly the reader_* symbols under READER_0
    let nm = Command::new("nm")
        .args(["-D", "--defined-only"])
        .arg(fixture.reader.join(".cpl/build/c/lib/libreader.so.0"))
        .output();
    match nm {
        Ok(o) if o.status.success() => {
            let mut names: Vec<String> = String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.split_whitespace().nth(2).map(str::to_string))
                .collect();
            names.sort();
            let expected = [
                "READER_0",
                // Milestone 3 grants Python bindings from C hosts through workers (spec/python.md P9).
                "reader_bind_image_codec@@READER_0",
                "reader_image_codec_new@@READER_0",
                "reader_image_width@@READER_0",
                "reader_library_info@@READER_0",
                "reader_load_page@@READER_0",
                "reader_load_text@@READER_0",
                "reader_page_read_new@@READER_0",
                "reader_paginate_ints@@READER_0",
                "reader_runtime_open@@READER_0",
                "reader_type_dimensions@@READER_0",
                "reader_type_image_codec@@READER_0",
                "reader_type_list_int@@READER_0",
                "reader_type_list_list_int@@READER_0",
                "reader_type_page@@READER_0",
                "reader_type_page_id@@READER_0",
                "reader_type_page_read@@READER_0",
                "reader_type_positive@@READER_0",
                "reader_type_result_dimensions_foreign_error@@READER_0",
                "reader_type_result_page_store_error@@READER_0",
                "reader_type_store_error@@READER_0",
            ];
            assert_eq!(names, expected);
        }
        _ => println!("skipped the symbol list: nm is unavailable"),
    }
    check_golden(
        "build_reader.json",
        &normalize_build(&r.stdout, &json, &fixture.root),
    );
}

#[test]
fn release_refuses_the_corpus() {
    let fixture = Fixture::committed("build-release").unwrap_or_else(|| {
        println!("git is not installed: an uncommitted copy");
        Fixture::new("build-release")
    });
    let count = |json: &Value| -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for d in json["diagnostics"].as_array().unwrap() {
            if d["severity"] == "error" {
                *m.entry(d["code"].as_str().unwrap().to_string())
                    .or_default() += 1;
            }
        }
        m
    };
    let r = cpl(&fixture.holes, &["build", "--release", "--json"]);
    r.expect_code(1);
    let json = r.json();
    assert_eq!(
        count(&json),
        BTreeMap::from([
            ("build_lock_missing".to_string(), 1),
            ("hole_in_release".to_string(), 2),
            ("verification_unsatisfied".to_string(), 1)
        ])
    );
    check_golden(
        "build_release_holes.json",
        &normalize_build(&r.stdout, &json, &fixture.root),
    );
    let r = cpl(&fixture.reader, &["build", "--release", "--json"]);
    r.expect_code(1);
    assert_eq!(
        count(&r.json()),
        BTreeMap::from([
            ("build_lock_missing".to_string(), 1),
            ("verification_unsatisfied".to_string(), 1)
        ])
    );
}

/// `cboundary_library_info()`'s `release` and `holes` through a host linked against the static library.
fn library_release_and_holes(dir: &Path) -> (u32, u32) {
    let cc = cpl_rt::testing::find_cc().unwrap();
    let src = dir.join("info.c");
    std::fs::write(
        &src,
        "#include <stdio.h>\n#include \"cboundary.h\"\nint main(void) { const cpl_library_info *i = cboundary_library_info(); printf(\"%u %u\\n\", i->release, i->holes); return 0; }\n",
    )
    .unwrap();
    let exe = dir.join("info");
    let out = dir.join(".cpl/build/c");
    let built = Command::new(&cc)
        .arg("-std=c11")
        .arg(format!("-I{}", out.join("include").display()))
        .arg(&src)
        .arg(out.join("lib/libcboundary.a"))
        .arg(format!("-L{}", dir.join("vendor/libextra").display()))
        .args(["-lextra", "-lpthread", "-o"])
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let run = Command::new(&exe).output().unwrap();
    let text = String::from_utf8_lossy(&run.stdout);
    let mut it = text.split_whitespace().map(|x| x.parse::<u32>().unwrap());
    (it.next().unwrap(), it.next().unwrap())
}

#[test]
fn the_lock_pins_the_build() {
    if !require_cc() {
        return;
    }
    let dir = cboundary("build-lock");
    // a development build has nothing to compare; --lock writes the C entries
    let r = cpl(&dir, &["build", "--lock", "--json"]);
    r.expect_code(0);
    let json = r.json();
    assert_eq!(json["lock"]["written"], true);
    check_golden(
        "build_cboundary.json",
        &normalize_build(&r.stdout, &json, dir.parent().unwrap()),
    );
    let lock = std::fs::read_to_string(dir.join("cpl.lock")).unwrap();
    for key in [
        "[foreign]\nc = { image_codec = {",
        "[adapters]\nimage_codec = { backend = \"c\"",
        "[targets]\nc = { abi = 1, api = \"0.1\"",
    ] {
        assert!(lock.contains(key), "cpl.lock lacks {key:?}:\n{lock}");
    }
    // a release build of the locked copy
    let r = cpl(&dir, &["build", "--release", "--json"]);
    r.expect_code(0);
    let json = r.json();
    assert_eq!(
        (json["release"].as_bool(), json["lock"]["checked"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(library_release_and_holes(&dir), (1, 0));
    // the build inputs: an adapter or a vendored source edited is project_lock_stale, before any build step
    for (file, difference) in [
        ("adapters/image_codec.c", "adapters"),
        ("vendor/imgtest/imgtest.c", "foreign.c"),
    ] {
        let path = dir.join(file);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, [original.as_slice(), b"/* edited */\n"].concat()).unwrap();
        let r = cpl(&dir, &["build", "--release", "--json"]);
        r.expect_code(1);
        let json = r.json();
        let e = errors(&json);
        assert_eq!(e.len(), 1, "{file}: {e:?}");
        assert_eq!(e[0].0, "project_lock_stale", "{file}");
        let stale = &json["diagnostics"].as_array().unwrap()[0];
        assert!(
            stale["differences"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d == difference),
            "{file}: {stale}"
        );
        std::fs::write(&path, original).unwrap();
    }
    // the compiler: a wrapper script around the same gcc
    let compiler = json["compiler"]["path"].as_str().unwrap().to_string();
    let wrapper = dir.parent().unwrap().join("cc-wrapper");
    std::fs::write(&wrapper, format!("#!/bin/sh\nexec {compiler} \"$@\"\n")).unwrap();
    let mut perms = std::fs::metadata(&wrapper).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&wrapper, perms).unwrap();
    let r = cpl(
        &dir,
        &[
            "build",
            "--release",
            "--cc",
            wrapper.to_str().unwrap(),
            "--json",
        ],
    );
    r.expect_code(1);
    assert_eq!(
        errors(&r.json()),
        [(
            "build_lock_mismatch".to_string(),
            vec![
                "targets.c.compiler_digest".to_string(),
                "targets.c.compiler_path".to_string()
            ]
        )]
    );
    // the profile: another flag set (and, as the debug headers differ, another header set)
    let r = cpl(
        &dir,
        &["build", "--release", "--profile", "debug", "--json"],
    );
    r.expect_code(1);
    let e = errors(&r.json());
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].0, "build_lock_mismatch");
    assert!(
        e[0].1.contains(&"targets.c.flags_digest".to_string()),
        "{e:?}"
    );
    // a header the binding's compilations read
    let header = dir.join("vendor/imgtest/imgtest.h");
    let original = std::fs::read(&header).unwrap();
    std::fs::write(&header, [original.as_slice(), b"/* edited */\n"].concat()).unwrap();
    let r = cpl(&dir, &["build", "--release", "--json"]);
    r.expect_code(1);
    assert_eq!(
        errors(&r.json()),
        [(
            "build_lock_mismatch".to_string(),
            vec!["foreign.c.image_codec.headers".to_string()]
        )]
    );
    std::fs::write(&header, original).unwrap();
    // a library the binding links, rebuilt with other content
    libextra(&dir, "int extra_more(void) { return 7; }\n");
    let r = cpl(&dir, &["build", "--release", "--json"]);
    r.expect_code(1);
    assert_eq!(
        errors(&r.json()),
        [(
            "build_lock_mismatch".to_string(),
            vec!["foreign.c.image_codec.libraries".to_string()]
        )]
    );
    libextra(&dir, "");
    cpl(&dir, &["build", "--release"]).expect_code(0);
    // an ABI break: --lock refuses to record it and leaves cpl.lock unchanged (T81)
    let pages = dir.join("src/reader/pages.cpl");
    let text = std::fs::read_to_string(&pages).unwrap();
    let edited = text.replace(
        "pub fn clean(page: Page) -> Page {\n    Page { ..page, text: trim(page.text) }\n}",
        "pub fn clean(page: Page, keep: Bool) -> Page {\n    if keep { page } else { Page { ..page, text: trim(page.text) } }\n}",
    );
    assert_ne!(text, edited, "the fixture's clean has the expected text");
    std::fs::write(&pages, edited).unwrap();
    let before = std::fs::read(dir.join("cpl.lock")).unwrap();
    let r = cpl(&dir, &["build", "--lock", "--json"]);
    r.expect_code(1);
    let json = r.json();
    let changed = with_code(&json, "build_abi_changed");
    assert_eq!(changed.len(), 1, "{json}");
    assert_eq!(
        (changed[0]["c_symbol"].as_str(), changed[0]["id"].as_str()),
        (Some("cboundary_clean"), Some("reader.pages.clean"))
    );
    assert!(
        changed[0]["expected"].is_string()
            && changed[0]["found"].is_string()
            && changed[0]["expected"] != changed[0]["found"]
    );
    assert_eq!(
        std::fs::read(dir.join("cpl.lock")).unwrap(),
        before,
        "cpl.lock was rewritten"
    );
}

/// The object cache of `spec/cabi.md` 1.6 (toolchain 7.2): a second build of an unchanged copy reuses every object and
/// reports `cached: true` with byte-identical artifacts; a changed vendored header recompiles the units that read it.
#[test]
fn a_second_build_reuses_the_object_cache() {
    if !require_cc() {
        return;
    }
    let dir = cboundary("build-cache");
    let build = || {
        let r = cpl(&dir, &["build", "--json"]);
        r.expect_code(0);
        let json = r.json();
        let lib = std::fs::read(dir.join(".cpl/build/c/lib/libcboundary.so.1")).unwrap();
        (json["cached"].as_bool().unwrap(), lib)
    };
    let (first, lib1) = build();
    assert!(!first, "a fresh cache reuses nothing");
    let (second, lib2) = build();
    assert!(second, "an unchanged build reuses every object");
    assert!(
        lib1 == lib2,
        "the shared library differs between a compiled and a cached build"
    );
    let header = dir.join("vendor/imgtest/imgtest.h");
    let original = std::fs::read(&header).unwrap();
    std::fs::write(&header, [original.as_slice(), b"/* edited */\n"].concat()).unwrap();
    let (third, _) = build();
    assert!(!third, "a changed header recompiles the units that read it");
    let (fourth, _) = build();
    assert!(fourth);
}
