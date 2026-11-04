//! Checked program evolution. All operations are available as library APIs and through the CLI.
//! Validation runs on immutable snapshots; evidence is bound to source, standard library, and options.
pub mod change;
pub mod concurrent;
pub mod differential;
pub mod equivalence;
pub mod explore;
pub mod merge;
pub mod migration;
pub mod query;
pub mod storage;
pub mod verify;

use cpl_diag::json::{Revision, RevisionState};
use cpl_diag::{Category, DiagCode, Diagnostic, FileOrigin};
use cpl_index::{Index, Target};
use cpl_project::{LoadOptions, Project, VirtualTree};
use cpl_sema::CheckResult;
use std::path::Path;

/// An immutable checked program. The index and digest are computed once, on first use, so
/// the fields must not be changed after construction.
pub struct Snapshot {
    pub tree: VirtualTree,
    pub project: Project,
    pub checked: CheckResult,
    pub revision: Revision,
    index: std::sync::OnceLock<cpl_index::IndexData>,
    digest: std::sync::OnceLock<String>,
    dependents:
        std::sync::OnceLock<std::collections::HashMap<cpl_diag::StableId, Vec<cpl_diag::StableId>>>,
}

impl Snapshot {
    pub fn new(tree: VirtualTree, options: &LoadOptions) -> Result<Self, Diagnostic> {
        let project = cpl_project::load_project_from_tree(&tree, FileOrigin::Virtual, options)
            .map_err(|e| error("evolution_load_failed", e.to_string()))?;
        let checked = cpl_sema::check(&project);
        if checked.has_errors() {
            return Err(error(
                "evolution_invalid_program",
                "the candidate program does not check",
            )
            .with_data(
                "diagnostics",
                serde_json::json!(checked
                    .diagnostics
                    .iter()
                    .map(|d| d.to_json(&project.source_map))
                    .collect::<Vec<_>>()),
            ));
        }
        let revision = Revision {
            commit: None,
            state: RevisionState::Worktree,
            digest: cpl_index::source_digest(&project),
        };
        Ok(Self {
            tree,
            project,
            checked,
            revision,
            index: std::sync::OnceLock::new(),
            digest: std::sync::OnceLock::new(),
            dependents: std::sync::OnceLock::new(),
        })
    }

    pub fn read(root: &Path, options: &LoadOptions) -> Result<Self, Diagnostic> {
        let project = cpl_project::load_project(root, options)
            .map_err(|e| error("evolution_load_failed", e.to_string()))?;
        let mut tree = VirtualTree::default();
        for entry in &project.files {
            if entry.origin == FileOrigin::Std {
                continue;
            }
            let source = project.source_map.file(entry.file);
            tree.files
                .insert(source.path.clone(), source.text().as_bytes().to_vec());
        }
        for path in ["cpl.toml", "cpl.lock"] {
            match std::fs::read(root.join(path)) {
                Ok(bytes) => {
                    tree.files.insert(path.into(), bytes);
                }
                Err(e) if path == "cpl.lock" && e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(error("evolution_io", e.to_string())),
            }
        }
        Self::new(tree, options)
    }

    pub fn index(&self) -> Index<'_> {
        let data = self.index.get_or_init(|| {
            cpl_index::IndexData::from_check(&self.checked, &self.project, self.revision.clone())
        });
        Index::with_data(&self.checked.program, data).with_holes(&self.checked.holes)
    }
    pub fn resolve(&self, target: &str) -> Result<cpl_hir::DefId, Diagnostic> {
        self.index().resolve(&Target::parse(target))
    }

    /// The reverse reference graph by stable identity, over every reference kind: for each
    /// declaration, the declarations that reference it.
    pub fn dependents(
        &self,
    ) -> &std::collections::HashMap<cpl_diag::StableId, Vec<cpl_diag::StableId>> {
        self.dependents.get_or_init(|| {
            let p = &self.checked.program;
            let mut graph: std::collections::HashMap<cpl_diag::StableId, Vec<cpl_diag::StableId>> =
                std::collections::HashMap::new();
            for r in &p.references {
                graph
                    .entry(p.def(r.to).stable_id.clone())
                    .or_default()
                    .push(p.def(r.from).stable_id.clone());
            }
            for list in graph.values_mut() {
                list.sort();
                list.dedup();
            }
            graph
        })
    }

    /// Includes the lock and std, unlike a source-set digest alone.
    pub fn digest(&self) -> String {
        self.digest.get_or_init(|| self.compute_digest()).clone()
    }

    fn compute_digest(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(b"cpl-evolution-v1\0");
        h.update(env!("CARGO_PKG_VERSION").as_bytes());
        for (path, bytes) in &self.tree.files {
            h.update(&(path.len() as u64).to_le_bytes());
            h.update(path.as_bytes());
            h.update(&(bytes.len() as u64).to_le_bytes());
            h.update(bytes);
        }
        h.update(self.project.std_digest().as_str().as_bytes());
        h.finalize().to_hex().to_string()
    }
}

pub fn error(code: &'static str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(DiagCode(code), Category::Index, message)
}
