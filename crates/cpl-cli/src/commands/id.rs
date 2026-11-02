//! `cpl id assign [paths] [--check] [--json]` (`spec/toolchain.md` 4.2,
//! 7.2): insert an `@id("...")` line for every top-level declaration that
//! lacks one, with `cpl_index::plan_id_assignments` (ids from OS
//! randomness, Section 1.3) and `cpl_index::apply_id_assignments`.
//!
//! | Form | Files |
//! | - | - |
//! | `cpl id assign` | every `.cpl` file of the project's source set |
//! | `cpl id assign paths...` in a project | the named source files of the located project, and those under the named directories; a named path that is no `.cpl` file of the source set is `usage_bad_argument` |
//! | `cpl id assign paths...` outside any project | the named `.cpl` files and those under the named directories, loaded together as one project of their own (so an id is never drawn twice among them) |
//!
//! A file that does not parse is skipped with its parse diagnostic
//! (`skipped_files`, exit 1). `--check` writes nothing and exits 1 when
//! any declaration lacks an id. Human output: one line per assignment,
//! `src/reader/pages.cpl:21  @id("fn_k7w3qz2mb6ptx4ve")  reader.pages.clean`.

use super::Ctx;
use crate::cli::IdAssignArgs;
use crate::codes;
use crate::output::{Failure, Report};
use crate::project::{absolute, load_options};
use cpl_diag::json::{IdAssignOutput, IdAssignmentJson, Revision, SkippedFileJson, Status};
use cpl_diag::{Category, Diagnostic, FileId, FileOrigin};
use cpl_index::IdAssignment;
use cpl_project::{load_project_from_tree, ParsedFile, Project, VirtualTree};
use cpl_syntax::FileKind;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The manifest of a loose project (named files outside any project).
const LOOSE_MANIFEST: &str =
    "[project]\nname = \"loose\"\nedition = \"0.1\"\n\n[build]\nsources = [\".\"]\n";

pub fn run(ctx: &Ctx, args: &IdAssignArgs) -> Result<Report<IdAssignOutput>, Failure> {
    let scope = if args.paths.is_empty() {
        let opened = ctx.session.open(false)?;
        let files = project_cpl_files(&opened.project).into_keys().collect();
        Scope {
            project: opened.project,
            revision: Some(opened.revision),
            write_to: WriteTo::Root(opened.root),
            files,
        }
    } else {
        match ctx.session.locate_optional()? {
            Some(root) => in_project(ctx, &root, &args.paths)?,
            None => loose(&ctx.session.cwd, &args.paths)?,
        }
    };
    assign(scope, args.check)
}

/// Where the files of a scope live on disk.
enum WriteTo {
    /// Project-relative paths under this root.
    Root(PathBuf),
    /// The path of each file of a loose project.
    Paths(BTreeMap<FileId, PathBuf>),
}

/// The project an assignment runs over and the files it covers.
struct Scope {
    project: Project,
    revision: Option<Revision>,
    write_to: WriteTo,
    files: BTreeSet<FileId>,
}

/// Every `.cpl` file of the source set, by FileId, with its path.
fn project_cpl_files(project: &Project) -> BTreeMap<FileId, String> {
    project
        .files
        .iter()
        .filter(|e| e.origin != FileOrigin::Std && e.kind == FileKind::Cpl)
        .map(|e| (e.file, project.source_map.file(e.file).path.clone()))
        .collect()
}

fn assign(scope: Scope, check: bool) -> Result<Report<IdAssignOutput>, Failure> {
    let project = &scope.project;
    let sm = &project.source_map;
    let plan: Vec<IdAssignment> = cpl_index::plan_id_assignments(project, &mut rand::rng())
        .into_iter()
        .filter(|a| scope.files.contains(&a.file))
        .collect();

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut skipped = Vec::new();
    for (entry, parsed) in project.files.iter().zip(&project.parsed) {
        if scope.files.contains(&entry.file) && matches!(parsed, ParsedFile::Failed) {
            skipped.push(SkippedFileJson {
                path: sm.file(entry.file).path.clone(),
                reason: "syntax errors".to_string(),
            });
            diagnostics.extend(
                project
                    .diagnostics
                    .iter()
                    .filter(|d| {
                        d.category == Category::Parse && d.span.map(|s| s.file) == Some(entry.file)
                    })
                    .cloned(),
            );
        }
    }
    skipped.sort_by(|a, b| a.path.cmp(&b.path));

    let mut write_failed = false;
    if !check {
        let mut by_file: BTreeMap<FileId, Vec<IdAssignment>> = BTreeMap::new();
        for a in &plan {
            by_file.entry(a.file).or_default().push(a.clone());
        }
        for (file, assignments) in by_file {
            let source = sm.file(file);
            let text = cpl_index::apply_id_assignments(source.text(), &assignments);
            let target = match &scope.write_to {
                WriteTo::Root(root) => root.join(&source.path),
                WriteTo::Paths(paths) => paths[&file].clone(),
            };
            if let Err(e) = std::fs::write(&target, text) {
                write_failed = true;
                diagnostics.push(
                    codes::internal("io", format!("{}: cannot write: {e}", source.path))
                        .with_data("path", json!(source.path)),
                );
            }
        }
    }

    let mut assignments: Vec<IdAssignmentJson> = plan.iter().map(|a| a.to_json(sm)).collect();
    assignments.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let stdout = assignments
        .iter()
        .map(|a| format!("{}:{}  @id(\"{}\")  {}\n", a.path, a.line, a.id, a.symbol))
        .collect();
    let status = if write_failed {
        Status::Error
    } else if !skipped.is_empty() || (check && !assignments.is_empty()) {
        Status::Unsuccessful
    } else {
        Status::Ok
    };
    let json = diagnostics.iter().map(|d| d.to_json(sm)).collect();
    Ok(Report::new(
        "id assign",
        scope.revision,
        status,
        json,
        IdAssignOutput {
            check,
            assignments,
            skipped_files: skipped,
        },
    )
    .with_stdout(stdout)
    .with_sources(sm.clone()))
}

/// Named paths inside a located project: its source files they name or
/// contain.
fn in_project(ctx: &Ctx, root: &Path, paths: &[PathBuf]) -> Result<Scope, Failure> {
    let project = ctx.session.load(root, false)?;
    let (_, revision) = crate::project::working_revision(&project, root)?;
    // A path is refused in the project at this revision (1.4.2).
    let refused =
        |given: &Path, why: &str| bad_path(given, why).with_revision(Some(revision.clone()));
    let root_real = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let members: Vec<(PathBuf, FileId)> = project_cpl_files(&project)
        .into_iter()
        .map(|(file, path)| (root_real.join(path), file))
        .collect();
    let mut files = BTreeSet::new();
    for given in paths {
        let full = absolute(&ctx.session.cwd, given);
        let meta = std::fs::metadata(&full).map_err(|e| refused(given, &e.to_string()))?;
        let real = real_path(&full);
        if meta.is_dir() {
            files.extend(
                members
                    .iter()
                    .filter(|(path, _)| path.starts_with(&real))
                    .map(|(_, file)| *file),
            );
        } else {
            match members.iter().find(|(path, _)| *path == real) {
                Some((_, file)) => {
                    files.insert(*file);
                }
                None => {
                    return Err(refused(
                        given,
                        &format!("not a .cpl file of the project at {}", root.display()),
                    ))
                }
            }
        }
    }
    Ok(Scope {
        project,
        revision: Some(revision),
        write_to: WriteTo::Root(root.to_path_buf()),
        files,
    })
}

/// Named paths outside any project, loaded as one project of their own.
fn loose(cwd: &Path, paths: &[PathBuf]) -> Result<Scope, Failure> {
    let cwd_real = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut named: BTreeMap<String, PathBuf> = BTreeMap::new();
    for given in paths {
        let full = absolute(cwd, given);
        let meta = std::fs::metadata(&full).map_err(|e| bad_path(given, &e.to_string()))?;
        let mut found = Vec::new();
        if meta.is_dir() {
            cpl_files_under(&full, &mut found).map_err(|e| {
                Failure::error(codes::internal(
                    "io",
                    format!("{}: cannot read the directory: {e}", given.display()),
                ))
            })?;
        } else if FileKind::from_path(&full) == Some(FileKind::Cpl) {
            found.push(full);
        } else {
            return Err(bad_path(given, "not a .cpl file"));
        }
        for path in found {
            let real = real_path(&path);
            let shown = real.strip_prefix(&cwd_real).unwrap_or(&real);
            named.insert(shown.to_string_lossy().replace('\\', "/"), path);
        }
    }
    let mut tree = VirtualTree::default();
    tree.files
        .insert("cpl.toml".to_string(), LOOSE_MANIFEST.as_bytes().to_vec());
    for (shown, path) in &named {
        let bytes = std::fs::read(path).map_err(|e| {
            Failure::error(
                codes::internal("io", format!("{shown}: cannot read: {e}"))
                    .with_data("path", json!(shown)),
            )
        })?;
        tree.files.insert(shown.clone(), bytes);
    }
    let project = load_project_from_tree(&tree, FileOrigin::Virtual, &load_options(false))
        .map_err(|e| {
            Failure::error(crate::project::load_error(
                &e,
                crate::project::LoadContext::WorkingTree,
            ))
        })?;
    let files = project_cpl_files(&project);
    let write_to = files
        .iter()
        .filter_map(|(file, shown)| named.get(shown).map(|path| (*file, path.clone())))
        .collect();
    Ok(Scope {
        files: files.into_keys().collect(),
        project,
        revision: None,
        write_to: WriteTo::Paths(write_to),
    })
}

/// A path with its directory resolved and its file name kept (a link to a
/// file is named by its own name).
fn real_path(path: &Path) -> PathBuf {
    match (
        path.parent().and_then(|d| d.canonicalize().ok()),
        path.file_name(),
    ) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
    }
}

/// Every `.cpl` file under `dir` (recursively, not through directory links).
fn cpl_files_under(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            cpl_files_under(&path, out)?;
        } else if FileKind::from_path(&path) == Some(FileKind::Cpl)
            && entry.file_name().len() > ".cpl".len()
            && (file_type.is_file() || std::fs::metadata(&path).is_ok_and(|m| m.is_file()))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn bad_path(given: &Path, why: &str) -> Failure {
    Failure::error(
        codes::bad_argument(format!("{}: {why}", given.display()))
            .with_data("path", json!(given.display().to_string())),
    )
}
