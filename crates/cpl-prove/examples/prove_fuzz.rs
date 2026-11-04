//! Differential soundness fuzzing of the verifier against the interpreter:
//! `cargo run --release -p cpl-prove --example prove_fuzz [cases] [first-seed]`.
//!
//! Each case is a random program of helpers and a target over `Int`, `Bool`, `Option<Int>`, an
//! enum, and a struct with an invariant. Every function is verified, then run on random inputs:
//!
//! * a fault must be attributable to an obligation that is not `proved`;
//! * the model of every `failed` obligation must fault when run;
//! * the target is mutated (some mutations preserve behavior, some do not), and a `proved`
//!   equivalence must agree with every run, a `failed` one must differ on its inputs.
//!
//! A violation is printed with its program and the case's seed and counted; the process exits 1
//! when there was one.

use cpl_diag::FileOrigin;
use cpl_eval::testing::programs::{generate, inputs, Rng};
use cpl_eval::{ClauseKind, EvalConfig, Fault, FaultKind, RunArg, RunError};
use cpl_hir::{DefId, Program};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use cpl_prove::{equivalent, verify, Report, Status};
use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::fmt::Write as _;

fn check(source: &str) -> Result<Program, Vec<String>> {
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
        .map_err(|e| vec![format!("{e:?}")])?;
    let checked = cpl_sema::check(&project);
    let errors: Vec<String> = checked
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{}: {}", d.code.0, d.message))
        .collect();
    if errors.is_empty() {
        Ok(checked.program)
    } else {
        Err(errors)
    }
}

fn functions(p: &Program) -> Vec<DefId> {
    p.defs()
        .filter(|d| p.module(d.module).path == ["m"])
        .filter(|d| {
            d.kind
                .as_fn()
                .is_some_and(|f| matches!(f.body, cpl_hir::FnBody::Block(_)))
        })
        .map(|d| d.id)
        .collect()
}

fn by_name(p: &Program, name: &str) -> DefId {
    p.defs()
        .find(|d| d.name.display() == name && p.module(d.module).path == ["m"])
        .unwrap_or_else(|| panic!("no {name}"))
        .id
}

#[derive(Debug, Clone, PartialEq)]
enum Out {
    Value(Json),
    Fault(String),
    Outside,
    Exhausted,
}

/// Run `f`; on a fault also the fault.
fn run(p: &Program, f: DefId, args: &[Json]) -> (Out, Option<Fault>) {
    let config = EvalConfig {
        max_steps: Some(200_000),
        ..EvalConfig::default()
    };
    let call = cpl_eval::run_function(
        p,
        &p.symbol_path(f),
        &args.iter().cloned().map(RunArg::Json).collect::<Vec<_>>(),
        &config,
    );
    let outcome = match call {
        Ok(o) => o,
        Err(RunError::Arg { .. }) => return (Out::Outside, None),
        Err(e) => panic!("not runnable: {e}"),
    };
    match outcome.value {
        Ok(v) => (
            Out::Value(cpl_eval::value_to_json(p, &v, &outcome.return_ty)),
            None,
        ),
        Err(fault) => {
            let top = fault.symbol.as_ref() == Some(&p.def(f).stable_id)
                && !fault.data.contains_key("caller");
            match fault.kind {
                FaultKind::RefinementViolation => (Out::Outside, None),
                FaultKind::ContractViolation(ClauseKind::Requires) if top => (Out::Outside, None),
                FaultKind::ResourceExhaustion(_) => (Out::Exhausted, Some(fault)),
                _ => {
                    // The verifier's identity of a fault: the code, the clause kind, and the
                    // callee whose own clause failed.
                    let mut code = fault.to_diagnostic(p).code.as_str().to_string();
                    if let FaultKind::ContractViolation(k) = fault.kind {
                        // The activation whose clause failed (a claimed fault keeps it).
                        let activation = fault.backtrace.first().map(|frame| frame.def);
                        let callee = match k {
                            ClauseKind::Requires => activation,
                            ClauseKind::Ensures => activation.filter(|d| *d != f),
                            _ => None,
                        };
                        code = match callee {
                            Some(d) => format!("{code}:{k:?}:{}", p.def(d).stable_id.as_str()),
                            None => format!("{code}:{k:?}"),
                        };
                    }
                    (Out::Fault(code), Some(fault))
                }
            }
        }
    }
}

/// The obligation kinds a fault can violate, and the functions that can own them.
fn attribution(p: &Program, fault: &Fault) -> (Vec<DefId>, &'static [&'static str]) {
    let mut owners = Vec::new();
    if let Some(s) = &fault.symbol {
        owners.extend(p.by_stable_id(s));
    }
    if let Some(frame) = fault.backtrace.first() {
        owners.push(frame.def);
    }
    if let Some(Json::String(caller)) = fault.data.get("caller") {
        owners.extend(
            p.defs()
                .find(|d| p.display_path(d.id) == *caller)
                .map(|d| d.id),
        );
    }
    let kinds: &'static [&'static str] = match fault.kind {
        FaultKind::DivisionByZero => &["division"],
        FaultKind::AssertionFailed => &["assert"],
        FaultKind::ContractViolation(ClauseKind::Ensures) => &["ensures", "call_requires"],
        FaultKind::ContractViolation(ClauseKind::Requires) => &["call_requires"],
        FaultKind::ContractViolation(ClauseKind::Invariant) => &[
            "invariant_init",
            "invariant_preserve",
            "constructor_invariant",
            "call_requires",
        ],
        FaultKind::ContractViolation(ClauseKind::Decreases) => {
            &["decreases", "loop_decreases", "call_requires"]
        }
        _ => &[],
    };
    (owners, kinds)
}

fn model_args(p: &Program, f: DefId, model: &Json) -> Option<Vec<Json>> {
    let fd = p.def(f).kind.as_fn().expect("a function");
    fd.params
        .iter()
        .map(|param| model.get(param.name.as_str()).cloned())
        .collect()
}

#[derive(Default)]
pub struct Stats {
    verify_ms: f64,
    equivalence_ms: f64,
    pub cases: usize,
    pub rejected: usize,
    verdicts: BTreeMap<(String, String), usize>,
    runs: usize,
    faults: usize,
    models: usize,
    equivalences: BTreeMap<String, usize>,
    pub violations: usize,
}

fn report_violation(stats: &mut Stats, seed: u64, what: &str, source: &str, detail: &str) {
    stats.violations += 1;
    println!("=== VIOLATION (seed {seed}): {what}\n{detail}\n--- program\n{source}");
}

fn verdict_summary(p: &Program, reports: &BTreeMap<DefId, Report>) -> String {
    let mut s = String::new();
    for (f, r) in reports {
        let _ = writeln!(s, "{}: {}", p.display_path(*f), r.status.as_str());
        for (site, v) in &r.verdicts {
            let _ = writeln!(
                s,
                "    {} {} {:?} {}",
                r.ids.get(site).map(String::as_str).unwrap_or("?"),
                v.status.as_str(),
                v.reason,
                v.evidence
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            );
        }
    }
    s
}

/// One case: generate, verify, run, and compare (the module documentation lists the checks).
pub fn one(seed: u64, stats: &mut Stats, verbose: bool) {
    if verbose {
        eprintln!("generating {seed}");
    }
    let generated = generate(seed, &[]);
    let source = generated.source;
    let target = generated.nodes.len() - 1;
    let nodes = generated.nodes[target];
    if verbose {
        println!("{source}");
    }
    let p = match check(&source) {
        Ok(p) => p,
        Err(errors) => {
            stats.rejected += 1;
            if verbose || stats.rejected <= 5 {
                println!("--- rejected (seed {seed}): {errors:?}\n{source}");
            }
            return;
        }
    };
    stats.cases += 1;
    let fns = functions(&p);
    let reports: BTreeMap<DefId, Report> = fns
        .iter()
        .map(|&f| {
            let began = std::time::Instant::now();
            let r = verify(&p, f).expect("a function");
            stats.verify_ms += began.elapsed().as_secs_f64() * 1e3;
            if verbose {
                eprintln!(
                    "verify {} {:.1} ms",
                    p.display_path(f),
                    began.elapsed().as_secs_f64() * 1e3
                );
            }
            (f, r)
        })
        .collect();
    for r in reports.values() {
        for (site, v) in &r.verdicts {
            *stats
                .verdicts
                .entry((
                    site.kind().to_string(),
                    format!("{} {}", v.status.as_str(), v.reason.unwrap_or("")),
                ))
                .or_default() += 1;
        }
    }
    if verbose {
        println!("{}", verdict_summary(&p, &reports));
    }
    let mut rng = Rng::new(seed ^ 0xABCDEF);
    let bench = std::env::var("FUZZ_BENCH").is_ok();
    // 1. every fault is attributable to an obligation that is not proved
    for f in fns.iter().filter(|_| !bench) {
        for k in 0..60 {
            let args = inputs(&p, *f, &mut rng, k % 3 == 0);
            let (out, fault) = run(&p, *f, &args);
            stats.runs += 1;
            if let (Out::Fault(code), Some(fault)) = (&out, fault) {
                stats.faults += 1;
                let (owners, kinds) = attribution(&p, &fault);
                if kinds.is_empty() {
                    report_violation(
                        stats,
                        seed,
                        &format!("unexpected fault {code}"),
                        &source,
                        &format!("{} on {args:?}: {}", p.display_path(*f), fault.message),
                    );
                    return;
                }
                let covered = owners.iter().any(|o| {
                    reports.get(o).is_some_and(|r| {
                        r.verdicts.iter().any(|(site, v)| {
                            kinds.contains(&site.kind()) && v.status != Status::Proved
                        })
                    })
                });
                if !covered {
                    report_violation(
                        stats,
                        seed,
                        &format!("fault {code} under proved obligations"),
                        &source,
                        &format!(
                            "{} on {}: {} {:?}\n{}",
                            p.display_path(*f),
                            Json::Array(args.clone()),
                            fault.message,
                            fault.data,
                            verdict_summary(&p, &reports)
                        ),
                    );
                    return;
                }
            }
        }
    }
    // 2. every failed obligation's model faults
    for (f, r) in reports.iter().filter(|_| !bench) {
        for (site, v) in &r.verdicts {
            if v.status != Status::Failed {
                continue;
            }
            let Some(model) = v.evidence.as_ref().and_then(|e| e.get("model")) else {
                report_violation(
                    stats,
                    seed,
                    "failed without a model",
                    &source,
                    &format!("{site:?}"),
                );
                return;
            };
            stats.models += 1;
            let Some(args) = model_args(&p, *f, model) else {
                report_violation(
                    stats,
                    seed,
                    "model misses a parameter",
                    &source,
                    &format!("{site:?} {model}"),
                );
                return;
            };
            let (out, fault) = run(&p, *f, &args);
            let ok = match (&out, &fault) {
                (Out::Fault(_), Some(fault)) => {
                    let (owners, kinds) = attribution(&p, fault);
                    owners.contains(f) && kinds.contains(&site.kind())
                }
                _ => false,
            };
            if !ok {
                report_violation(
                    stats,
                    seed,
                    &format!("model of failed {} does not fault there", site.kind()),
                    &source,
                    &format!(
                        "{} {} model {model}: ran to {out:?} {}\n{}",
                        p.display_path(*f),
                        r.ids.get(site).map(String::as_str).unwrap_or("?"),
                        fault
                            .map(|f| format!("{} {:?}", f.message, f.data))
                            .unwrap_or_default(),
                        verdict_summary(&p, &reports)
                    ),
                );
                return;
            }
        }
    }
    // 3. equivalence of the target and a mutant
    if nodes == 0 {
        return;
    }
    let mut mrng = Rng::new(seed ^ 0x5555);
    for _ in 0..2 {
        let at = 1 + mrng.below(nodes as u64) as usize;
        let mseed = mrng.next_u64();
        let generated = generate(seed, &[(target, at, mseed)]);
        let mutant = generated.source;
        let Some(mutation) = generated.mutations.first().cloned() else {
            continue;
        };
        let Ok(q) = check(&mutant) else {
            continue;
        };
        let (fo, fnew) = (by_name(&p, "target"), by_name(&q, "target"));
        let target_id = p.def(fo).stable_id.clone();
        let began = std::time::Instant::now();
        let e = equivalent(&p, fo, &q, fnew, &|id| *id != target_id);
        stats.equivalence_ms += began.elapsed().as_secs_f64() * 1e3;
        if verbose {
            eprintln!("equivalent {:.1} ms", began.elapsed().as_secs_f64() * 1e3);
        }
        *stats
            .equivalences
            .entry(format!(
                "{}{}",
                e.status.as_str(),
                e.reason.map(|r| format!(" ({r})")).unwrap_or_default()
            ))
            .or_default() += 1;
        let detail = |extra: &str| {
            format!(
                "mutation {mutation}\nequivalence {} {:?} {}\n{extra}\n--- mutant\n{mutant}",
                e.status.as_str(),
                e.reason,
                e.evidence
            )
        };
        match e.status {
            _ if bench => {}
            Status::Proved => {
                for k in 0..80 {
                    let args = inputs(&p, fo, &mut rng, k % 3 == 0);
                    let (a, _) = run(&p, fo, &args);
                    if matches!(a, Out::Outside | Out::Exhausted) {
                        continue;
                    }
                    let (b, _) = run(&q, fnew, &args);
                    let b = if b == Out::Outside {
                        Out::Fault("contract_violation:Requires".into())
                    } else {
                        b
                    };
                    if a != b && b != Out::Exhausted {
                        report_violation(
                            stats,
                            seed,
                            "proved equivalent but runs differ",
                            &source,
                            &detail(&format!("on {}: {a:?} vs {b:?}", Json::Array(args))),
                        );
                        return;
                    }
                }
            }
            Status::Failed => {
                let Some(model) = e.evidence.get("inputs") else {
                    report_violation(
                        stats,
                        seed,
                        "failed equivalence without inputs",
                        &source,
                        &detail(""),
                    );
                    return;
                };
                let Some(args) = model_args(&p, fo, model) else {
                    report_violation(
                        stats,
                        seed,
                        "equivalence inputs miss a parameter",
                        &source,
                        &detail(""),
                    );
                    return;
                };
                let (a, _) = run(&p, fo, &args);
                let (b, _) = run(&q, fnew, &args);
                let b = if b == Out::Outside {
                    Out::Fault("contract_violation:Requires".into())
                } else {
                    b
                };
                if a == b || a == Out::Outside {
                    report_violation(
                        stats,
                        seed,
                        "failed equivalence but runs agree",
                        &source,
                        &detail(&format!("on {}: {a:?} vs {b:?}", Json::Array(args))),
                    );
                    return;
                }
            }
            Status::Unknown => {}
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cases: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(200);
    let first: u64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    let verbose = std::env::var("FUZZ_VERBOSE").is_ok();
    let mut stats = Stats::default();
    let start = std::time::Instant::now();
    let trace = std::env::var("FUZZ_TRACE").is_ok();
    for seed in first..first + cases {
        if trace {
            eprintln!("seed {seed}");
        }
        let began = std::time::Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut local = Stats::default();
            one(seed, &mut local, verbose);
            local
        }));
        let took = began.elapsed().as_secs_f64();
        if took > 2.0 {
            println!("=== SLOW (seed {seed}): {took:.1}s");
        }
        match result {
            Ok(local) => {
                stats.cases += local.cases;
                stats.rejected += local.rejected;
                stats.runs += local.runs;
                stats.faults += local.faults;
                stats.models += local.models;
                stats.violations += local.violations;
                stats.verify_ms += local.verify_ms;
                stats.equivalence_ms += local.equivalence_ms;
                for (k, v) in local.verdicts {
                    *stats.verdicts.entry(k).or_default() += v;
                }
                for (k, v) in local.equivalences {
                    *stats.equivalences.entry(k).or_default() += v;
                }
            }
            Err(_) => {
                stats.violations += 1;
                let source = generate(seed, &[]).source;
                println!("=== PANIC (seed {seed})\n{source}");
            }
        }
    }
    println!(
        "cases {} rejected {} runs {} faults {} models {} violations {} in {:.1}s",
        stats.cases,
        stats.rejected,
        stats.runs,
        stats.faults,
        stats.models,
        stats.violations,
        start.elapsed().as_secs_f64()
    );
    println!(
        "verify {:.0} ms, equivalence {:.0} ms",
        stats.verify_ms, stats.equivalence_ms
    );
    for ((kind, status), n) in &stats.verdicts {
        println!("  {kind:22} {status:30} {n}");
    }
    for (k, n) in &stats.equivalences {
        println!("  equivalence {k:30} {n}");
    }
    if stats.violations > 0 {
        std::process::exit(1);
    }
}
