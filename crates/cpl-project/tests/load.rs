//! Loading (`spec/toolchain.md` 1.6, 3.2, 3.7): discovery of the source set,
//! the `FileId` order of T15, parse diagnostics, the module-location
//! diagnostics, every `LoadError` variant, and `find_project_root`.

mod common;

use common::*;
use cpl_diag::{Category, FileId, FileOrigin, Severity};
use cpl_project::{
    codes, find_project_root, load_project, load_project_from_tree, LoadError, LoadOptions,
    ParsedFile,
};
use cpl_syntax::FileKind;
use serde_json::json;
use std::fs;

const WORK: &str = "change c {\n    base git(\"main\")\n    target git(\"main\")\n}\n";

fn manifest_with_sources(sources: &str) -> String {
    format!("{MANIFEST}\n[build]\nsources = {sources}\n")
}

// ---- find_project_root ---------------------------------------------------------

#[test]
fn find_project_root_finds_the_nearest_ancestor_with_cpl_toml() {
    let dir = temp_dir("find-root");
    write(&dir, "outer/cpl.toml", MANIFEST);
    write(&dir, "outer/src/a/b.cpl", "module a.b\n");
    write(&dir, "outer/inner/cpl.toml", MANIFEST);
    write(&dir, "outer/inner/src/x.cpl", "module x\n");
    let outer = dir.join("outer");
    let inner = outer.join("inner");
    assert_eq!(find_project_root(&outer), Some(outer.clone()));
    assert_eq!(find_project_root(&outer.join("src/a")), Some(outer.clone()));
    assert_eq!(
        find_project_root(&outer.join("src/a/b.cpl")),
        Some(outer.clone())
    );
    assert_eq!(find_project_root(&inner.join("src")), Some(inner.clone()));
    assert_eq!(
        find_project_root(&outer.join("does/not/exist")),
        Some(outer.clone())
    );
    // A directory named cpl.toml does not make a project.
    fs::create_dir_all(dir.join("fake/cpl.toml")).unwrap();
    assert_ne!(find_project_root(&dir.join("fake")), Some(dir.join("fake")));
}

#[test]
fn find_project_root_makes_a_relative_start_absolute() {
    let found = find_project_root(std::path::Path::new("."));
    if let Some(root) = found {
        assert!(root.is_absolute());
        assert!(root.join("cpl.toml").is_file());
    }
}

// ---- LoadError::NoManifest ---------------------------------------------------------

#[test]
fn a_root_without_cpl_toml_is_load_error_no_manifest() {
    let dir = temp_dir("no-manifest");
    write(&dir, "src/a.cpl", "module a\n");
    match load_project(&dir, &LoadOptions::default()) {
        Err(LoadError::NoManifest(path)) => assert_eq!(path, dir),
        other => panic!("expected LoadError::NoManifest, found {other:?}"),
    }
    let err = load_project(&dir.join("missing"), &LoadOptions::default()).unwrap_err();
    assert!(matches!(err, LoadError::NoManifest(_)));
    assert!(err.to_string().starts_with("no cpl.toml found at "));

    match load_project_from_tree(
        &tree(&[("src/a.cpl", "module a\n")]),
        FileOrigin::Virtual,
        &LoadOptions::default(),
    ) {
        Err(LoadError::NoManifest(path)) => assert_eq!(path, std::path::PathBuf::from(".")),
        other => panic!("expected LoadError::NoManifest, found {other:?}"),
    }
}

// ---- discovery -----------------------------------------------------------------------

#[test]
fn the_source_set_is_cpl_files_under_sources_and_work_files_directly_in_work() {
    let tree = tree(&[
        ("cpl.toml", MANIFEST),
        ("cpl.lock_backup", "ignored"),
        ("README.md", "ignored"),
        ("src/a.cpl", "module a\n"),
        ("src/a/b.cpl", "module a.b\n"),
        ("src/a/b/c/d.cpl", "module a.b.c.d\n"),
        ("src/notes.txt", "ignored"),
        ("src/a.cpl.bak", "ignored"),
        ("src/.cpl", "ignored: no stem"),
        ("other/x.cpl", "module x\n"),
        ("work/one.work", "change c {\n}\n"),
        ("work/two.patch", "patch\n"),
        ("work/three.cpl", "ignored: not under a source directory"),
        ("work/nested/four.work", "ignored: not directly in work/"),
        ("work/.work", "ignored: no stem"),
    ]);
    let project = load_tree(&tree, false);
    assert_eq!(
        project_paths(&project),
        [
            "src/a.cpl",
            "src/a/b.cpl",
            "src/a/b/c/d.cpl",
            "work/one.work",
            "work/two.patch",
        ]
    );
    let kinds: Vec<FileKind> = project.files[5..].iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            FileKind::Cpl,
            FileKind::Cpl,
            FileKind::Cpl,
            FileKind::Work,
            FileKind::Patch
        ]
    );
    assert!(project.diagnostics.is_empty(), "{:?}", project.diagnostics);
}

#[test]
fn file_ids_follow_path_order_whatever_the_file_kind() {
    let tree = tree(&[
        ("cpl.toml", &manifest_with_sources("[\"zsrc\", \"asrc\"]")),
        ("zsrc/z.cpl", "module z\n"),
        ("asrc/a.cpl", "module a\n"),
        ("work/w.work", WORK),
    ]);
    let project = load_tree(&tree, false);
    assert_eq!(
        project_paths(&project),
        ["asrc/a.cpl", "work/w.work", "zsrc/z.cpl"]
    );
    for (i, entry) in project.files.iter().enumerate() {
        assert_eq!(entry.file, FileId(i as u32));
    }
    assert_eq!(project.files[6].kind, FileKind::Work);
    assert!(matches!(project.parsed[6], ParsedFile::Skipped));
}

#[test]
fn a_dot_source_directory_is_the_project_root() {
    let tree = tree(&[
        ("cpl.toml", &manifest_with_sources("[\".\"]")),
        ("a.cpl", "module a\n"),
        ("x/y.cpl", "module x.y\n"),
        ("work/w.cpl", "module work.w\n"),
    ]);
    let project = load_tree(&tree, false);
    assert_eq!(project_paths(&project), ["a.cpl", "work/w.cpl", "x/y.cpl"]);
    assert!(project.diagnostics.is_empty(), "{:?}", project.diagnostics);
}

#[test]
fn origins_are_the_given_origin_for_project_files_and_std_for_std() {
    let commit = "0123456789abcdef0123456789abcdef01234567".to_string();
    let tree = tree(&[
        ("cpl.toml", MANIFEST),
        ("src/a.cpl", "module a\n"),
        ("work/w.work", WORK),
    ]);
    let project =
        load_project_from_tree(&tree, FileOrigin::Revision(commit.clone()), &opts(true)).unwrap();
    assert_eq!(project.root, None);
    for entry in &project.files {
        let file = project.source_map.file(entry.file);
        let want = if file.path.starts_with("std/") {
            FileOrigin::Std
        } else {
            FileOrigin::Revision(commit.clone())
        };
        assert_eq!(entry.origin, want, "{}", file.path);
        assert_eq!(file.origin, want, "{}", file.path);
    }
}

// ---- project_source_dir_missing, project_no_sources, project_duplicate_file_module ----

#[test]
fn a_missing_source_directory_is_project_source_dir_missing() {
    let tree = tree(&[
        (
            "cpl.toml",
            &manifest_with_sources("[\"src\", \"lib\", \"gen/out\"]"),
        ),
        ("src/a.cpl", "module a\n"),
        ("lib", "a file, not a directory"),
    ]);
    let project = load_tree(&tree, false);
    assert_eq!(
        codes(&project.diagnostics),
        ["project_source_dir_missing", "project_source_dir_missing"]
    );
    for (d, dir) in project.diagnostics.iter().zip(["lib", "gen/out"]) {
        assert_eq!(d.code, codes::PROJECT_SOURCE_DIR_MISSING);
        assert_eq!(
            (d.category, d.severity, d.span),
            (Category::Project, Severity::Error, None)
        );
        assert_eq!(d.data["path"], json!(dir));
    }
    assert_eq!(project_paths(&project), ["src/a.cpl"]);
}

#[test]
fn no_cpl_file_is_the_warning_project_no_sources() {
    // An empty source directory exists on disk ...
    let dir = temp_dir("no-sources");
    write(&dir, "cpl.toml", MANIFEST);
    write(&dir, "src/readme.txt", "no sources\n");
    write(&dir, "work/w.work", WORK);
    let project = load_dir(&dir, false);
    assert_eq!(codes(&project.diagnostics), ["project_no_sources"]);
    let d = &project.diagnostics[0];
    assert_eq!(
        (d.category, d.severity, d.span),
        (Category::Project, Severity::Warning, None)
    );
    assert_eq!(d.data["sources"], json!(["src"]));
    assert_eq!(project_paths(&project), ["work/w.work"]);

    // ... and without any source directory both facts are reported.
    let project = load_tree(&tree(&[("cpl.toml", MANIFEST)]), false);
    assert_eq!(
        codes(&project.diagnostics),
        ["project_source_dir_missing", "project_no_sources"]
    );

    let project = load_tree(&tree(&[("cpl.toml", &manifest_with_sources("[]"))]), false);
    assert_eq!(codes(&project.diagnostics), ["project_no_sources"]);
}

#[test]
fn overlapping_source_directories_are_project_duplicate_file_module() {
    let tree = tree(&[
        ("cpl.toml", &manifest_with_sources("[\"src\", \"src/a\"]")),
        ("src/a/b.cpl", "module a.b\n"),
        ("src/c.cpl", "module c\n"),
    ]);
    let project = load_tree(&tree, false);
    assert_eq!(
        codes(&project.diagnostics),
        ["project_duplicate_file_module"]
    );
    let d = &project.diagnostics[0];
    assert_eq!(
        (d.category, d.severity, d.span),
        (Category::Project, Severity::Error, None)
    );
    assert_eq!(d.data["path"], json!("src/a/b.cpl"));
    assert_eq!(d.data["sources"], json!(["src", "src/a"]));
    // The file is loaded once; it is conventional under `src`.
    assert_eq!(project_paths(&project), ["src/a/b.cpl", "src/c.cpl"]);

    let tree = tree_with_duplicate_entry();
    let project = load_tree(&tree, false);
    assert_eq!(
        codes(&project.diagnostics),
        ["project_duplicate_file_module"]
    );
}

fn tree_with_duplicate_entry() -> cpl_project::VirtualTree {
    tree(&[
        ("cpl.toml", &manifest_with_sources("[\"src\", \"src/\"]")),
        ("src/a.cpl", "module a\n"),
    ])
}

// ---- parse diagnostics -------------------------------------------------------------

#[test]
fn a_file_with_invalid_utf8_fails_with_invalid_utf8() {
    let mut tree = tree(&[("cpl.toml", MANIFEST), ("src/b.cpl", "module b\n")]);
    tree.files.insert(
        "src/a.cpl".to_string(),
        b"module a\nfn f() -> Text { \"\xff\" }\n".to_vec(),
    );
    let project = load_tree(&tree, false);
    assert_eq!(parsed_kinds(&project)[5..], ["failed", "module"]);
    assert_eq!(codes(&project.diagnostics), ["invalid_utf8"]);
    let d = &project.diagnostics[0];
    assert_eq!(d.category, Category::Parse);
    let span = d.span.unwrap();
    assert_eq!(span.file, FileId(5));
    assert_eq!(project.source_map.line_col(span.file, span.start).line, 2);
    // The file is registered, lossily decoded, and digested from its bytes.
    assert!(project
        .source_map
        .file(FileId(5))
        .text()
        .contains('\u{fffd}'));
    assert_eq!(
        project.files[5].digest.0,
        blake3::hash(&tree.files["src/a.cpl"]).to_hex().to_string()
    );
}

#[test]
fn a_file_that_does_not_parse_fails_with_one_diagnostic_and_the_rest_still_parse() {
    let tree = tree(&[
        ("cpl.toml", MANIFEST),
        ("src/a.cpl", "module a\nfn f() -> Int { 1 ; 2 }\nfn g( {\n"),
        ("src/b.cpl", "module b\n"),
        ("src/c.cpl", "module c\nfn h( ->\n"),
    ]);
    let project = load_tree(&tree, false);
    assert_eq!(parsed_kinds(&project)[5..], ["failed", "module", "failed"]);
    assert_eq!(project.diagnostics.len(), 2);
    for (d, file) in project.diagnostics.iter().zip([FileId(5), FileId(7)]) {
        assert_eq!(d.category, Category::Parse);
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.span.unwrap().file, file);
    }
}

#[test]
fn work_file_diagnostics_are_reported_only_when_work_files_are_included() {
    let mut tree = tree(&[
        ("cpl.toml", MANIFEST),
        ("src/a.cpl", "module a\n"),
        ("work/bad.work", "change {\n"),
        (
            "work/good.work",
            "change c {\n    base git(\"main\")\n    target git(\"main\")\n}\n",
        ),
    ]);
    tree.files
        .insert("work/bytes.patch".to_string(), vec![0xff, 0xfe]);
    let without = load_tree(&tree, false);
    assert!(without.diagnostics.is_empty(), "{:?}", without.diagnostics);
    assert_eq!(
        parsed_kinds(&without)[5..],
        ["module", "skipped", "skipped", "skipped"]
    );

    let with = load_tree(&tree, true);
    assert_eq!(
        project_paths(&with),
        [
            "src/a.cpl",
            "work/bad.work",
            "work/bytes.patch",
            "work/good.work"
        ]
    );
    assert_eq!(
        parsed_kinds(&with)[5..],
        ["module", "failed", "failed", "work"]
    );
    assert_eq!(with.diagnostics.len(), 2);
    assert_eq!(with.diagnostics[0].span.unwrap().file, FileId(6));
    assert_eq!(with.diagnostics[1].code, cpl_syntax::codes::INVALID_UTF8);
    assert_eq!(with.files, without.files);
}

// ---- module_file_misplaced, reserved_module_path ------------------------------------

#[test]
fn modules_at_their_conventional_locations_have_no_diagnostics() {
    let tree = tree(&[
        ("cpl.toml", &manifest_with_sources("[\"src\", \"more\"]")),
        ("src/reader/pages.cpl", "module reader.pages\n"),
        ("src/reader/pages/extra.cpl", "module reader.pages\n"),
        ("more/tests/pagination.cpl", "module tests.pagination\n"),
        ("more/top.cpl", "module top\n"),
    ]);
    let project = load_tree(&tree, false);
    assert!(project.diagnostics.is_empty(), "{:?}", project.diagnostics);
}

#[test]
fn a_module_elsewhere_is_the_warning_module_file_misplaced_on_its_module_path() {
    let tree = tree(&[
        ("cpl.toml", &manifest_with_sources("[\"src\", \"more\"]")),
        ("src/wrong_place.cpl", "module cases.lib\n"),
        (
            "src/cases/lib/deep/er.cpl",
            "// a comment first\nmodule cases.lib\n",
        ),
        ("more/cases.cpl", "module cases.lib\n"),
    ]);
    let project = load_tree(&tree, false);
    assert_eq!(codes(&project.diagnostics), ["module_file_misplaced"; 3]);
    let expected = [
        ("more/cases.cpl", 1, "more/cases/lib.cpl"),
        ("src/cases/lib/deep/er.cpl", 2, "src/cases/lib.cpl"),
        ("src/wrong_place.cpl", 1, "src/cases/lib.cpl"),
    ];
    for (d, (path, line, conventional)) in project.diagnostics.iter().zip(expected) {
        assert_eq!(d.code, codes::MODULE_FILE_MISPLACED);
        assert_eq!(
            (d.category, d.severity),
            (Category::Resolve, Severity::Warning)
        );
        let span = d.span.expect("module_file_misplaced spans the module path");
        let file = project.source_map.file(span.file);
        assert_eq!(file.path, path);
        assert_eq!(project.source_map.text(span), "cases.lib");
        assert_eq!(
            project.source_map.line_col(span.file, span.start).line,
            line
        );
        assert_eq!(d.data["module"], json!("cases.lib"));
        assert_eq!(d.data["expected"], json!(conventional));
    }
}

#[test]
fn std_and_gen_module_paths_are_reserved_module_path() {
    let tree = tree(&[
        ("cpl.toml", MANIFEST),
        ("src/std/extra.cpl", "module std.extra\n"),
        ("src/gen/extra.cpl", "module gen.extra\n"),
        ("src/elsewhere.cpl", "module gen\n"),
        ("src/stdx/a.cpl", "module stdx.a\n"),
        ("src/general/a.cpl", "module general.a\n"),
    ]);
    let project = load_tree(&tree, false);
    // FileId order: src/elsewhere.cpl, src/gen/extra.cpl, src/general/a.cpl, src/std/extra.cpl, src/stdx/a.cpl
    assert_eq!(codes(&project.diagnostics), ["reserved_module_path"; 3]);
    let modules: Vec<_> = project
        .diagnostics
        .iter()
        .map(|d| d.data["module"].clone())
        .collect();
    assert_eq!(
        modules,
        [json!("gen"), json!("gen.extra"), json!("std.extra")]
    );
    for d in &project.diagnostics {
        assert_eq!(
            (d.category, d.severity),
            (Category::Resolve, Severity::Error)
        );
        let span = d.span.unwrap();
        assert_eq!(
            project.source_map.text(span),
            d.data["module"].as_str().unwrap()
        );
    }
}

#[test]
fn project_diagnostics_come_first_then_parse_then_resolve() {
    let tree = tree(&[
        ("cpl.toml", "[project]\nname = \"t\"\nedition = \"0.2\"\n\n[build]\nsources = [\"src\", \"lib\"]\nextra = 1\n"),
        ("src/a.cpl", "module std.a\n"),
        ("src/b.cpl", "module b\nfn (\n"),
        ("src/c.cpl", "module wrong\n"),
    ]);
    let project = load_tree(&tree, false);
    assert_eq!(
        codes(&project.diagnostics),
        [
            "project_manifest_unknown_key",
            "project_source_dir_missing",
            "syntax_error",
            "unsupported_edition",
            "reserved_module_path",
            "module_file_misplaced",
        ]
    );
}

// ---- LoadError::Io ---------------------------------------------------------------------

// ---- symbolic links (toolchain T54, D-244) --------------------------------------------
//
// A link to a file is that file; a link to a directory is never followed, so
// nothing reached only through it is a member; a link that leads to no file
// (dangling, a loop, a link to a directory, a path through a file) is not a
// member. Each test also loads the tree a commit of the directory yields
// (`git ls-tree` records a link, not the tree behind it, and `tree_at` leaves
// out every link that leads to no file), which must be the same project.

#[cfg(unix)]
#[test]
fn symlinks_that_lead_to_no_file_are_not_members() {
    use std::os::unix::fs::symlink;
    let dir = temp_dir("t54-no-file");
    write(&dir, "cpl.toml", MANIFEST);
    write(&dir, "src/a.cpl", "module a\n");
    write(&dir, "work/w.work", WORK);
    // Dangling, a loop, a path through a file, and a link to a directory.
    symlink(dir.join("src/nowhere.cpl"), dir.join("src/broken.cpl")).unwrap();
    symlink(dir.join("src/loop.cpl"), dir.join("src/loop.cpl")).unwrap();
    symlink(dir.join("cpl.toml/x.cpl"), dir.join("src/through.cpl")).unwrap();
    fs::create_dir_all(dir.join("elsewhere/d.cpl")).unwrap();
    symlink(dir.join("elsewhere/d.cpl"), dir.join("src/d.cpl")).unwrap();
    symlink(dir.join("work/nowhere.work"), dir.join("work/broken.work")).unwrap();
    symlink(dir.join("work/loop.patch"), dir.join("work/loop.patch")).unwrap();
    // A dangling link that is not a source file is not even looked at.
    symlink(dir.join("nowhere"), dir.join("src/notes.txt")).unwrap();
    // cpl.lock is read under the same rule: a looping link is no lock.
    symlink(dir.join("cpl.lock"), dir.join("cpl.lock")).unwrap();
    let project = load_dir(&dir, true);
    assert_eq!(project_paths(&project), ["src/a.cpl", "work/w.work"]);
    assert!(
        project.diagnostics.is_empty(),
        "{:?}",
        codes(&project.diagnostics)
    );
    assert!(project.lock.is_none() && project.lock_digest.is_none());
    let commit = tree(&[
        ("cpl.toml", MANIFEST),
        ("src/a.cpl", "module a\n"),
        ("work/w.work", WORK),
    ]);
    assert_same_project(&project, &load_tree(&commit, true), false);

    // cpl.toml that leads to no file is no manifest.
    let dir = temp_dir("t54-no-manifest");
    write(&dir, "src/a.cpl", "module a\n");
    symlink(dir.join("cpl.toml"), dir.join("cpl.toml")).unwrap();
    assert!(matches!(
        load_project(&dir, &LoadOptions::default()),
        Err(LoadError::NoManifest(_))
    ));
    fs::remove_file(dir.join("cpl.toml")).unwrap();
    symlink(dir.join("nowhere.toml"), dir.join("cpl.toml")).unwrap();
    assert!(matches!(
        load_project(&dir, &LoadOptions::default()),
        Err(LoadError::NoManifest(_))
    ));
}

#[cfg(unix)]
#[test]
fn symlinked_files_are_read_and_symlinked_directories_are_not_followed() {
    use std::os::unix::fs::symlink;
    let dir = temp_dir("symlinks");
    write(&dir, "elsewhere/manifest.toml", MANIFEST);
    write(&dir, "elsewhere/b.cpl", "module b\n");
    write(&dir, "elsewhere/inner/c.cpl", "module inner.c\n");
    write(&dir, "src/a.cpl", "module a\n");
    // cpl.toml itself may be a link to a file.
    symlink(dir.join("elsewhere/manifest.toml"), dir.join("cpl.toml")).unwrap();
    symlink(dir.join("elsewhere/b.cpl"), dir.join("src/b.cpl")).unwrap();
    symlink(dir.join("elsewhere/inner"), dir.join("src/inner")).unwrap();
    let project = load_dir(&dir, false);
    assert_eq!(project_paths(&project), ["src/a.cpl", "src/b.cpl"]);
    let commit = tree(&[
        ("cpl.toml", MANIFEST),
        ("src/a.cpl", "module a\n"),
        ("src/b.cpl", "module b\n"),
    ]);
    assert_same_project(&project, &load_tree(&commit, false), false);
}

/// A `[build] sources` directory that is itself a link, or is reached
/// through one, is never followed: it does not exist for loading, exactly
/// as in a commit, so it is `project_source_dir_missing` and none of the
/// files behind it is a member (`read_dir` alone would follow the link).
#[cfg(unix)]
#[test]
fn a_source_directory_that_is_a_symlink_is_not_followed() {
    use std::os::unix::fs::symlink;
    let dir = temp_dir("t54-source-dir");
    write(
        &dir,
        "cpl.toml",
        manifest_with_sources("[\"src\", \"lib/src\", \"real\"]"),
    );
    write(&dir, "real/r.cpl", "module r\n");
    write(&dir, "elsewhere/a.cpl", "module a\n");
    write(&dir, "shared/src/b.cpl", "module b\n");
    symlink(dir.join("elsewhere"), dir.join("src")).unwrap();
    symlink(dir.join("shared"), dir.join("lib")).unwrap();
    let project = load_dir(&dir, false);
    assert_eq!(project_paths(&project), ["real/r.cpl"]);
    assert_eq!(
        codes(&project.diagnostics),
        ["project_source_dir_missing", "project_source_dir_missing"]
    );
    let missing: Vec<_> = project
        .diagnostics
        .iter()
        .map(|d| d.data["path"].clone())
        .collect();
    assert_eq!(missing, [json!("src"), json!("lib/src")]);
    let commit = tree(&[
        (
            "cpl.toml",
            &manifest_with_sources("[\"src\", \"lib/src\", \"real\"]"),
        ),
        ("real/r.cpl", "module r\n"),
    ]);
    assert_same_project(&project, &load_tree(&commit, false), false);

    // With every source directory a link, the source set has no .cpl file.
    let dir = temp_dir("t54-source-dir-only");
    write(&dir, "cpl.toml", MANIFEST);
    write(&dir, "elsewhere/a.cpl", "module a\n");
    symlink(dir.join("elsewhere"), dir.join("src")).unwrap();
    let project = load_dir(&dir, false);
    assert!(project_paths(&project).is_empty());
    assert_eq!(
        codes(&project.diagnostics),
        ["project_source_dir_missing", "project_no_sources"]
    );
    assert_same_project(
        &project,
        &load_tree(&tree(&[("cpl.toml", MANIFEST)]), false),
        false,
    );
}

/// `work/` that is a link to a directory is never followed: its files are
/// not members, parsed or not.
#[cfg(unix)]
#[test]
fn a_work_directory_that_is_a_symlink_is_not_followed() {
    use std::os::unix::fs::symlink;
    let dir = temp_dir("t54-work-dir");
    write(&dir, "cpl.toml", MANIFEST);
    write(&dir, "src/a.cpl", "module a\n");
    write(&dir, "elsewhere/w.work", WORK);
    write(&dir, "elsewhere/p.patch", "patch\n");
    symlink(dir.join("elsewhere"), dir.join("work")).unwrap();
    for include_work_files in [false, true] {
        let project = load_dir(&dir, include_work_files);
        assert_eq!(project_paths(&project), ["src/a.cpl"]);
        assert!(
            project.diagnostics.is_empty(),
            "{:?}",
            codes(&project.diagnostics)
        );
        let commit = tree(&[("cpl.toml", MANIFEST), ("src/a.cpl", "module a\n")]);
        assert_same_project(&project, &load_tree(&commit, include_work_files), false);
    }
}

#[cfg(unix)]
#[test]
fn an_unreadable_source_file_is_load_error_io() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("io-unreadable");
    write(&dir, "cpl.toml", MANIFEST);
    write(&dir, "src/a.cpl", "module a\n");
    let file = dir.join("src/a.cpl");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&file).is_ok() {
        // Running with privileges that ignore permissions; nothing to test.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }
    let result = load_project(&dir, &LoadOptions::default());
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    match result {
        Err(LoadError::Io { path, source }) => {
            assert_eq!(path, "src/a.cpl");
            assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
            let shown = LoadError::Io { path, source }.to_string();
            assert!(shown.starts_with("src/a.cpl: "), "{shown}");
        }
        other => panic!("expected LoadError::Io, found {other:?}"),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_source_file_name_that_is_not_utf8_is_load_error_io() {
    use std::os::unix::ffi::OsStrExt;
    let dir = temp_dir("io-non-utf8-name");
    write(&dir, "cpl.toml", MANIFEST);
    write(&dir, "src/a.cpl", "module a\n");
    let name = std::ffi::OsStr::from_bytes(b"bad\xff.cpl");
    fs::write(dir.join("src").join(name), "module bad\n").unwrap();
    match load_project(&dir, &LoadOptions::default()) {
        Err(LoadError::Io { path, source }) => {
            assert_eq!(path, "src/bad\u{fffd}.cpl");
            assert_eq!(source.kind(), std::io::ErrorKind::InvalidData);
        }
        other => panic!("expected LoadError::Io, found {other:?}"),
    }
}

#[test]
fn every_load_error_variant_displays_its_path_and_message() {
    let cases = [
        (LoadError::NoManifest("x".into()), "no cpl.toml found at x"),
        (
            LoadError::Manifest {
                path: "cpl.toml".into(),
                message: "m".into(),
                line: Some(1),
            },
            "cpl.toml: m",
        ),
        (
            LoadError::Lock {
                path: "cpl.lock".into(),
                message: "l".into(),
                line: None,
            },
            "cpl.lock: l",
        ),
        (
            LoadError::Io {
                path: "src/a.cpl".into(),
                source: std::io::Error::other("boom"),
            },
            "src/a.cpl: boom",
        ),
        (
            LoadError::Std("gone".into()),
            "standard library unavailable: gone",
        ),
    ];
    for (err, shown) in cases {
        assert_eq!(err.to_string(), shown);
    }
}
