//! Deterministic transitions and receipt verification; no command handlers are bound.
use super::{check_diagnostics_for_execution, run::driver_config, run::native_failure, Ctx};
use crate::cli::ReplayArgs;
use crate::output::{Failure, Report};
use cpl_diag::{json::Status, Category, DiagCode, Diagnostic};
use cpl_eval::machine::{MachineContext, TransitionRecord};
use cpl_eval::{EvalConfig, Interpreter, RunEngine};
use cpl_hir::{DefId, Program};
use cpl_index::{Index, Target};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize)]
pub struct ReplayOutput {
    engine: String,
    state: Value,
    sequence: u64,
    records: Vec<TransitionRecord>,
}

enum Input {
    Events(Vec<Value>),
    Records(Vec<TransitionRecord>),
}

pub fn run(ctx: &Ctx, args: &ReplayArgs) -> Result<Report<ReplayOutput>, Failure> {
    let read = |text: &str| {
        crate::json_arg::read(text).map_err(|e| {
            ctx.session
                .refuse(crate::codes::bad_argument(e.to_string()))
        })
    };
    let initial = read(&args.state)?;
    let input = if let Some(records) = &args.records {
        Input::Records(serde_json::from_value(read(records)?).map_err(|e| {
            ctx.session
                .refuse(crate::codes::bad_argument(format!("invalid receipts: {e}")))
        })?)
    } else {
        let events = read(args.events.as_deref().unwrap_or("[]"))?;
        Input::Events(
            events
                .as_array()
                .ok_or_else(|| {
                    ctx.session
                        .refuse(crate::codes::bad_argument("--events must be a JSON array"))
                })?
                .clone(),
        )
    };
    let opened = ctx.session.open(false)?;
    let project = &opened.project;
    let revision = Some(opened.revision.clone());
    let sm = project.source_map.clone();
    let checked = cpl_sema::check(project);
    let mut diagnostics = check_diagnostics_for_execution(&checked.diagnostics);
    if checked.has_errors() {
        return Err(Failure::new(
            Status::Unsuccessful,
            revision,
            &diagnostics,
            Some(sm),
        ));
    }
    let p = &checked.program;
    let index = Index::build(p, project, opened.revision.clone());
    let id = index
        .resolve(&Target::parse(&args.machine))
        .map_err(|d| Failure::new(Status::Error, revision.clone(), &[d], Some(sm.clone())))?;
    let cfg = EvalConfig {
        max_call_depth: args
            .max_call_depth
            .unwrap_or(project.manifest.run.max_call_depth),
        time_budget: Some(std::time::Duration::from_millis(args.timeout_ms)),
        ..EvalConfig::default()
    };
    // Validate the selection and starting snapshot before launching a native process.
    MachineContext::new(p, id, &initial, &cfg)
        .map_err(|d| Failure::new(Status::Error, revision.clone(), &[d], Some(sm.clone())))?;
    let eval = (args.engine != "native").then(|| {
        let mut engine = Interpreter::new(p, cfg.clone());
        drive(p, id, &initial, &cfg, &input, &mut engine)
    });
    let native = if args.engine != "eval" {
        let plan = checked
            .interop
            .clone()
            .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
        let failure = |e| {
            let (status, ds) = native_failure(e);
            Failure::new(status, revision.clone(), &ds, Some(sm.clone()))
        };
        let mut engine =
            cpl_cabi::NativeSession::start(p, project, &plan, &cfg, &driver_config(project))
                .map_err(failure)?;
        engine.set_request_timeout(Some(std::time::Duration::from_secs(60)));
        let result = drive(p, id, &initial, &cfg, &input, &mut engine);
        let shutdown = engine.shutdown().map_err(failure)?;
        if shutdown.live_handles != 0
            || shutdown.live_objects != 0
            || shutdown.live_caps != 0
            || shutdown.live_buffers != 0
            || !shutdown.exit_status.success()
        {
            return Err(Failure::new(
                Status::Error,
                revision,
                &[Diagnostic::error(
                    DiagCode("native_shutdown_leak"),
                    Category::Internal,
                    "the native machine driver retained objects at shutdown",
                )],
                Some(sm),
            ));
        }
        Some(result)
    } else {
        None
    };
    if let (Some(a), Some(b)) = (&eval, &native) {
        let normalize = |r: &(ReplayOutput, Option<Diagnostic>)| {
            let mut value = json!({"state": r.0.state, "sequence": r.0.sequence, "records": r.0.records,
                "fault": r.1.as_ref().map(|d| d.to_json(&sm))});
            cpl_cabi::compare::normalize(&mut value);
            value
        };
        if normalize(a) != normalize(b) {
            diagnostics.push(
                Diagnostic::error(
                    cpl_cabi::codes::ENGINE_DIVERGENCE,
                    Category::Internal,
                    "the engines disagree on the transition history or fault",
                )
                .with_data("eval", normalize(a))
                .with_data("native", normalize(b)),
            );
        }
    }
    let (mut output, fault) = eval.or(native).expect("one engine selected");
    let mut status = if fault.is_some() {
        Status::Unsuccessful
    } else {
        Status::Ok
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
    output.engine.clone_from(&args.engine);
    let stdout = format!("{}\n", output.state);
    Ok(Report::new(
        "replay",
        revision,
        status,
        super::to_json(&diagnostics, &sm),
        output,
    )
    .with_stdout(stdout)
    .with_sources(sm))
}

fn drive(
    p: &Program,
    id: DefId,
    initial: &Value,
    cfg: &EvalConfig,
    input: &Input,
    engine: &mut dyn RunEngine,
) -> (ReplayOutput, Option<Diagnostic>) {
    let mut context =
        MachineContext::new(p, id, initial, cfg).expect("initial state already validated");
    let mut records = Vec::new();
    let mut fault = None;
    let count = match input {
        Input::Events(v) => v.len(),
        Input::Records(v) => v.len(),
    };
    for i in 0..count {
        let result = match input {
            Input::Events(events) => context.dispatch(engine, &events[i]),
            Input::Records(expected) => context
                .replay(engine, &expected[i])
                .map(|()| expected[i].clone()),
        };
        match result {
            Ok(record) => records.push(record),
            Err(d) => {
                fault = Some(d);
                break;
            }
        }
    }
    (
        ReplayOutput {
            engine: String::new(),
            state: context.snapshot(),
            sequence: context.sequence(),
            records,
        },
        fault,
    )
}
