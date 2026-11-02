//! A directory and a `VirtualTree` of it produce equal `Project`s
//! (`spec/toolchain.md` 1.6), apart from `root` and the origin of the
//! project files, which is the one the tree is loaded with.

mod common;

use common::*;
use cpl_diag::FileOrigin;
use cpl_project::{load_project_from_tree, Project};
use std::path::{Path, PathBuf};

/// Every directory case of the sema corpus: `ok/<case>/`, `errors/project/<case>/`, `release/<case>/`.
fn corpus_directory_cases() -> Vec<PathBuf> {
    let mut cases = Vec::new();
    for group in ["ok", "errors/project", "release"] {
        let dir = sema_corpus().join(group);
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .filter(|p| p.join("cpl.toml").is_file())
            .collect();
        found.sort();
        cases.extend(found);
    }
    assert!(
        cases.len() >= 15,
        "found only {} directory cases",
        cases.len()
    );
    cases
}

fn check_equal(dir: &Path) {
    for include_work_files in [false, true] {
        let from_dir = load_dir(dir, include_work_files);
        let tree = tree_of_dir(dir);
        let virtual_origin = load_tree(&tree, include_work_files);
        assert_eq!(from_dir.root.as_deref(), Some(dir));
        assert_eq!(virtual_origin.root, None);
        assert_same_project(&from_dir, &virtual_origin, false);

        let project_origin: Project =
            load_project_from_tree(&tree, FileOrigin::Project, &opts(include_work_files)).unwrap();
        assert_same_project(&from_dir, &project_origin, true);
    }
}

#[test]
fn the_examples_load_equally_from_a_directory_and_a_tree() {
    check_equal(&example("reader"));
    check_equal(&example("holes"));
}

#[test]
fn every_corpus_directory_case_loads_equally_from_a_directory_and_a_tree() {
    for dir in corpus_directory_cases() {
        check_equal(&dir);
    }
}

#[test]
fn a_project_with_every_kind_of_loading_diagnostic_loads_equally() {
    let dir = temp_dir("tree-equality-messy");
    write(
        &dir,
        "cpl.toml",
        "[project]\nname = \"messy\"\nedition = \"0.2\"\nextra = true\n\n[build]\nsources = [\"src\", \"src/deep\", \"missing\"]\n",
    );
    write(&dir, "src/ok/a.cpl", "module ok.a\n");
    write(&dir, "src/deep/b.cpl", "module deep.b\n");
    write(&dir, "src/elsewhere.cpl", "module some.place\n");
    write(&dir, "src/std/x.cpl", "module std.x\n");
    write(&dir, "src/broken.cpl", "module broken\nfn (\n");
    write(&dir, "src/bytes.cpl", b"module bytes\n\xc3\x28\n");
    write(&dir, "src/notes.md", "not a source\n");
    write(&dir, "work/w.work", "change c {\n}\n");
    write(&dir, "work/p.patch", "not a patch\n");
    write(&dir, "work/deeper/x.work", "ignored\n");
    let project = load_dir(&dir, true);
    assert_eq!(
        codes(&project.diagnostics),
        [
            "project_manifest_unknown_key",
            "project_source_dir_missing",
            "project_duplicate_file_module",
            "syntax_error",
            "invalid_utf8",
            "syntax_error",
            "unsupported_edition",
            "module_file_misplaced",
            "reserved_module_path",
        ]
    );
    check_equal(&dir);
}
