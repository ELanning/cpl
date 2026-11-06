//! Validated checkout with a durable rollback journal. Cooperating writers use an OS file lock;
//! immutable candidate publication uses Git's atomic compare-and-swap reference update.
use crate::{change::Candidate, error, merge::Merge, Snapshot};
use cpl_diag::Diagnostic;
use cpl_project::LoadOptions;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

pub fn safe_relative(path: &str) -> Result<(), Diagnostic> {
    if path.is_empty()
        || path.contains('\\')
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path.split('/').any(|s| s == ".git" || s == ".cpl")
    {
        return Err(error(
            "change_path_invalid",
            format!("invalid project-relative source path {path:?}"),
        ));
    }
    Ok(())
}
fn io(e: std::io::Error) -> Diagnostic {
    error("evolution_io", e.to_string())
}
fn no_symlinks(root: &Path, relative: &str) -> Result<PathBuf, Diagnostic> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(error("change_path_invalid", "non-normal path component"));
        }
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(error(
                    "change_symlink",
                    "transaction writes may not traverse symbolic links",
                ))
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io(e)),
        }
    }
    Ok(path)
}
fn lock(root: &Path) -> Result<File, Diagnostic> {
    let directory = no_symlinks(root, ".cpl")?;
    fs::create_dir_all(&directory).map_err(io)?;
    let path = no_symlinks(root, ".cpl/evolution.lock")?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io)?;
    file.try_lock().map_err(|e| {
        error(
            "change_busy",
            format!("another evolution operation holds the project lock: {e}"),
        )
    })?;
    Ok(file)
}
static NEXT: AtomicU64 = AtomicU64::new(0);
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Diagnostic> {
    let parent = path
        .parent()
        .ok_or_else(|| error("change_path_invalid", "file has no parent"))?;
    fs::create_dir_all(parent).map_err(io)?;
    let temp = parent.join(format!(
        ".cpl-write-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(io)?;
        if let Ok(meta) = fs::metadata(path) {
            file.set_permissions(meta.permissions()).map_err(io)?;
        }
        file.write_all(bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        fs::rename(&temp, path).map_err(io)?;
        #[cfg(unix)]
        File::open(parent).and_then(|f| f.sync_all()).map_err(io)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    format: u32,
    files: BTreeMap<String, FileChange>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileChange {
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}
fn bytes(path: &Path) -> Result<Option<Vec<u8>>, Diagnostic> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(e)),
    }
}

/// A validated program that may replace `base`, with the record stored beside it. Only accepted,
/// sealed change and merge results produce one.
pub struct Install<'a> {
    base: String,
    snapshot: &'a Snapshot,
    record: Vec<u8>,
    evidence: String,
    title: String,
}

impl Candidate {
    pub fn install(&self, base: &Snapshot) -> Result<Install<'_>, Diagnostic> {
        if !self.accepted_for(base) {
            return Err(error(
                "change_not_accepted",
                "only the validated candidate can be installed",
            ));
        }
        Ok(Install {
            base: base.digest(),
            snapshot: &self.snapshot,
            record: serde_json::to_vec_pretty(&self.report).unwrap(),
            evidence: self.report.evidence.digest.clone(),
            title: format!("CPL change {}", self.report.name),
        })
    }
}

impl Merge {
    /// Install over `left`: the accepted composition replaces the left side's files.
    pub fn install(&self, left: &Snapshot) -> Result<Install<'_>, Diagnostic> {
        match (&self.snapshot, &self.report.evidence) {
            (Some(snapshot), Some(evidence)) if self.accepted_for(left) => Ok(Install {
                base: left.digest(),
                snapshot,
                record: serde_json::to_vec_pretty(&self.report).unwrap(),
                evidence: evidence.digest.clone(),
                title: "CPL semantic merge".into(),
            }),
            _ => Err(error(
                "merge_not_accepted",
                "only an accepted merge of this left side can be installed",
            )),
        }
    }
}

pub fn apply(
    root: &Path,
    base: &Snapshot,
    install: &Install,
    options: &LoadOptions,
) -> Result<Vec<String>, Diagnostic> {
    if install.base != base.digest() {
        return Err(error(
            "change_not_accepted",
            "the validated candidate replaces a different program",
        ));
    }
    let candidate = install.snapshot;
    let _lock = lock(root)?;
    let journal_path = no_symlinks(root, ".cpl/evolution-journal.json")?;
    if journal_path.exists() {
        return Err(error(
            "change_recovery_required",
            "run cpl transaction --recover before applying another change",
        ));
    }
    let current = Snapshot::read(root, options)?;
    if current.digest() != base.digest() {
        return Err(error(
            "change_stale",
            "project inputs changed during validation; recompute the candidate",
        ));
    }
    let mut files = BTreeMap::new();
    for path in base.tree.files.keys().chain(candidate.tree.files.keys()) {
        let before = base.tree.files.get(path);
        let after = candidate.tree.files.get(path);
        if before == after {
            continue;
        }
        safe_relative(path)?;
        no_symlinks(root, path)?;
        files.insert(
            path.clone(),
            FileChange {
                before: before.cloned(),
                after: after.cloned(),
            },
        );
    }
    let journal = Journal { format: 1, files };
    atomic_write(&journal_path, &serde_json::to_vec(&journal).unwrap())?;
    let result = (|| {
        for (path, change) in &journal.files {
            let path = no_symlinks(root, path)?;
            if bytes(&path)? != change.before {
                return Err(error(
                    "change_stale",
                    "a source file changed during installation",
                ));
            }
            if let Some(after) = &change.after {
                atomic_write(&path, after)?;
            } else {
                fs::remove_file(path).map_err(io)?;
            }
        }
        // Confirm the complete input set, including additions by noncooperating editors.
        if Snapshot::read(root, options)?.digest() != candidate.digest() {
            return Err(error(
                "change_stale",
                "project inputs changed during installation",
            ));
        }
        let evidence_path = no_symlinks(root, ".cpl/last-change.json")?;
        atomic_write(&evidence_path, &install.record)?;
        fs::remove_file(&journal_path).map_err(io)?;
        Ok(journal.files.keys().cloned().collect())
    })();
    if let Err(original) = result {
        if let Err(recovery) = restore(root, &journal, &journal_path) {
            return Err(original.with_note(
                format!(
                    "rollback needs intervention: {}; journal retained",
                    recovery.message
                ),
                None,
            ));
        }
        return Err(original);
    }
    result
}

pub fn recover(root: &Path) -> Result<usize, Diagnostic> {
    let _lock = lock(root)?;
    let path = no_symlinks(root, ".cpl/evolution-journal.json")?;
    let Some(data) = bytes(&path)? else {
        return Ok(0);
    };
    let journal: Journal = serde_json::from_slice(&data)
        .map_err(|e| error("change_journal_invalid", e.to_string()))?;
    restore(root, &journal, &path)?;
    Ok(journal.files.len())
}
fn restore(root: &Path, journal: &Journal, path: &Path) -> Result<(), Diagnostic> {
    if journal.format != 1 {
        return Err(error("change_journal_invalid", "unknown journal format"));
    }
    for (file, change) in &journal.files {
        safe_relative(file)?;
        let current = bytes(&no_symlinks(root, file)?)?;
        if current != change.before && current != change.after {
            return Err(error(
                "change_recovery_conflict",
                format!("{file} has independent edits; recovery left it untouched"),
            ));
        }
    }
    for (file, change) in &journal.files {
        let path = no_symlinks(root, file)?;
        if bytes(&path)? == change.before {
            continue;
        }
        if let Some(before) = &change.before {
            atomic_write(&path, before)?;
        } else if path.exists() {
            fs::remove_file(path).map_err(io)?;
        }
    }
    fs::remove_file(path).map_err(io)?;
    Ok(())
}

/// Publish a validated immutable Git commit. No checkout, user index, or current branch is changed.
pub fn publish(
    root: &Path,
    base: &Snapshot,
    install: &Install,
    reference: &str,
    options: &LoadOptions,
) -> Result<String, Diagnostic> {
    if !reference.starts_with("refs/cpl/changes/") || install.base != base.digest() {
        return Err(error(
            "change_publish_invalid",
            "publish requires an accepted candidate and refs/cpl/changes/NAME",
        ));
    }
    let _lock = lock(root)?;
    if Snapshot::read(root, options)?.digest() != base.digest() {
        return Err(error("change_stale", "project changed during validation"));
    }
    let repo = repository(root)?;
    let head = rev(&repo, "HEAD")?;
    let parent_tree =
        cpl_index::tree_at(&repo, &head).map_err(|e| error("change_git", e.to_string()))?;
    if Snapshot::new(parent_tree.clone(), options)?.digest() != base.digest() {
        return Err(error("change_publish_dirty", "publication needs a committed project baseline matching HEAD; commit the baseline or use --apply"));
    }
    commit(
        root,
        &repo,
        &head,
        &parent_tree,
        &[&head],
        install,
        reference,
        || {
            if Snapshot::read(root, options)?.digest() != base.digest()
                || rev(&repo, "HEAD")? != head
            {
                return Err(error(
                    "change_stale",
                    "source or HEAD changed during publication",
                ));
            }
            Ok(())
        },
    )
}

/// Publish an accepted merge as a commit whose parents are the two merged commits.
pub fn publish_merge(
    root: &Path,
    left: &str,
    right: &str,
    install: &Install,
    reference: &str,
    options: &LoadOptions,
) -> Result<String, Diagnostic> {
    if !reference.starts_with("refs/cpl/changes/") {
        return Err(error(
            "change_publish_invalid",
            "publish requires an accepted merge and refs/cpl/changes/NAME",
        ));
    }
    let _lock = lock(root)?;
    let repo = repository(root)?;
    let (left, right) = (rev(&repo, left)?, rev(&repo, right)?);
    let left_tree =
        cpl_index::tree_at(&repo, &left).map_err(|e| error("change_git", e.to_string()))?;
    if Snapshot::new(left_tree.clone(), options)?.digest() != install.base {
        return Err(error(
            "change_stale",
            "the left commit differs from the merged left side",
        ));
    }
    commit(
        root,
        &repo,
        &left,
        &left_tree,
        &[&left, &right],
        install,
        reference,
        || Ok(()),
    )
}

fn repository(root: &Path) -> Result<cpl_index::GitRepo, Diagnostic> {
    cpl_index::discover_repo(root)
        .map_err(|e| error("change_git", e.to_string()))?
        .ok_or_else(|| error("change_git", "publication requires a Git repository"))
}
fn rev(repo: &cpl_index::GitRepo, name: &str) -> Result<String, Diagnostic> {
    // a name that begins with `-` would be read by git as an option
    if name.is_empty() || name.starts_with('-') {
        return Err(error("change_git", format!("`{name}` is not a revision")));
    }
    let out = cpl_work::git(
        repo,
        &["rev-parse", "--verify", &format!("{name}^{{commit}}")],
    )
    .map_err(|e| error("change_git", e.to_string()))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Build a commit from `start`'s tree with the project files replaced by the candidate's, plus its
/// record under `.cpl/evidence/`, then advance `reference` with a compare-and-swap.
#[allow(clippy::too_many_arguments)]
fn commit(
    root: &Path,
    repo: &cpl_index::GitRepo,
    start: &str,
    start_tree: &cpl_project::VirtualTree,
    parents: &[&str],
    install: &Install,
    reference: &str,
    before_update: impl Fn() -> Result<(), Diagnostic>,
) -> Result<String, Diagnostic> {
    let git =
        |args: &[&str]| cpl_work::git(repo, args).map_err(|e| error("change_git", e.to_string()));
    if reference.starts_with('-') {
        return Err(error("change_git", format!("`{reference}` is not a reference name")));
    }
    git(&["check-ref-format", reference])?;
    let old = match git(&["rev-parse", "--verify", reference]) {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => String::new(),
    };
    let index_path = no_symlinks(
        root,
        &format!(
            ".cpl/evolution-index-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
    )?;
    let index_path = if index_path.is_absolute() {
        index_path
    } else {
        std::env::current_dir().map_err(io)?.join(index_path)
    };
    let index_git = |args: &[&str], input: Option<&[u8]>| -> Result<String, Diagnostic> {
        let mut cmd = Command::new("git");
        // a repository's own config may not run a program on a read (`core.fsmonitor`)
        cmd.args(["-c", "core.fsmonitor=false"])
            .arg("-C")
            .arg(&repo.toplevel)
            .args(args)
            .env("GIT_INDEX_FILE", &index_path)
            .env("GIT_LITERAL_PATHSPECS", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(io)?;
        if let Some(input) = input {
            child.stdin.take().unwrap().write_all(input).map_err(io)?;
        } else {
            drop(child.stdin.take());
        }
        let out = child.wait_with_output().map_err(io)?;
        if !out.status.success() {
            return Err(error(
                "change_git",
                String::from_utf8_lossy(&out.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let files = &install.snapshot.tree.files;
    let result = (|| {
        index_git(&["read-tree", start], None)?;
        for path in start_tree.files.keys().filter(|p| !files.contains_key(*p)) {
            index_git(
                &[
                    "update-index",
                    "--force-remove",
                    "--",
                    &format!("{}{path}", repo.project_prefix),
                ],
                None,
            )?;
        }
        for (path, data) in files {
            safe_relative(path)?;
            let hash = index_git(&["hash-object", "-w", "--stdin"], Some(data))?;
            index_git(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{hash},{}{path}", repo.project_prefix),
                ],
                None,
            )?;
        }
        let hash = index_git(&["hash-object", "-w", "--stdin"], Some(&install.record))?;
        index_git(
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!(
                    "100644,{hash},{}.cpl/evidence/{}.json",
                    repo.project_prefix, install.evidence
                ),
            ],
            None,
        )?;
        let tree = index_git(&["write-tree"], None)?;
        let commit = cpl_work::commit_tree(
            repo,
            &tree,
            parents,
            &format!("{}\n\nEvidence: {}", install.title, install.evidence),
        )
        .map_err(|e| error("change_git", e.to_string()))?;
        before_update()?;
        cpl_work::update_ref_cas(repo, reference, &commit, &old)
            .map_err(|e| error("change_publish_race", e.to_string()))?;
        Ok(commit)
    })();
    let _ = fs::remove_file(index_path);
    result
}
