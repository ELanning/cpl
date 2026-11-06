//! `cpl build [--target c|python|rust|go|java] [--release] [--profile optimized|debug] [--sanitize LIST]
//! [--artifact KIND]... [--out DIR] [--lock] [--cc PATH] [--python PATH] [--jobs N] [--json]` (`spec/toolchain.md`
//! 7.2, T72; `spec/cabi.md` Section 1; `spec/python.md` Section 1; `spec/hosts.md`).
//!
//! Checks the project (any error: exit 1, nothing is built; under `--release` the release check, with
//! `build_lock_missing` in place of the warning `project_lock_missing`), refuses a project without the target in
//! `[interop] targets` (`build_target_not_enabled`, exit 1), reports the interop plan's
//! `project_export_skipped_generic` infos, and runs `cpl_emit_c::package::build` (C) or `build_python` (Python, with
//! the interpreter of `--python`, else `CPL_PYTHON`, else `python3`). The source targets `rust`, `go`, and `java`
//! (`kotlin` and `jvm` name `java`) write the host package of `cpl_emit_host` (`spec/hosts.md`) and compile nothing.
//! An environment that cannot do what was asked (no or an unsupported compiler, target, sanitizer, or Python) exits
//! 2; a failure of the project's own inputs exits 1.

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
        "rust" => "rust",
        "go" => "go",
        "java" | "jvm" | "kotlin" => "java",
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
                codes::bad_argument(format!(
                    "--target {other}: the target is c, python, rust, go, or java"
                ))
                .with_data("target", json!(other)),
            ))
        }
    };
    if target == "python" && !args.artifact.is_empty() {
        return Err(ctx.session.refuse(codes::bad_argument(
            "--artifact applies to --target c: the Python target builds its package and wheel",
        )));
    }
    let source = matches!(target, "rust" | "go" | "java");
    if source {
        let flag = if !args.artifact.is_empty() {
            Some("--artifact")
        } else if args.sanitize.as_deref().is_some_and(|s| !s.is_empty()) {
            Some("--sanitize")
        } else if args.lock {
            Some("--lock")
        } else {
            None
        };
        if let Some(flag) = flag {
            return Err(ctx.session.refuse(codes::bad_argument(format!(
                "{flag} does not apply to --target {target}: the build writes the {target} package, which the host's own toolchain compiles"
            ))));
        }
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
        && !source
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
    let enabled = |t: &String| t == target || (target == "java" && t == "kotlin");
    if !targets.iter().any(enabled) {
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
    if source {
        return build_source(
            target,
            args,
            project,
            program,
            &out_dir,
            diagnostics,
            revision,
            sm,
        );
    }
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

/// `--target rust|go|java`: the host package of `spec/hosts.md`, written under `out_dir`.
#[allow(clippy::too_many_arguments)]
fn build_source(
    target: &str,
    args: &BuildArgs,
    project: &cpl_project::Project,
    program: &cpl_hir::Program,
    out_dir: &std::path::Path,
    mut diagnostics: Vec<Diagnostic>,
    revision: Option<cpl_diag::json::Revision>,
    sm: std::sync::Arc<cpl_diag::SourceMap>,
) -> Result<Report<BuildOutput>, Failure> {
    use cpl_diag::json::{
        BuildArtifact, BuildExport, BuildLock, CompilerInfo, Digest, RuntimeInfo,
    };
    let started = std::time::Instant::now();
    let host = cpl_emit_host::Target::parse(target).expect("a source target");
    let name = project.manifest.project.name.clone();
    let package = match cpl_emit_host::emit(
        program,
        host,
        &cpl_emit_host::Options {
            name: name.clone(),
            driver: false,
        },
    ) {
        Ok(p) => p,
        Err(e) => {
            diagnostics.push(Diagnostic::error(
                cpl_emit_c::codes::BUILD_TARGET_UNSUPPORTED,
                Category::Build,
                e,
            ));
            return Err(Failure::new(
                Status::Unsuccessful,
                revision,
                &diagnostics,
                Some(sm),
            ));
        }
    };
    if let Err(e) = package.write(out_dir) {
        diagnostics.push(
            Diagnostic::error(
                cpl_emit_c::codes::BUILD_TARGET_UNSUPPORTED,
                Category::Build,
                format!(
                    "cannot write the {target} package to {}: {e}",
                    out_dir.display()
                ),
            )
            .with_data("out_dir", json!(out_dir.display().to_string())),
        );
        return Err(Failure::new(
            Status::Error,
            revision,
            &diagnostics,
            Some(sm),
        ));
    }
    let digest = |text: &str| Digest(blake3::hash(text.as_bytes()).to_hex()[..32].to_string());
    let shown = |path: &std::path::Path| match &project.root {
        Some(root) => path
            .strip_prefix(root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.display().to_string()),
        None => path.display().to_string(),
    };
    let artifacts: Vec<BuildArtifact> = package
        .files
        .iter()
        .map(|(path, text)| BuildArtifact {
            kind: "source".into(),
            path: shown(&out_dir.join(path)),
            digest: digest(text),
        })
        .collect();
    // each export's definition by its qualified name, the first of the program's order
    let mut by_path = std::collections::HashMap::new();
    for d in program.defs() {
        by_path.entry(program.display_path(d.id)).or_insert(d);
    }
    let exports: Vec<BuildExport> = package
        .exports
        .iter()
        .filter_map(|f| {
            let d = *by_path.get(f)?;
            Some(BuildExport {
                symbol: f.clone(),
                function: f.clone(),
                id: d.stable_id.clone(),
                signature: digest(&program.display_ty(&cpl_hir::Ty::Error)),
                int: "bigint".into(),
                effects: Vec::new(),
            })
        })
        .collect();
    let all: String = package.files.values().map(String::as_str).collect();
    let output = BuildOutput {
        target: target.to_string(),
        release: args.release,
        profile: "source".into(),
        sanitize: Vec::new(),
        out_dir: shown(out_dir),
        compiler: CompilerInfo {
            path: String::new(),
            family: "cpl-emit-host".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            target: target.to_string(),
            digest: digest(""),
        },
        runtime: RuntimeInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            api: String::new(),
            source: "embedded".into(),
            digest: digest(target),
        },
        program_digest: digest(&all),
        exports_digest: digest(&package.exports.join("\n")),
        exports,
        bindings: Vec::new(),
        artifacts,
        inputs: Vec::new(),
        holes: 0,
        lock: BuildLock::default(),
        cached: false,
        duration_ms: started.elapsed().as_millis() as u64,
        python: None,
    };
    let next = match target {
        "rust" => format!(
            "add it to a Cargo.toml as {name} = {{ path = \"{}\" }}",
            output.out_dir
        ),
        "go" => format!(
            "require {name} with replace {name} => {} in a go.mod",
            output.out_dir
        ),
        _ => format!(
            "compile {}/src with javac 17 or later (Kotlin calls it as Java)",
            output.out_dir
        ),
    };
    let stdout = format!(
        "build: {name} ({target} source): {} exports, {} files in {}; {next}\n",
        output.exports.len(),
        output.artifacts.len(),
        output.out_dir
    );
    let json = diagnostics.iter().map(|d| d.to_json(&sm)).collect();
    Ok(Report::new("build", revision, Status::Ok, json, output)
        .with_stdout(stdout)
        .with_sources(sm))
}
