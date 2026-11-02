//! Shared helpers of the cpl-index integration tests.
#![allow(dead_code)] // each test binary uses a different subset

use cpl_diag::json::{Revision, RevisionState};
use cpl_diag::FileOrigin;
use cpl_index::source_digest;
use cpl_project::{
    load_project, load_project_from_tree, LoadOptions, ParsedFile, Project, VirtualTree,
};
use cpl_syntax::ast;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The workspace root (`/mnt/c/code/rpl`).
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A directory of the workspace, e.g. `examples/reader`.
pub fn workspace_path(relative: &str) -> PathBuf {
    workspace().join(relative)
}

/// This crate's directory.
pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// True when golden files are to be rewritten (`UPDATE_GOLDEN=1`).
pub fn update_golden() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1")
}

/// Compare `actual` with the golden file at `path`, or rewrite it under
/// `UPDATE_GOLDEN=1`.
#[track_caller]
pub fn check_golden(path: &Path, actual: &str) {
    if update_golden() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the golden directory");
        }
        std::fs::write(path, actual).expect("write the golden file");
        return;
    }
    let expected = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (run with UPDATE_GOLDEN=1 to create it)",
            path.display()
        )
    });
    if expected != actual {
        let first = expected
            .lines()
            .zip(actual.lines())
            .position(|(e, a)| e != a)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "{} differs from the computed output (first difference at line {}):\n--- expected\n{}\n--- actual\n{}\n(run with UPDATE_GOLDEN=1 to accept)",
            path.display(),
            first + 1,
            expected.lines().nth(first).unwrap_or("<end>"),
            actual.lines().nth(first).unwrap_or("<end>"),
        );
    }
}

/// Load a project directory, `work/` files included.
#[track_caller]
pub fn load_dir(root: &Path) -> Project {
    load_project(
        root,
        &LoadOptions {
            include_work_files: true,
            ..LoadOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
}

/// Load an in-memory tree with the default options.
#[track_caller]
pub fn load_tree(tree: &VirtualTree) -> Project {
    load_project_from_tree(tree, FileOrigin::Virtual, &LoadOptions::default())
        .unwrap_or_else(|e| panic!("virtual tree: {e}"))
}

/// A tree from `(path, text)` pairs, with a minimal `cpl.toml` unless one
/// is given.
pub fn tree(files: &[(&str, &str)]) -> VirtualTree {
    let mut tree = VirtualTree::default();
    tree.files.insert(
        "cpl.toml".to_string(),
        b"[project]\nname = \"t\"\nedition = \"0.1\"\n".to_vec(),
    );
    for (path, text) in files {
        tree.files
            .insert(path.to_string(), text.as_bytes().to_vec());
    }
    tree
}

/// Every file of a directory, recursively, as a tree keyed by `/` paths.
pub fn tree_of_dir(root: &Path) -> VirtualTree {
    fn walk(root: &Path, dir: &Path, tree: &mut VirtualTree) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, tree);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                tree.files
                    .insert(relative, std::fs::read(&path).expect("read file"));
            }
        }
    }
    let mut tree = VirtualTree::default();
    walk(root, root, &mut tree);
    tree
}

/// The revision of a loaded tree outside any repository.
pub fn unversioned(project: &Project) -> Revision {
    Revision {
        commit: None,
        state: RevisionState::Worktree,
        digest: source_digest(project),
    }
}

/// The parsed module at a project-relative path.
#[track_caller]
pub fn module<'p>(project: &'p Project, path: &str) -> &'p ast::Module {
    let id = project
        .source_map
        .find(path)
        .unwrap_or_else(|| panic!("no file {path}"));
    let index = project
        .files
        .iter()
        .position(|e| e.file == id)
        .expect("a source entry");
    match &project.parsed[index] {
        ParsedFile::Module(m) => m,
        other => panic!("{path} is not a parsed module: {other:?}"),
    }
}

/// The module path of a parsed module.
pub fn module_path(module: &ast::Module) -> Vec<String> {
    module
        .header
        .path
        .segments
        .iter()
        .map(|s| s.text.clone())
        .collect()
}

/// The declaration with this display name.
#[track_caller]
pub fn decl<'m>(module: &'m ast::Module, name: &str) -> &'m ast::Declaration {
    module
        .declarations
        .iter()
        .find(|d| d.display_name() == name)
        .unwrap_or_else(|| panic!("no declaration {name}"))
}

/// The temporary directories handed out in this process.
static TEMP_NAMES: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// A fresh, empty directory under `CARGO_TARGET_TMPDIR`, named after the
/// test binary and `name` (unique within the binary), so that a later run
/// reuses and clears it instead of accumulating directories.
pub fn temp_dir(name: &str) -> PathBuf {
    let unique = format!("cpl-index-{}-{name}", env!("CARGO_CRATE_NAME"));
    assert!(
        TEMP_NAMES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(unique.clone()),
        "temporary directory {name} requested twice"
    );
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(unique);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear the temporary directory");
    }
    std::fs::create_dir_all(&dir).expect("create the temporary directory");
    dir
}

/// Write `(path, text)` files under `root`, creating directories.
pub fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create directories");
        }
        std::fs::write(&full, text).expect("write file");
    }
}
