//! The diagnostic codes this crate produces (`spec/toolchain.md` Appendix
//! B): the three `index` codes of the semantic index, the `git` codes of
//! revisions (Sections 6.3, 9.3), and the codes of other categories that
//! the command helpers of [`crate::query`] and [`crate::evidence`] report
//! on the command's behalf: `usage_not_provable` (`cpl prove` on a target
//! that is no function, Section 7.2), `verification_result` (the
//! diagnostic of `cpl prove`, semantics S36), and `internal_error`.

use cpl_diag::{Category, DiagCode};

/// The category of the `index_*` codes.
pub const INDEX: Category = Category::Index;
/// The category of the `git_*` codes.
pub const GIT: Category = Category::Git;

/// An inspect, run, or prove target that names no declaration (Section 6.1).
pub const INDEX_UNKNOWN_SYMBOL: DiagCode = DiagCode("index_unknown_symbol");
/// A bare name that names several declarations (Section 6.1); `candidates`.
pub const INDEX_AMBIGUOUS_SYMBOL: DiagCode = DiagCode("index_ambiguous_symbol");
/// The project has errors, so callers, dependencies, and tests may be
/// incomplete (warning, Section 7.2 `cpl inspect`).
pub const INDEX_PARTIAL: DiagCode = DiagCode("index_partial");

/// A repository is required (`cpl inspect --include changes` outside one).
pub const GIT_NOT_A_REPOSITORY: DiagCode = DiagCode("git_not_a_repository");
/// `rev-parse --verify` failed, or the name is empty or starts with `-`.
pub const GIT_UNKNOWN_REVISION: DiagCode = DiagCode("git_unknown_revision");
/// No `cpl.toml` at the project prefix in that commit.
pub const GIT_NO_PROJECT_AT_REVISION: DiagCode = DiagCode("git_no_project_at_revision");
/// Any other git failure (exit 2).
pub const GIT_COMMAND_FAILED: DiagCode = DiagCode("git_command_failed");

/// `cpl prove` on a target that is not a function (category `usage`, exit 2).
pub const USAGE_NOT_PROVABLE: DiagCode = DiagCode("usage_not_provable");
/// The verification status of one function (category `contract`, info;
/// semantics S36, 9.4).
pub const VERIFICATION_RESULT: DiagCode = DiagCode("verification_result");
/// An unrecoverable condition (category `internal`, exit 2).
pub const INTERNAL_ERROR: DiagCode = DiagCode("internal_error");
