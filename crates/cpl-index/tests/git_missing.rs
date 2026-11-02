//! Git that cannot be run (`spec/toolchain.md` 6.3, 1.8): every operation
//! is `GitError::NotInstalled`, and `current_revision` reports the
//! unversioned revision silently. This binary empties `PATH` for its one
//! test, so it holds nothing else.

mod common;

use common::*;
use cpl_diag::json::RevisionState;
use cpl_index::{
    current_revision, discover_repo, merge_base, resolve_rev, source_digest, tree_at, GitError,
    GitRepo,
};

#[test]
fn without_git_every_operation_is_not_installed() {
    let dir = temp_dir("no-git");
    write_files(
        &dir,
        &[
            ("cpl.toml", "[project]\nname = \"t\"\nedition = \"0.1\"\n"),
            ("src/t.cpl", "module t\n"),
        ],
    );
    let empty_path = temp_dir("no-git-path");
    // Edition 2021: setting the environment is safe, and this test binary
    // has no other test that could read it concurrently.
    std::env::set_var("PATH", &empty_path);
    assert!(matches!(discover_repo(&dir), Err(GitError::NotInstalled)));
    let repo = GitRepo {
        toplevel: dir.clone(),
        project_prefix: String::new(),
    };
    assert!(matches!(
        resolve_rev(&repo, "HEAD"),
        Err(GitError::NotInstalled)
    ));
    assert!(matches!(
        merge_base(&repo, "a", "b"),
        Err(GitError::NotInstalled)
    ));
    assert!(matches!(
        tree_at(&repo, "HEAD"),
        Err(GitError::NotInstalled)
    ));
    // Reading the revision never fails for want of git (Section 6.3).
    let project = load_dir(&dir);
    let revision = current_revision(&project, Some(&repo)).expect("silently unversioned");
    assert_eq!(revision.commit, None);
    assert_eq!(revision.state, RevisionState::Worktree);
    assert_eq!(revision.digest, source_digest(&project));
}
