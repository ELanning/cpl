//! The project files the index reads (`spec/toolchain.md` Section 0): the
//! parsed `.cpl` modules of the source set in path order, and the digest of
//! every source-set file. Shared by stable-id assignment, the facet map,
//! revisions, and receipts, so that all of them read one definition of the
//! source set.

use cpl_diag::json::Digest;
use cpl_diag::{FileId, FileOrigin};
use cpl_project::{ParsedFile, Project};
use cpl_syntax::ast;

/// The manifest's project-relative path: the one source-set member that is
/// not a `SourceEntry` (its digest is `Project::manifest_digest`).
pub(crate) const MANIFEST: &str = "cpl.toml";

/// One parsed `.cpl` file of the source set.
pub(crate) struct ProjectModule<'p> {
    pub file: FileId,
    /// Project-relative, `/`-separated (`src/reader/pages.cpl`).
    pub path: &'p str,
    pub text: &'p str,
    pub module: &'p ast::Module,
}

/// Every parsed `.cpl` file of the source set, in lexicographic order of
/// project-relative path: std files (which are never members of the source
/// set) and files that failed to parse (which have no tree, D59) are
/// left out.
pub(crate) fn project_modules(project: &Project) -> Vec<ProjectModule<'_>> {
    let mut modules: Vec<ProjectModule<'_>> = project
        .files
        .iter()
        .zip(&project.parsed)
        .filter_map(|(entry, parsed)| {
            if entry.origin == FileOrigin::Std {
                return None;
            }
            let ParsedFile::Module(module) = parsed else {
                return None;
            };
            let file = project.source_map.file(entry.file);
            Some(ProjectModule {
                file: entry.file,
                path: file.path.as_str(),
                text: file.text(),
                module: module.as_ref(),
            })
        })
        .collect();
    modules.sort_by(|a, b| a.path.cmp(b.path));
    modules
}

/// `(project-relative path, blake3 of the bytes)` of every file of the
/// source set (Section 0: `cpl.toml`, the `.cpl` files under the source
/// directories, the `.work` and `.patch` files directly under `work/`;
/// never `cpl.lock`, never std), in path order.
pub(crate) fn source_set(project: &Project) -> Vec<(String, Digest)> {
    let mut files = vec![(MANIFEST.to_string(), project.manifest_digest.clone())];
    for entry in &project.files {
        if entry.origin != FileOrigin::Std {
            let path = &project.source_map.file(entry.file).path;
            files.push((path.clone(), entry.digest.clone()));
        }
    }
    files.sort();
    files
}

/// The segments of a file's `module` header (`["reader", "pages"]`).
pub(crate) fn module_path(module: &ast::Module) -> Vec<String> {
    module
        .header
        .path
        .segments
        .iter()
        .map(|segment| segment.text.clone())
        .collect()
}

/// blake3-256 of some bytes, 64 lowercase hex characters.
pub(crate) fn digest(bytes: &[u8]) -> Digest {
    Digest(blake3::hash(bytes).to_hex().to_string())
}
