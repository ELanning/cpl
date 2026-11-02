//! Helpers shared by the integration tests of `cpl-project`.
#![allow(dead_code)]

use cpl_diag::{Diagnostic, FileOrigin, Severity};
use cpl_project::{
    load_project, load_project_from_tree, LoadOptions, ParsedFile, Project, VirtualTree,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root.
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// `examples/<name>`.
pub fn example(name: &str) -> PathBuf {
    workspace().join("examples").join(name)
}

/// `crates/cpl-sema/tests/corpus` (read-only).
pub fn sema_corpus() -> PathBuf {
    workspace()
        .join("crates")
        .join("cpl-sema")
        .join("tests")
        .join("corpus")
}

/// A fresh, empty directory under `CARGO_TARGET_TMPDIR`, unique per `name`.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cpl-project")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("remove an old temporary directory");
    }
    fs::create_dir_all(&dir).expect("create a temporary directory");
    dir
}

/// Write `contents` to `root/rel`, creating directories.
pub fn write(root: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("a parent directory")).expect("create directories");
    fs::write(path, contents).expect("write a file");
}

/// A minimal valid manifest.
pub const MANIFEST: &str = "[project]\nname = \"t\"\nedition = \"0.1\"\n";

/// A tree of text files.
pub fn tree(files: &[(&str, &str)]) -> VirtualTree {
    VirtualTree {
        files: files
            .iter()
            .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()))
            .collect(),
    }
}

/// Every file under `root`, recursively, by '/'-separated relative path.
pub fn tree_of_dir(root: &Path) -> VirtualTree {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("read a directory") {
            let entry = entry.expect("a directory entry");
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .components()
                    .map(|c| c.as_os_str().to_str().expect("a UTF-8 name"))
                    .collect::<Vec<_>>()
                    .join("/");
                files.insert(rel, fs::read(&path).expect("read a file"));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    VirtualTree { files }
}

pub fn opts(include_work_files: bool) -> LoadOptions {
    LoadOptions {
        include_work_files,
        ..LoadOptions::default()
    }
}

/// Load a directory, panicking on a `LoadError`.
pub fn load_dir(root: &Path, include_work_files: bool) -> Project {
    load_project(root, &opts(include_work_files))
        .unwrap_or_else(|e| panic!("{} does not load: {e}", root.display()))
}

/// Load a tree with `FileOrigin::Virtual`, panicking on a `LoadError`.
pub fn load_tree(tree: &VirtualTree, include_work_files: bool) -> Project {
    load_project_from_tree(tree, FileOrigin::Virtual, &opts(include_work_files))
        .unwrap_or_else(|e| panic!("the tree does not load: {e}"))
}

/// The source-map paths in `FileId` order.
pub fn paths(project: &Project) -> Vec<String> {
    project.source_map.files().map(|f| f.path.clone()).collect()
}

/// The paths of the project files (not std) in `FileId` order.
pub fn project_paths(project: &Project) -> Vec<String> {
    project
        .source_map
        .files()
        .filter(|f| f.origin != FileOrigin::Std)
        .map(|f| f.path.clone())
        .collect()
}

/// The codes of a diagnostic list, in order.
pub fn codes(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.0).collect()
}

/// A diagnostic as the corpus markers see it: (line, severity, code), the
/// line `None` for a span-less diagnostic.
pub type Marker = (Option<u32>, Severity, String);

pub fn marker_of(project: &Project, d: &Diagnostic) -> Marker {
    let line = d
        .span
        .map(|span| project.source_map.line_col(span.file, span.start).line);
    (line, d.severity, d.code.0.to_string())
}

/// The kind of each parsed file: "module", "work", "patch", "skipped", "failed".
pub fn parsed_kinds(project: &Project) -> Vec<&'static str> {
    project
        .parsed
        .iter()
        .map(|p| match p {
            ParsedFile::Module(_) => "module",
            ParsedFile::Work(_) => "work",
            ParsedFile::Patch(_) => "patch",
            ParsedFile::Skipped => "skipped",
            ParsedFile::Failed => "failed",
        })
        .collect()
}

/// Assert that two projects are equal in everything but `root` and, unless
/// `same_origin`, the origin of their project files.
pub fn assert_same_project(a: &Project, b: &Project, same_origin: bool) {
    assert_eq!(a.manifest, b.manifest, "manifest");
    assert_eq!(a.lock, b.lock, "lock");
    assert_eq!(a.manifest_digest, b.manifest_digest, "manifest digest");
    assert_eq!(a.diagnostics, b.diagnostics, "diagnostics");
    assert_eq!(a.parsed, b.parsed, "parsed files");
    assert_eq!(a.files.len(), b.files.len(), "number of entries");
    for (x, y) in a.files.iter().zip(&b.files) {
        assert_eq!(x.file, y.file);
        assert_eq!(x.kind, y.kind);
        assert_eq!(x.digest, y.digest);
        if same_origin || x.origin == FileOrigin::Std || y.origin == FileOrigin::Std {
            assert_eq!(x.origin, y.origin);
        }
    }
    assert_eq!(a.source_map.len(), b.source_map.len(), "number of files");
    for (x, y) in a.source_map.files().zip(b.source_map.files()) {
        assert_eq!(x.id, y.id);
        assert_eq!(x.path, y.path);
        assert_eq!(x.text(), y.text(), "text of {}", x.path);
        if same_origin || x.origin == FileOrigin::Std || y.origin == FileOrigin::Std {
            assert_eq!(x.origin, y.origin, "origin of {}", x.path);
        }
    }
}
