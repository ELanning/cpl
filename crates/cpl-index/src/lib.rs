//! cpl-index: stable identities, facets and content hashes, revisions and
//! git trees, the semantic diff, receipts, and the semantic index over a
//! checked program (`spec/toolchain.md` 1.8 and Sections 4, 5, 6, 8, 9).
//!
//! Everything but the semantic index is computed from a loaded
//! [`cpl_project::Project`] (re-exported by the checker as
//! `cpl_sema::project`) without type checking: facet maps and semantic
//! diffs are purely syntactic, so they work for revisions that do not
//! typecheck (T31). The [`Index`] reads a checked `Program`
//! (`cpl_sema::hir`) and answers `cpl inspect` (Section 6); with it come
//! the evidence `cpl check`, `cpl prove`, and `cpl test` attach to what the
//! checker and the runner report (hole dependencies and receipts, Sections
//! 7.2 and 8), so that `cpl-cli` holds no logic of its own (1.10, T3).
//!
//! | Module | Contents | Specification |
//! | - | - | - |
//! | [`id`] | [`IdKind`], [`generate_id`], [`is_generated_format`], [`IdAssignment`], [`plan_id_assignments`], [`apply_id_assignments`] | 4.1, 4.2 |
//! | [`facet`] | [`applicable_facets`], [`facet_text`], [`facet_hash`], [`definition_hash`], [`declaration_identity`], [`symbol_path`], [`symbol_version`], [`FacetEntry`], [`FacetMap`], [`facet_map`], [`module_content_hash`] | 4.3-4.5, 5, semantics S48 |
//! | [`revision`] | [`GitRepo`], [`discover_repo`], [`source_digest`], [`current_revision`], [`resolve_rev`], [`merge_base`], [`tree_at`], [`GitError`] | 6.3, 9.3 |
//! | [`history`] | [`facet_map_at`], [`changes_base`], [`git_error_diagnostic`], [`DEFAULT_SINCE`] | 6.2 `changes`, 9.3, Appendix B |
//! | [`diff`] | [`semantic_diff`] | 9.1, 9.2 |
//! | [`receipt`] | [`ReceiptInputs`], [`receipt_inputs`], [`verification_receipt`], [`test_receipt`], [`receipt_digest`] | 8 |
//! | [`query`] | [`Target`], [`IncludeSet`], [`Include`], [`Dependency`], [`Index`] (build, resolution, callers, dependencies, tests, obligations, [`Index::inspect`]) | 6, 8.1 |
//! | [`evidence`] | hole dependencies, receipt objects, check completion, [`Index::prove`] ([`Proof`]), test receipts ([`test_check_configuration`], [`test_environment`]) | 7.2, 8, semantics S48, S59 |
//! | [`codes`] | the diagnostic codes this crate produces | Appendix B |
//!
//! Every item is also re-exported at the crate root (`cpl_index::facet_map`,
//! `cpl_index::plan_id_assignments`, ...), the paths the specifications use.
//!
//! Readings of the specifications this crate implements (each is cited at
//! its item):
//!
//! - A facet map covers the declarations of the *source set* (Section 0):
//!   std, which `Project::parsed` also holds, is not part of a revision.
//! - The generator argument is `&mut dyn rand::Rng`: rand 0.10 renamed the
//!   `RngCore` of Section 1.8 to `Rng` (the old name survives deprecated).
//! - `cpl id assign` inserts the line `@id("...")` LF at column 0 of the
//!   line of the first annotation or keyword (4.2 step 3), before any
//!   indentation it has, which `IdAssignment::indent` records: 1.8's "always
//!   `""` for top-level items" holds for formatted files only, since
//!   indentation is never significant (grammar 1.1).
//! - A diff statement's `facet` is the facet whose hashes differ
//!   (`identity_renamed` is about `name`, the effect statements about
//!   `effects`); `identity_added`, `identity_removed`, and `moved` have none.
//!   A kind change is `identity_removed` plus `identity_added`, each with
//!   `"note": "kind_changed"` in its `detail`. A statement's `path` is the
//!   new side's whenever the id is in the new map, else the old side's (9.1:
//!   "ordered by `path` (new side, else old)"), so both statements of a kind
//!   change that also renamed show the new path, and the removed one keeps
//!   the old side's version (with its path) in `detail.version`.
//! - A clause that contains an `if` or `match` keeps the formatter's block
//!   layout in canonical text (2.4 rule 8, `format_fragment` as 5.2
//!   prescribes): each clause begins an unindented line, but 5.2's "every
//!   clause ... is one line" holds only for clauses without blocks, since a
//!   contract block may hold `let` statements (semantics S32) and two
//!   statements never share a line (grammar 2.3).
//! - `tree_at` of a commit without `cpl.toml` is an empty tree, which loads
//!   as `LoadError::NoManifest` (the CLI's `git_no_project_at_revision`).
//! - `tree_at` reads a commit as `cpl_project::load_project` reads a
//!   checkout of it, so that a facet map "is the same whether built from a
//!   directory or from a git tree" (5.4): a symbolic link among the files of
//!   the source set is followed inside the commit's tree, and one that leads
//!   to no file of the commit is not part of it.
//! - The semantic index's readings (bare-name targets, call sites, which
//!   facet texts a symbol block carries) are listed in [`query`]; those of
//!   receipts and hole dependencies (`source_revision` and `dirty`, the
//!   modules listed, the Milestone 1 verification status) in [`evidence`];
//!   the numbering of obligations is documented at [`Index::obligations`].

pub mod codes;
pub mod diff;
pub mod evidence;
pub mod facet;
pub mod history;
pub mod id;
mod obligations;
pub mod query;
pub mod receipt;
pub mod revision;
mod source;
mod syntax;
mod walk;

pub use diff::semantic_diff;
pub use evidence::{test_check_configuration, test_environment, Proof};
pub use facet::{
    applicable_facets, declaration_identity, definition_hash, facet_hash, facet_map, facet_text,
    module_content_hash, symbol_path, symbol_version, symbol_version_cached, FacetEntry, FacetMap,
};
pub use history::{changes_base, facet_map_at, git_error_diagnostic, DEFAULT_SINCE};
pub use id::{
    apply_id_assignments, generate_id, is_generated_format, plan_id_assignments, IdAssignment,
    IdKind, StableId,
};
pub use query::{Dependency, Include, IncludeSet, Index, IndexData, Target};
pub use receipt::{
    receipt_digest, receipt_inputs, test_receipt, verification_receipt, ReceiptInputs,
};
pub use revision::{
    current_revision, discover_repo, merge_base, resolve_rev, source_digest, tree_at, GitError,
    GitRepo,
};
