//! Shared helpers of the cpl-work integration tests.
#![allow(dead_code)] // each test binary uses a different subset

use cpl_diag::json::{Revision, RevisionState};
use cpl_diag::{Diagnostic, FileOrigin};
use cpl_index::{facet_map, source_digest, FacetMap};
use cpl_project::{load_project_from_tree, LoadOptions, ParsedFile, Project, VirtualTree};
use cpl_sema::hir::Program;
use cpl_syntax::ast;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A directory of the workspace, e.g. `examples/reader`.
pub fn workspace_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// Every file of a directory, recursively, keyed by `/` paths.
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

/// `examples/reader` as a tree.
pub fn reader_tree() -> VirtualTree {
    tree_of_dir(&workspace_path("examples/reader"))
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

/// `tree` with one file replaced or added.
pub fn with_file(tree: &VirtualTree, path: &str, text: &str) -> VirtualTree {
    let mut tree = tree.clone();
    tree.files
        .insert(path.to_string(), text.as_bytes().to_vec());
    tree
}

/// `tree` without one file.
pub fn without_file(tree: &VirtualTree, path: &str) -> VirtualTree {
    let mut tree = tree.clone();
    tree.files.remove(path);
    tree
}

/// The text of a file of a tree.
#[track_caller]
pub fn text(tree: &VirtualTree, path: &str) -> String {
    String::from_utf8(
        tree.files
            .get(path)
            .unwrap_or_else(|| panic!("no {path}"))
            .clone(),
    )
    .expect("utf-8")
}

/// Load a tree, `work/` files included; panics on a load error or on any
/// parse diagnostic.
#[track_caller]
pub fn load(tree: &VirtualTree) -> Project {
    let project = load_project_from_tree(
        tree,
        FileOrigin::Virtual,
        &LoadOptions {
            include_work_files: true,
            ..LoadOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("virtual tree: {e}"));
    let failed: Vec<&Diagnostic> = project
        .diagnostics
        .iter()
        .filter(|d| d.category == cpl_diag::Category::Parse)
        .collect();
    assert!(failed.is_empty(), "parse diagnostics: {failed:?}");
    project
}

/// The revision of a loaded tree outside any repository.
pub fn unversioned(project: &Project) -> Revision {
    Revision {
        commit: None,
        state: RevisionState::Worktree,
        digest: source_digest(project),
    }
}

/// The facet map of a tree.
#[track_caller]
pub fn map_of(tree: &VirtualTree) -> FacetMap {
    let project = load(tree);
    let revision = unversioned(&project);
    facet_map(&project, revision)
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

/// The parsed patch file at a project-relative path.
#[track_caller]
pub fn patch_file<'p>(project: &'p Project, path: &str) -> &'p ast::PatchFile {
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
        ParsedFile::Patch(p) => p,
        other => panic!("{path} is not a parsed patch file: {other:?}"),
    }
}

/// The parsed work file at a project-relative path.
#[track_caller]
pub fn work_file<'p>(project: &'p Project, path: &str) -> &'p ast::WorkFile {
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
        ParsedFile::Work(w) => w,
        other => panic!("{path} is not a parsed work file: {other:?}"),
    }
}

/// Load a tree ([`load`]) and check it: the project and its checked
/// program; panics when the check reports an error.
#[track_caller]
pub fn checked(tree: &VirtualTree) -> (Project, Program) {
    let project = load(tree);
    let result = cpl_sema::check(&project);
    let errors: Vec<&Diagnostic> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "check errors: {errors:?}");
    (project, result.program)
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

/// The codes of some diagnostics, in order.
pub fn codes(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.0).collect()
}

/// The temporary directories handed out in this process.
static TEMP_NAMES: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// A fresh, empty directory under `CARGO_TARGET_TMPDIR`, named after the
/// test binary and `name` (unique within the binary), so that a later run
/// reuses and clears it instead of accumulating directories.
pub fn temp_dir(name: &str) -> PathBuf {
    let unique = format!("cpl-work-{}-{name}", env!("CARGO_CRATE_NAME"));
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
