//! The git wrappers of `spec/toolchain.md` 10.7 against a temporary
//! repository created in the test (`git init -b main`, two commits; the
//! "git wrappers" row of Section 12): `update_ref_cas` success and conflict,
//! an isolated worktree snapshotted into a commit that then advances the
//! branch, and the error mapping. Every test is skipped, with a message,
//! when `git` is not installed.

mod common;

use common::*;
use cpl_work::{
    commit_tree, git, update_ref_cas, worktree_add, worktree_remove, write_tree, GitError, GitRepo,
};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git_available() -> bool {
    let found = Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("skipped: git is not installed");
    }
    found
}

/// Run git in `dir` for the test's own setup and checks.
fn run(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .trim()
        .to_string()
}

/// A repository under the test's temporary directory with a fixed identity
/// and no hooks, signing, or line-ending conversion (in its own config, so
/// the wrappers, which use the configuration unchanged, see it), and two
/// commits on `main`.
struct TestRepo {
    root: PathBuf,
    repo: GitRepo,
    first: String,
    second: String,
}

impl TestRepo {
    fn new(name: &str) -> TestRepo {
        let root = temp_dir(name);
        let dir = root.join("repo");
        std::fs::create_dir_all(&dir).expect("create the repository directory");
        run(&dir, &["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "cpl test"),
            ("user.email", "cpl@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            run(&dir, &["config", key, value]);
        }
        std::fs::write(
            dir.join("cpl.toml"),
            "[project]\nname = \"t\"\nedition = \"0.1\"\n",
        )
        .expect("write");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "first"]);
        let first = run(&dir, &["rev-parse", "HEAD"]);
        std::fs::create_dir_all(dir.join("src/m")).expect("create src");
        std::fs::write(dir.join("src/m/m.cpl"), "module m\n").expect("write");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "second"]);
        let second = run(&dir, &["rev-parse", "HEAD"]);
        TestRepo {
            repo: GitRepo {
                toplevel: dir,
                project_prefix: String::new(),
            },
            root,
            first,
            second,
        }
    }

    fn head_of(&self, reference: &str) -> String {
        run(&self.repo.toplevel, &["rev-parse", reference])
    }
}

#[test]
fn update_ref_cas_moves_a_ref_only_from_the_expected_value() {
    if !git_available() {
        return;
    }
    let t = TestRepo::new("cas");
    // Success: main is at `second`, as expected.
    update_ref_cas(&t.repo, "refs/heads/main", &t.first, &t.second).expect("swapped");
    assert_eq!(t.head_of("refs/heads/main"), t.first);
    // Conflict: main is no longer at `second`; nothing changes.
    let error = update_ref_cas(&t.repo, "refs/heads/main", &t.second, &t.second)
        .expect_err("the ref moved");
    match &error {
        GitError::Command {
            args,
            status,
            stderr,
        } => {
            assert_eq!(args[0], "update-ref");
            assert_eq!(*status, 128);
            assert!(stderr.contains("but expected"), "{stderr}");
        }
        other => panic!("expected GitError::Command, found {other:?}"),
    }
    assert_eq!(t.head_of("refs/heads/main"), t.first);
    // An empty old value creates a ref only if it does not exist yet.
    update_ref_cas(&t.repo, "refs/cpl/candidates/cand_1", &t.second, "").expect("created");
    assert_eq!(t.head_of("refs/cpl/candidates/cand_1"), t.second);
    assert!(update_ref_cas(&t.repo, "refs/cpl/candidates/cand_1", &t.first, "").is_err());
    assert_eq!(t.head_of("refs/cpl/candidates/cand_1"), t.second);
}

#[test]
fn update_ref_cas_refuses_option_like_arguments() {
    if !git_available() {
        return;
    }
    let t = TestRepo::new("options");
    for (reference, new, old) in [
        ("-d", t.first.as_str(), t.second.as_str()),
        ("refs/heads/main", "--stdin", t.second.as_str()),
        ("refs/heads/main", t.first.as_str(), "-z"),
        ("", t.first.as_str(), t.second.as_str()),
    ] {
        let error = update_ref_cas(&t.repo, reference, new, old).expect_err("refused");
        assert!(matches!(error, GitError::UnknownRevision(_)), "{error:?}");
    }
    assert_eq!(t.head_of("refs/heads/main"), t.second);
}

#[test]
fn a_worktree_snapshot_becomes_a_commit_that_advances_the_branch() {
    if !git_available() {
        return;
    }
    let t = TestRepo::new("worktree");
    let worktree = t.root.join("wt-implementation");
    worktree_add(&t.repo, &worktree, &t.first).expect("worktree added");
    assert!(worktree.join("cpl.toml").is_file());
    assert!(
        !worktree.join("src/m/m.cpl").exists(),
        "the checkout is of `first`"
    );
    assert_eq!(run(&worktree, &["rev-parse", "HEAD"]), t.first);
    assert_eq!(
        run(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "HEAD",
        "detached"
    );

    // A worker's edits, snapshotted.
    std::fs::create_dir_all(worktree.join("src/m")).expect("create src");
    std::fs::write(
        worktree.join("src/m/m.cpl"),
        "module m\n\nfn f() -> Int {\n    1\n}\n",
    )
    .expect("write");
    let tree = write_tree(&t.repo, &worktree).expect("tree written");
    assert_eq!(tree.len(), 40);
    assert_eq!(run(&t.repo.toplevel, &["cat-file", "-t", &tree]), "tree");
    assert_eq!(
        run(&t.repo.toplevel, &["show", &format!("{tree}:src/m/m.cpl")]),
        "module m\n\nfn f() -> Int {\n    1\n}"
    );

    // A commit object on top of `second` (no ref moves) ...
    let message = "Integrate pagination_update\n\nCPL-Change: chg_mzxw6ytbmzxw6ytb";
    let commit = commit_tree(&t.repo, &tree, &[&t.second], message).expect("committed");
    assert_eq!(
        run(&t.repo.toplevel, &["rev-parse", &format!("{commit}^")]),
        t.second
    );
    assert_eq!(
        run(&t.repo.toplevel, &["log", "-1", "--format=%B", &commit]),
        message
    );
    assert_eq!(t.head_of("refs/heads/main"), t.second);
    // ... that the compare-and-swap then makes the branch head.
    update_ref_cas(&t.repo, "refs/heads/main", &commit, &t.second).expect("advanced");
    assert_eq!(t.head_of("refs/heads/main"), commit);

    // A merge-shaped commit and a root commit.
    let merge =
        commit_tree(&t.repo, &tree, &[&t.first, &t.second], "two parents").expect("committed");
    assert_eq!(
        run(
            &t.repo.toplevel,
            &["rev-list", "--parents", "-n", "1", &merge]
        )
        .split(' ')
        .count(),
        3
    );
    let root = commit_tree(&t.repo, &tree, &[], "no parent").expect("committed");
    assert_eq!(
        run(
            &t.repo.toplevel,
            &["rev-list", "--parents", "-n", "1", &root]
        ),
        root
    );

    worktree_remove(&t.repo, &worktree).expect("worktree removed");
    assert!(!worktree.exists());
    assert!(!run(&t.repo.toplevel, &["worktree", "list"]).contains("wt-implementation"));
}

#[test]
fn relative_worktree_paths_are_relative_to_the_top_level() {
    if !git_available() {
        return;
    }
    let t = TestRepo::new("relative");
    worktree_add(&t.repo, Path::new("../wt-relative"), &t.second).expect("added");
    let absolute = t.root.join("wt-relative");
    assert!(absolute.join("src/m/m.cpl").is_file());
    let tree = write_tree(&t.repo, Path::new("../wt-relative")).expect("tree");
    assert_eq!(
        tree,
        run(
            &t.repo.toplevel,
            &["rev-parse", &format!("{}^{{tree}}", t.second)]
        )
    );
    worktree_remove(&t.repo, Path::new("../wt-relative")).expect("removed");
    assert!(!absolute.exists());
}

#[test]
fn git_maps_failures_to_git_errors() {
    if !git_available() {
        return;
    }
    let t = TestRepo::new("errors");
    let output = git(&t.repo, &["rev-parse", "HEAD"]).expect("runs");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), t.second);
    match git(
        &t.repo,
        &["cat-file", "-t", "0000000000000000000000000000000000000000"],
    ) {
        Err(GitError::Command { status, args, .. }) => {
            assert_ne!(status, 0);
            assert_eq!(
                args,
                ["cat-file", "-t", "0000000000000000000000000000000000000000"]
            );
        }
        other => panic!("expected GitError::Command, found {other:?}"),
    }
    let outside = GitRepo {
        toplevel: temp_dir("not-a-repository"),
        project_prefix: String::new(),
    };
    assert!(matches!(
        git(&outside, &["status"]),
        Err(GitError::NotARepository(_))
    ));
    assert!(matches!(
        worktree_add(&t.repo, &t.root.join("wt"), "-b"),
        Err(GitError::UnknownRevision(_))
    ));
    assert!(matches!(
        commit_tree(&t.repo, "--help", &[], "m"),
        Err(GitError::UnknownRevision(_))
    ));
    assert!(worktree_remove(&t.repo, &t.root.join("no-such-worktree")).is_err());
}
