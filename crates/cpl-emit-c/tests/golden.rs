//! The emitter goldens (toolchain 12.7): `include/reader.h` of the verbatim corpus, the headers of the `cboundary`
//! fixture, and the emitted C of four small modules (`divide_exactly`, `positive_size`, a loop with an invariant and a
//! measure, a `match` over `StoreError`), so that printer changes are reviewed as diffs. Emission needs no C compiler.
//! The texts are normalized (every 64-hex digest is `<digest>`, the target triple `<target>`) and compared byte for
//! byte with `tests/golden/`, rewritten under `UPDATE_GOLDEN=1`.

use cpl_diag::FileOrigin;
use cpl_project::{load_project, load_project_from_tree, LoadOptions, Project, VirtualTree};
use std::path::{Path, PathBuf};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Every 64-hex digest becomes `<digest>`, and the target triple of a banner `<target>`.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
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
    match out.find("; target ") {
        Some(at) => {
            let end = out[at..].find(" */").map(|e| at + e).unwrap_or(out.len());
            format!("{}; target <target>{}", &out[..at], &out[end..])
        }
        None => out,
    }
}

fn check_golden(rel: &str, actual: &str) {
    let path = golden_dir().join(rel);
    let actual = normalize(actual);
    if std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("no golden {} (UPDATE_GOLDEN=1 writes it)", path.display()));
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        panic!(
            "{rel} differs from its golden at line {}:\n  golden: {}\n  actual: {}\n(UPDATE_GOLDEN=1 rewrites it)",
            line + 1,
            expected.lines().nth(line).unwrap_or(""),
            actual.lines().nth(line).unwrap_or("")
        );
    }
}

fn emit(project: &Project) -> cpl_emit_c::EmittedPackage {
    let checked = cpl_sema::check(project);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let plan = checked
        .interop
        .clone()
        .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
    let cfg = cpl_emit_c::EmitConfig {
        target: "x86_64-linux-gnu".to_string(),
        ..Default::default()
    };
    cpl_emit_c::emit(&checked.program, project, &plan, &cfg).expect("emits")
}

fn file<'a>(pkg: &'a cpl_emit_c::EmittedPackage, path: &str) -> &'a str {
    &pkg.files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no emitted {path}"))
        .text
}

#[test]
fn reader_header_of_the_verbatim_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/reader");
    let project = load_project(&root, &LoadOptions::default()).unwrap();
    let pkg = emit(&project);
    check_golden("reader/include/reader.h", file(&pkg, "include/reader.h"));
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

#[test]
fn cboundary_headers() {
    // a copy with the vendored files the fixture's binding names (toolchain 12.7)
    let cabi = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cpl-cabi/tests");
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden/cboundary");
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&cabi.join("projects/cboundary"), &root);
    copy_dir(&cabi.join("clib/imgtest"), &root.join("vendor/imgtest"));
    std::fs::create_dir_all(root.join("vendor/libextra")).unwrap();
    let project = load_project(&root, &LoadOptions::default()).unwrap();
    let pkg = emit(&project);
    check_golden(
        "cboundary/include/cboundary.h",
        file(&pkg, "include/cboundary.h"),
    );
    check_golden(
        "cboundary/include/cboundary_adapter_image_codec.h",
        file(&pkg, "include/cboundary_adapter_image_codec.h"),
    );
}

const SMALL: [(&str, &str); 4] = [
    (
        "divide",
        "module divide.ops\n\npub fn divide_exactly(a: Int, b: Int) -> Int\n    requires b != 0\n    requires a % b == 0\n    ensures result * b == a\n{\n    a / b\n}\n",
    ),
    (
        "positive",
        "module positive.ops\n\nuse std.text.{trim}\nuse std.text.{parse_int}\n\npub type Positive = Int where self > 0\n\nfn parse_size(text: Text) -> Result<Int, ConstraintError> {\n    parse_int(trim(text))\n}\n\npub fn positive_size(text: Text) -> Result<Positive, ConstraintError> {\n    let n = parse_size(text)?\n    check<Positive>(n)\n}\n",
    ),
    (
        "loops",
        "module loops.ops\n\npub fn count_up(n: Int) -> Int\n    requires n >= 0\n{\n    var i = 0\n    while i < n\n        invariant i <= n\n        decreases n - i\n    {\n        i = i + 1\n    }\n    i\n}\n",
    ),
    (
        "store",
        "module store.ops\n\npub type PageId(Text)\n\npub enum StoreError {\n    Missing(PageId)\n    Unavailable\n}\n\npub fn describe(e: StoreError) -> Text {\n    match e {\n        Missing(_) => \"Missing page\"\n        Unavailable => \"Storage unavailable\"\n    }\n}\n",
    ),
];

#[test]
fn emitted_c_of_four_small_modules() {
    for (name, source) in SMALL {
        let tree = VirtualTree {
            files: [
                (
                    "cpl.toml".to_string(),
                    format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n").into_bytes(),
                ),
                (format!("src/{name}/ops.cpl"), source.as_bytes().to_vec()),
            ]
            .into_iter()
            .collect(),
        };
        let project =
            load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).unwrap();
        let pkg = emit(&project);
        check_golden(
            &format!("small/{name}_prog.c"),
            file(&pkg, &format!("src/{name}_prog.c")),
        );
    }
}

/// The version script and `build.json` of a `cboundary` build (they need the C compiler): the JSON with its
/// volatile values (`duration_ms`, the compiler's path and version) replaced as well.
#[test]
fn cboundary_build_outputs() {
    let Some(cc) = cpl_rt::testing::find_cc() else {
        if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
        }
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    };
    let cabi = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cpl-cabi/tests");
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden/cboundary-build");
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&cabi.join("projects/cboundary"), &root);
    copy_dir(&cabi.join("clib/imgtest"), &root.join("vendor/imgtest"));
    let extra = root.join("vendor/libextra");
    std::fs::create_dir_all(&extra).unwrap();
    let o = extra.join("extra.o");
    assert!(std::process::Command::new(&cc)
        .args(["-fPIC", "-c"])
        .arg(cabi.join("clib/libextra/extra.c"))
        .arg("-o")
        .arg(&o)
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("ar")
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
    let cfg = cpl_emit_c::package::BuildConfig {
        out_dir: out.clone(),
        cache_dir: root.join(".cpl/cache/c"),
        profile: cpl_emit_c::cc::Profile::Optimized,
        sanitize: vec![],
        artifacts: vec![
            cpl_emit_c::package::ArtifactKind::Shared,
            cpl_emit_c::package::ArtifactKind::Static,
            cpl_emit_c::package::ArtifactKind::Executable,
        ],
        release: false,
        lock: false,
        cc: None,
        jobs: 4,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    let built = cpl_emit_c::package::build(&project, &checked.program, &plan, &cfg)
        .unwrap_or_else(|ds| panic!("{ds:#?}"));
    check_golden(
        "cboundary/src/cboundary.map",
        &std::fs::read_to_string(out.join("src/cboundary.map")).unwrap(),
    );
    let mut json = std::fs::read_to_string(out.join("build.json")).unwrap();
    json = json.replace(
        &format!("\"{}\"", built.output.compiler.path),
        "\"<compiler_path>\"",
    );
    json = json.replace(
        &format!("\"{}\"", built.output.compiler.version),
        "\"<compiler_version>\"",
    );
    let duration = format!("\"duration_ms\": {}", built.output.duration_ms);
    json = json.replace(&duration, "\"duration_ms\": 0");
    // the compiler's target triple (x86_64-linux-gnu, x86_64-redhat-linux, ...: toolchain 12.4)
    json = json.replace(
        &format!("\"target\": \"{}\"", built.output.compiler.target),
        "\"target\": \"<target>\"",
    );
    check_golden("cboundary/build.json", &json);
}
