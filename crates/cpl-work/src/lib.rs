//! cpl-work: the work coordinator's data model, semantic patches, scope
//! enforcement, the task state machine, and its git commands
//! (`spec/toolchain.md` 1.9 and Section 10; grammar 4).
//!
//! | Module | Contents | Specification |
//! | - | - | - |
//! | [`model`] | [`WorkModel`], [`ChangeSpec`], [`TaskSpec`], [`ResolvedSelector`], [`ResolvedNamespace`], [`FileSelector`], [`RequirementSpec`]; the records [`ChangeRunId`], [`TaskId`], [`CandidateId`], [`FencingToken`], [`Timestamp`], [`Actor`], [`Authority`], [`FrozenInputs`], [`ChangeRecord`], [`ChangeState`], [`TaskRecord`], [`Transition`], [`Lease`], [`Baseline`], [`Candidate`], [`PatchSummary`], [`ReviewRecord`], [`Verdict`] | 1.9, 10.1, 5.5, T42, T43 |
//! | [`validate`] | [`validate_work_file`], [`validate_work_file_with_policy`], [`WorkCheckResult`] | 7.2 |
//! | [`patch`] | [`ResolvedPatch`], [`resolve_patches`], [`validate_patch`], [`apply_patch`], [`parse_submitted_patch`] | 10.5, 5.5, D39 |
//! | [`mod@enforce`] | [`enforce()`], [`Enforcement`], [`TreeDiff`], [`tree_diff`], [`FileChange`], [`FileStatus`], [`ReboundName`], [`module_header`] | 10.5 |
//! | [`scheduler`] | [`TaskState`], [`Grant`], [`RunPolicy`], [`CheckRunner`], [`Sandbox`], [`ExecutionBoundary`]; the state machine [`ChangeRun`] with [`Submission`], [`SubmitOutcome`], [`ScheduleError`]; [`Coordinator`] (`.cpl/work/`) | 10.2, 10.3, 10.6, 10.8, T2, T33, T40 |
//! | [`mod@git`] | [`git()`], [`update_ref_cas`], [`worktree_add`], [`worktree_remove`], [`commit_tree`], [`write_tree`] | 10.7 |
//! | [`codes`] | every `work_*` and `patch_*` code of Appendix B | Appendix B |
//!
//! Two entry points need a checked `cpl_sema::hir::Program` (Section 1.2):
//! [`validate_work_file`] (`cpl work check`, Section 7.2), which resolves a
//! `.work` file's selectors with `cpl_sema::resolve_selector`, and
//! [`resolve_patches`], which resolves a patch form's selectors in the
//! submitting task's scope. Everything else works on a loaded
//! [`cpl_project::Project`] and the syntactic facet maps of `cpl-index`.
//! The Milestone 6 scheduler is implemented as pure functions over the
//! records ([`ChangeRun`]); the orchestration that drives them with a
//! `CheckRunner`, git, and candidate assembly (Sections 10.4 and 10.6) is
//! not.
//!
//! Every item except the code constants is also re-exported at the crate
//! root.
//!
//! Readings of the specifications this crate implements (each is cited at
//! its item):
//!
//! - The [`Baseline`] JSON is the object of Section 5.5 plus what a
//!   restarted coordinator needs to rebuild the facet map: `snapshot_commit`
//!   (a 10.1 field), and per entry `kind`, `module`, `name`, `file`, and
//!   `effects`, with `duplicates` when there are any.
//! - [`CandidateId::derive`] hashes `<task id>\n<attempt>\n<token>\n<tree>`
//!   (10.1 names the fields, not their encoding); [`ChangeRunId`] encodes 80
//!   random bits the caller draws, like a generated `@id`.
//! - [`validate_work_file`] has no manifest and judges file selectors by
//!   the default `[work.policy]`; [`validate_work_file_with_policy`] takes
//!   the project's, and [`WorkCheckResult::output`] is the wire type of
//!   `cpl work check --json` (T3). The readings of 7.2 it implements are
//!   listed in [`validate`].
//! - [`resolve_patches`] reports a selector that names no declaration as
//!   `patch_unknown_symbol` and a namespace or whole-declaration selector
//!   as `patch_facet_unsupported` (D39).
//! - [`ChangeRun::open`] refuses `workers 0` (`work_bad_workers`), as
//!   `cpl work check` does.
//! - [`validate_patch`] reports `patch_facet_unsupported` also for a
//!   declaration without a `body` facet, and [`apply_patch`] replaces test
//!   and property bodies as well as `fn` bodies (5.1 gives them a `body`).
//! - [`validate_patch`] and [`apply_patch`] "blame the target facet" (1.9)
//!   only together with the target's `symbol` and `qualified_name`, as
//!   semantics S49 requires: a target whose qualified name is unknown (in
//!   neither facet map, or declared by no file of the tree) is named by the
//!   payload's `target` instead, without a `facet`.
//! - [`enforce()`] excludes `identity` from "every applicable facet", pairs
//!   the identities of one declaration by qualified name in step 4, and
//!   refuses a changed `@id` value whatever the grant (4.6).
//! - [`enforce()`] step 5 also needs write authority over every unchanged
//!   signature, contract, effects, or body that uses a name whose import
//!   changed (a repointed `use` line, [`ReboundName`]): imports are scoped
//!   to the file (semantics S5), and design lines 285 and 353 make
//!   everything outside the write and add sets read-only, existing
//!   acceptance tests included, where 10.5 rule 5 lets `use` lines ride on
//!   any granted statement of the file.
//! - [`ChangeRun::open`] also rejects unordered tasks with overlapping
//!   write sets or `add` namespaces (`work_overlapping_writes`): the design
//!   says the scheduler rejects them, the specification states the rule
//!   only for `cpl work check` (7.2).
//! - [`ChangeRun::assign`] judges a grant enforceable iff
//!   `sandbox.enforceable().within(&grant.boundary)` (10.8's "host" row,
//!   [`Sandbox::enforceable`] being the strictest boundary the host can
//!   enforce); 10.8's "refusal" row writes the comparison the other way.
//! - [`ChangeRun::unblock`] needs a snapshot that layers exactly the
//!   current candidates of the task's predecessors (10.4), in any order.
//! - The state machine fails closed: a timestamp that is not RFC 3339
//!   expires a lease and is refused by every operation that records a
//!   time, every recorded time is respelled in UTC (10.1), a token handed
//!   to the sandbox is never reissued, and a sandbox that reports a wider
//!   boundary than granted is refused.

pub mod codes;
pub mod enforce;
pub mod git;
pub mod model;
pub mod patch;
pub mod reach;
pub mod scheduler;
mod time;
pub mod validate;

pub use cpl_index::{GitError, GitRepo};
pub use enforce::{
    enforce, module_header, tree_diff, Enforcement, FileChange, FileStatus, ReboundName, TreeDiff,
};
pub use git::{commit_tree, git, update_ref_cas, worktree_add, worktree_remove, write_tree};
pub use model::{
    namespace_covers, Actor, Authority, Baseline, Candidate, CandidateId, ChangeRecord,
    ChangeRunId, ChangeSpec, ChangeState, FencingToken, FileSelector, FrozenInputs, Lease,
    PatchSummary, RequirementSpec, ResolvedNamespace, ResolvedNamespaceOrSelector,
    ResolvedSelector, ReviewRecord, TaskId, TaskRecord, TaskSpec, Timestamp, Transition, Verdict,
    WorkModel,
};
pub use patch::{
    apply_patch, parse_submitted_patch, resolve_patches, validate_patch, ResolvedPatch,
};
pub use scheduler::{
    boundary_text, ChangeRun, CheckOutcome, CheckRunner, Coordinator, CoordinatorError,
    ExecutionBoundary, FsAccess, Grant, NetworkAccess, ProcessAccess, ProveOutcome, RunPolicy,
    Sandbox, SandboxError, ScheduleError, Submission, SubmitOutcome, TaskState, TestOutcome,
};
pub use validate::{validate_work_file, validate_work_file_with_policy, WorkCheckResult};
