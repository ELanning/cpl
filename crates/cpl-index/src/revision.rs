//! Revisions and git (`spec/toolchain.md` 6.3, 9.3): the digest of a
//! source set, the revision a loaded tree is, and the git queries the
//! semantic diff and baselines need, through the `git` command line
//! (Section 1.3: the user's installed Git, unchanged).
//!
//! Every invocation is `git -C <dir> ...` with `LC_ALL=C` and
//! `GIT_TERMINAL_PROMPT=0` (Section 9.3), stdout and stderr captured, and
//! with `GIT_LITERAL_PATHSPECS=1`, so that a source directory or project
//! prefix containing `*`, `?`, `[`, or a leading `:` names itself rather
//! than a pattern.
//!
//! Failures map to [`GitError`]: `git` that cannot be started is
//! [`GitError::NotInstalled`]; a directory outside any repository is
//! [`GitError::NotARepository`] (or `Ok(None)` from [`discover_repo`]); a
//! revision name `rev-parse` does not verify as a commit, whatever git's
//! exit status, is [`GitError::UnknownRevision`]; any other non-zero status
//! is [`GitError::Command`].

use crate::source::{source_set, MANIFEST};
use cpl_diag::json::{Digest, Revision, RevisionState};
use cpl_project::{parse_manifest, Project, VirtualTree};
use std::collections::BTreeMap;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The lock's project-relative path: read with the tree, never digested.
const LOCK: &str = "cpl.lock";
/// The directory whose `*.work` and `*.patch` files belong to the source set.
const WORK_DIR: &str = "work";

/// The repository containing a project (Section 0).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GitRepo {
    /// `git rev-parse --show-toplevel`, run from the project root.
    pub toplevel: PathBuf,
    /// The project root relative to `toplevel`: `""` at the top level, else
    /// `"dir/"` with a trailing `/` (`git rev-parse --show-prefix`).
    pub project_prefix: String,
}

/// A failed git operation (Section 1.8).
#[derive(thiserror::Error, Debug)]
pub enum GitError {
    #[error("git is not installed or not on PATH")]
    NotInstalled,
    #[error("not a git repository: {0}")]
    NotARepository(String),
    #[error("unknown revision {0}")]
    UnknownRevision(String),
    #[error("git {args:?} failed with status {status}: {stderr}")]
    Command {
        /// The arguments after `-C <dir>`.
        args: Vec<String>,
        /// The exit status, or -1 when git was killed by a signal.
        status: i32,
        stderr: String,
    },
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Running git
// ---------------------------------------------------------------------------

/// Run `git -C <dir> <args>`, feeding `input` on stdin when given.
fn run_git(dir: &Path, args: &[&str], input: Option<Vec<u8>>) -> Result<Output, GitError> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => GitError::NotInstalled,
        _ => GitError::Io(e),
    })?;
    // Feed stdin from a thread while `wait_with_output` drains stdout and
    // stderr, so that neither side can block on a full pipe.
    let writer = match (input, child.stdin.take()) {
        (Some(bytes), Some(mut stdin)) => Some(std::thread::spawn(move || {
            stdin.write_all(&bytes)
            // `stdin` is dropped here, closing the pipe.
        })),
        _ => None,
    };
    let output = child.wait_with_output()?;
    if let Some(writer) = writer {
        let written = writer
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the thread writing git's input panicked")));
        // A broken pipe means git stopped reading; its status says why.
        if let Err(e) = written {
            if output.status.success() {
                return Err(GitError::Io(e));
            }
        }
    }
    Ok(output)
}

fn command_error(dir: &Path, args: &[&str], output: &Output) -> GitError {
    let stderr = String::from_utf8_lossy(&output.stderr)
        .trim_end()
        .to_string();
    if stderr.contains("not a git repository") {
        return GitError::NotARepository(dir.display().to_string());
    }
    GitError::Command {
        args: args.iter().map(|a| a.to_string()).collect(),
        status: output.status.code().unwrap_or(-1),
        stderr,
    }
}

/// Run git in `dir` and return its stdout, mapping a non-zero status.
fn git_stdout(dir: &Path, args: &[&str], input: Option<Vec<u8>>) -> Result<Vec<u8>, GitError> {
    let output = run_git(dir, args, input)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(command_error(dir, args, &output))
    }
}

/// The text of a one-line answer, without its line terminator.
fn first_line(stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(stdout);
    text.lines().next().unwrap_or("").to_string()
}

/// `git rev-parse --verify -q <spec>`: `Some(object id)`, or `None` when
/// the name does not verify (status 1, `-q` keeps git silent).
fn verify(repo: &GitRepo, spec: &str) -> Result<Option<String>, GitError> {
    let args = ["rev-parse", "--verify", "-q", spec];
    let output = run_git(&repo.toplevel, &args, None)?;
    match output.status.code() {
        Some(0) => Ok(Some(first_line(&output.stdout))),
        Some(1) => Ok(None),
        _ => Err(command_error(&repo.toplevel, &args, &output)),
    }
}

// ---------------------------------------------------------------------------
// The repository
// ---------------------------------------------------------------------------

/// The repository containing `project_root`, found with `git rev-parse
/// --show-toplevel` run from the project root (Section 9.3), with the
/// project's prefix from `--show-prefix`. `Ok(None)` when the root is not
/// inside a git work tree; [`GitError::NotInstalled`] when `git` cannot be
/// run (every command but `cpl work check` then reports `commit: null`
/// silently, Section 6.3).
pub fn discover_repo(project_root: &Path) -> Result<Option<GitRepo>, GitError> {
    let args = ["rev-parse", "--show-toplevel", "--show-prefix"];
    let output = run_git(project_root, &args, None)?;
    if !output.status.success() {
        return match command_error(project_root, &args, &output) {
            GitError::NotARepository(_) => Ok(None),
            other => Err(other),
        };
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let toplevel = lines.next().unwrap_or("").to_string();
    let project_prefix = lines.next().unwrap_or("").to_string();
    if toplevel.is_empty() {
        return Err(GitError::Command {
            args: args.iter().map(|a| a.to_string()).collect(),
            status: 0,
            stderr: "git printed no top-level directory".to_string(),
        });
    }
    Ok(Some(GitRepo {
        toplevel: PathBuf::from(toplevel),
        project_prefix,
    }))
}

/// The digest of a project's source set (Section 6.3): for every file of
/// the source set (`cpl.toml`, the `.cpl`, `.work`, and `.patch` files;
/// never `cpl.lock`, never std) in path order, the line `<path> <hex blake3
/// of bytes>` terminated by LF; the digest is blake3 of the concatenation.
pub fn source_digest(project: &Project) -> Digest {
    let mut hasher = blake3::Hasher::new();
    for (path, digest) in source_set(project) {
        hasher.update(path.as_bytes());
        hasher.update(b" ");
        hasher.update(digest.as_str().as_bytes());
        hasher.update(b"\n");
    }
    Digest(hasher.finalize().to_hex().to_string())
}

/// The revision a loaded working tree is (Section 6.3): `digest` is always
/// [`source_digest`]; `commit` is `HEAD` (40 hex) when there is a repository
/// with commits, and `state` is `clean` when `git status --porcelain
/// --untracked-files=all` lists nothing under the source-set roots
/// (`cpl.toml`, each `[build] sources` directory, `work/`, prefixed with
/// [`GitRepo::project_prefix`]), else `worktree`. Without a repository, in a
/// repository without commits, or when `git` cannot be run, the revision is
/// `{ commit: null, state: worktree }` and no error is reported.
pub fn current_revision(project: &Project, repo: Option<&GitRepo>) -> Result<Revision, GitError> {
    let digest = source_digest(project);
    let unversioned = |digest| Revision {
        commit: None,
        state: RevisionState::Worktree,
        digest,
    };
    let Some(repo) = repo else {
        return Ok(unversioned(digest));
    };
    let head = match verify(repo, "HEAD^{commit}") {
        Ok(Some(head)) => head,
        Ok(None) | Err(GitError::NotInstalled) => return Ok(unversioned(digest)),
        Err(e) => return Err(e),
    };
    let roots = source_roots(project, &repo.project_prefix);
    let mut args = vec![
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--untracked-files=all",
        "--",
    ];
    args.extend(roots.iter().map(String::as_str));
    let status = match git_stdout(&repo.toplevel, &args, None) {
        Ok(status) => status,
        Err(GitError::NotInstalled) => return Ok(unversioned(digest)),
        Err(e) => return Err(e),
    };
    let state = if status.is_empty() {
        RevisionState::Clean
    } else {
        RevisionState::Worktree
    };
    Ok(Revision {
        commit: Some(head),
        state,
        digest,
    })
}

/// The source-set roots as repository-relative pathspecs: `cpl.toml`, every
/// `[build] sources` directory (`"."` is the project root itself), `work`.
fn source_roots(project: &Project, prefix: &str) -> Vec<String> {
    let rooted = |path: &str| {
        if path.is_empty() {
            match prefix.strip_suffix('/') {
                Some(dir) => dir.to_string(),
                None => ".".to_string(),
            }
        } else {
            format!("{prefix}{path}")
        }
    };
    let mut roots = vec![rooted(MANIFEST)];
    for source in &project.manifest.build.sources {
        roots.push(rooted(if source == "." { "" } else { source }));
    }
    roots.push(rooted(WORK_DIR));
    roots
}

/// The full commit id `rev` names: `git rev-parse --verify <rev>^{commit}`.
/// Every failure of that command is [`GitError::UnknownRevision`] (Section
/// 7.2: "failures are `git_unknown_revision`"; Appendix B: "`rev-parse`
/// failed"), whatever its exit status: git exits 1 for a name that is not
/// an object, but 128, even under `-q`, for well-formed names that name
/// nothing here, such as `HEAD@{5}` past the end of the reflog or
/// `@{upstream}` of a branch without one. An empty name and a name starting
/// with `-` (which git would read as an option) are unknown revisions too.
/// The other errors are a `git` that cannot be run, a repository that is
/// gone ([`GitError::NotARepository`]), and a `git` killed by a signal.
pub fn resolve_rev(repo: &GitRepo, rev: &str) -> Result<String, GitError> {
    let unknown = || GitError::UnknownRevision(rev.to_string());
    if rev.is_empty() || rev.starts_with('-') {
        return Err(unknown());
    }
    match verify(repo, &format!("{rev}^{{commit}}")) {
        Ok(Some(commit)) => Ok(commit),
        Ok(None) | Err(GitError::Command { status: 0.., .. }) => Err(unknown()),
        Err(e) => Err(e),
    }
}

/// `git merge-base <a> <b>` (the old side of `cpl diff A...B`, Section 7.2):
/// both names are resolved first ([`resolve_rev`]); two commits without a
/// common ancestor are [`GitError::Command`] with git's status 1.
pub fn merge_base(repo: &GitRepo, a: &str, b: &str) -> Result<String, GitError> {
    let a = resolve_rev(repo, a)?;
    let b = resolve_rev(repo, b)?;
    let args = ["merge-base", a.as_str(), b.as_str()];
    let output = run_git(&repo.toplevel, &args, None)?;
    if output.status.success() {
        return Ok(first_line(&output.stdout));
    }
    let mut error = command_error(&repo.toplevel, &args, &output);
    if let GitError::Command {
        status: 1, stderr, ..
    } = &mut error
    {
        if stderr.is_empty() {
            *stderr = format!("{a} and {b} have no common ancestor");
        }
    }
    Err(error)
}

// ---------------------------------------------------------------------------
// 9.3 Obtaining a tree
// ---------------------------------------------------------------------------

/// The project files of `commit` (Section 9.3), keyed by project-relative
/// path: `cpl.toml`, `cpl.lock` (read, never digested), every `*.cpl` file
/// under the `[build] sources` directories of that commit's manifest, and
/// every `*.work` and `*.patch` file directly under `work/`. The tree is
/// read with `ls-tree` and `cat-file --batch`, never by checking out (T32).
///
/// The files are read as a checkout of the commit reads them, because the
/// facet map "is the same whether built from a directory or from a git
/// tree" (Section 5.4) and `cpl_project::load_project` reads a symbolic
/// link to a file as that file: a link among them is followed inside the
/// commit's tree (`git cat-file --batch --follow-symlinks`) and holds the
/// bytes of the file it leads to, wherever that file is in the repository.
/// A link that leads to no file of the commit (to a directory, to nothing,
/// around a loop, through a file, or out of the repository, whose files no
/// commit holds) is not part of the tree, and neither is a submodule.
///
/// A commit without `cpl.toml` at the project prefix yields an empty tree,
/// which `load_project_from_tree` reports as `LoadError::NoManifest`, the
/// `git_no_project_at_revision` of `cpl diff`. A manifest that does not
/// parse yields `cpl.toml` and `cpl.lock` alone, so that loading reports the
/// manifest error just as for a working tree.
pub fn tree_at(repo: &GitRepo, commit: &str) -> Result<VirtualTree, GitError> {
    let commit = resolve_rev(repo, commit)?;
    let prefix = repo.project_prefix.as_str();
    // 1-2. cpl.toml at the prefix (read like every other file, so that a link
    // is followed), and the source directories it names.
    let manifest_path = format!("{prefix}{MANIFEST}");
    let mut manifest_request = None;
    for entry in ls_tree(repo, &commit, std::slice::from_ref(&manifest_path))? {
        if entry.path == manifest_path.as_bytes() {
            manifest_request = entry.request(&commit, &manifest_path)?;
        }
    }
    let manifest = match manifest_request {
        Some(request) => read_files(repo, &[request])?.pop().flatten(),
        None => None,
    };
    let Some(manifest) = manifest else {
        return Ok(VirtualTree::default());
    };
    let sources: Vec<String> = std::str::from_utf8(&manifest)
        .ok()
        .and_then(|text| parse_manifest(text).ok())
        .map(|(manifest, _)| manifest.build.sources)
        .unwrap_or_default();
    let source_dirs: Vec<&str> = sources
        .iter()
        .map(|s| if s == "." { "" } else { s.as_str() })
        .collect();

    // 3. The files of the source set.
    let mut paths = vec![manifest_path.clone(), format!("{prefix}{LOCK}")];
    for dir in &source_dirs {
        paths.push(if dir.is_empty() {
            prefix.strip_suffix('/').unwrap_or(".").to_string()
        } else {
            format!("{prefix}{dir}/")
        });
    }
    paths.push(format!("{prefix}{WORK_DIR}/"));
    let mut wanted: Vec<String> = Vec::new();
    let mut requests: Vec<FileRequest> = Vec::new();
    for entry in ls_tree(repo, &commit, &paths)? {
        if !entry.is_regular() && !entry.is_link() {
            continue;
        }
        let path = match std::str::from_utf8(&entry.path) {
            Ok(path) => path,
            Err(_) => {
                let lossy = String::from_utf8_lossy(&entry.path);
                let relative = lossy.strip_prefix(prefix).unwrap_or(&lossy);
                if in_source_set(relative, &source_dirs) {
                    return Err(unreadable(&lossy, "file name is not valid UTF-8"));
                }
                continue;
            }
        };
        let Some(relative) = path.strip_prefix(prefix) else {
            continue;
        };
        if in_source_set(relative, &source_dirs) {
            if let Some(request) = entry.request(&commit, path)? {
                wanted.push(relative.to_string());
                requests.push(request);
            }
        }
    }

    // 4. Their contents, in one process.
    let contents = read_files(repo, &requests)?;

    // 5. The tree, keyed without the prefix; a link to no file is left out.
    let files: BTreeMap<String, Vec<u8>> = wanted
        .into_iter()
        .zip(contents)
        .filter_map(|(path, bytes)| Some((path, bytes?)))
        .collect();
    Ok(VirtualTree { files })
}

/// Whether a project-relative path is a source-set file or `cpl.lock`:
/// `source_dirs` are the `[build] sources` entries with `""` for `"."`.
fn in_source_set(path: &str, source_dirs: &[&str]) -> bool {
    if path == MANIFEST || path == LOCK {
        return true;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some(rest) = path.strip_prefix("work/") {
        if !rest.contains('/') && has_stem_and_extension(rest, &[".work", ".patch"]) {
            return true;
        }
    }
    has_stem_and_extension(name, &[".cpl"])
        && source_dirs.iter().any(|dir| {
            dir.is_empty()
                || path
                    .strip_prefix(dir)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

/// `name` ends with one of `extensions` and has a non-empty stem.
fn has_stem_and_extension(name: &str, extensions: &[&str]) -> bool {
    extensions
        .iter()
        .any(|ext| name.len() > ext.len() && name.ends_with(ext))
}

/// A file of the commit that cannot be read: [`GitError::Io`] with
/// [`io::ErrorKind::InvalidData`].
fn unreadable(path: &str, why: &str) -> GitError {
    GitError::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{path}: {why}"),
    ))
}

/// One record of `git ls-tree -z`: `<mode> SP <type> SP <oid> TAB <path>`.
struct TreeEntry {
    mode: String,
    kind: String,
    oid: String,
    /// Repository-relative, as git stores it (not necessarily UTF-8).
    path: Vec<u8>,
}

impl TreeEntry {
    /// A regular file: a blob of mode `100644` or `100755`.
    fn is_regular(&self) -> bool {
        self.kind == "blob" && (self.mode == "100644" || self.mode == "100755")
    }

    /// A symbolic link: a blob of mode `120000`, which holds the link's
    /// target.
    fn is_link(&self) -> bool {
        self.kind == "blob" && self.mode == "120000"
    }

    /// How to read this entry, whose repository-relative path is `path`, as
    /// a file of `commit`: a regular file by its blob; a link by
    /// `<commit>:<path>`, which `cat-file --follow-symlinks` follows inside
    /// the commit's tree; `None` for a directory or a submodule. A link whose
    /// path holds a line break cannot be named on a line of the batch input
    /// and is [`GitError::Io`].
    fn request(&self, commit: &str, path: &str) -> Result<Option<FileRequest>, GitError> {
        if self.is_regular() {
            return Ok(Some(FileRequest::Blob(self.oid.clone())));
        }
        if !self.is_link() {
            return Ok(None);
        }
        if path.contains(['\n', '\r']) {
            return Err(unreadable(
                path,
                "a symbolic link whose name holds a line break cannot be followed in a commit",
            ));
        }
        Ok(Some(FileRequest::Link(format!("{commit}:{path}"))))
    }
}

/// One line of `git cat-file --batch --follow-symlinks` input.
enum FileRequest {
    /// A regular file, by its blob's object id.
    Blob(String),
    /// A symbolic link, as `<commit>:<path>`.
    Link(String),
}

impl FileRequest {
    fn line(&self) -> &str {
        match self {
            FileRequest::Blob(line) | FileRequest::Link(line) => line,
        }
    }
}

/// `git ls-tree -r -z <commit> -- <paths>` (literal pathspecs, [`run_git`]).
fn ls_tree(repo: &GitRepo, commit: &str, paths: &[String]) -> Result<Vec<TreeEntry>, GitError> {
    let mut args = vec!["ls-tree", "-r", "-z", commit, "--"];
    args.extend(paths.iter().map(String::as_str));
    let listing = git_stdout(&repo.toplevel, &args, None)?;
    Ok(listing
        .split(|&b| b == 0)
        .filter_map(|record| {
            let tab = record.iter().position(|&b| b == b'\t')?;
            let meta = std::str::from_utf8(&record[..tab]).ok()?;
            let mut fields = meta.split(' ');
            let (mode, kind, oid) = (fields.next()?, fields.next()?, fields.next()?);
            Some(TreeEntry {
                mode: mode.to_string(),
                kind: kind.to_string(),
                oid: oid.to_string(),
                path: record[tab + 1..].to_vec(),
            })
        })
        .collect())
}

/// The bytes of the files `requests` name, in order, read by one `git
/// cat-file --batch --follow-symlinks`; `None` for a link that leads to no
/// file of the commit.
fn read_files(repo: &GitRepo, requests: &[FileRequest]) -> Result<Vec<Option<Vec<u8>>>, GitError> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let mut input = Vec::with_capacity(requests.len() * 41);
    for request in requests {
        input.extend_from_slice(request.line().as_bytes());
        input.push(b'\n');
    }
    let args = ["cat-file", "--batch", "--follow-symlinks"];
    let batch = git_stdout(&repo.toplevel, &args, Some(input))?;
    parse_batch(&batch, requests)
}

/// The answers of `git cat-file --batch --follow-symlinks`, one per request:
/// `<oid> SP <type> SP <size> LF <contents> LF` for the object reached (a
/// blob's contents are the file); `<name> SP missing LF` for nothing; and
/// for a link that leads to no object of the commit, `dangling`, `loop`,
/// `notdir`, or `symlink` (it leaves the repository) `SP <size> LF <text>
/// LF`. A blob requested by its id must be there.
fn parse_batch(
    mut batch: &[u8],
    requests: &[FileRequest],
) -> Result<Vec<Option<Vec<u8>>>, GitError> {
    let malformed = |what: &str| {
        GitError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected `git cat-file --batch` output: {what}"),
        ))
    };
    let mut files = Vec::with_capacity(requests.len());
    for request in requests {
        let link = matches!(request, FileRequest::Link(_));
        let header_end = batch
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| malformed("truncated header"))?;
        let header = String::from_utf8_lossy(&batch[..header_end]).to_string();
        batch = &batch[header_end + 1..];
        if header.ends_with(" missing") {
            if link {
                files.push(None);
                continue;
            }
            return Err(malformed(&header));
        }
        let fields: Vec<&str> = header.split(' ').collect();
        let (kind, size) = match fields[..] {
            [_oid, kind, size] => (kind, size),
            ["dangling" | "loop" | "notdir" | "symlink", size] if link => ("", size),
            _ => return Err(malformed(&header)),
        };
        let size: usize = size.parse().map_err(|_| malformed(&header))?;
        if batch.len() <= size || batch[size] != b'\n' {
            return Err(malformed("truncated contents"));
        }
        let contents = &batch[..size];
        batch = &batch[size + 1..];
        if kind == "blob" {
            files.push(Some(contents.to_vec()));
        } else if link {
            files.push(None);
        } else {
            return Err(malformed(&header));
        }
    }
    Ok(files)
}
