//! Differential fuzzing of the native engine against the evaluator on random programs:
//! `cargo run --release -p cpl-cabi --example native_fuzz [cases] [first-seed]`.
//!
//! Each case is a program of `cpl_eval::testing::programs` (helpers and a target with contracts,
//! loops, recursion, matches, structs with invariants, refinements, and `concurrent` blocks),
//! compiled to a native driver and run by both engines on random arguments, with contracts
//! checked and unchecked; every outcome is compared by the rendering `cpl run` uses (value JSON
//! and value text, or the fault's diagnostic). A divergence is printed with its program and seed;
//! the process exits 1 when there was one.

use cpl_cabi::NativeSession;
use cpl_diag::FileOrigin;
use cpl_eval::testing::programs::{generate, inputs, Rng};
use cpl_eval::{prepare_run, run_function, value_text, value_to_json, EvalConfig, RunArg};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use std::path::PathBuf;

fn render(
    p: &cpl_hir::Program,
    sm: &cpl_diag::SourceMap,
    ty: &cpl_hir::Ty,
    r: &Result<cpl_eval::Value, cpl_eval::Fault>,
) -> String {
    match r {
        Ok(v) => format!("value {} {}", value_to_json(p, v, ty), value_text(p, v, ty)),
        Err(f) => format!(
            "fault {}",
            serde_json::to_string(&f.to_diagnostic(p).to_json(sm)).unwrap()
        ),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cases: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(50);
    let first: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    let optimized = std::env::var("FUZZ_DEBUG_PROFILE").is_err();
    if cpl_rt::testing::find_cc().is_none() {
        eprintln!("no C compiler");
        std::process::exit(2);
    }
    let driver = cpl_emit_c::package::DriverConfig {
        cache_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/native-fuzz-cache"),
        profile: if optimized {
            cpl_emit_c::cc::Profile::Optimized
        } else {
            cpl_emit_c::cc::Profile::Debug
        },
        sanitize: Vec::new(),
        coverage: false,
        cc: None,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    let (mut compared, mut divergences, mut programs) = (0usize, 0usize, 0usize);
    let start = std::time::Instant::now();
    for seed in first..first + cases {
        let source = generate(seed, &[]).source;
        let tree = VirtualTree {
            files: [
                (
                    "cpl.toml".to_string(),
                    b"[project]\nname = \"m\"\nedition = \"0.1\"\n".to_vec(),
                ),
                ("src/m.cpl".to_string(), source.as_bytes().to_vec()),
            ]
            .into_iter()
            .collect(),
        };
        let project = load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default())
            .expect("the project loads");
        let checked = cpl_sema::check(&project);
        if checked.has_errors() {
            continue;
        }
        programs += 1;
        let p = &checked.program;
        let sm = project.source_map.clone();
        let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
        let functions: Vec<_> = p
            .defs()
            .filter(|d| p.module(d.module).path == ["m"])
            .filter(|d| {
                d.kind
                    .as_fn()
                    .is_some_and(|f| matches!(f.body, cpl_hir::FnBody::Block(_)))
            })
            .map(|d| d.id)
            .collect();
        let mut rng = Rng::new(seed ^ 0x0DD_BA11);
        let mut session =
            match NativeSession::start(p, &project, &plan, &EvalConfig::default(), &driver) {
                Ok(s) => s,
                Err(e) => {
                    divergences += 1;
                    println!(
                        "=== NO DRIVER (seed {seed}): {:?}\n{source}",
                        e.into_diagnostics()
                    );
                    continue;
                }
            };
        session.set_request_timeout(Some(std::time::Duration::from_secs(30)));
        for check_contracts in [true, false] {
            // Both engines honor a time budget (the evaluator's step budget is its own); a case
            // that meets it on either side is skipped, as the differential suites do (S75).
            let cfg = EvalConfig {
                check_contracts,
                time_budget: Some(std::time::Duration::from_millis(500)),
                ..EvalConfig::default()
            };
            session.reconfigure(&cfg).expect("reconfigure");
            'functions: for f in &functions {
                let path = p.symbol_path(*f);
                for k in 0..40 {
                    let args: Vec<RunArg> = inputs(p, *f, &mut rng, k % 4 == 0)
                        .into_iter()
                        .map(RunArg::Json)
                        .collect();
                    let Ok(eval) = run_function(p, &path, &args, &cfg) else {
                        continue;
                    };
                    // a case the evaluator does not finish is not sent to the driver
                    if matches!(&eval.value, Err(f) if matches!(f.kind, cpl_eval::FaultKind::ResourceExhaustion(_)))
                    {
                        continue;
                    }
                    let prepared = match prepare_run(p, &path, &args, &|_| false) {
                        Ok(prepared) => prepared,
                        Err(e) => {
                            divergences += 1;
                            println!("=== PREPARE (seed {seed}): {e}\n{source}");
                            break 'functions;
                        }
                    };
                    let native =
                        match session.call(prepared.function, &prepared.arguments, &[], true) {
                            Ok(native) => native,
                            Err(e) => {
                                divergences += 1;
                                println!(
                                    "=== DRIVER FAILED (seed {seed}) {} {:?}: {:?}\n{source}",
                                    p.display_path(*f),
                                    args,
                                    e.into_diagnostics()
                                );
                                break 'functions;
                            }
                        };
                    let timed_out = |r: &Result<cpl_eval::Value, cpl_eval::Fault>| matches!(r, Err(f) if matches!(f.kind, cpl_eval::FaultKind::ResourceExhaustion(_)));
                    if timed_out(&eval.value) || timed_out(&native) {
                        continue;
                    }
                    compared += 1;
                    let (a, b) = (
                        render(p, &sm, &eval.return_ty, &eval.value),
                        render(p, &sm, &eval.return_ty, &native),
                    );
                    if a != b {
                        divergences += 1;
                        println!(
                            "=== DIVERGENCE (seed {seed}, check_contracts {check_contracts}) {} {:?}\n  eval:   {a}\n  native: {b}\n{source}",
                            p.display_path(*f),
                            args
                        );
                        break 'functions;
                    }
                }
            }
        }
        if let Err(e) = session.shutdown() {
            println!("=== SHUTDOWN (seed {seed}): {e:?}");
        }
    }
    println!(
        "programs {programs} compared {compared} divergences {divergences} in {:.1}s",
        start.elapsed().as_secs_f64()
    );
    if divergences > 0 {
        std::process::exit(1);
    }
}
