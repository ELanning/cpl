//! Helpers shared by the integration tests of `cpl-sema`: check a single
//! source string or a set of files in a [`VirtualTree`] with a synthetic
//! `cpl.toml`, and look at the result as (line, severity, code) triples or
//! as the JSON of the diagnostics (semantics S49).
#![allow(dead_code)]

use cpl_diag::json::DiagnosticJson;
use cpl_diag::{Diagnostic, FileOrigin, Severity};
use cpl_sema::project::{load_project, load_project_from_tree, LoadOptions, Project, VirtualTree};
use cpl_sema::{check_with, CheckConfig, CheckResult};
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
pub fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
}

/// A synthetic manifest (`[project] name`, `edition = "0.1"`).
pub fn manifest(name: &str) -> String {
    format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n")
}

/// The load options of `cpl check`: work files parsed for syntax (S53).
pub fn check_options() -> LoadOptions {
    LoadOptions {
        include_work_files: true,
        ..LoadOptions::default()
    }
}

/// `module a.b.c` -> `src/a/b/c.cpl` (toolchain 12.2: a single file is
/// placed at the conventional path of its header). The header is the first
/// `module` line; a file without one is `src/main.cpl`.
pub fn conventional_path(source: &str) -> String {
    for line in source.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("module ") {
            let path: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            return format!("src/{}.cpl", path.replace('.', "/"));
        }
    }
    "src/main.cpl".to_string()
}

/// A tree of text files.
pub fn tree(files: &[(&str, &str)]) -> VirtualTree {
    VirtualTree {
        files: files
            .iter()
            .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()))
            .collect(),
    }
}

/// Load a tree (`FileOrigin::Virtual`), panicking on a `LoadError`.
pub fn load_tree(tree: &VirtualTree) -> Project {
    load_project_from_tree(tree, FileOrigin::Virtual, &check_options())
        .unwrap_or_else(|e| panic!("the tree does not load: {e}"))
}

/// Load a project directory, panicking on a `LoadError`.
pub fn load_dir(root: &Path) -> Project {
    load_project(root, &check_options())
        .unwrap_or_else(|e| panic!("{} does not load: {e}", root.display()))
}

/// Check one source file placed at its conventional path in a project
/// named `t`.
pub fn check_source(source: &str) -> CheckResult {
    check_source_with(source, &CheckConfig::default())
}

/// [`check_source`] with a configuration.
pub fn check_source_with(source: &str, config: &CheckConfig) -> CheckResult {
    let path = conventional_path(source);
    let manifest = manifest("t");
    check_files_with(
        &[("cpl.toml", manifest.as_str()), (path.as_str(), source)],
        config,
    )
}

/// Check a set of files; a synthetic `cpl.toml` is added when the set has
/// none.
pub fn check_files(files: &[(&str, &str)]) -> CheckResult {
    check_files_with(files, &CheckConfig::default())
}

/// [`check_files`] with a configuration.
pub fn check_files_with(files: &[(&str, &str)], config: &CheckConfig) -> CheckResult {
    let manifest = manifest("t");
    let mut all: Vec<(&str, &str)> = files.to_vec();
    if !all.iter().any(|(p, _)| *p == "cpl.toml") {
        all.push(("cpl.toml", manifest.as_str()));
    }
    let project = load_tree(&tree(&all));
    check_with(&project, config)
}

/// A diagnostic as the corpus markers see it: (line, severity, code), the
/// line 0 for a span-less diagnostic.
pub type Triple = (u32, Severity, String);

pub fn triple(result: &CheckResult, d: &Diagnostic) -> Triple {
    let line = d.span.map_or(0, |s| {
        result.program.source_map().line_col(s.file, s.start).line
    });
    (line, d.severity, d.code.0.to_string())
}

/// Every diagnostic of a result as a triple, in the result's order.
pub fn triples(result: &CheckResult) -> Vec<Triple> {
    result
        .diagnostics
        .iter()
        .map(|d| triple(result, d))
        .collect()
}

/// The diagnostics of a result whose span lies in a project (non-std)
/// file, or that have no span, as triples.
pub fn project_triples(result: &CheckResult) -> Vec<Triple> {
    result
        .diagnostics
        .iter()
        .filter(|d| {
            d.span
                .is_none_or(|s| result.program.source_map().file(s.file).origin != FileOrigin::Std)
        })
        .map(|d| triple(result, d))
        .collect()
}

/// The codes of a result's diagnostics, in order.
pub fn codes(result: &CheckResult) -> Vec<String> {
    result
        .diagnostics
        .iter()
        .map(|d| d.code.0.to_string())
        .collect()
}

/// The diagnostics of a result as S49 JSON objects.
pub fn json(result: &CheckResult) -> Vec<DiagnosticJson> {
    result
        .diagnostics
        .iter()
        .map(|d| d.to_json(result.program.source_map()))
        .collect()
}

/// The diagnostics of a result as JSON values.
pub fn json_values(result: &CheckResult) -> Vec<serde_json::Value> {
    json(result)
        .iter()
        .map(|d| serde_json::to_value(d).expect("a diagnostic serializes"))
        .collect()
}

/// A short rendering of the diagnostics for assertion messages.
pub fn render(result: &CheckResult) -> String {
    cpl_diag::render_all(&result.diagnostics, result.program.source_map())
}

/// Every file under `root`, recursively, by '/'-separated relative path.
pub fn tree_of_dir(root: &Path) -> VirtualTree {
    fn walk(root: &Path, dir: &Path, out: &mut VirtualTree) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("read a directory")
            .map(|e| e.expect("a directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .components()
                    .map(|c| c.as_os_str().to_str().expect("a UTF-8 name"))
                    .collect::<Vec<_>>()
                    .join("/");
                out.files.insert(rel, fs::read(&path).expect("read a file"));
            }
        }
    }
    let mut out = VirtualTree::default();
    walk(root, root, &mut out);
    out
}
