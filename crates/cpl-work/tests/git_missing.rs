//! Without a runnable `git`, every wrapper of `spec/toolchain.md` 10.7 is
//! `GitError::NotInstalled`, and `cpl work check` skips `base`/`target`
//! resolution with the warning `git_unavailable` (Section 7.2). A test
//! binary of its own, because it empties the process's `PATH` (every test
//! here sets the same empty value).

mod common;

use common::*;
use cpl_work::{git, update_ref_cas, validate_work_file, write_tree, GitError, GitRepo};
use serde_json::json;
use std::path::{Path, PathBuf};

#[test]
fn git_that_cannot_be_started_is_not_installed() {
    std::env::set_var("PATH", "");
    let repo = GitRepo {
        toplevel: PathBuf::from(env!("CARGO_TARGET_TMPDIR")),
        project_prefix: String::new(),
    };
    assert!(matches!(
        git(&repo, &["status"]),
        Err(GitError::NotInstalled)
    ));
    let commit = "0".repeat(40);
    assert!(matches!(
        update_ref_cas(&repo, "refs/heads/main", &commit, ""),
        Err(GitError::NotInstalled)
    ));
    assert!(matches!(
        write_tree(&repo, Path::new("wt")),
        Err(GitError::NotInstalled)
    ));
}

#[test]
fn work_check_without_git_skips_ref_resolution() {
    std::env::set_var("PATH", "");
    let repo = GitRepo {
        toplevel: PathBuf::from(env!("CARGO_TARGET_TMPDIR")),
        project_prefix: String::new(),
    };
    let (project, program) = checked(&reader_tree());
    let result = validate_work_file(
        work_file(&project, "work/pagination.work"),
        &program,
        Some(&repo),
    );
    assert_eq!(
        codes(&result.diagnostics),
        ["work_provisional_identity", "git_unavailable"]
    );
    let git = &result.diagnostics[1];
    assert_eq!(git.data["reason"], json!("not_installed"));
    assert_eq!(
        git.message,
        "git is not installed or not on PATH: the `base` and `target` refs were not resolved"
    );
    assert!(result.model.is_some());
    assert_eq!(result.output.changes[0].base_commit, None);
}
