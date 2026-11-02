//! Earlier revisions for `cpl inspect --include changes` (`spec/toolchain.md`
//! 6.2, 7.2, 9.3): the facet map of the revision `--since` names, and the
//! diagnostics of the git and loading failures on the way, so that a
//! command reports them with the codes of Appendix B.

use crate::codes;
use crate::facet::{facet_map, FacetMap};
use crate::revision::{resolve_rev, source_digest, tree_at, GitError, GitRepo};
use cpl_diag::json::{Revision, RevisionState};
use cpl_diag::{Category, Diagnostic, FileOrigin};
use cpl_project::{load_project_from_tree, LoadError, LoadOptions};
use serde_json::json;

/// The base `changes` compares with when `--since` is absent (Section 6.2).
pub const DEFAULT_SINCE: &str = "HEAD~1";

/// The diagnostic of a git failure (Appendix B, all errors, exit 2):
/// [`GitError::NotARepository`] is `git_not_a_repository` (`path`);
/// [`GitError::UnknownRevision`] is `git_unknown_revision` (`revision`);
/// every other failure, git that cannot be run included, is
/// `git_command_failed`, a failed command with its `args`, `exit_status`
/// (-1 for a signal), and `stderr` (`status` is the verification and test
/// vocabulary's key, semantics S50, and is not used for it).
pub fn git_error_diagnostic(error: &GitError) -> Diagnostic {
    match error {
        GitError::NotARepository(path) => Diagnostic::error(
            codes::GIT_NOT_A_REPOSITORY,
            codes::GIT,
            format!("not a git repository: {path}"),
        )
        .with_data("path", json!(path)),
        GitError::UnknownRevision(revision) => Diagnostic::error(
            codes::GIT_UNKNOWN_REVISION,
            codes::GIT,
            format!("unknown revision `{revision}`"),
        )
        .with_data("revision", json!(revision)),
        GitError::Command {
            args,
            status,
            stderr,
        } => Diagnostic::error(
            codes::GIT_COMMAND_FAILED,
            codes::GIT,
            format!("git {} failed with status {status}", args.join(" ")),
        )
        .with_data("args", json!(args))
        .with_data("exit_status", json!(status))
        .with_data("stderr", json!(stderr)),
        GitError::NotInstalled | GitError::Io(_) => Diagnostic::error(
            codes::GIT_COMMAND_FAILED,
            codes::GIT,
            format!("git could not be run: {error}"),
        ),
    }
}

/// The facet map of the project at `rev` (Section 9.3 for one revision):
/// `rev` resolved to a commit, the commit's project files materialized
/// with `tree_at`, loaded with the default options, and mapped at the
/// revision `{ commit, state: clean, digest }`. Errors: the git failures
/// of [`git_error_diagnostic`]; a commit without `cpl.toml` at the project
/// prefix is `git_no_project_at_revision` (`revision`, `commit`); a
/// manifest or lock of that commit that does not load is
/// `project_manifest_invalid` or `project_lock_invalid` naming the
/// revision; any other loading failure is `internal_error`.
pub fn facet_map_at(repo: &GitRepo, rev: &str) -> Result<FacetMap, Diagnostic> {
    let commit = resolve_rev(repo, rev).map_err(|e| git_error_diagnostic(&e))?;
    let tree = tree_at(repo, &commit).map_err(|e| git_error_diagnostic(&e))?;
    let project = load_project_from_tree(
        &tree,
        FileOrigin::Revision(commit.clone()),
        &LoadOptions::default(),
    )
    .map_err(|e| load_error_at(rev, &commit, &e))?;
    let revision = Revision {
        commit: Some(commit),
        state: RevisionState::Clean,
        digest: source_digest(&project),
    };
    Ok(facet_map(&project, revision))
}

/// The base of `changes` (Section 6.2): the facet map at `since`
/// ([`facet_map_at`]); without a repository, `git_not_a_repository`.
pub fn changes_base(repo: Option<&GitRepo>, since: &str) -> Result<FacetMap, Diagnostic> {
    match repo {
        Some(repo) => facet_map_at(repo, since),
        None => Err(no_repository()),
    }
}

/// `changes` without a repository to read the earlier revision from.
pub(crate) fn no_repository() -> Diagnostic {
    Diagnostic::error(
        codes::GIT_NOT_A_REPOSITORY,
        codes::GIT,
        "`changes` compares with an earlier revision (`--since`), which needs a git repository",
    )
}

fn load_error_at(rev: &str, commit: &str, error: &LoadError) -> Diagnostic {
    match error {
        LoadError::NoManifest(_) => Diagnostic::error(
            codes::GIT_NO_PROJECT_AT_REVISION,
            codes::GIT,
            format!("revision `{rev}` ({commit}) has no cpl.toml at the project's path"),
        )
        .with_data("revision", json!(rev))
        .with_data("commit", json!(commit)),
        LoadError::Manifest { path, message, .. } => Diagnostic::error(
            cpl_project::codes::PROJECT_MANIFEST_INVALID,
            cpl_project::codes::CATEGORY,
            format!("{path} at revision `{rev}`: {message}"),
        )
        .with_data("revision", json!(rev)),
        LoadError::Lock { path, message, .. } => Diagnostic::error(
            cpl_project::codes::PROJECT_LOCK_INVALID,
            cpl_project::codes::CATEGORY,
            format!("{path} at revision `{rev}`: {message}"),
        )
        .with_data("revision", json!(rev)),
        other => Diagnostic::error(
            codes::INTERNAL_ERROR,
            Category::Internal,
            format!("the project at revision `{rev}` could not be loaded: {other}"),
        )
        .with_data("kind", json!("load_failed"))
        .with_data("detail", json!(other.to_string())),
    }
}
