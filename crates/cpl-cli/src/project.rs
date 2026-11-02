//! Locating, loading, and versioning the project (`spec/toolchain.md` 7.1,
//! 6.3, 9.3): project discovery and `usage_no_project`, the `LoadError` and
//! `GitError` mappings, the repository and the revision every envelope
//! reports, and loading a commit's tree for the commands that read
//! revisions.
//!
//! | Failure | Code | Exit |
//! | - | - | - |
//! | no `cpl.toml` (`--project DIR` without one, or none above the current directory) | `usage_no_project` | 2 |
//! | `LoadError::NoManifest` of a commit's tree (`tree_at` of a commit without `cpl.toml` at the project prefix) | `git_no_project_at_revision` (`revision`, `commit`) | 2 |
//! | `LoadError::Manifest` | `project_manifest_invalid` (`path`, `line`; at a revision also `revision`, `commit`) | 2 |
//! | `LoadError::Lock` | `project_lock_invalid` (`path`, `line`; at a revision also `revision`, `commit`) | 2 |
//! | `LoadError::Io`, `LoadError::Std` | `internal_error` (`kind` `io` or `std_unavailable`) | 2 |
//! | a `GitError` | `cpl_index::git_error_diagnostic`: `git_not_a_repository`, `git_unknown_revision`, or `git_command_failed` | 2 |
//! | a command that reads revisions (`cpl diff`, `inspect --include changes`) without a repository | `git_not_a_repository` | 2 |
//! | ... or without a runnable `git` | `git_command_failed` | 2 |
//!
//! The revision of a working tree (Section 6.3) is reported silently as
//! `{ commit: null, state: worktree }` without a repository or without a
//! runnable `git`; any other git failure stops the command
//! (`git_command_failed`, exit 2, Section 7.1).
//!
//! Reading of 1.4.2 (`revision` is `None` "when no project is involved"):
//! a command that a parsed command line runs reports the revision of the
//! project it runs in whenever one is located and loads, whichever check
//! stopped it: a refusal found before the project is loaded (an `--arg`
//! value, a test selector, `--lint` or `--include`, [`Session::refuse`])
//! names the revision exactly as one found after loading does, so the
//! value does not depend on the order of the checks (design line 245:
//! every response identifies the source revision). It is `None` only when
//! no project is involved: none is located, the located one does not load,
//! the form reads none (`fmt --stdin`, loose files, the reserved `work`
//! commands), or no command ran (clap refused the command line); the report
//! of a panic (`internal_error`, kind `panic`) has none either, since the
//! crashed command's state is gone.

use crate::codes;
use crate::output::Failure;
use cpl_diag::json::{Revision, Status};
use cpl_diag::{Category, Diagnostic, FileOrigin};
use cpl_index::{git_error_diagnostic, GitError, GitRepo};
use cpl_project::{
    find_project_root, load_project, load_project_from_tree, LoadError, LoadOptions, Project,
    StdSource,
};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The environment variable that overrides the embedded standard library
/// (Section 3.6; library development only).
pub const STD_DIR_VAR: &str = "CPL_STD_DIR";

/// Where a project was loaded from, for the `LoadError` mapping.
#[derive(Copy, Clone, Debug)]
pub enum LoadContext<'a> {
    /// A directory on disk (the working tree).
    WorkingTree,
    /// The tree of a commit (`cpl_index::tree_at`): `name` is the revision
    /// as the command line wrote it (`main`, `HEAD~1`), or the commit id
    /// for a revision that has no name of its own (the merge base of `cpl
    /// diff A...B`).
    Revision { name: &'a str, commit: &'a str },
}

/// The command line's view of the file system: the current directory and
/// `--project`.
#[derive(Clone, Debug)]
pub struct Session {
    pub cwd: PathBuf,
    /// `--project DIR`.
    pub project_flag: Option<PathBuf>,
}

impl Session {
    pub fn new(project_flag: Option<PathBuf>) -> Session {
        Session {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            project_flag,
        }
    }

    /// The project root (Section 7.1): `--project DIR`, which must contain
    /// `cpl.toml`, else the nearest ancestor of the current directory that
    /// contains it; otherwise `usage_no_project` (exit 2).
    pub fn locate(&self) -> Result<PathBuf, Failure> {
        match self.locate_optional()? {
            Some(root) => Ok(root),
            None => Err(Failure::error(no_project(&format!(
                "no cpl.toml found in {} or any parent directory",
                self.cwd.display()
            )))),
        }
    }

    /// [`Session::locate`] for the commands that also work without a
    /// project (`cpl fmt` and `cpl id assign` with explicit paths): `None`
    /// when no ancestor of the current directory has `cpl.toml`. An
    /// explicit `--project` without one is still `usage_no_project`.
    pub fn locate_optional(&self) -> Result<Option<PathBuf>, Failure> {
        if let Some(dir) = &self.project_flag {
            let dir = absolute(&self.cwd, dir);
            if dir.join("cpl.toml").is_file() {
                return Ok(Some(dir));
            }
            return Err(Failure::error(no_project(&format!(
                "--project {}: no cpl.toml in that directory",
                dir.display()
            ))));
        }
        Ok(find_project_root(&self.cwd))
    }

    /// Load the project at `root` (Section 7.1), mapping a [`LoadError`]
    /// to its diagnostic.
    pub fn load(&self, root: &Path, include_work_files: bool) -> Result<Project, Failure> {
        load_project(root, &load_options(include_work_files))
            .map_err(|e| Failure::error(load_error(&e, LoadContext::WorkingTree)))
    }

    /// The revision of the project this command runs in: the located
    /// project's, when one is located and loads; `None` otherwise, silently
    /// (module docs).
    pub fn revision(&self) -> Option<Revision> {
        let root = self.locate_optional().ok()??;
        let project = self.load(&root, false).ok()?;
        working_revision(&project, &root)
            .ok()
            .map(|(_, revision)| revision)
    }

    /// A usage error found before the command loads its project (a bad
    /// argument): exit 2, with the located project's revision when it loads
    /// ([`Session::revision`]).
    pub fn refuse(&self, diagnostic: Diagnostic) -> Failure {
        Failure::new(Status::Error, self.revision(), &[diagnostic], None)
    }

    /// Locate and load in one step, with the repository and the revision of
    /// the loaded tree.
    pub fn open(&self, include_work_files: bool) -> Result<Opened, Failure> {
        let root = self.locate()?;
        let project = self.load(&root, include_work_files)?;
        let repo = discover(&root)?;
        let revision = cpl_index::current_revision(&project, repo.repo())
            .map_err(|e| Failure::error(git_error_diagnostic(&e)))?;
        Ok(Opened {
            root,
            project,
            repo,
            revision,
        })
    }
}

/// A loaded working tree, its repository, and its revision.
pub struct Opened {
    pub root: PathBuf,
    pub project: Project,
    /// What `git` says about the repository containing the project.
    pub repo: RepoState,
    pub revision: Revision,
}

/// What `git` says about the repository of a project root (Section 6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoState {
    /// The repository containing the project root.
    Found(GitRepo),
    /// The project root is in no git work tree.
    NoRepository,
    /// `git` cannot be run (not installed, or not on `PATH`).
    GitMissing,
}

impl RepoState {
    /// The repository, when there is one.
    pub fn repo(&self) -> Option<&GitRepo> {
        match self {
            RepoState::Found(repo) => Some(repo),
            RepoState::NoRepository | RepoState::GitMissing => None,
        }
    }

    /// The repository a command that reads revisions needs (`cpl diff`,
    /// `inspect --include changes`): without one, `git_not_a_repository`;
    /// without a runnable `git`, `git_command_failed` (Appendix B: "any
    /// other git failure"; `git_unavailable` is `cpl work check`'s alone).
    /// Both exit 2. `what` names what needs the repository.
    pub fn required(&self, what: &str) -> Result<&GitRepo, Diagnostic> {
        match self {
            RepoState::Found(repo) => Ok(repo),
            RepoState::NoRepository => Err(Diagnostic::error(
                cpl_index::codes::GIT_NOT_A_REPOSITORY,
                Category::Git,
                format!(
                    "{what} reads revisions from git, but the project is not in a git repository"
                ),
            )
            .with_data("path", json!("."))),
            RepoState::GitMissing => Err(git_error_diagnostic(&GitError::NotInstalled)),
        }
    }
}

/// The repository containing `root` (Section 0): `discover_repo`, with a
/// missing `git` reported as [`RepoState::GitMissing`] rather than as a
/// failure; any other git failure is `git_command_failed` (exit 2).
pub fn discover(root: &Path) -> Result<RepoState, Failure> {
    match cpl_index::discover_repo(root) {
        Ok(Some(repo)) => Ok(RepoState::Found(repo)),
        Ok(None) => Ok(RepoState::NoRepository),
        Err(GitError::NotInstalled) => Ok(RepoState::GitMissing),
        Err(e) => Err(Failure::error(git_error_diagnostic(&e))),
    }
}

/// The [`LoadOptions`] of every command: the embedded standard library, or
/// the `CPL_STD_DIR` override when that variable is set (Section 3.6).
pub fn load_options(include_work_files: bool) -> LoadOptions {
    let std = match std::env::var_os(STD_DIR_VAR) {
        Some(dir) if !dir.is_empty() => StdSource::Dir(PathBuf::from(dir)),
        _ => StdSource::Embedded,
    };
    LoadOptions {
        std,
        include_work_files,
    }
}

/// `path` against `base` when relative, without resolving links.
pub fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// A path with its directory resolved and its file name kept, so that a
/// link to a file is named by its own name (as T54 reads the source set).
pub fn real_path(path: &Path) -> PathBuf {
    match (
        path.parent().and_then(|d| d.canonicalize().ok()),
        path.file_name(),
    ) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
    }
}

fn no_project(message: &str) -> Diagnostic {
    codes::usage(codes::USAGE_NO_PROJECT, message)
}

/// The repository of a loaded working tree and the tree's revision
/// (Section 6.3). Without a repository, or when `git` cannot run, the
/// revision is `{ commit: null, state: worktree }`, silently; any other
/// git failure is `git_command_failed`.
pub fn working_revision(
    project: &Project,
    root: &Path,
) -> Result<(Option<GitRepo>, Revision), Failure> {
    let repo = discover(root)?.repo().cloned();
    let revision = cpl_index::current_revision(project, repo.as_ref())
        .map_err(|e| Failure::error(git_error_diagnostic(&e)))?;
    Ok((repo, revision))
}

/// Load the project tree of `commit` (Section 9.3) for the commands that
/// read revisions (`cpl diff`): `tree_at`, then `load_project_from_tree`
/// with `FileOrigin::Revision(commit)`. `name` is the revision as written
/// (the payload's `revision`). A commit without `cpl.toml` at the project
/// prefix yields an empty tree, which loads as `LoadError::NoManifest`:
/// `git_no_project_at_revision`.
pub fn load_revision(
    repo: &GitRepo,
    name: &str,
    commit: &str,
    include_work_files: bool,
) -> Result<Project, Diagnostic> {
    let tree = cpl_index::tree_at(repo, commit).map_err(|e| git_error_diagnostic(&e))?;
    load_project_from_tree(
        &tree,
        FileOrigin::Revision(commit.to_string()),
        &load_options(include_work_files),
    )
    .map_err(|e| load_error(&e, LoadContext::Revision { name, commit }))
}

/// The diagnostic of a project that could not be loaded (exit 2). Every one
/// is span-less: `cpl.toml` and `cpl.lock` are not source files (semantics
/// S63 (6)), so `path` and, when known, `line` are payload fields; a tree
/// loaded from a commit adds `revision` and `commit` (the keys
/// `cpl_index::facet_map_at` gives the same failures, Appendix B).
pub fn load_error(error: &LoadError, context: LoadContext<'_>) -> Diagnostic {
    let at = match context {
        LoadContext::WorkingTree => None,
        LoadContext::Revision { name, commit } => Some((name, commit)),
    };
    let d = match error {
        LoadError::NoManifest(dir) => match at {
            None => no_project(&format!("no cpl.toml found at {}", dir.display())),
            Some((name, commit)) => Diagnostic::error(
                codes::GIT_NO_PROJECT_AT_REVISION,
                Category::Git,
                format!(
                    "{} has no cpl.toml at the project's path",
                    revision_phrase(name, commit)
                ),
            ),
        },
        LoadError::Manifest {
            path,
            message,
            line,
        } => file_error(codes::PROJECT_MANIFEST_INVALID, path, message, *line, at),
        LoadError::Lock {
            path,
            message,
            line,
        } => file_error(codes::PROJECT_LOCK_INVALID, path, message, *line, at),
        LoadError::Io { path, source } => {
            codes::internal("io", format!("{path}: {source}")).with_data("path", json!(path))
        }
        LoadError::Std(message) => codes::internal(
            "std_unavailable",
            format!("standard library unavailable: {message}"),
        ),
    };
    match at {
        Some((name, commit)) => d
            .with_data("revision", json!(name))
            .with_data("commit", json!(commit)),
        None => d,
    }
}

/// `revision `main` (<commit>)`, or `commit <commit>` when the revision has
/// no name but its id.
fn revision_phrase(name: &str, commit: &str) -> String {
    if name == commit {
        format!("commit {commit}")
    } else {
        format!("revision `{name}` ({commit})")
    }
}

fn file_error(
    code: cpl_diag::DiagCode,
    path: &str,
    message: &str,
    line: Option<u32>,
    at: Option<(&str, &str)>,
) -> Diagnostic {
    let message = match at {
        None => format!("{path}: {message}"),
        Some((name, commit)) => format!("{path} at {}: {message}", revision_phrase(name, commit)),
    };
    let mut d = Diagnostic::error(code, Category::Project, message).with_data("path", json!(path));
    if let Some(line) = line {
        d = d.with_data("line", json!(line));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_without_a_manifest_is_no_project_at_that_revision() {
        let error = load_project_from_tree(
            &cpl_project::VirtualTree::default(),
            FileOrigin::Revision("ab".repeat(20)),
            &LoadOptions::default(),
        )
        .expect_err("an empty tree has no cpl.toml");
        assert!(matches!(error, LoadError::NoManifest(_)));
        let commit = "ab".repeat(20);
        let d = load_error(
            &error,
            LoadContext::Revision {
                name: "main",
                commit: &commit,
            },
        );
        assert_eq!(d.code, codes::GIT_NO_PROJECT_AT_REVISION);
        assert_eq!(d.category, Category::Git);
        assert!(d.is_error());
        assert_eq!(d.data["commit"], json!(commit));
        assert_eq!(d.data["revision"], json!("main"));
        assert_eq!(
            d.message,
            format!("revision `main` ({commit}) has no cpl.toml at the project's path")
        );
        let d = load_error(
            &error,
            LoadContext::Revision {
                name: &commit,
                commit: &commit,
            },
        );
        assert_eq!(
            d.message,
            format!("commit {commit} has no cpl.toml at the project's path")
        );
        let d = load_error(&error, LoadContext::WorkingTree);
        assert_eq!(d.code, codes::USAGE_NO_PROJECT);
        assert_eq!(d.category, Category::Usage);
        assert!(!d.data.contains_key("commit"));
    }

    #[test]
    fn manifest_and_lock_errors_carry_path_and_line() {
        let d = load_error(
            &LoadError::Manifest {
                path: "cpl.toml".into(),
                message: "missing key `name`".into(),
                line: Some(2),
            },
            LoadContext::WorkingTree,
        );
        assert_eq!(d.code, codes::PROJECT_MANIFEST_INVALID);
        assert_eq!(d.category, Category::Project);
        assert_eq!(d.data["path"], json!("cpl.toml"));
        assert_eq!(d.data["line"], json!(2));
        assert!(d.span.is_none());
        assert!(!d.data.contains_key("revision"));
        let d = load_error(
            &LoadError::Lock {
                path: "cpl.lock".into(),
                message: "bad".into(),
                line: None,
            },
            LoadContext::WorkingTree,
        );
        assert_eq!(d.code, codes::PROJECT_LOCK_INVALID);
        assert!(!d.data.contains_key("line"));
        // At a revision, the revision is named.
        let d = load_error(
            &LoadError::Manifest {
                path: "cpl.toml".into(),
                message: "not TOML".into(),
                line: Some(1),
            },
            LoadContext::Revision {
                name: "HEAD~1",
                commit: "c0ffee",
            },
        );
        assert_eq!(d.code, codes::PROJECT_MANIFEST_INVALID);
        assert_eq!(d.data["revision"], json!("HEAD~1"));
        assert_eq!(d.data["commit"], json!("c0ffee"));
        assert_eq!(
            d.message,
            "cpl.toml at revision `HEAD~1` (c0ffee): not TOML"
        );
    }

    #[test]
    fn commands_that_read_revisions_need_a_repository_and_git() {
        let d = RepoState::NoRepository
            .required("`cpl diff`")
            .expect_err("no repository");
        assert_eq!(d.code, cpl_index::codes::GIT_NOT_A_REPOSITORY);
        assert_eq!(d.category, Category::Git);
        assert_eq!(d.data["path"], json!("."));
        let d = RepoState::GitMissing
            .required("`cpl diff`")
            .expect_err("no git");
        assert_eq!(d.code, cpl_index::codes::GIT_COMMAND_FAILED);
        let repo = GitRepo {
            toplevel: PathBuf::from("/r"),
            project_prefix: String::new(),
        };
        let found = RepoState::Found(repo.clone());
        assert_eq!(found.required("x").ok(), Some(&repo));
        assert_eq!(found.repo(), Some(&repo));
        assert_eq!(RepoState::GitMissing.repo(), None);
    }
}
