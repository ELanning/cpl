//! The diagnostic codes the command line raises (`spec/toolchain.md`
//! Appendix B, producer `cli`), the codes of other producers it reports on
//! their behalf, and small constructors.
//!
//! The `index_*` and `git_*` codes are `cpl-index`'s (Appendix B, producer
//! `index`), `git_command_failed` included; the run refusals and
//! `empty_test_selection` are `cpl-sema`'s, the fatal manifest and lock
//! codes `cpl-project`'s.

use cpl_diag::{Category, DiagCode, Diagnostic};
use serde_json::json;

pub use cpl_eval::run::USAGE_BAD_ARGUMENT;
pub use cpl_index::codes::GIT_NO_PROJECT_AT_REVISION;
pub use cpl_project::codes::{PROJECT_LOCK_INVALID, PROJECT_MANIFEST_INVALID};
pub use cpl_sema::codes::{
    CANNOT_RUN_EFFECTFUL, CANNOT_RUN_GENERIC, EMPTY_TEST_SELECTION, INTERNAL_ERROR,
};

/// No `cpl.toml` found (Section 7.1).
pub const USAGE_NO_PROJECT: DiagCode = DiagCode("usage_no_project");
/// `cpl run`'s target is not a function, or is a generator (T61).
pub const USAGE_NOT_A_FUNCTION: DiagCode = DiagCode("usage_not_a_function");
/// A reserved command (Milestone 6), or one this build does not have yet.
pub const USAGE_NOT_IMPLEMENTED: DiagCode = DiagCode("usage_not_implemented");

/// The lints `--lint` may enable (Section 7.2): `id_missing` (Section 4.3).
pub const LINTS: &[&str] = &["id_missing"];

/// A usage error (category `usage`, severity error).
pub fn usage(code: DiagCode, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, Category::Usage, message)
}

/// `usage_bad_argument`.
pub fn bad_argument(message: impl Into<String>) -> Diagnostic {
    usage(USAGE_BAD_ARGUMENT, message)
}

/// `internal_error` with the payload of toolchain 1.7.3 (`kind`,
/// `detail`): an unrecoverable condition, exit 2.
pub fn internal(kind: &str, detail: impl Into<String>) -> Diagnostic {
    let detail = detail.into();
    Diagnostic::error(INTERNAL_ERROR, Category::Internal, detail.clone())
        .with_data("kind", json!(kind))
        .with_data("detail", json!(detail))
}
