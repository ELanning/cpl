//! Locating and loading a project (`spec/toolchain.md` 1.6, 3.2, 3.6, 3.7):
//! source discovery, `FileId` assignment (T15), the standard library,
//! parsing, and the diagnostics loading raises. A directory and a
//! [`VirtualTree`] of it go through one code path, so they produce equal
//! [`Project`]s (apart from [`Project::root`] and the origin of the files).

use crate::codes;
use crate::lock::{parse_lock, Lock};
use crate::manifest::{parse_manifest, Manifest};
use crate::stdlib::{digest_bytes, std_digest, STD_FILES};
use crate::util::line_at;
use cpl_diag::json::Digest;
use cpl_diag::{version, Category, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_syntax::{ast, parse_module, parse_patch, parse_work, FileKind, ParseResult};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};

/// The manifest's project-relative path.
const MANIFEST: &str = "cpl.toml";
/// The lock's project-relative path.
const LOCK: &str = "cpl.lock";
/// The directory whose `*.work` and `*.patch` files belong to the source set.
const WORK_DIR: &str = "work";

// ---------------------------------------------------------------------------
// The public API
// ---------------------------------------------------------------------------

/// Nearest ancestor of `start` (inclusive) that contains `cpl.toml` as a
/// file. A relative `start` is first made absolute against the current
/// directory (without resolving symlinks), so the result is absolute and
/// the search continues above the current directory.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let start = std::path::absolute(start).unwrap_or_else(|_| start.to_path_buf());
    start
        .ancestors()
        .find(|dir| dir.join(MANIFEST).is_file())
        .map(Path::to_path_buf)
}

/// How [`load_project`] and [`load_project_from_tree`] load.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadOptions {
    /// Default `Embedded`.
    pub std: StdSource,
    /// Parse `work/*.work` and `work/*.patch` into [`Project::parsed`].
    /// Default false: `cpl run` and `cpl test` never read work files
    /// (spec/semantics.md 1.1); `cpl check` sets it to parse them for syntax
    /// only (semantics S53), as do `cpl fmt`, `cpl work check`, and the coordinator.
    /// The files are members of the source set either way: they are
    /// registered in the source map (so `FileId`s do not depend on this
    /// option) and digested, and without this option they are
    /// [`ParsedFile::Skipped`] and none of their diagnostics, `invalid_utf8`
    /// included, is reported.
    pub include_work_files: bool,
}

/// Where the standard-library stubs come from (Section 3.6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StdSource {
    /// The stubs embedded at build time ([`crate::STD_FILES`]).
    #[default]
    Embedded,
    /// The `CPL_STD_DIR` override (library development only): every `*.cpl`
    /// file directly in the directory, in sorted name order, registered as
    /// `std/<file name>`.
    Dir(PathBuf),
}

/// An in-memory source tree: '/'-separated project-relative paths to bytes.
/// Built by cpl-index from a git commit (Section 9.3) or by tests. Loading
/// from a tree and from a directory produce identical Projects.
///
/// Paths are normalized (`src/a/b.cpl`, never `./src/a/b.cpl` or
/// `src//a/b.cpl`); a directory exists in the tree when some file is under
/// it, so an empty directory, which a git tree cannot record, is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtualTree {
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Load the project rooted at `root` (the directory containing `cpl.toml`).
///
/// The source set is `cpl.toml`, every `*.cpl` file under the `[build]
/// sources` directories (recursively), and every `*.work` and `*.patch`
/// file directly in `work/`; `cpl.lock` is read when present. Every project
/// file has [`FileOrigin::Project`]. Symbolic links follow toolchain T54,
/// as a git tree does, which records a link rather than the tree behind
/// it: a link to a file is read as that file; a link to a directory is
/// never followed, so a `[build] sources` directory or `work/` that is a
/// link, or is reached through one, is absent (`project_source_dir_missing`
/// for a source directory); and a link that leads to no file (dangling, a
/// loop, a path through a file) is not a member.
pub fn load_project(root: &Path, opts: &LoadOptions) -> Result<Project, LoadError> {
    let tree = DiskTree {
        root,
        confine: std::cell::OnceCell::new(),
    };
    load(
        &tree,
        Some(root.to_path_buf()),
        root.to_path_buf(),
        FileOrigin::Project,
        opts,
    )
}

/// Load a project from an in-memory tree. `origin` is the FileOrigin of
/// every file of the tree: `Revision(commit)` for a git tree (Section 9.3),
/// `Virtual` in tests. A tree without `cpl.toml` is
/// [`LoadError::NoManifest`] with the path `.`.
pub fn load_project_from_tree(
    tree: &VirtualTree,
    origin: FileOrigin,
    opts: &LoadOptions,
) -> Result<Project, LoadError> {
    load(tree, None, PathBuf::from("."), origin, opts)
}

/// A loaded project: manifest, lock, source map, and parsed files.
#[derive(Clone, Debug)]
pub struct Project {
    /// `None` for a [`VirtualTree`].
    pub root: Option<PathBuf>,
    /// Section 11.1.
    pub manifest: Manifest,
    /// Section 11.2; `None` when `cpl.lock` is absent.
    pub lock: Option<Lock>,
    /// blake3 of the bytes of `cpl.toml`: the one source-set member that is
    /// not a [`SourceEntry`].
    pub manifest_digest: Digest,
    /// blake3 of the bytes of `cpl.lock` when it is present: the `inputs.lock`
    /// of every receipt (Section 8). The lock is read but is not a member of
    /// the source set (Section 0), so no other digest covers it.
    pub lock_digest: Option<Digest>,
    /// Project files, std files, nothing else; shared with `Program`.
    /// `cpl.toml` and `cpl.lock` are not registered (their diagnostics have
    /// no span, semantics S63 (6)).
    pub source_map: Arc<SourceMap>,
    /// The `.cpl`, `.work`, and `.patch` files of the source set plus std:
    /// std first, in sorted name order, then the project files in
    /// lexicographic order of project-relative path (T15). `files[i].file`
    /// is `FileId(i)`.
    pub files: Vec<SourceEntry>,
    /// One per [`SourceEntry`] (`Failed` for a file with a parse diagnostic, D59/D70).
    pub parsed: Vec<ParsedFile>,
    /// Manifest, lock, io, lexical, and syntax diagnostics, and the module
    /// diagnostics of loading. Emitted in stage order: the `project`
    /// diagnostics (manifest, lock, source discovery), then the `parse`
    /// diagnostics in `FileId` order, then the `resolve` diagnostics
    /// (`unsupported_edition`, then `reserved_module_path` and
    /// `module_file_misplaced` in `FileId` order).
    pub diagnostics: Vec<Diagnostic>,
}

impl Project {
    /// `std_digest` (Section 3.6) of the standard library this project was
    /// loaded with: the files of origin [`FileOrigin::Std`], named by their
    /// file name (`prelude.cpl`). With [`StdSource::Embedded`] this is
    /// `std_digest(STD_FILES)`.
    pub fn std_digest(&self) -> Digest {
        let files: Vec<(&str, &str)> = self
            .files
            .iter()
            .filter(|entry| entry.origin == FileOrigin::Std)
            .map(|entry| {
                let file = self.source_map.file(entry.file);
                let name = file.path.strip_prefix("std/").unwrap_or(&file.path);
                (name, file.text())
            })
            .collect();
        std_digest(&files)
    }
}

/// One file of the source set or of std.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEntry {
    pub file: FileId,
    pub kind: FileKind,
    /// blake3 of the file's bytes.
    pub digest: Digest,
    pub origin: FileOrigin,
}

/// The parse of one [`SourceEntry`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedFile {
    Module(Arc<ast::Module>),
    Work(Arc<ast::WorkFile>),
    Patch(Arc<ast::PatchFile>),
    /// A `.work`/`.patch` file present in the source set but not parsed
    /// because `include_work_files` is false.
    Skipped,
    /// The file has a lexical or syntax error or is not valid UTF-8 (its one
    /// diagnostic is in [`Project::diagnostics`]).
    Failed,
}

/// Why a project could not be loaded at all (exit code 2). Paths are
/// project-relative (`cpl.toml`, `src/reader/pages.cpl`).
#[derive(thiserror::Error, Debug)]
pub enum LoadError {
    /// The root has no `cpl.toml` (the CLI's `usage_no_project`).
    #[error("no cpl.toml found at {0}")]
    NoManifest(PathBuf),
    /// `cpl.toml` is unusable: `project_manifest_invalid` (Section 11.1).
    #[error("{path}: {message}")]
    Manifest {
        path: String,
        message: String,
        line: Option<u32>,
    },
    /// `cpl.lock` does not parse: `project_lock_invalid` (Section 11.2).
    #[error("{path}: {message}")]
    Lock {
        path: String,
        message: String,
        line: Option<u32>,
    },
    /// A file or directory of the project could not be read.
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    // Invalid UTF-8 in a source file is not a LoadError: it is the parse
    // diagnostic `invalid_utf8`, raised by the SourceMap when the bytes are
    // read (`SourceMap::add_bytes`, grammar D70), and the file is
    // `ParsedFile::Failed`.
    /// The standard library could not be read: a [`StdSource::Dir`] that is
    /// not a readable directory or has no `*.cpl` file.
    #[error("standard library unavailable: {0}")]
    Std(String),
}

// ---------------------------------------------------------------------------
// Trees
// ---------------------------------------------------------------------------

/// Read access to the files of a project: a directory or a [`VirtualTree`].
trait Tree {
    /// The bytes of the regular file at a project-relative path; `None` when
    /// there is none.
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, LoadError>;
    /// Whether a project-relative directory exists (`""` is the root).
    fn has_dir(&self, dir: &str) -> bool;
    /// The project-relative paths of the files in `dir` (recursively, or
    /// only those directly in it) whose file name satisfies `keep`.
    fn list(
        &self,
        dir: &str,
        recursive: bool,
        keep: &dyn Fn(&str) -> bool,
    ) -> Result<Vec<String>, LoadError>;
}

struct DiskTree<'a> {
    root: &'a Path,
    /// Where a file of the project may lie once links are followed (`confinement`).
    confine: std::cell::OnceCell<Option<PathBuf>>,
}

impl DiskTree<'_> {
    /// T54: the directory every file of the project must lie in once its links are followed:
    /// the git work tree that contains the project (the nearest ancestor of its canonical root
    /// with a `.git`), else the project root. A commit's tree holds only files of the commit, so a
    /// link there leads only to them; on disk the same rule keeps a checked-out repository from
    /// making `cpl check` read, and print in its diagnostics, any file the user can read
    /// (`/proc/self/environ`, a key in the home directory). `None` when the root cannot be
    /// resolved: then no link is followed.
    fn confinement(&self) -> Option<&Path> {
        self.confine
            .get_or_init(|| {
                let root = fs::canonicalize(self.root).ok()?;
                let repo = root
                    .ancestors()
                    .find(|dir| fs::symlink_metadata(dir.join(".git")).is_ok())
                    .map(Path::to_path_buf);
                Some(repo.unwrap_or(root))
            })
            .as_deref()
    }

    /// Whether the file at `full` (it exists) lies inside `confinement` once resolved.
    fn confined(&self, full: &Path) -> bool {
        match (fs::canonicalize(full), self.confinement()) {
            (Ok(target), Some(within)) => target.starts_with(within),
            _ => false,
        }
    }

    fn full(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.root.to_path_buf()
        } else {
            self.root.join(rel)
        }
    }

    fn walk(
        &self,
        dir: &str,
        recursive: bool,
        keep: &dyn Fn(&str) -> bool,
        out: &mut Vec<String>,
    ) -> Result<(), LoadError> {
        let shown = if dir.is_empty() { "." } else { dir };
        let entries = fs::read_dir(self.full(dir)).map_err(|e| io_error(shown, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_error(shown, e))?;
            let file_type = entry.file_type().map_err(|e| io_error(shown, e))?;
            let name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(raw) => {
                    // A path of the source set is a String; a name that is
                    // not UTF-8 is unreadable input when it could matter.
                    let lossy = raw.to_string_lossy().into_owned();
                    if (file_type.is_dir() && recursive) || keep(&lossy) {
                        return Err(io_error(
                            &join(dir, &lossy),
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "file name is not valid UTF-8",
                            ),
                        ));
                    }
                    continue;
                }
            };
            let path = join(dir, &name);
            // `DirEntry::file_type` does not follow a link, so a link to a
            // directory is never walked into (T54).
            if file_type.is_dir() {
                if recursive {
                    self.walk(&path, true, keep, out)?;
                }
            } else if keep(&name) {
                if file_type.is_file() {
                    out.push(path);
                } else if file_type.is_symlink() {
                    // T54: a link to a file is that file; a link to a
                    // directory, or one that leads to no file, is not a member.
                    if follow(&entry.path(), &path)?.is_some_and(|meta| meta.is_file())
                        && self.confined(&entry.path())
                    {
                        out.push(path);
                    }
                }
            }
        }
        Ok(())
    }
}

/// What a path of the project leads to, following symbolic links (T54):
/// its metadata, or `None` when nothing is there or a link on it leads to
/// no file (dangling, a loop, a path through a file). A path that cannot be
/// inspected otherwise (no permission) is [`LoadError::Io`].
fn follow(full: &Path, shown: &str) -> Result<Option<fs::Metadata>, LoadError> {
    match fs::metadata(full) {
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            let is_link = fs::symlink_metadata(full).is_ok_and(|m| m.file_type().is_symlink());
            if is_link && e.kind() != io::ErrorKind::PermissionDenied {
                Ok(None)
            } else {
                Err(io_error(shown, e))
            }
        }
    }
}

impl Tree for DiskTree<'_> {
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, LoadError> {
        let full = self.full(path);
        match follow(&full, path)? {
            Some(meta) if meta.is_file() => {
                // T54: a file that lies outside the confinement once its links (its own or a
                // directory's on the way) are followed is no file of the project
                if !self.confined(&full) {
                    return Ok(None);
                }
                fs::read(&full).map(Some).map_err(|e| io_error(path, e))
            }
            _ => Ok(None),
        }
    }

    /// T54: a link to a directory is never followed, so a directory that is
    /// a link, or is reached through one, does not exist here, as in a git
    /// tree (`git ls-tree -r c -- src/` lists nothing when `src` is a
    /// link). Only the project root itself is taken as given.
    fn has_dir(&self, dir: &str) -> bool {
        let mut path = self.root.to_path_buf();
        if !path.is_dir() {
            return false;
        }
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            path.push(part);
            // `symlink_metadata` describes a link as a link, not as its target.
            if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                return false;
            }
        }
        true
    }

    fn list(
        &self,
        dir: &str,
        recursive: bool,
        keep: &dyn Fn(&str) -> bool,
    ) -> Result<Vec<String>, LoadError> {
        let mut out = Vec::new();
        if self.has_dir(dir) {
            self.walk(dir, recursive, keep, &mut out)?;
        }
        Ok(out)
    }
}

impl Tree for VirtualTree {
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, LoadError> {
        Ok(self.files.get(path).cloned())
    }

    fn has_dir(&self, dir: &str) -> bool {
        if dir.is_empty() {
            return true;
        }
        let prefix = format!("{dir}/");
        self.files.keys().any(|path| path.starts_with(&prefix))
    }

    fn list(
        &self,
        dir: &str,
        recursive: bool,
        keep: &dyn Fn(&str) -> bool,
    ) -> Result<Vec<String>, LoadError> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        Ok(self
            .files
            .keys()
            .filter_map(|path| {
                let rest = path.strip_prefix(&prefix)?;
                if !recursive && rest.contains('/') {
                    return None;
                }
                let name = rest.rsplit('/').next().unwrap_or(rest);
                keep(name).then(|| path.clone())
            })
            .collect())
    }
}

fn io_error(path: &str, source: io::Error) -> LoadError {
    LoadError::Io {
        path: path.to_string(),
        source,
    }
}

/// `dir/name`, or `name` in the root.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// A `*.cpl` file name (with a stem).
fn is_cpl_name(name: &str) -> bool {
    name.len() > ".cpl".len() && name.ends_with(".cpl")
}

/// A `*.work` or `*.patch` file name (with a stem).
fn is_work_name(name: &str) -> bool {
    (name.len() > ".work".len() && name.ends_with(".work"))
        || (name.len() > ".patch".len() && name.ends_with(".patch"))
}

/// The tree-relative directory of a `[build] sources` entry: `""` for `"."`.
fn source_dir(entry: &str) -> &str {
    if entry == "." {
        ""
    } else {
        entry
    }
}

/// The prefix of the paths under a `[build] sources` entry: `""` or `"src/"`.
fn source_prefix(entry: &str) -> String {
    match source_dir(entry) {
        "" => String::new(),
        dir => format!("{dir}/"),
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// A project file found by discovery.
struct Found {
    kind: FileKind,
    /// For a `.cpl` file: the indices of the `[build] sources` entries it is under.
    roots: Vec<usize>,
}

fn load(
    tree: &dyn Tree,
    root: Option<PathBuf>,
    shown_root: PathBuf,
    origin: FileOrigin,
    opts: &LoadOptions,
) -> Result<Project, LoadError> {
    // cpl.toml (Section 11.1).
    let manifest_bytes = tree
        .read(MANIFEST)?
        .ok_or(LoadError::NoManifest(shown_root))?;
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|e| LoadError::Manifest {
        path: MANIFEST.to_string(),
        message: format!(
            "cpl.toml is not valid UTF-8 (invalid byte sequence at byte {})",
            e.valid_up_to()
        ),
        line: Some(line_at(&manifest_bytes, e.valid_up_to())),
    })?;
    let (manifest, manifest_diagnostics) =
        parse_manifest(manifest_text).map_err(|e| LoadError::Manifest {
            path: MANIFEST.to_string(),
            message: e.message,
            line: e.line,
        })?;
    let manifest_digest = digest_bytes(&manifest_bytes);

    // Diagnostics by stage (Project::diagnostics).
    let mut project_stage = Vec::new();
    let mut parse_stage = Vec::new();
    let mut resolve_stage = Vec::new();
    for diagnostic in manifest_diagnostics {
        if diagnostic.category == Category::Project {
            project_stage.push(diagnostic);
        } else {
            resolve_stage.push(diagnostic);
        }
    }

    // cpl.lock (Section 11.2): parsed when present; only its toolchain is
    // compared here. Staleness and absence are `cpl check --release`'s (T37).
    let (lock, lock_digest) = match tree.read(LOCK)? {
        None => (None, None),
        Some(bytes) => {
            let text = std::str::from_utf8(&bytes).map_err(|e| LoadError::Lock {
                path: LOCK.to_string(),
                message: format!(
                    "cpl.lock is not valid UTF-8 (invalid byte sequence at byte {})",
                    e.valid_up_to()
                ),
                line: Some(line_at(&bytes, e.valid_up_to())),
            })?;
            let lock = parse_lock(text).map_err(|e| LoadError::Lock {
                path: LOCK.to_string(),
                message: e.message,
                line: e.line,
            })?;
            if lock.toolchain.cpl != version::TOOLCHAIN
                || lock.toolchain.edition != version::EDITION
            {
                project_stage.push(toolchain_mismatch(&lock));
            }
            (Some(lock), Some(digest_bytes(&bytes)))
        }
    };

    // The standard library (Section 3.6).
    let std_files = read_std(&opts.std)?;

    // Discovery: the .cpl files under every source directory, then the
    // .work and .patch files directly in work/ (Section 0, semantics 1.1).
    let sources = &manifest.build.sources;
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    for (index, entry) in sources.iter().enumerate() {
        let dir = source_dir(entry);
        if !tree.has_dir(dir) {
            project_stage.push(
                Diagnostic::error(
                    codes::PROJECT_SOURCE_DIR_MISSING,
                    codes::CATEGORY,
                    format!("source directory `{entry}` of `[build] sources` does not exist"),
                )
                .with_data("path", json!(entry)),
            );
            continue;
        }
        for path in tree.list(dir, true, &is_cpl_name)? {
            found
                .entry(path)
                .or_insert_with(|| Found {
                    kind: FileKind::Cpl,
                    roots: Vec::new(),
                })
                .roots
                .push(index);
        }
    }
    for (path, file) in &found {
        if file.roots.len() > 1 {
            let reaching: Vec<&str> = file.roots.iter().map(|&i| sources[i].as_str()).collect();
            project_stage.push(
                Diagnostic::error(
                    codes::PROJECT_DUPLICATE_FILE_MODULE,
                    codes::CATEGORY,
                    format!(
                        "`{path}` is reachable from more than one `[build] sources` directory ({}); it is loaded once",
                        reaching.join(", ")
                    ),
                )
                .with_data("path", json!(path))
                .with_data("sources", json!(reaching)),
            );
        }
    }
    if found.is_empty() {
        let listed = if sources.is_empty() {
            "`[build] sources` is empty".to_string()
        } else {
            format!("searched {}", sources.join(", "))
        };
        project_stage.push(
            Diagnostic::warning(
                codes::PROJECT_NO_SOURCES,
                codes::CATEGORY,
                format!("no .cpl files found ({listed})"),
            )
            .with_data("sources", json!(sources)),
        );
    }
    if tree.has_dir(WORK_DIR) {
        for path in tree.list(WORK_DIR, false, &is_work_name)? {
            let kind = if path.ends_with(".work") {
                FileKind::Work
            } else {
                FileKind::Patch
            };
            found.entry(path).or_insert(Found {
                kind,
                roots: Vec::new(),
            });
        }
    }

    // Registration in FileId order (T15): std, then the project files by path.
    let mut map = SourceMap::new();
    let mut files = Vec::new();
    let mut parsed = Vec::new();
    for (name, bytes) in &std_files {
        let (file, utf8) = map.add_bytes(format!("std/{name}"), FileOrigin::Std, bytes);
        files.push(SourceEntry {
            file,
            kind: FileKind::Cpl,
            digest: digest_bytes(bytes),
            origin: FileOrigin::Std,
        });
        parsed.push(parse_file(
            &map,
            file,
            FileKind::Cpl,
            utf8,
            true,
            &mut parse_stage,
        ));
    }
    for (path, file) in &found {
        let bytes = tree.read(path)?.ok_or_else(|| {
            io_error(
                path,
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "the file disappeared while loading",
                ),
            )
        })?;
        let (id, utf8) = map.add_bytes(path.clone(), origin.clone(), &bytes);
        files.push(SourceEntry {
            file: id,
            kind: file.kind,
            digest: digest_bytes(&bytes),
            origin: origin.clone(),
        });
        let parse = file.kind == FileKind::Cpl || opts.include_work_files;
        let result = parse_file(&map, id, file.kind, utf8, parse, &mut parse_stage);
        if let ParsedFile::Module(module) = &result {
            check_module_location(module, path, &file.roots, sources, &mut resolve_stage);
        }
        parsed.push(result);
    }

    let mut diagnostics = project_stage;
    diagnostics.extend(parse_stage);
    diagnostics.extend(resolve_stage);
    Ok(Project {
        root,
        manifest,
        lock,
        manifest_digest,
        lock_digest,
        source_map: Arc::new(map),
        files,
        parsed,
        diagnostics,
    })
}

/// The `(file name, bytes)` of every std file in sorted name order.
fn read_std(std: &StdSource) -> Result<Vec<(String, Vec<u8>)>, LoadError> {
    let files: Vec<(String, Vec<u8>)> = match std {
        StdSource::Embedded => STD_FILES
            .iter()
            .map(|(name, text)| (name.to_string(), text.as_bytes().to_vec()))
            .collect(),
        StdSource::Dir(dir) => {
            let unreadable =
                |e: io::Error| LoadError::Std(format!("cannot read {}: {e}", dir.display()));
            let mut names = Vec::new();
            for entry in fs::read_dir(dir).map_err(unreadable)? {
                let entry = entry.map_err(unreadable)?;
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if is_cpl_name(&name) && entry.path().is_file() {
                    names.push(name);
                }
            }
            names.sort();
            let mut files = Vec::with_capacity(names.len());
            for name in names {
                let path = dir.join(&name);
                let bytes = fs::read(&path)
                    .map_err(|e| LoadError::Std(format!("cannot read {}: {e}", path.display())))?;
                files.push((name, bytes));
            }
            files
        }
    };
    if files.is_empty() {
        return Err(LoadError::Std(match std {
            StdSource::Embedded => "the embedded standard library has no files".to_string(),
            StdSource::Dir(dir) => format!("{} contains no .cpl files", dir.display()),
        }));
    }
    Ok(files)
}

/// Parse one registered file, appending its diagnostics: `Skipped` when
/// `parse` is false, `Failed` (with the `invalid_utf8` diagnostic) for bytes
/// that are not UTF-8, otherwise the parser's result.
fn parse_file(
    map: &SourceMap,
    file: FileId,
    kind: FileKind,
    utf8: Option<Diagnostic>,
    parse: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> ParsedFile {
    if !parse {
        return ParsedFile::Skipped;
    }
    if let Some(diagnostic) = utf8 {
        diagnostics.push(diagnostic);
        return ParsedFile::Failed;
    }
    let text = map.file(file).text();
    match kind {
        FileKind::Cpl => finish(parse_module(text, file), diagnostics, ParsedFile::Module),
        FileKind::Work => finish(parse_work(text, file), diagnostics, ParsedFile::Work),
        FileKind::Patch => finish(parse_patch(text, file), diagnostics, ParsedFile::Patch),
    }
}

fn finish<T>(
    result: ParseResult<T>,
    diagnostics: &mut Vec<Diagnostic>,
    wrap: fn(Arc<T>) -> ParsedFile,
) -> ParsedFile {
    diagnostics.extend(result.diagnostics);
    match result.value {
        Some(value) => wrap(Arc::new(value)),
        None => ParsedFile::Failed,
    }
}

/// `reserved_module_path` (semantics 1.1) and `module_file_misplaced`
/// (toolchain 3.7 rule 5, semantics 1.2) for one project module, both
/// spanning the header's module path (semantics S63 (3)). A file under
/// several source directories is conventional when it is so under any of
/// them; `expected` names the location under the first. A reserved header
/// is not also reported as misplaced: one diagnostic per mistake.
fn check_module_location(
    module: &ast::Module,
    path: &str,
    roots: &[usize],
    sources: &[String],
    out: &mut Vec<Diagnostic>,
) {
    let header = &module.header.path;
    let segments: Vec<&str> = header.segments.iter().map(|s| s.text.as_str()).collect();
    let Some(&first) = segments.first() else {
        return;
    };
    let dotted = header.dotted();
    if first == "std" || first == "gen" {
        let reason = if first == "std" {
            "module paths beginning with `std` are the standard library's".to_string()
        } else {
            "`gen` is the prelude's alias of `std.gen`".to_string()
        };
        out.push(
            Diagnostic::error(
                codes::RESERVED_MODULE_PATH,
                Category::Resolve,
                format!("module path `{dotted}` is reserved: {reason}"),
            )
            .with_span(header.span)
            .with_data("module", json!(dotted)),
        );
        return;
    }
    let conventional = roots.iter().any(|&i| {
        path.strip_prefix(source_prefix(&sources[i]).as_str())
            .is_some_and(|rel| is_conventional(rel, &segments))
    });
    if conventional {
        return;
    }
    let Some(&first_root) = roots.first() else {
        return;
    };
    let expected = format!(
        "{}{}.cpl",
        source_prefix(&sources[first_root]),
        segments.join("/")
    );
    out.push(
        Diagnostic::warning(
            codes::MODULE_FILE_MISPLACED,
            Category::Resolve,
            format!(
                "module `{dotted}` is conventionally located at `{expected}` (or in `{}/`); this file is `{path}`",
                expected.trim_end_matches(".cpl")
            ),
        )
        .with_span(header.span)
        .with_data("module", json!(dotted))
        .with_data("expected", json!(expected)),
    );
}

/// `a/b/c.cpl`, or `a/b/c/<name>.cpl`, for the module `a.b.c`; `rel` is the
/// path below the source directory.
fn is_conventional(rel: &str, segments: &[&str]) -> bool {
    let Some(stem) = rel.strip_suffix(".cpl") else {
        return false;
    };
    let parts: Vec<&str> = stem.split('/').collect();
    parts.as_slice() == segments
        || (parts.len() == segments.len() + 1 && parts[..segments.len()] == *segments)
}

/// `project_lock_toolchain_mismatch` (Section 11.2, reading).
fn toolchain_mismatch(lock: &Lock) -> Diagnostic {
    Diagnostic::warning(
        codes::PROJECT_LOCK_TOOLCHAIN_MISMATCH,
        codes::CATEGORY,
        format!(
            "cpl.lock was written by cpl {} for edition {}; this toolchain is cpl {} for edition {}",
            lock.toolchain.cpl,
            lock.toolchain.edition,
            version::TOOLCHAIN,
            version::EDITION
        ),
    )
    .with_data(
        "expected",
        json!({ "cpl": version::TOOLCHAIN, "edition": version::EDITION }),
    )
    .with_data(
        "found",
        json!({ "cpl": lock.toolchain.cpl, "edition": lock.toolchain.edition }),
    )
}
