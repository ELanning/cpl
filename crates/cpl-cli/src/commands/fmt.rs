//! `cpl fmt [--check] [--stdin KIND] [paths]` (`spec/toolchain.md` 7.2,
//! 2.4, T7, T14).
//!
//! | Form | Files |
//! | - | - |
//! | `cpl fmt` | every `.cpl`, `.work`, and `.patch` file of the project's source set, in path order |
//! | `cpl fmt paths...` | the named files, and every `.cpl`/`.work`/`.patch` file under the named directories (recursively, not through directory links, as T54 reads a source tree); no project required |
//! | `--stdin KIND` | one file read from stdin, formatted to stdout; paths are refused |
//!
//! A file is rewritten only when formatting changes it; `--check` writes
//! nothing and lists the files that would change on stdout. A file that
//! does not parse (or is not UTF-8) is reported with its parse diagnostic,
//! left untouched (T7), and makes the exit code 1; so does any file
//! `--check` would change. A path that does not exist, a named file of
//! another kind, and `--stdin` with paths are `usage_bad_argument` (exit 2).
//!
//! Paths in the output are project-relative when the file is inside the
//! located project, else relative to the current directory when under it,
//! else absolute. The envelope's revision is the project's when a project
//! is located and loads (a refused path included), and `null` otherwise
//! (loose files, `--stdin`).
//!
//! Reading of 7.1 and 7.2: `--stdin` without `--check` writes the
//! formatted text to stdout, which `--json` reserves for exactly one
//! envelope; the two together are `usage_bad_argument` (with `--check`,
//! nothing but the envelope is printed, so `--stdin --check --json` is
//! accepted).

use super::Ctx;
use crate::cli::{FmtArgs, StdinKind};
use crate::codes;
use crate::output::{Failure, Report};
use crate::project::absolute;
use cpl_diag::json::{FmtFileJson, FmtOutput, FmtResult, Status};
use cpl_diag::{Category, Diagnostic, FileId, FileOrigin, SourceMap};
use cpl_project::ParsedFile;
use cpl_syntax::{format_source, FileKind};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The source-map path of the text read by `--stdin` (a virtual file,
/// Section 1.4).
pub const STDIN_PATH: &str = "<virtual:stdin>";

pub fn run(ctx: &Ctx, args: &FmtArgs) -> Result<Report<FmtOutput>, Failure> {
    if let Some(kind) = args.stdin {
        if !args.paths.is_empty() {
            return Err(Failure::error(codes::bad_argument(
                "--stdin reads one file from stdin and takes no paths",
            )));
        }
        if ctx.json() && !args.check {
            return Err(Failure::error(codes::bad_argument(
                "--stdin writes the formatted text to stdout, which --json reserves for the envelope; add --check to report only whether it would change",
            )));
        }
        return stdin(kind, args.check);
    }
    if args.paths.is_empty() {
        project(ctx, args.check)
    } else {
        paths(ctx, args)
    }
}

/// The outcome of formatting one file.
struct Formatted {
    result: FmtResult,
    /// The new text when formatting changed the file.
    text: Option<String>,
    diagnostics: Vec<Diagnostic>,
}

/// Format the text of `file` (`sm` holds it); `utf8` is the `invalid_utf8`
/// diagnostic of reading it, if any.
fn format_file(
    sm: &SourceMap,
    file: FileId,
    kind: FileKind,
    utf8: Option<Diagnostic>,
    check: bool,
) -> Formatted {
    if let Some(d) = utf8 {
        return Formatted {
            result: FmtResult::Error,
            text: None,
            diagnostics: vec![d],
        };
    }
    let text = sm.file(file).text();
    match format_source(text, kind, file) {
        Ok(formatted) if formatted == text => Formatted {
            result: FmtResult::Unchanged,
            text: None,
            diagnostics: Vec::new(),
        },
        Ok(formatted) => Formatted {
            result: if check {
                FmtResult::WouldFormat
            } else {
                FmtResult::Formatted
            },
            text: Some(formatted),
            diagnostics: Vec::new(),
        },
        Err(diagnostics) => Formatted {
            result: FmtResult::Error,
            text: None,
            diagnostics,
        },
    }
}

/// The envelope status of a file list: `unsuccessful` when any file would
/// change or failed, `error` when a write failed.
fn status_of(files: &[FmtFileJson], write_failed: bool) -> Status {
    if write_failed {
        Status::Error
    } else if files
        .iter()
        .any(|f| matches!(f.result, FmtResult::WouldFormat | FmtResult::Error))
    {
        Status::Unsuccessful
    } else {
        Status::Ok
    }
}

/// `--check`'s stdout: the files that would change, one per line.
fn check_listing(files: &[FmtFileJson]) -> String {
    files
        .iter()
        .filter(|f| f.result == FmtResult::WouldFormat)
        .map(|f| format!("{}\n", f.path))
        .collect()
}

fn write_error(path: &str, error: &std::io::Error) -> Diagnostic {
    codes::internal(
        "io",
        format!("{path}: cannot write the formatted file: {error}"),
    )
    .with_data("path", json!(path))
}

fn stdin(kind: StdinKind, check: bool) -> Result<Report<FmtOutput>, Failure> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .map_err(|e| Failure::error(codes::internal("io", format!("cannot read stdin: {e}"))))?;
    let mut sm = SourceMap::new();
    let (file, utf8) = sm.add_bytes(STDIN_PATH.to_string(), FileOrigin::Virtual, &bytes);
    let formatted = format_file(&sm, file, kind.file_kind(), utf8, check);
    let stdout = match (&formatted.result, check) {
        (FmtResult::Error, _) => String::new(),
        (FmtResult::WouldFormat, true) => format!("{STDIN_PATH}\n"),
        (_, true) => String::new(),
        (_, false) => formatted
            .text
            .clone()
            .unwrap_or_else(|| sm.file(file).text().to_string()),
    };
    let files = vec![FmtFileJson {
        path: STDIN_PATH.to_string(),
        result: formatted.result,
    }];
    let status = status_of(&files, false);
    let diagnostics = formatted
        .diagnostics
        .iter()
        .map(|d| d.to_json(&sm))
        .collect();
    Ok(
        Report::new("fmt", None, status, diagnostics, FmtOutput { check, files })
            .with_stdout(stdout)
            .with_sources(Arc::new(sm)),
    )
}

fn project(ctx: &Ctx, check: bool) -> Result<Report<FmtOutput>, Failure> {
    let opened = ctx.session.open(true)?;
    let project = &opened.project;
    let sm = &project.source_map;
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    let mut write_failed = false;
    for (entry, parsed) in project.files.iter().zip(&project.parsed) {
        if entry.origin == FileOrigin::Std {
            continue;
        }
        let path = sm.file(entry.file).path.clone();
        let formatted = if matches!(parsed, ParsedFile::Failed) {
            // Its one diagnostic (D59, D70), raised by loading.
            Formatted {
                result: FmtResult::Error,
                text: None,
                diagnostics: project
                    .diagnostics
                    .iter()
                    .filter(|d| {
                        d.category == Category::Parse && d.span.map(|s| s.file) == Some(entry.file)
                    })
                    .cloned()
                    .collect(),
            }
        } else {
            format_file(sm, entry.file, entry.kind, None, check)
        };
        let mut result = formatted.result;
        if let (Some(text), false) = (&formatted.text, check) {
            if let Err(e) = std::fs::write(opened.root.join(&path), text) {
                diagnostics.push(write_error(&path, &e));
                result = FmtResult::Error;
                write_failed = true;
            }
        }
        diagnostics.extend(formatted.diagnostics);
        files.push(FmtFileJson { path, result });
    }
    let status = status_of(&files, write_failed);
    let stdout = if check {
        check_listing(&files)
    } else {
        String::new()
    };
    Ok(Report::new(
        "fmt",
        Some(opened.revision.clone()),
        status,
        diagnostics.iter().map(|d| d.to_json(sm)).collect(),
        FmtOutput { check, files },
    )
    .with_stdout(stdout)
    .with_sources(sm.clone()))
}

/// One file named on the command line or found under a named directory.
struct Named {
    /// Where to read and write it.
    path: PathBuf,
    /// Its path in the output.
    display: String,
    kind: FileKind,
}

fn paths(ctx: &Ctx, args: &FmtArgs) -> Result<Report<FmtOutput>, Failure> {
    let root = ctx.session.locate_optional()?;
    // Formatting named files needs no project, so one that is not located
    // or does not load only leaves the revision `null`.
    let revision = ctx.session.revision();
    let named = collect(&ctx.session.cwd, root.as_deref(), &args.paths)
        .map_err(|failure| failure.with_revision(revision.clone()))?;
    let mut sm = SourceMap::new();
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    let mut write_failed = false;
    for file in &named {
        let bytes = std::fs::read(&file.path).map_err(|e| {
            Failure::error(
                codes::internal("io", format!("{}: cannot read: {e}", file.display))
                    .with_data("path", json!(file.display)),
            )
            .with_revision(revision.clone())
        })?;
        let (id, utf8) = sm.add_bytes(file.display.clone(), FileOrigin::Project, &bytes);
        let formatted = format_file(&sm, id, file.kind, utf8, args.check);
        let mut result = formatted.result;
        if let (Some(text), false) = (&formatted.text, args.check) {
            if let Err(e) = std::fs::write(&file.path, text) {
                diagnostics.push(write_error(&file.display, &e));
                result = FmtResult::Error;
                write_failed = true;
            }
        }
        diagnostics.extend(formatted.diagnostics);
        files.push(FmtFileJson {
            path: file.display.clone(),
            result,
        });
    }
    let status = status_of(&files, write_failed);
    let stdout = if args.check {
        check_listing(&files)
    } else {
        String::new()
    };
    let diagnostics = diagnostics.iter().map(|d| d.to_json(&sm)).collect();
    Ok(Report::new(
        "fmt",
        revision,
        status,
        diagnostics,
        FmtOutput {
            check: args.check,
            files,
        },
    )
    .with_stdout(stdout)
    .with_sources(Arc::new(sm)))
}

/// The files the named paths cover, by display path (each once).
fn collect(cwd: &Path, root: Option<&Path>, paths: &[PathBuf]) -> Result<Vec<Named>, Failure> {
    let root = root.and_then(|r| r.canonicalize().ok());
    let cwd_real = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut found: BTreeMap<String, Named> = BTreeMap::new();
    for given in paths {
        let full = absolute(cwd, given);
        let meta = std::fs::metadata(&full).map_err(|e| {
            Failure::error(
                codes::bad_argument(format!("{}: {e}", given.display()))
                    .with_data("path", json!(given.display().to_string())),
            )
        })?;
        let mut files = Vec::new();
        if meta.is_dir() {
            walk(&full, &mut files).map_err(|e| {
                Failure::error(codes::internal(
                    "io",
                    format!("{}: cannot read the directory: {e}", given.display()),
                ))
            })?;
        } else {
            match FileKind::from_path(&full) {
                Some(kind) => files.push((full.clone(), kind)),
                None => {
                    return Err(Failure::error(
                        codes::bad_argument(format!(
                            "{}: not a .cpl, .work, or .patch file",
                            given.display()
                        ))
                        .with_data("path", json!(given.display().to_string())),
                    ))
                }
            }
        }
        for (path, kind) in files {
            let display = display_path(&path, root.as_deref(), &cwd_real);
            found.entry(display.clone()).or_insert(Named {
                path,
                display,
                kind,
            });
        }
    }
    Ok(found.into_values().collect())
}

/// Every `.cpl`, `.work`, and `.patch` file under `dir`, recursively; a link
/// to a directory is not followed (T54), a link to a file is that file,
/// and a file named just `.cpl` (no stem) is no source file (Section 0).
fn walk(dir: &Path, out: &mut Vec<(PathBuf, FileKind)>) -> std::io::Result<()> {
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk(&path, out)?;
            continue;
        }
        let Some(kind) = FileKind::from_path(&path) else {
            continue;
        };
        let has_stem = entry.file_name().len() > kind.extension().len() + 1;
        let is_file = file_type.is_file()
            || (file_type.is_symlink() && std::fs::metadata(&path).is_ok_and(|m| m.is_file()));
        if has_stem && is_file {
            out.push((path, kind));
        }
    }
    Ok(())
}

/// The output path of a file: project-relative inside the project root,
/// else relative to the current directory under it, else absolute; with
/// `/` separators.
fn display_path(path: &Path, root: Option<&Path>, cwd: &Path) -> String {
    // The directory is resolved, the file name kept: a link to a file is
    // shown under its own name.
    let real = match (
        path.parent().and_then(|d| d.canonicalize().ok()),
        path.file_name(),
    ) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    };
    let shown = root
        .and_then(|r| real.strip_prefix(r).ok())
        .or_else(|| real.strip_prefix(cwd).ok())
        .unwrap_or(&real);
    shown.to_string_lossy().replace('\\', "/")
}
