//! The git commands of the work coordinator (`spec/toolchain.md` 10.7):
//! a general runner ([`git`]), the compare-and-swap ref update that makes
//! integration atomic ([`update_ref_cas`], Section 10.6), isolated
//! worktrees ([`worktree_add`], [`worktree_remove`]), and the snapshot and
//! commit objects of candidates and integrations ([`write_tree`],
//! [`commit_tree`]). The repository queries of the first six rows of the
//! 10.7 table are `cpl-index`'s ([`cpl_index::revision`]).
//!
//! Every invocation is `git -C <dir> ...` with `LC_ALL=C` and
//! `GIT_TERMINAL_PROMPT=0` (Section 9.3), plus `GIT_LITERAL_PATHSPECS=1` as
//! in `cpl-index`, with stdin closed and stdout and stderr captured; the
//! user's git configuration is used unchanged (Section 1.3). A `git` that
//! cannot be started is [`GitError::NotInstalled`], a directory outside any
//! repository [`GitError::NotARepository`], and any other non-zero status
//! [`GitError::Command`] (`git_command_failed`). A revision, ref, or tree
//! argument that is empty or starts with `-`, which git would read as an
//! option, is [`GitError::UnknownRevision`] without running git. A
//! relative worktree path is relative to the repository's top level.

pub use cpl_index::{GitError, GitRepo};

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Run `git -C <toplevel> <args>`; `Ok` only for exit status 0.
pub fn git(repo: &GitRepo, args: &[&str]) -> Result<Output, GitError> {
    run(&repo.toplevel, args.iter().map(OsStr::new))
}

/// `git update-ref <reference> <new> <old>`: move `reference` to `new` only
/// if it still names `old` (an empty `old` requires that the ref does not
/// exist yet). Git refuses the update when the ref no longer equals `old`,
/// which is [`GitError::Command`] (git's status 128), and the ref is left
/// as it was: the compare-and-swap of Section 10.6.
pub fn update_ref_cas(
    repo: &GitRepo,
    reference: &str,
    new: &str,
    old: &str,
) -> Result<(), GitError> {
    revision_argument(reference)?;
    revision_argument(new)?;
    if old.starts_with('-') {
        return Err(GitError::UnknownRevision(old.to_string()));
    }
    git(repo, &["update-ref", reference, new, old]).map(|_| ())
}

/// `git worktree add --detach <path> <commit>`: an isolated checkout of
/// `commit` with a detached `HEAD`.
pub fn worktree_add(repo: &GitRepo, path: &Path, commit: &str) -> Result<(), GitError> {
    revision_argument(commit)?;
    let path = within(repo, path);
    run(
        &repo.toplevel,
        [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--detach"),
            OsStr::new("--"),
            path.as_os_str(),
            OsStr::new(commit),
        ],
    )
    .map(|_| ())
}

/// `git worktree remove --force <path>`: the worktree's directory and its
/// administrative files, local changes included.
pub fn worktree_remove(repo: &GitRepo, path: &Path) -> Result<(), GitError> {
    let path = within(repo, path);
    run(
        &repo.toplevel,
        [
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--force"),
            OsStr::new("--"),
            path.as_os_str(),
        ],
    )
    .map(|_| ())
}

/// `git commit-tree <tree> -p <parent>... -m <message>`: a commit object
/// (no ref moves); returns its id. The author and committer come from the
/// user's configuration.
pub fn commit_tree(
    repo: &GitRepo,
    tree: &str,
    parents: &[&str],
    message: &str,
) -> Result<String, GitError> {
    revision_argument(tree)?;
    let mut args = vec!["commit-tree", tree];
    for parent in parents {
        revision_argument(parent)?;
        args.push("-p");
        args.push(parent);
    }
    args.push("-m");
    args.push(message);
    let output = git(repo, &args)?;
    first_line(&output, &args)
}

/// `git -C <worktree> add -A` then `git -C <worktree> write-tree`: the tree
/// object of everything in the worktree (its `.gitignore` rules apply);
/// returns its id. The worktree's index is updated as a side effect.
pub fn write_tree(repo: &GitRepo, worktree: &Path) -> Result<String, GitError> {
    let dir = within(repo, worktree);
    run(&dir, ["add", "-A"])?;
    let output = run(&dir, ["write-tree"])?;
    first_line(&output, &["write-tree"])
}

/// A path as git sees it from `-C <toplevel>`, made absolute so that it
/// also holds for commands run in another directory.
fn within(repo: &GitRepo, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.toplevel.join(path)
    }
}

/// Refuse an argument git would parse as an option, or an empty one.
fn revision_argument(arg: &str) -> Result<(), GitError> {
    if arg.is_empty() || arg.starts_with('-') {
        Err(GitError::UnknownRevision(arg.to_string()))
    } else {
        Ok(())
    }
}

fn run<I, S>(dir: &Path, args: I) -> Result<Output, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args
        .into_iter()
        .map(|a| a.as_ref().to_os_string())
        .collect();
    let output = Command::new("git")
        // a repository's own config may not run a program on a read (`core.fsmonitor`)
        .args(["-c", "core.fsmonitor=false"])
        .arg("-C")
        .arg(dir)
        .args(&args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .output()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => GitError::NotInstalled,
            _ => GitError::Io(e),
        })?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr)
        .trim_end()
        .to_string();
    if stderr.contains("not a git repository") {
        return Err(GitError::NotARepository(dir.display().to_string()));
    }
    Err(GitError::Command {
        args: args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        status: output.status.code().unwrap_or(-1),
        stderr,
    })
}

/// The object id git printed, or a [`GitError::Command`] when it printed none.
fn first_line(output: &Output, args: &[&str]) -> Result<String, GitError> {
    let text = String::from_utf8_lossy(&output.stdout);
    match text.lines().next().map(str::trim) {
        Some(id) if !id.is_empty() => Ok(id.to_string()),
        _ => Err(GitError::Command {
            args: args.iter().map(|a| a.to_string()).collect(),
            status: 0,
            stderr: "git printed no object id".to_string(),
        }),
    }
}
