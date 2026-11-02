//! `cpl run <function> [arg]... [--arg JSON]... [--json] [--max-call-depth N]`
//! (`spec/toolchain.md` 7.2; semantics S39, S40).
//!
//! Checks the project (any error: exit 1 with the diagnostics, nothing
//! runs), resolves `<function>` with the semantic index
//! (`cpl_index::Index::resolve`: an `@id`, a qualified name, or a
//! provisional id), and calls `cpl_eval::run_function` with the arguments
//! in command-line order: a bare argument is a constant expression, `--arg`
//! value JSON read strictly ([`crate::json_arg`]: a duplicate key or a
//! nesting beyond the reader's limit is `usage_bad_argument`).
//!
//! | Outcome | Code | Exit |
//! | - | - | - |
//! | not found or ambiguous / not a function, or a generator (T61) | `index_unknown_symbol`, `index_ambiguous_symbol` / `usage_not_a_function` | 2 |
//! | generic | `cannot_run_generic`, with a note naming a monomorphic wrapper | 2 |
//! | `using` parameters | `cannot_run_effectful` | 2 |
//! | wrong argument count, an argument that does not parse or decode | `usage_bad_argument` (`argument`: the 0-based index; `pointer` where the value has one) | 2 |
//! | the call faults | the fault's diagnostic, also `fault` of `RunOutput` | 1 |
//! | the call returns | the value (`result`, `result_text`; an `Err(...)` is a value) | 0 |
//!
//! Human output: the result's value text on one line on stdout.
//!
//! `RunOutput::arguments` holds the value JSON of every argument as
//! `run_function` decoded it (`RunOutcome::arguments`, each encoded at its
//! parameter's type), so the two forms of one value report the same value
//! JSON (7.2: "the same values spelled two ways"), in its canonical
//! spelling (`--arg -0` reports `0`, S40).
//!
//! Readings: the target resolves as every index target does (Section 6.1),
//! so beside the qualified name and the `@id` of 7.2 a bare name that names
//! exactly one definition resolves too (an ambiguous one is
//! `index_ambiguous_symbol`, exit 2). The `--arg` values are read when the
//! command line is read, before the project is loaded, so a malformed one is
//! reported before a target that does not resolve (`run_function`'s refusal
//! order applies to everything after reading); its envelope still names the
//! located project's revision (`Session::refuse`). A constant-expression
//! argument's diagnostic spans its own text, registered as the virtual file
//! `<virtual:argument N>`, so that both forms show where in the argument
//! the problem is.

use super::{check_diagnostics_for_execution, Ctx};
use crate::cli::{RunArgText, RunArgs};
use crate::codes;
use crate::json_arg::{self, JsonArgError};
use crate::output::{Failure, Report};
use cpl_diag::json::{EngineKind, EngineRunOutput, RunBinding, RunOutput, Status};
use cpl_diag::{Category, Diagnostic, FileId, FileOrigin, SourceMap, Span};
use cpl_eval::run::ARGUMENT_FILE;
use cpl_eval::{
    prepare_run, run_function_with_hosts, value_text, value_to_json, EvalConfig, PreparedRun,
    RunArg, RunError, Value,
};
use cpl_hir::{DefId, DefKind, Program, Ty};
use cpl_index::{Index, Target};
use cpl_sema::CheckConfig;
use cpl_syntax::ast::{DeclKind, Declaration};
use serde_json::json;
use std::sync::Arc;

pub fn run(ctx: &Ctx, args: &RunArgs) -> Result<Report<EngineRunOutput>, Failure> {
    let engine = match args.engine.as_str() {
        "native" => EngineKind::Native,
        "both" => EngineKind::Both,
        _ => EngineKind::Eval,
    };
    let mut run_args = Vec::with_capacity(args.ordered.len());
    for (index, arg) in args.ordered.iter().enumerate() {
        run_args.push(match arg {
            RunArgText::Const(text) => RunArg::ConstExpr(text.clone()),
            RunArgText::Json(text) => match json_arg::read(text) {
                Ok(value) => RunArg::Json(value),
                Err(error) => return Err(ctx.session.refuse(json_arg_error(index, &error))),
            },
        });
    }

    let opened = ctx.session.open(false)?;
    let project = &opened.project;
    let revision = Some(opened.revision.clone());
    let checked = cpl_sema::check_with(project, &CheckConfig::default());
    let mut diagnostics = check_diagnostics_for_execution(&checked.diagnostics);
    let sm = project.source_map.clone();
    if checked.has_errors() {
        return Err(Failure::new(
            Status::Unsuccessful,
            revision,
            &diagnostics,
            Some(sm),
        ));
    }
    let program = &checked.program;
    let index = Index::build(program, project, opened.revision.clone());
    let def = match index.resolve(&Target::parse(&args.function)) {
        Ok(def) => def,
        Err(d) => {
            diagnostics.push(d);
            return Err(Failure::new(
                Status::Error,
                revision,
                &diagnostics,
                Some(sm),
            ));
        }
    };
    let config = EvalConfig {
        max_call_depth: args
            .max_call_depth
            .unwrap_or(project.manifest.run.max_call_depth),
        ..EvalConfig::default()
    };
    let path = program.symbol_path(def);
    let refuse = |error: RunError, diagnostics: &mut Vec<Diagnostic>| -> Failure {
        let (d, sm) = refusal(&index, &sm, def, &args.ordered, error);
        diagnostics.push(d);
        Failure::new(Status::Error, revision.clone(), diagnostics, Some(sm))
    };
    let plan_for_hosts = checked
        .interop
        .clone()
        .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
    let host_driver = driver_config(project);
    // The evaluator's outcome (eval and both): run_function_with_hosts, whose refusals every engine shares; a
    // using parameter is served by the CAdapter of its backend-c binding (semantics S78, cabi.md 7.2).
    let mut eval_bindings = Vec::new();
    let eval = if engine != EngineKind::Native {
        let mut hosts =
            cpl_cabi::adapter::manifest_hosts(program, project, &plan_for_hosts, &host_driver)
                .with_python(args.python.as_deref());
        let result = run_function_with_hosts(program, &path, &run_args, &config, &mut hosts);
        for (parameter, binding, handle) in &hosts.created {
            eval_bindings.push(RunBinding {
                parameter: parameter.clone(),
                binding: binding.clone(),
                handle: *handle,
                inputs: binding_inputs(project, binding),
            });
        }
        match result {
            Ok(outcome) => Some((outcome.arguments, outcome.return_ty, outcome.value)),
            Err(RunError::Bind(ds)) => {
                let status = if ds.iter().any(|d| d.code.0 == "interop_bind_failed") {
                    Status::Unsuccessful
                } else {
                    Status::Error
                };
                diagnostics.extend(ds);
                return Err(Failure::new(status, revision, &diagnostics, Some(sm)));
            }
            Err(error) => return Err(refuse(error, &mut diagnostics)),
        }
    } else {
        None
    };
    // The native engine's outcome (native and both).
    let native = if engine != EngineKind::Eval {
        let plan = checked
            .interop
            .clone()
            .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
        let has_host = |param: &cpl_hir::ParamDef| match &param.ty {
            Ty::Capability(c) => {
                plan.binding_for(*c).is_some() || plan.python_binding_for(*c).is_some()
            }
            _ => false,
        };
        let prepared = match prepare_run(program, &path, &run_args, &has_host) {
            Ok(prepared) => prepared,
            Err(error) => return Err(refuse(error, &mut diagnostics)),
        };
        match native_outcome(
            project,
            program,
            &plan,
            &config,
            &prepared,
            args.python.as_deref(),
        ) {
            Ok((value, bindings)) => {
                Some((prepared.arguments, prepared.return_ty, value, bindings))
            }
            Err((status, ds)) => {
                diagnostics.extend(ds);
                return Err(Failure::new(status, revision, &diagnostics, Some(sm)));
            }
        }
    } else {
        None
    };
    let function = index.version(def).clone();
    let render = |arguments: &[Value], return_ty: &Ty, value: &Result<Value, cpl_eval::Fault>| {
        let params = program.def(def).kind.as_fn().map_or(&[][..], |f| &f.params);
        let arguments = arguments
            .iter()
            .zip(params)
            .map(|(value, param)| value_to_json(program, value, &param.ty))
            .collect();
        match value {
            Ok(value) => {
                let text = value_text(program, value, return_ty);
                let output = RunOutput {
                    function: function.clone(),
                    arguments,
                    result: Some(value_to_json(program, value, return_ty)),
                    result_text: Some(text.clone()),
                    fault: None,
                };
                (output, Status::Ok, format!("{text}\n"), None)
            }
            Err(fault) => {
                let d = fault.to_diagnostic(program);
                let output = RunOutput {
                    function: function.clone(),
                    arguments,
                    result: None,
                    result_text: None,
                    fault: Some(d.to_json(&sm)),
                };
                (output, Status::Unsuccessful, String::new(), Some(d))
            }
        }
    };
    let (output, mut status, stdout, fault, bindings) = match (&eval, &native) {
        (Some((a, t, v)), None) => {
            let (o, s, out, f) = render(a, t, v);
            (o, s, out, f, eval_bindings.clone())
        }
        (None, Some((a, t, v, b))) => {
            let (o, s, out, f) = render(a, t, v);
            (o, s, out, f, b.clone())
        }
        (Some((a, t, v)), Some((na, nt, nv, b))) => {
            let (o, s, out, f) = render(a, t, v);
            let (no, _, _, _) = render(na, nt, nv);
            if let Some(div) = cpl_cabi::compare::run_outputs(&o, &no) {
                diagnostics.push(divergence(&div, &program.display_path(def)));
            }
            (o, s, out, f, b.clone())
        }
        (None, None) => unreachable!("an engine ran"),
    };
    if let Some(d) = fault {
        diagnostics.push(d);
    }
    if diagnostics
        .iter()
        .any(|d| d.code == cpl_cabi::codes::ENGINE_DIVERGENCE)
    {
        status = Status::Error;
    }
    let json = diagnostics.iter().map(|d| d.to_json(&sm)).collect();
    let output = EngineRunOutput {
        run: output,
        engine: (engine != EngineKind::Eval).then_some(engine),
        bindings,
    };
    Ok(Report::new("run", revision, status, json, output)
        .with_stdout(stdout)
        .with_sources(sm))
}

/// The driver configuration of `cpl run --engine native` and `cpl test --engine native` (toolchain 7.2, T73): the
/// optimized profile, cached under the project's `.cpl/cache/c` (spec/parallelism.md).
pub(crate) fn driver_config(project: &cpl_project::Project) -> cpl_emit_c::package::DriverConfig {
    let cache_dir = match &project.root {
        Some(root) => root.join(".cpl").join("cache").join("c"),
        None => std::env::temp_dir().join("cpl-cache-c"),
    };
    let runtime = match std::env::var_os("CPL_RUNTIME_DIR") {
        Some(dir) => cpl_rt::RuntimeSource::Dir(dir.into()),
        None => cpl_rt::RuntimeSource::Embedded,
    };
    cpl_emit_c::package::DriverConfig {
        cache_dir,
        profile: cpl_emit_c::cc::Profile::Optimized,
        sanitize: Vec::new(),
        coverage: false,
        cc: None,
        runtime,
        werror: false,
    }
}

/// The status and diagnostics of a native engine that failed (toolchain 7.2, T73): a build failure as `cpl build`
/// reports it, a grant that failed exit 1 (semantics S78), every other failure exit 2.
pub(crate) fn native_failure(e: cpl_cabi::NativeError) -> (Status, Vec<Diagnostic>) {
    let status = match &e {
        cpl_cabi::NativeError::Refused(d) if d.code.0 == "interop_bind_failed" => {
            Status::Unsuccessful
        }
        cpl_cabi::NativeError::Build(ds)
            if ds.iter().all(|d| {
                d.category == Category::Build
                    && d.code.0 != "build_cc_unavailable"
                    && d.code.0 != "build_cc_unsupported"
            }) =>
        {
            Status::Unsuccessful
        }
        _ => Status::Error,
    };
    (status, e.into_diagnostics())
}

/// A native run's outcome and bindings, or the failure of the engine.
type NativeOutcome =
    Result<(Result<Value, cpl_eval::Fault>, Vec<RunBinding>), (Status, Vec<Diagnostic>)>;

/// The outcome of the prepared run on the native engine, with the bindings of its using parameters.
fn native_outcome(
    project: &cpl_project::Project,
    program: &Program,
    plan: &cpl_sema::interop::InteropPlan,
    config: &EvalConfig,
    prepared: &PreparedRun,
    python: Option<&std::path::Path>,
) -> NativeOutcome {
    let mut session =
        cpl_cabi::NativeSession::start(program, project, plan, config, &driver_config(project))
            .map_err(native_failure)?;
    let mut handles = Vec::new();
    let mut bindings = Vec::new();
    let using = program
        .def(prepared.function)
        .kind
        .as_fn()
        .map_or(&[][..], |f| &f.using);
    let mut workers_configured = false;
    for param in using {
        let Ty::Capability(cap) = &param.ty else {
            continue;
        };
        let name = match (plan.binding_for(*cap), plan.python_binding_for(*cap)) {
            (Some(b), _) => b.name.clone(),
            (None, Some(b)) => {
                // the driver's Python workers get the toolchain's interpreter and path (spec/python.md 6.5)
                if !workers_configured {
                    session
                        .configure_workers(&cpl_cabi::worker::WorkerConfig::for_project(
                            project, python,
                        ))
                        .map_err(native_failure)?;
                    workers_configured = true;
                }
                b.name.clone()
            }
            (None, None) => continue,
        };
        let h = session.bind(&name).map_err(native_failure)?;
        bindings.push(RunBinding {
            parameter: param.name.clone(),
            binding: name.clone(),
            handle: h.0,
            inputs: binding_inputs(project, &name),
        });
        handles.push(h);
    }
    let value = session
        .call(prepared.function, &prepared.arguments, &handles, true)
        .map_err(native_failure)?;
    let report = session.shutdown().map_err(native_failure)?;
    let _ = report;
    Ok((value, bindings))
}

/// The digest of a binding's C inputs (its adapter file and vendored sources, in order): `RunBinding::inputs`.
fn binding_inputs(project: &cpl_project::Project, binding: &str) -> cpl_diag::json::Digest {
    // Milestone 3: a backend-python binding's input digest (spec/python.md Section 9)
    if let Some(d) =
        cpl_project::python_input_digest(&project.manifest, project.root.as_deref(), binding)
    {
        return d;
    }
    let mut h = blake3::Hasher::new();
    if let Some(root) = &project.root {
        if let Some(b) = project.manifest.bindings.get(binding) {
            h.update(&std::fs::read(root.join(&b.adapter)).unwrap_or_default());
        }
        if let Some(c) = project.manifest.binding_c.get(binding) {
            for src in &c.sources {
                h.update(&std::fs::read(root.join(src)).unwrap_or_default());
            }
        }
    }
    cpl_diag::json::Digest(h.finalize().to_hex().to_string())
}

/// `engine_divergence` (toolchain T73): the two outputs as compact JSON.
pub(crate) fn divergence(div: &cpl_cabi::compare::Divergence, function: &str) -> Diagnostic {
    Diagnostic::error(
        cpl_cabi::codes::ENGINE_DIVERGENCE,
        Category::Internal,
        format!(
            "the native engine's output differs from the evaluator's at byte {}",
            div.offset
        ),
    )
    .with_data("eval", json!(div.eval))
    .with_data("native", json!(div.native))
    .with_data("function", json!(function))
}

/// `usage_bad_argument` for an `--arg` value the reader refused.
fn json_arg_error(index: usize, error: &JsonArgError) -> Diagnostic {
    let mut d = codes::bad_argument(format!("argument {index}: --arg value is {error}"))
        .with_data("argument", json!(index));
    if let JsonArgError::Duplicate { pointer, key } = error {
        d = d
            .with_data("pointer", json!(pointer))
            .with_data("key", json!(key));
    }
    d
}

/// The diagnostic of a refused run of `def` (exit 2) and the source map
/// that resolves its spans: the project's, plus the argument's own text
/// for a constant expression that is not one.
fn refusal(
    index: &Index<'_>,
    sm: &Arc<SourceMap>,
    def: DefId,
    args: &[RunArgText],
    error: RunError,
) -> (Diagnostic, Arc<SourceMap>) {
    let program = index.program();
    let name = program.display_path(def);
    let d = match error {
        RunError::ProgramHasErrors => codes::internal(
            "program_has_errors",
            "the checked program has a definition that failed to check",
        ),
        RunError::NotFound(_) => codes::internal(
            "target_not_found",
            format!("{name} resolved, but cpl_eval::run_function did not find it"),
        ),
        RunError::NotAFunction(_) => {
            // T61: a generator is refused as no function is.
            let generator = program
                .def(def)
                .kind
                .as_fn()
                .is_some_and(|f| matches!(f.ret, Ty::Gen(_)));
            let message = if generator {
                format!("{name} is a generator, not a function; a generator has a value only in a property's for clause, and cpl run calls a function")
            } else {
                format!("{name} is not a function; cpl run calls a function")
            };
            blame(
                program,
                def,
                codes::usage(codes::USAGE_NOT_A_FUNCTION, message),
            )
        }
        RunError::Generic(_) => {
            let d = Diagnostic::error(
                codes::CANNOT_RUN_GENERIC,
                Category::Resolve,
                format!("{name} is generic; run a monomorphic wrapper"),
            );
            let found = wrappers(index, def);
            let note = if found.is_empty() {
                "a type parameter has no argument to instantiate it from; write a monomorphic wrapper that calls it with concrete types and run that".to_string()
            } else {
                format!(
                    "a type parameter has no argument to instantiate it from; run a monomorphic wrapper such as {}",
                    found.join(", ")
                )
            };
            blame(program, def, d).with_note(note, None)
        }
        RunError::CannotRunEffectful(_) => {
            let d = Diagnostic::error(
                codes::CANNOT_RUN_EFFECTFUL,
                Category::Resolve,
                format!("{name} has using parameters"),
            );
            blame(program, def, d).with_note(
                "no capability handle exists in Milestone 1: there are no host bindings to pass as its using arguments",
                None,
            )
        }
        RunError::ArgCount { expected, got } => codes::bad_argument(format!(
            "{name} takes {expected} argument{}, got {got}",
            if expected == 1 { "" } else { "s" }
        ))
        .with_data("parameters", json!(expected))
        .with_data("arguments", json!(got)),
        RunError::Arg { index, error } => {
            codes::bad_argument(format!("argument {index}: {}", error.message))
                .with_data("argument", json!(index))
                .with_data("pointer", json!(error.pointer))
        }
        // Milestone 2 (toolchain 1.7.4, 7.2): the diagnostics of a `using`
        // parameter's host that could not be created (`interop_bind_failed`,
        // or a driver that cannot be built). `run_function` binds no host, so
        // this arm is reached only once `cpl run` binds adapters; the first
        // diagnostic is reported, any further one as its note.
        RunError::Bind(found) => {
            let mut found = found.into_iter();
            match found.next() {
                Some(first) => found.fold(first, |d, more| d.with_note(more.message, more.span)),
                None => codes::internal(
                    "bind_failed",
                    format!("a using parameter of {name} could not be bound"),
                ),
            }
        }
        RunError::ArgNotConstant { index, diagnostic } => {
            let mut map = (**sm).clone();
            let text = match args.get(index) {
                Some(RunArgText::Const(text)) => text.clone(),
                _ => String::new(),
            };
            let file = map.add(
                format!("<virtual:argument {index}>"),
                FileOrigin::Virtual,
                text,
            );
            let d = not_constant(index, diagnostic, file);
            return (d, Arc::new(map));
        }
    };
    (d, sm.clone())
}

/// A constant-expression argument that does not parse, or is not one:
/// `usage_bad_argument` with the argument's index, spanning the offending
/// part of the argument's text (now `file`).
fn not_constant(index: usize, mut d: Diagnostic, file: FileId) -> Diagnostic {
    let remap = |span: Option<Span>| match span {
        Some(s) if s.file == ARGUMENT_FILE => Some(Span::new(file, s.start, s.end)),
        other => other,
    };
    d.span = remap(d.span);
    for note in &mut d.notes {
        note.span = remap(note.span);
    }
    if d.code != codes::USAGE_BAD_ARGUMENT {
        let parsed = d;
        let mut wrapped = codes::bad_argument(format!(
            "argument {index} does not parse as a constant expression: {}",
            parsed.message
        ))
        .with_data("argument", json!(index))
        .with_data("parse_code", json!(parsed.code.as_str()));
        wrapped.span = parsed.span;
        return wrapped;
    }
    if !d.data.contains_key("argument") {
        d = d.with_data("argument", json!(index));
    }
    d
}

/// Blame the target definition and span its name token (semantics S63 (3)).
fn blame(program: &Program, def: DefId, d: Diagnostic) -> Diagnostic {
    d.with_symbol(
        program.def(def).stable_id.clone(),
        program.display_path(def),
        None,
    )
    .with_span(name_span(program.ast(def)))
}

/// A declaration's name token: the identifier, or a test's or property's
/// name literal.
fn name_span(decl: &Declaration) -> Span {
    match &decl.kind {
        DeclKind::Test(t) => t.name.span,
        DeclKind::Property(p) => p.name.span,
        _ => decl.name().map_or(decl.span, |n| n.span),
    }
}

/// The callers of `def` that could themselves be run: functions that are
/// neither generic nor have `using` parameters (the monomorphic wrappers
/// of semantics S39), by qualified name.
fn wrappers(index: &Index<'_>, def: DefId) -> Vec<String> {
    let program = index.program();
    index
        .callers(def)
        .into_iter()
        .filter(|&caller| caller != def)
        .filter(|&caller| match &program.def(caller).kind {
            DefKind::Fn(f) => f.generics.is_empty() && f.using.is_empty(),
            _ => false,
        })
        .map(|caller| program.display_path(caller))
        .collect()
}
