//! The diagnostic codes this crate produces (`spec/toolchain.md` Appendix
//! B): every `work_*` code (category `work`), every `patch_*` code (category
//! `patch`), the two `git` codes the work commands report, and the two
//! codes of other categories that enforcement and patch application raise:
//! `duplicate_identity` (`resolve`, semantics S2; 10.5 step 4) and
//! `internal_error` (`internal`).
//!
//! Codes marked (M6) are produced by the scheduler of Milestone 6
//! ([`crate::scheduler`]); `cpl work check` (Section 7.2,
//! [`crate::validate`]) produces the rest of the `work_*` codes and the two
//! `git` codes, together with the `resolve` codes `unresolved_module`,
//! `unresolved_name`, and `private_item` of its `use` lines, which are the
//! checker's (`cpl_sema::codes`).

use cpl_diag::{Category, DiagCode};

/// The category of the `work_*` codes.
pub const WORK: Category = Category::Work;
/// The category of the `patch_*` codes.
pub const PATCH: Category = Category::Patch;

/// `cpl work check` only: no `git` or no repository, so `base`/`target` resolution is skipped (warning).
pub const GIT_UNAVAILABLE: DiagCode = DiagCode("git_unavailable");
/// Any other git failure (exit 2).
pub const GIT_COMMAND_FAILED: DiagCode = DiagCode("git_command_failed");

/// `base` absent from a change.
pub const WORK_MISSING_BASE: DiagCode = DiagCode("work_missing_base");
/// `target` absent from a change.
pub const WORK_MISSING_TARGET: DiagCode = DiagCode("work_missing_target");
/// `workers` < 1.
pub const WORK_BAD_WORKERS: DiagCode = DiagCode("work_bad_workers");
/// `base`/`target` does not resolve.
pub const WORK_UNKNOWN_REF: DiagCode = DiagCode("work_unknown_ref");
/// `target` resolves but is not a branch (grammar D35).
pub const WORK_TARGET_NOT_BRANCH: DiagCode = DiagCode("work_target_not_branch");
/// A `write`/`depend`/`preserve` selector matches no declaration and is not a file selector.
pub const WORK_SELECTOR_EMPTY: DiagCode = DiagCode("work_selector_empty");
/// An `add` namespace under which no module exists yet (warning).
pub const WORK_ADD_NAMESPACE_NEW: DiagCode = DiagCode("work_add_namespace_new");
/// A file selector's spelling also names a declaration, which wins (warning, T42).
pub const WORK_FILE_SELECTOR_SHADOWED: DiagCode = DiagCode("work_file_selector_shadowed");
/// A file selector not listed in `[work.policy] files` (T42).
pub const WORK_FILE_NOT_GRANTABLE: DiagCode = DiagCode("work_file_not_grantable");
/// A declaration named by a task or requirement has no `@id` (warning, T47).
pub const WORK_PROVISIONAL_IDENTITY: DiagCode = DiagCode("work_provisional_identity");
/// A facet that does not apply to the declaration kind (Section 5.1).
pub const WORK_FACET_NOT_APPLICABLE: DiagCode = DiagCode("work_facet_not_applicable");
/// `identity` in a `write`/`add` set (Section 10.3).
pub const WORK_IDENTITY_NOT_WRITABLE: DiagCode = DiagCode("work_identity_not_writable");
/// `after`/`review` names no task.
pub const WORK_UNKNOWN_TASK: DiagCode = DiagCode("work_unknown_task");
/// The `after` graph has a cycle.
pub const WORK_CYCLE: DiagCode = DiagCode("work_cycle");
/// Unordered tasks write the same facet or add to the same namespace.
pub const WORK_OVERLAPPING_WRITES: DiagCode = DiagCode("work_overlapping_writes");
/// A change declares more than `reach::MAX_TASKS_PER_CHANGE` tasks.
pub const WORK_TOO_MANY_TASKS: DiagCode = DiagCode("work_too_many_tasks");
/// `require test(sel)` matches no test.
pub const WORK_TEST_SELECTOR_EMPTY: DiagCode = DiagCode("work_test_selector_empty");
/// `require prove(sel)` names a non-function.
pub const WORK_PROVE_NOT_FUNCTION: DiagCode = DiagCode("work_prove_not_function");
/// A facet both written and preserved by one task.
pub const WORK_PRESERVE_WRITTEN: DiagCode = DiagCode("work_preserve_written");
/// A task with empty `write` and `add` not used by `review` (warning).
pub const WORK_TASK_WITHOUT_AUTHORITY: DiagCode = DiagCode("work_task_without_authority");
/// (M6) Fencing token superseded or lease expired (Section 10.2).
pub const WORK_STALE_SUBMISSION: DiagCode = DiagCode("work_stale_submission");
/// (M6) Two predecessors changed one facet (Section 10.4).
pub const WORK_CONFLICT: DiagCode = DiagCode("work_conflict");
/// (M6) A candidate's `expect base` fails against the target head (Section 10.6).
pub const WORK_STALE_AGAINST_TARGET: DiagCode = DiagCode("work_stale_against_target");
/// (M6) The review's reviewed candidates are not the current ones.
pub const WORK_REVIEW_STALE: DiagCode = DiagCode("work_review_stale");
/// (M6) Integration by an actor without integration authority (Section 10.6); exit 2.
pub const WORK_NOT_AUTHORIZED: DiagCode = DiagCode("work_not_authorized");
/// (M6) The host `Sandbox` cannot enforce a task's granted boundary (Section 10.8).
pub const WORK_BOUNDARY_UNENFORCEABLE: DiagCode = DiagCode("work_boundary_unenforceable");

/// A patch targets a facet other than `body` (D39).
pub const PATCH_FACET_UNSUPPORTED: DiagCode = DiagCode("patch_facet_unsupported");
/// The patch's selector is not in the baseline (Section 5.5 step 2).
pub const PATCH_UNKNOWN_SYMBOL: DiagCode = DiagCode("patch_unknown_symbol");
/// The target is not in the tree being patched (Section 5.5 step 3).
pub const PATCH_TARGET_MISSING: DiagCode = DiagCode("patch_target_missing");
/// The `expect base` hash differs (Section 5.5 step 4).
pub const PATCH_STALE: DiagCode = DiagCode("patch_stale");
/// A change outside the grant (Section 10.5).
pub const PATCH_OUT_OF_SCOPE: DiagCode = DiagCode("patch_out_of_scope");
/// A preserved facet changed (Section 10.5 step 3).
pub const PATCH_PRESERVED_FACET: DiagCode = DiagCode("patch_preserved_facet");
/// An identity added or removed outside the grant (Section 10.5 step 4).
pub const PATCH_IDENTITY_CHANGED: DiagCode = DiagCode("patch_identity_changed");
/// A submitted `.patch` file does not parse; `notes` carry the parse diagnostic.
pub const PATCH_SYNTAX: DiagCode = DiagCode("patch_syntax");

/// A stable id declared twice (`resolve`, semantics S2); 10.5 step 4 rejects a candidate that has one.
pub const DUPLICATE_IDENTITY: DiagCode = DiagCode("duplicate_identity");
/// An unrecoverable condition (`internal`): a caller broke an invariant of an API of this crate.
pub const INTERNAL_ERROR: DiagCode = DiagCode("internal_error");

/// The `work_*` codes, in the order of Appendix B.
pub const WORK_CODES: &[DiagCode] = &[
    WORK_MISSING_BASE,
    WORK_MISSING_TARGET,
    WORK_BAD_WORKERS,
    WORK_UNKNOWN_REF,
    WORK_TARGET_NOT_BRANCH,
    WORK_SELECTOR_EMPTY,
    WORK_ADD_NAMESPACE_NEW,
    WORK_FILE_SELECTOR_SHADOWED,
    WORK_FILE_NOT_GRANTABLE,
    WORK_PROVISIONAL_IDENTITY,
    WORK_FACET_NOT_APPLICABLE,
    WORK_IDENTITY_NOT_WRITABLE,
    WORK_UNKNOWN_TASK,
    WORK_CYCLE,
    WORK_OVERLAPPING_WRITES,
    WORK_TOO_MANY_TASKS,
    WORK_TEST_SELECTOR_EMPTY,
    WORK_PROVE_NOT_FUNCTION,
    WORK_PRESERVE_WRITTEN,
    WORK_TASK_WITHOUT_AUTHORITY,
    WORK_STALE_SUBMISSION,
    WORK_CONFLICT,
    WORK_STALE_AGAINST_TARGET,
    WORK_REVIEW_STALE,
    WORK_NOT_AUTHORIZED,
    WORK_BOUNDARY_UNENFORCEABLE,
];

/// The `patch_*` codes, in the order of Appendix B.
pub const PATCH_CODES: &[DiagCode] = &[
    PATCH_FACET_UNSUPPORTED,
    PATCH_UNKNOWN_SYMBOL,
    PATCH_TARGET_MISSING,
    PATCH_STALE,
    PATCH_OUT_OF_SCOPE,
    PATCH_PRESERVED_FACET,
    PATCH_IDENTITY_CHANGED,
    PATCH_SYNTAX,
];
