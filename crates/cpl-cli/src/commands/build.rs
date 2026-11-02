//! `cpl build [--target c|python] [--release] [--profile optimized|debug] [--sanitize LIST] [--artifact KIND]...
//! [--out DIR] [--lock] [--cc PATH] [--python PATH] [--jobs N] [--json]` (`spec/toolchain.md` 7.2, T72;
//! `spec/cabi.md` Section 1; `spec/python.md` Section 1).
//!
//! Checks the project (any error: exit 1, nothing is built; under `--release` the release check, with
//! `build_lock_missing` in place of the warning `project_lock_missing`), refuses a project without the target in
//! `[interop] targets` (`build_target_not_enabled`, exit 1), reports the interop plan's
//! `project_export_skipped_generic` infos, and runs `cpl_emit_c::package::build` (C) or `build_python` (Python, with
//! the interpreter of `--python`, else `CPL_PYTHON`, else `python3`). An environment that cannot do what was asked (no
//! or an unsupported compiler, target, sanitizer, or Python) exits 2; a failure of the project's own inputs exits 1.

use super::Ctx;
use crate::cli::BuildArgs;
use crate::codes;
use crate::output::{Failure, Report};
use cpl_diag::json::{BuildOutput, Status};
use cpl_diag::{Category, Diagnostic, Severity};
use cpl_emit_c::cc::{Profile, Sanitizer};
use cpl_emit_c::package::{ArtifactKind, BuildConfig, PythonConfig};
use cpl_sema::CheckConfig;
use serde_json::json;

fn exit_status(ds: &[Diagnostic]) -> Status {
    let environment = [
        "build_cc_unavailable",
        "build_cc_unsupported",
        "build_target_unsupported",
        "build_sanitizer_unavailable",
        "build_python_unavailable",
    ];
    if ds
        .iter()
        .any(|d| d.category == Category::Internal || environment.contains(&d.code.0))
    {
        Status::Error
    } else {
        Status::Unsuccessful
    }
}

pub fn run(ctx: &Ctx, args: &BuildArgs) -> Result<Report<BuildOutput>, Failure> {
    let target = match args.target.as_str() {
        "c" | "" => "c",
        "python" => "python",
        "node" => {
            let d = codes::usage(
                codes::USAGE_NOT_IMPLEMENTED,
                "cpl build --target node is not implemented in this edition",
            )
            .with_note("the node target arrives with Milestone 4", None);
            return Err(ctx.session.refuse(d));
        }
        other => {
            return Err(ctx.session.refuse(
                codes::bad_argument(format!("--target {other}: the target is c or python"))
                    .with_data("target", json!(other)),
            ))
        }
    };
    if target == "python" && !args.artifact.is_empty() {
        return Err(ctx.session.refuse(codes::bad_argument(
            "--artifact applies to --target c: the Python target builds its package and wheel",
        )));
    }
    let sanitize: Vec<Sanitizer> = match args.sanitize.as_deref() {
        None | Some("") => Vec::new(),
        Some("address") => vec![Sanitizer::Address],
        Some("undefined") => vec![Sanitizer::Undefined],
        Some("address,undefined") | Some("undefined,address") => {
            vec![Sanitizer::Address, Sanitizer::Undefined]
        }
        Some("thread") => vec![Sanitizer::Thread],
        Some(other) => {
            return Err(ctx.session.refuse(
                codes::bad_argument(format!(
                    "--sanitize {other}: address and undefined (together or alone), or thread alone"
                ))
                .with_data("sanitize", json!(other)),
            ))
        }
    };
    if args.jobs == Some(0) {
        return Err(ctx
            .session
            .refuse(codes::bad_argument("--jobs 0: at least one job")));
    }
    let opened = ctx.session.open(false)?;
    let project = &opened.project;
    let revision = Some(opened.revision.clone());
    let sm = project.source_map.clone();
    let checked = cpl_sema::check_with(
        project,
        &CheckConfig {
            release: args.release,
            ..CheckConfig::default()
        },
    );
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for d in &checked.diagnostics {
        if d.severity == Severity::Info {
            continue;
        }
        if args.release && d.code.0 == "project_lock_missing" {
            diagnostics.push(Diagnostic::error(
                cpl_emit_c::codes::BUILD_LOCK_MISSING,
                Category::Build,
                format!(
                    "cpl build --release needs cpl.lock with its targets.{target} entries; write it with cpl build{} --lock",
                    if target == "c" { "" } else { " --target python" }
                ),
            ));
            continue;
        }
        diagnostics.push(d.clone());
    }
    if args.release
        && project.lock.as_ref().is_some_and(|l| {
            if target == "c" {
                l.target_c().is_none()
            } else {
                l.target_python().is_none()
            }
        })
        && !diagnostics
            .iter()
            .any(|d| d.code == cpl_emit_c::codes::BUILD_LOCK_MISSING)
    {
        diagnostics.push(Diagnostic::error(
            cpl_emit_c::codes::BUILD_LOCK_MISSING,
            Category::Build,
            format!(
                "cpl build --release needs cpl.lock with its targets.{target} entries; write them with cpl build{} --lock",
                if target == "c" { "" } else { " --target python" }
            ),
        ));
    }
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(Failure::new(
            Status::Unsuccessful,
            revision,
            &diagnostics,
            Some(sm),
        ));
    }
    let targets = &project.manifest.interop.targets;
    if !targets.iter().any(|t| t == target) {
        diagnostics.push(
            Diagnostic::error(
                cpl_emit_c::codes::BUILD_TARGET_NOT_ENABLED,
                Category::Build,
                format!("the project does not list \"{target}\" in [interop] targets"),
            )
            .with_data("targets", json!(targets)),
        );
        return Err(Failure::new(
            Status::Unsuccessful,
            revision,
            &diagnostics,
            Some(sm),
        ));
    }
    let program = &checked.program;
    let plan = checked
        .interop
        .clone()
        .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
    diagnostics.extend(plan.skipped_diagnostics(program));
    let root = project
        .root
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("cpl-build"));
    let out_dir = match &args.out {
        Some(o) if o.is_absolute() => o.clone(),
        Some(o) => ctx.session.cwd.join(o),
        None => root.join(".cpl").join("build").join(target),
    };
    let artifacts: Vec<ArtifactKind> = if args.artifact.is_empty() {
        project
            .manifest
            .interop_c
            .as_ref()
            .map(|c| c.artifacts.clone())
            .unwrap_or_else(|| vec![ArtifactKind::Shared, ArtifactKind::Static])
    } else {
        args.artifact
            .iter()
            .map(|a| match a.as_str() {
                "shared" => ArtifactKind::Shared,
                "static" => ArtifactKind::Static,
                _ => ArtifactKind::Executable,
            })
            .collect()
    };
    let cfg = BuildConfig {
        out_dir,
        cache_dir: root.join(".cpl").join("cache").join("c"),
        profile: if args.profile == "debug" {
            Profile::Debug
        } else {
            Profile::Optimized
        },
        sanitize,
        artifacts,
        release: args.release,
        lock: args.lock,
        cc: args.cc.clone(),
        jobs: args
            .jobs
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get())),
        runtime: match std::env::var_os("CPL_RUNTIME_DIR") {
            Some(dir) => cpl_rt::RuntimeSource::Dir(dir.into()),
            None => cpl_rt::RuntimeSource::Embedded,
        },
        werror: false,
    };
    let result = if target == "python" {
        let interpreter = args
            .python
            .clone()
            .or_else(|| {
                std::env::var_os("CPL_PYTHON")
                    .filter(|v| !v.is_empty())
                    .map(Into::into)
            })
            .unwrap_or_else(|| "python3".into());
        cpl_emit_c::package::build_python(
            project,
            program,
            &plan,
            &cfg,
            &PythonConfig { interpreter },
        )
    } else {
        cpl_emit_c::package::build(project, program, &plan, &cfg)
    };
    match result {
        Ok(built) => {
            diagnostics.extend(built.diagnostics);
            let mut output = built.output;
            if args.lock {
                // the lock of the tree with this build's entries (Section 11.2, T75; spec/python.md 9)
                let mut lock = super::check::current_lock(project, &opened.revision);
                match built.lock.python {
                    Some(python) => {
                        lock.set_target_python(Some(python.target));
                        lock.set_python_adapters(python.adapters);
                    }
                    None => {
                        lock.set_target_c(Some(built.lock.target));
                        lock.set_c_adapters(built.lock.adapters);
                        lock.set_foreign_c(built.lock.foreign);
                    }
                }
                if let Err(d) = super::check::save_lock(&opened.root, &lock) {
                    diagnostics.push(d);
                    return Err(Failure::new(
                        Status::Error,
                        revision,
                        &diagnostics,
                        Some(sm),
                    ));
                }
                output.lock.written = true;
                let json = serde_json::to_string_pretty(&output).expect("BuildOutput serializes");
                let _ = std::fs::write(cfg.out_dir.join("build.json"), json + "\n");
            }
            let mut stdout = String::new();
            for a in &output.artifacts {
                stdout.push_str(&format!("{}  {}\n", a.kind, a.path));
            }
            let mut what = format!("{target}, {}", output.profile);
            if output.release {
                what.push_str(", release");
            }
            if !output.sanitize.is_empty() {
                what.push_str(&format!(", sanitize {}", output.sanitize.join(",")));
            }
            stdout.push_str(&format!(
                "build: {} ({what}): {} exports, {} bindings, {} artifacts in {}\n",
                cpl_emit_c::unit_prefix(&plan),
                output.exports.len(),
                output.bindings.len(),
                output.artifacts.len(),
                output.out_dir
            ));
            let json = diagnostics.iter().map(|d| d.to_json(&sm)).collect();
            Ok(Report::new("build", revision, Status::Ok, json, output)
                .with_stdout(stdout)
                .with_sources(sm))
        }
        Err(ds) => {
            let status = exit_status(&ds);
            diagnostics.extend(ds);
            Err(Failure::new(status, revision, &diagnostics, Some(sm)))
        }
    }
}
