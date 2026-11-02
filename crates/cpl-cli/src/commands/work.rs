//! `cpl work check <file> [--json]` (`spec/toolchain.md` 7.2), and the
//! reserved `cpl work run <file>` and `cpl work integrate <change>`
//! (Milestone 6, Section 10).
//!
//! `work check` parses `<file>` as a `.work` file, loads and checks the
//! project, and validates the declaration against the checked program with
//! the project's `[work.policy]` (`cpl_work::validate_work_file_with_policy`,
//! whose `WorkCheckOutput` is the envelope's result). `base` and `target`
//! are resolved in the project's repository; without one, or without `git`,
//! resolution is skipped with the warning `git_unavailable` (`reason`:
//! `no_repository` or `not_installed`), the one command that reports it.
//!
//! | Outcome | Code | Exit |
//! | - | - | - |
//! | the declaration is valid (warnings allowed) | the validation's warnings | 0 |
//! | it breaks a rule of the 7.2 table | the validation's diagnostics | 1 |
//! | the file does not parse | its parse diagnostic | 1 |
//! | the project does not check | the check's errors and warnings; nothing is validated | 1 |
//! | the file does not exist or is a directory | `usage_bad_argument` (`path`) | 2 |
//! | the file or the project cannot be read | `internal_error` (`io`), the load failures of 7.1 | 2 |
//!
//! The file may be any path: a member of the source set (`work/*.work`) is
//! the project's own parse of it, and any other file is read, registered
//! beside the project's files, and parsed on its own; its `file` is shown
//! project-relative when it lies under the project root, else relative to
//! the current directory when under it, else absolute (as `cpl fmt` shows
//! named files).
//!
//! Human output: the diagnostics on stderr, then `work check: <file>: N
//! changes, M tasks, ok` on stdout (`..., K errors` when the declaration
//! has errors).
//!
//! Readings: like `cpl run`, `cpl test`, and `cpl prove`, validation needs
//! a project that checks without errors (selectors resolve against the
//! checked program, and one in which a file failed to parse has no
//! definitions at all, semantics S52, so every selector would be a
//! cascade, S62); the check's diagnostics of `.work` and `.patch` files are
//! not the program's (semantics S53), so another work file that does not
//! parse does not stop the validation of this one. With no runnable `git`,
//! the validator is handed the project root as the repository, so that its
//! first git command reports `not_installed` (the API's way to tell "no
//! git" from "no repository").

use super::{check_diagnostics_for_execution, Ctx};
use crate::cli::WorkFileArgs;
use crate::codes;
use crate::output::{plural, Failure, Report};
use crate::project::{absolute, real_path, RepoState};
use cpl_diag::json::{NoOutput, Status, WorkCheckOutput};
use cpl_diag::{Category, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_index::GitRepo;
use cpl_project::{ParsedFile, Project};
use cpl_sema::CheckConfig;
use cpl_syntax::{ast, FileKind};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

pub fn check(ctx: &Ctx, args: &WorkFileArgs) -> Result<Report<WorkCheckOutput>, Failure> {
    let opened = ctx.session.open(true)?;
    let project = &opened.project;
    let revision = Some(opened.revision.clone());
    let given = &args.file;
    let full = absolute(&ctx.session.cwd, given);
    let refused = |why: &str| bad_path(given, why).with_revision(revision.clone());
    let meta = std::fs::metadata(&full).map_err(|e| refused(&e.to_string()))?;
    if meta.is_dir() {
        return Err(refused("a directory, not a .work file"));
    }
    let named = name_file(&opened.root, &ctx.session.cwd, &full, project)?;
    let stop = |status, diagnostics: &[Diagnostic], sm: &Arc<SourceMap>| {
        Failure::new(status, revision.clone(), diagnostics, Some(sm.clone()))
    };

    // Parse.
    let (work, sm): (Arc<ast::WorkFile>, Arc<SourceMap>) = match &named {
        Named::Member { index, file } => match &project.parsed[*index] {
            ParsedFile::Work(work) => (work.clone(), project.source_map.clone()),
            ParsedFile::Failed => {
                let parse: Vec<Diagnostic> = project
                    .diagnostics
                    .iter()
                    .filter(|d| {
                        d.category == Category::Parse && d.span.map(|s| s.file) == Some(*file)
                    })
                    .cloned()
                    .collect();
                return Err(stop(Status::Unsuccessful, &parse, &project.source_map));
            }
            ParsedFile::Module(_) | ParsedFile::Patch(_) | ParsedFile::Skipped => {
                let text = project.source_map.file(*file).text().to_string();
                parse(project.source_map.clone(), *file, &text)
                    .map_err(|d| stop(Status::Unsuccessful, &d, &project.source_map))?
            }
        },
        Named::Other { display } => {
            let bytes = std::fs::read(&full).map_err(|e| {
                Failure::error(
                    codes::internal("io", format!("{display}: cannot read: {e}"))
                        .with_data("path", json!(display)),
                )
                .with_revision(revision.clone())
            })?;
            let mut map = (*project.source_map).clone();
            let (file, utf8) = map.add_bytes(display.clone(), FileOrigin::Project, &bytes);
            let map = Arc::new(map);
            if let Some(d) = utf8 {
                return Err(stop(Status::Unsuccessful, &[d], &map));
            }
            let text = map.file(file).text().to_string();
            parse(map.clone(), file, &text).map_err(|d| stop(Status::Unsuccessful, &d, &map))?
        }
    };

    // Check: the program the selectors resolve against.
    let checked = cpl_sema::check_with(project, &CheckConfig::default());
    let program_diagnostics: Vec<Diagnostic> = checked
        .diagnostics
        .iter()
        .filter(|d| !in_work_file(project, d))
        .cloned()
        .collect();
    if program_diagnostics.iter().any(Diagnostic::is_error) {
        let reported = check_diagnostics_for_execution(&program_diagnostics);
        return Err(stop(Status::Unsuccessful, &reported, &sm));
    }

    // Validate.
    let fallback;
    let repo: Option<&GitRepo> = match &opened.repo {
        RepoState::Found(repo) => Some(repo),
        RepoState::NoRepository => None,
        RepoState::GitMissing => {
            fallback = GitRepo {
                toplevel: opened.root.clone(),
                project_prefix: String::new(),
            };
            Some(&fallback)
        }
    };
    let result = cpl_work::validate_work_file_with_policy(
        &work,
        &checked.program,
        &project.manifest.work.policy,
        repo,
    );
    let mut output = result.output.clone();
    if let Named::Other { display } = &named {
        output.file = display.clone();
    }
    let status = if result.has_errors() {
        Status::Unsuccessful
    } else {
        Status::Ok
    };
    let tasks: usize = output.changes.iter().map(|c| c.tasks.len()).sum();
    let errors = result.diagnostics.iter().filter(|d| d.is_error()).count();
    let verdict = if errors == 0 {
        "ok".to_string()
    } else {
        plural(errors as u64, "error", "errors")
    };
    let stdout = format!(
        "work check: {}: {}, {}, {verdict}\n",
        output.file,
        plural(output.changes.len() as u64, "change", "changes"),
        plural(tasks as u64, "task", "tasks"),
    );
    let json = result.diagnostics.iter().map(|d| d.to_json(&sm)).collect();
    Ok(
        Report::new("work check", revision.clone(), status, json, output)
            .with_stdout(stdout)
            .with_sources(sm.clone()),
    )
}

/// `cpl work run` and `cpl work integrate`, reserved for Milestone 6
/// (Sections 7.1, 7.2): exit 2 with `usage_not_implemented` and a note
/// naming the milestone. Nothing is read: no project is needed.
pub fn reserved(name: &str) -> Result<Report<NoOutput>, Failure> {
    Err(Failure::error(
        codes::usage(
            codes::USAGE_NOT_IMPLEMENTED,
            format!("`cpl {name}` is not implemented"),
        )
        .with_note(
            "reserved for Milestone 6 (swarm integration, spec/toolchain.md Section 10)",
            None,
        ),
    ))
}

/// The work file named on the command line.
enum Named {
    /// A `.work` file of the source set: `project.files[index]`.
    Member { index: usize, file: FileId },
    /// Any other file, shown as `display`.
    Other { display: String },
}

/// Whether the named file is a member of the source set, and how to show
/// it otherwise (module docs).
fn name_file(root: &Path, cwd: &Path, full: &Path, project: &Project) -> Result<Named, Failure> {
    let real = real_path(full);
    let root_real = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if let Ok(relative) = real.strip_prefix(&root_real) {
        let relative = relative.to_string_lossy().replace('\\', "/");
        let member = project.files.iter().position(|entry| {
            entry.origin == FileOrigin::Project
                && entry.kind == FileKind::Work
                && project.source_map.file(entry.file).path == relative
        });
        return Ok(match member {
            Some(index) => Named::Member {
                index,
                file: project.files[index].file,
            },
            None => Named::Other { display: relative },
        });
    }
    let cwd_real = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let shown = real.strip_prefix(&cwd_real).unwrap_or(&real);
    let mut display = shown.to_string_lossy().replace('\\', "/");
    // A path relative to the current directory that spells a project file's
    // path would name that file in the source map: show it absolute.
    if project.source_map.find(&display).is_some() {
        display = real.to_string_lossy().replace('\\', "/");
    }
    Ok(Named::Other { display })
}

/// Parse `text` (the file `file` of `sm`) as a `.work` file: the tree, or
/// its parse diagnostic (D59).
fn parse(
    sm: Arc<SourceMap>,
    file: FileId,
    text: &str,
) -> Result<(Arc<ast::WorkFile>, Arc<SourceMap>), Vec<Diagnostic>> {
    let parsed = cpl_syntax::parse_work(text, file);
    match parsed.value {
        Some(work) if !parsed.has_errors() => Ok((Arc::new(work), sm)),
        _ => Err(parsed.diagnostics),
    }
}

/// A diagnostic located in a `.work` or `.patch` file of the project:
/// those files take no part in checking (semantics S53).
fn in_work_file(project: &Project, d: &Diagnostic) -> bool {
    d.span.is_some_and(|span| {
        project
            .files
            .iter()
            .any(|e| e.file == span.file && matches!(e.kind, FileKind::Work | FileKind::Patch))
    })
}

fn bad_path(given: &Path, why: &str) -> Failure {
    Failure::error(
        codes::bad_argument(format!("{}: {why}", given.display()))
            .with_data("path", json!(given.display().to_string())),
    )
}
