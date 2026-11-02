//! `cpl check [--json] [--jsonl] [--release] [--project DIR] [--lint NAME]... [--deny-warnings] [--lock]`
//! (`spec/toolchain.md` 7.2; semantics S47, S48, S59).
//!
//! Loads the project with its `work/` files (parsed for syntax only,
//! semantics S53), runs `cpl_sema::check_with`, and adds what the checker
//! leaves to the command line through the semantic index
//! (`cpl_index::Index::complete_check_diagnostics`): every `hole`
//! diagnostic's `dependencies` and every `verification_result`
//! diagnostic's `receipt`. The output is `Index::check_output`. `--lock`
//! writes `cpl.lock` after a successful check.
//!
//! Human output: the diagnostics on stderr and one summary line on stdout,
//! `check: 0 errors, 2 warnings, 1 hole, 14 definitions in 3 modules`.
//! Exit 1 on any error (or warning with `--deny-warnings`), 2 when the
//! project cannot be loaded, else 0.

use super::{to_json, Ctx};
use crate::cli::CheckArgs;
use crate::codes;
use crate::output::{plural, Failure, Report};
use cpl_diag::json::{CheckOutput, Revision, Status};
use cpl_diag::{version, Diagnostic};
use cpl_index::Index;
use cpl_project::{render_lock, Lock, LockToolchain, Project};
use cpl_sema::CheckConfig;
use serde_json::json;
use std::path::Path;

pub fn run(ctx: &Ctx, args: &CheckArgs) -> Result<Report<CheckOutput>, Failure> {
    for lint in &args.lints {
        if !codes::LINTS.contains(&lint.as_str()) {
            return Err(ctx.session.refuse(
                codes::bad_argument(format!(
                    "--lint {lint}: unknown lint; the lints are {}",
                    codes::LINTS.join(", ")
                ))
                .with_data("lint", json!(lint)),
            ));
        }
    }
    let opened = ctx.session.open(true)?;
    let project = &opened.project;
    let config = CheckConfig {
        release: args.release,
        warnings_as_errors: args.deny_warnings,
        lints: args.lints.iter().cloned().collect(),
    };
    let result = cpl_sema::check_with(project, &config);
    let index = Index::from_check(&result, project, opened.revision.clone());
    let mut diagnostics = index.complete_check_diagnostics(&result.diagnostics, args.release);
    let output = index.check_output(&result, args.release);

    let failed = result.has_errors();
    let mut status = if failed {
        Status::Unsuccessful
    } else {
        Status::Ok
    };
    if args.lock && !failed {
        if let Err(d) = write_lock(&opened.root, project, &opened.revision) {
            diagnostics.push(d);
            status = Status::Error;
        }
    }
    let summary = &output.summary;
    let stdout = format!(
        "check: {}, {}, {}, {} in {}\n",
        plural(summary.errors.into(), "error", "errors"),
        plural(summary.warnings.into(), "warning", "warnings"),
        plural(summary.holes.into(), "hole", "holes"),
        plural(summary.definitions.into(), "definition", "definitions"),
        plural(summary.modules.into(), "module", "modules"),
    );
    Ok(Report::new(
        "check",
        Some(opened.revision.clone()),
        status,
        to_json(&diagnostics, &project.source_map),
        output,
    )
    .with_stdout(stdout)
    .with_sources(project.source_map.clone()))
}

/// Write `cpl.lock` for the current tree (Section 11.2): the toolchain and
/// std digest, the digest of every source-set file (the receipt inputs'
/// `sources`), the manifest's `[verification]`, and the reserved sections
/// of a present lock carried through.
fn write_lock(root: &Path, project: &Project, revision: &Revision) -> Result<(), Diagnostic> {
    save_lock(root, &current_lock(project, revision))
}

/// The lock of the current tree (Section 11.2) with the reserved sections of a present lock carried through.
pub(crate) fn current_lock(project: &Project, revision: &Revision) -> Lock {
    let carried = project.lock.as_ref();
    Lock {
        version: 1,
        toolchain: LockToolchain {
            cpl: version::TOOLCHAIN.to_string(),
            edition: version::EDITION.to_string(),
            std: project.std_digest(),
        },
        sources: cpl_index::receipt_inputs(project, revision.clone()).sources,
        verification: project.manifest.verification.clone(),
        foreign: carried.map(|l| l.foreign.clone()).unwrap_or_default(),
        adapters: carried.map(|l| l.adapters.clone()).unwrap_or_default(),
        targets: carried.map(|l| l.targets.clone()).unwrap_or_default(),
    }
}

/// Write `cpl.lock` under `root`.
pub(crate) fn save_lock(root: &Path, lock: &Lock) -> Result<(), Diagnostic> {
    let path = root.join("cpl.lock");
    std::fs::write(&path, render_lock(lock)).map_err(|e| {
        codes::internal(
            "io",
            format!("cpl.lock: cannot write {}: {e}", path.display()),
        )
        .with_data("path", json!("cpl.lock"))
    })
}
