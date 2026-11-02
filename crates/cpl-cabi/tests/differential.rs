//! The differential suites (toolchain 12.6): native execution against the evaluator, the oracle, through coverage
//! drivers, with the coverage gate over what the compared cases exercised. D1: the interpreter corpus of
//! `crates/cpl-eval/tests/corpus` (T49), each case with contracts checked and unchecked. D2 and D4: the modules of
//! `differential/corpus/*.cpl` with their `*.diff.json` cases (D2 is `stdlib.cpl`, every row of stdlib Section 12 and
//! the laws of 10.7), each module's tests and properties compared record by record, and D4's scripted capabilities
//! (`differential/scripted.rs`). Every case is compared by the rendering `cpl run` uses (rule 1, with the S75
//! normalization of guard and time faults) and the C renderings against the Rust ones (rule 2), and every driver shuts
//! down clean (rule 3).

use cpl_cabi::NativeSession;
use cpl_diag::{FileOrigin, SymbolPath};
use cpl_eval::testing::corpus::parse_cases;
use cpl_eval::{prepare_run, run_function, value_text, value_to_json, EvalConfig, RunArg};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[path = "differential/builder.rs"]
mod builder;
#[path = "differential/fuzz.rs"]
mod fuzz;
#[path = "differential/harvest.rs"]
mod harvest;
#[path = "differential/scripted.rs"]
mod scripted;

fn eval_corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../cpl-eval/tests/corpus")
}

fn diff_corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/differential/corpus")
}

pub(crate) fn require_cc() -> bool {
    if cpl_rt::testing::find_cc().is_some() {
        return true;
    }
    if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
        panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
    }
    println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
    false
}

/// Debug drivers retain every HIR node for the coverage gate. D8 exercises optimized lowering under ASan/UBSan
/// without coverage, so constant folding and ownership optimizations are compared across the whole corpus.
pub(crate) fn driver_config(sanitize: bool) -> cpl_emit_c::package::DriverConfig {
    use cpl_emit_c::cc::Sanitizer;
    cpl_emit_c::package::DriverConfig {
        cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("differential-cache"),
        profile: if sanitize {
            cpl_emit_c::cc::Profile::Optimized
        } else {
            cpl_emit_c::cc::Profile::Debug
        },
        sanitize: if sanitize {
            vec![Sanitizer::Address, Sanitizer::Undefined]
        } else {
            vec![]
        },
        coverage: !sanitize,
        cc: None,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    }
}

/// A checked project of one module source.
pub(crate) fn project_of(name: &str, module_path: &str, source: &str) -> cpl_project::Project {
    let file = format!("src/{}.cpl", module_path.replace('.', "/"));
    let tree = VirtualTree {
        files: [
            (
                "cpl.toml".to_string(),
                format!("[project]\nname = \"{name}\"\nedition = \"0.1\"\n").into_bytes(),
            ),
            (file, source.as_bytes().to_vec()),
        ]
        .into_iter()
        .collect(),
    };
    load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).unwrap()
}

/// The rendering of an outcome by the code `cpl run` uses.
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

/// What the compared cases exercised, for the coverage gate (toolchain 12.6), and what diverged.
#[derive(Default)]
pub(crate) struct Gate {
    pub nodes: BTreeMap<String, u64>,
    pub builtins: BTreeMap<String, u64>,
    pub claims: BTreeMap<String, u64>,
    pub fault_codes: BTreeSet<String>,
    /// (`clause_kind`, `attributed_to`) of every contract violation
    pub clause_pairs: BTreeSet<(String, String)>,
    /// `stack` (the counter), `stack_guard` (with `depth`), `time`
    pub resources: BTreeSet<String>,
    /// the outcomes of semantics S77 the scripted cases produced
    pub s77: BTreeSet<String>,
    pub compared: usize,
    pub divergences: Vec<String>,
    /// D5: the swept cases compared and skipped, and the undrawable functions
    pub swept: usize,
    pub sweep_skipped: usize,
    pub undrawable: Vec<String>,
    /// D8: the drivers are built with ASan and UBSan
    pub sanitize: bool,
}

impl Gate {
    /// Records a fault's diagnostic JSON.
    pub fn observe(&mut self, d: &Json) {
        let code = d["code"].as_str().unwrap_or_default().to_string();
        if let (Some(k), Some(a)) = (d["clause_kind"].as_str(), d["attributed_to"].as_str()) {
            self.clause_pairs.insert((k.to_string(), a.to_string()));
        }
        match (d["resource"].as_str(), d.get("depth").is_some()) {
            (Some("stack"), true) => {
                self.resources.insert("stack_guard".to_string());
            }
            (Some(r), _) => {
                self.resources.insert(r.to_string());
            }
            _ => {}
        }
        self.fault_codes.insert(code);
    }

    /// Adds a driver's coverage counters.
    pub fn absorb(&mut self, session: &mut NativeSession<'_>) {
        if self.sanitize {
            return;
        }
        let c = session
            .coverage()
            .expect("a coverage driver answers coverage");
        for (k, n) in c.nodes {
            *self.nodes.entry(k).or_default() += n;
        }
        for (k, n) in c.builtins {
            *self.builtins.entry(k).or_default() += n;
        }
        for (k, n) in c.claims {
            *self.claims.entry(k).or_default() += n;
        }
    }

    /// Rules 1 and 2 for one case, the S75 normalization of guard and time faults included; the outcome's faults are
    /// observed.
    #[allow(clippy::too_many_arguments)]
    pub fn compare(
        &mut self,
        session: &NativeSession<'_>,
        p: &cpl_hir::Program,
        sm: &cpl_diag::SourceMap,
        ty: &cpl_hir::Ty,
        eval: &Result<cpl_eval::Value, cpl_eval::Fault>,
        native: &Result<cpl_eval::Value, cpl_eval::Fault>,
        what: String,
    ) {
        self.compared += 1;
        if let (Err(a), Err(b)) = (eval, native) {
            let (a, b) = (
                serde_json::to_value(a.to_diagnostic(p).to_json(sm)).unwrap(),
                serde_json::to_value(b.to_diagnostic(p).to_json(sm)).unwrap(),
            );
            self.observe(&a);
            let guard_or_time = |f: &Json| f.get("depth").is_some() || f["resource"] == "time";
            if guard_or_time(&a) || guard_or_time(&b) {
                // S75 rule 2: reduced, or not compared when only one side is such a fault
                if let Some(d) = cpl_cabi::compare::faults(&a, &b) {
                    self.divergences.push(format!(
                        "{what}:\n  eval:   {}\n  native: {}",
                        d.eval, d.native
                    ));
                }
                return;
            }
        }
        if let Some(d) = compare(session, p, sm, ty, eval, native) {
            self.divergences.push(format!("{what}: {d}"));
        }
    }
}

/// Rules 1 and 2 of the comparison (toolchain 12.6) for one case: the renderings `cpl run` uses, then the native
/// side's own renderings of its outcome against the Rust renderings of it; the divergence, if any.
pub(crate) fn compare(
    session: &NativeSession<'_>,
    p: &cpl_hir::Program,
    sm: &cpl_diag::SourceMap,
    ty: &cpl_hir::Ty,
    eval: &Result<cpl_eval::Value, cpl_eval::Fault>,
    native: &Result<cpl_eval::Value, cpl_eval::Fault>,
) -> Option<String> {
    let a = render(p, sm, ty, eval);
    let b = render(p, sm, ty, native);
    if a != b {
        return Some(format!("\n  eval:   {a}\n  native: {b}"));
    }
    let r = session.native_renderings().unwrap_or_default();
    match native {
        Ok(v) => {
            let json = serde_json::to_string(&cpl_eval::json::value_to_json_untyped(p, v)).unwrap();
            let text = cpl_eval::json::value_text_untyped(p, v);
            if r.json.as_deref() != Some(json.as_str()) || r.text.as_deref() != Some(text.as_str())
            {
                return Some(format!(
                    "C renderings differ:\n  rust json {json}\n  c    json {:?}\n  rust text {text}\n  c    text {:?}",
                    r.json, r.text
                ));
            }
        }
        Err(f) => {
            let d = serde_json::to_string(&f.to_diagnostic(p).to_json(sm)).unwrap();
            if r.diagnostic.as_deref() != Some(d.as_str()) {
                return Some(format!(
                    "the C diagnostic differs:\n  rust {d}\n  c    {:?}",
                    r.diagnostic
                ));
            }
        }
    }
    None
}

/// Rule 3: the driver's runtime closes with zero live handles and objects.
pub(crate) fn assert_clean_shutdown(session: NativeSession<'_>, what: &str) {
    let report = session.shutdown().expect("shutdown");
    assert!(
        report.exit_status.success(),
        "{what}: the driver exited with {:?}",
        report.exit_status
    );
    assert_eq!(
        (
            report.live_handles,
            report.live_caps,
            report.live_objects,
            report.live_buffers
        ),
        (0, 0, 0, 0),
        "{what}: the driver's runtime is not clean at shutdown"
    );
}

/// D5 on one module (toolchain 12.6): every monomorphic project function with at least one parameter, only data
/// parameters, and no `using` parameter, N = 200 x `CPL_DIFF_SCALE` argument tuples, tuple i drawn at size min(i, 100)
/// by the sweep rules from a ChaCha20 stream keyed by `CPL_DIFF_SEED` and the function's identity, each case under a
/// time budget of 250 ms on both engines. A case whose outcome on either engine is a time or guard fault is skipped
/// (one the evaluator skips is not sent to the driver), as is a result whose value JSON exceeds 16 MiB.
pub(crate) fn sweep_module(
    gate: &mut Gate,
    p: &cpl_hir::Program,
    sm: &cpl_diag::SourceMap,
    session: &mut NativeSession<'_>,
) {
    use cpl_eval::testing::sweep::Sweep;
    let seed = std::env::var("CPL_DIFF_SEED").unwrap_or_else(|_| "0".to_string());
    let scale: usize = std::env::var("CPL_DIFF_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let n = 200 * scale;
    let cfg = EvalConfig {
        time_budget: Some(std::time::Duration::from_millis(250)),
        ..EvalConfig::default()
    };
    session.reconfigure(&cfg).expect("reconfigure");
    let skipped_fault = |r: &Result<cpl_eval::Value, cpl_eval::Fault>| match r {
        Err(f) => {
            let d = serde_json::to_value(f.to_diagnostic(p).to_json(sm)).unwrap();
            d.get("depth").is_some() || d["resource"] == "time"
        }
        Ok(_) => false,
    };
    for def in p.defs() {
        let Some(fd) = def.kind.as_fn() else { continue };
        let module = &p.module(def.module).path;
        if module.first().is_some_and(|m| m == "std")
            || !matches!(fd.body, cpl_hir::FnBody::Block(_))
            || !fd.generics.is_empty()
            || fd.params.is_empty()
            || !fd.using.is_empty()
        {
            continue;
        }
        let path = p.symbol_path(def.id);
        let key = blake3::hash(format!("{seed}\n{}", def.stable_id.as_str()).as_bytes());
        let mut sweep = Sweep::new(*key.as_bytes());
        let (mut compared, mut skipped) = (0usize, 0usize);
        'cases: for i in 0..n {
            let size = (i as u32).min(100);
            let mut args = Vec::with_capacity(fd.params.len());
            for prm in &fd.params {
                match sweep.draw(p, &prm.ty, size) {
                    Ok(v) => args.push(RunArg::Json(value_to_json(p, &v, &prm.ty))),
                    Err(why) => {
                        gate.undrawable
                            .push(format!("{} ({why:?})", p.display_path(def.id)));
                        break 'cases;
                    }
                }
            }
            let eval = run_function(p, &path, &args, &cfg).expect("the evaluator runs the case");
            if skipped_fault(&eval.value) {
                skipped += 1;
                continue;
            }
            if let Ok(v) = &eval.value {
                if value_to_json(p, v, &eval.return_ty).to_string().len() > 16 << 20 {
                    skipped += 1;
                    continue;
                }
            }
            let prepared = prepare_run(p, &path, &args, &|_| false).expect("prepare_run");
            let native = session
                .call(prepared.function, &prepared.arguments, &[], true)
                .unwrap_or_else(|e| {
                    panic!(
                        "{}: the driver failed: {:?}",
                        def.stable_id.as_str(),
                        e.into_diagnostics()
                    )
                });
            if skipped_fault(&native) {
                skipped += 1;
                continue;
            }
            compared += 1;
            let before = gate.divergences.len();
            gate.compare(
                session,
                p,
                sm,
                &eval.return_ty,
                &eval.value,
                &native,
                format!("D5 {} (seed {seed}, case {i})", p.display_path(def.id)),
            );
            if gate.divergences.len() > before {
                let shown: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        RunArg::Json(j) => j.to_string(),
                        _ => String::new(),
                    })
                    .collect();
                let last = gate.divergences.last_mut().unwrap();
                last.push_str(&format!("\n  arguments: [{}]", shown.join(", ")));
            }
        }
        gate.swept += compared;
        gate.sweep_skipped += skipped;
        if std::env::var("CPL_DIFF_VERBOSE").is_ok() {
            println!(
                "  D5 {}: {compared} compared, {skipped} skipped",
                p.display_path(def.id)
            );
        }
    }
    session
        .reconfigure(&EvalConfig::default())
        .expect("reconfigure");
}

/// D1: the interpreter corpus, each case with contracts checked and unchecked.
fn d1(gate: &mut Gate) {
    let mut names: Vec<String> = std::fs::read_dir(eval_corpus_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".cpl").map(str::to_string))
        .collect();
    names.sort();
    for name in &names {
        let source =
            std::fs::read_to_string(eval_corpus_dir().join(format!("{name}.cpl"))).unwrap();
        let cases = parse_cases(
            &std::fs::read_to_string(eval_corpus_dir().join(format!("{name}.run.json"))).unwrap(),
        )
        .unwrap();
        let module = source
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("module "))
            .unwrap()
            .trim()
            .to_string();
        let project = project_of("corpus", &module, &source);
        let checked = cpl_sema::check(&project);
        assert!(!checked.has_errors(), "{name}: {:?}", checked.diagnostics);
        let p = &checked.program;
        let sm = project.source_map.clone();
        let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
        for check_contracts in [true, false] {
            let cfg = EvalConfig {
                check_contracts,
                ..EvalConfig::default()
            };
            let mut session =
                NativeSession::start(p, &project, &plan, &cfg, &driver_config(gate.sanitize))
                    .unwrap_or_else(|e| {
                        panic!(
                            "{name}: the native engine does not start: {:?}",
                            e.into_diagnostics()
                        )
                    });
            session.set_request_timeout(Some(std::time::Duration::from_secs(60)));
            for case in &cases {
                let path = SymbolPath::parse(&case.function).unwrap();
                let args: Vec<RunArg> = case.args.iter().cloned().map(RunArg::Json).collect();
                let eval =
                    run_function(p, &path, &args, &cfg).expect("the evaluator runs the case");
                let prepared = prepare_run(p, &path, &args, &|_| false).expect("prepare_run");
                let native = session
                    .call(prepared.function, &prepared.arguments, &[], true)
                    .unwrap_or_else(|e| {
                        panic!(
                            "{}: the driver failed: {:?}",
                            case.function,
                            e.into_diagnostics()
                        )
                    });
                gate.compare(
                    &session,
                    p,
                    &sm,
                    &eval.return_ty,
                    &eval.value,
                    &native,
                    format!(
                        "D1 {} {:?} (check_contracts {check_contracts})",
                        case.function, case.args
                    ),
                );
            }
            if check_contracts {
                sweep_module(gate, p, &sm, &mut session);
            }
            gate.absorb(&mut session);
            assert_clean_shutdown(session, name);
        }
    }
}

/// One case of a `*.diff.json` file (toolchain 12.6): T49's `function` and `args`, and optionally `config` and
/// `expect`; any other key is a harness error.
struct DiffCase {
    function: String,
    args: Vec<Json>,
    config: EvalConfig,
    expect: Option<Json>,
}

fn parse_diff_cases(text: &str) -> Vec<DiffCase> {
    let doc: Json = serde_json::from_str(text).expect("a *.diff.json is JSON");
    doc.as_array()
        .expect("a *.diff.json is an array of cases")
        .iter()
        .map(|c| {
            let m = c.as_object().expect("a case is an object");
            for k in m.keys() {
                assert!(
                    ["function", "args", "config", "expect"].contains(&k.as_str()),
                    "a case key {k:?}: {c}"
                );
            }
            let mut config = EvalConfig::default();
            if let Some(cf) = m.get("config").and_then(Json::as_object) {
                for (k, v) in cf {
                    match k.as_str() {
                        "max_call_depth" => config.max_call_depth = v.as_u64().unwrap() as u32,
                        "time_budget_ms" => {
                            config.time_budget =
                                Some(std::time::Duration::from_millis(v.as_u64().unwrap()))
                        }
                        "check_contracts" => config.check_contracts = v.as_bool().unwrap(),
                        _ => panic!("a config key {k:?}: {c}"),
                    }
                }
            }
            DiffCase {
                function: m["function"].as_str().unwrap().to_string(),
                args: m["args"].as_array().unwrap().clone(),
                config,
                expect: m.get("expect").cloned(),
            }
        })
        .collect()
}

/// D2 and D4's JSON cases: every `differential/corpus/*.cpl` with its `*.diff.json`; then the module's tests and
/// properties, record by record.
fn corpus(gate: &mut Gate) {
    let mut names: Vec<String> = std::fs::read_dir(diff_corpus_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".diff.json").map(str::to_string))
        .collect();
    names.sort();
    for name in &names {
        let started = std::time::Instant::now();
        let source =
            std::fs::read_to_string(diff_corpus_dir().join(format!("{name}.cpl"))).unwrap();
        let cases = parse_diff_cases(
            &std::fs::read_to_string(diff_corpus_dir().join(format!("{name}.diff.json"))).unwrap(),
        );
        let module = source
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("module "))
            .unwrap()
            .trim()
            .to_string();
        let project = project_of(name, &module, &source);
        let checked = cpl_sema::check(&project);
        assert!(!checked.has_errors(), "{name}: {:?}", checked.diagnostics);
        let p = &checked.program;
        let sm = project.source_map.clone();
        let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
        let mut session = NativeSession::start(
            p,
            &project,
            &plan,
            &EvalConfig::default(),
            &driver_config(gate.sanitize),
        )
        .unwrap_or_else(|e| {
            panic!(
                "{name}: the native engine does not start: {:?}",
                e.into_diagnostics()
            )
        });
        session.set_request_timeout(Some(std::time::Duration::from_secs(60)));
        let mut current = EvalConfig::default();
        for case in &cases {
            let path = SymbolPath::parse(&case.function).unwrap();
            let case_started = std::time::Instant::now();
            let args: Vec<RunArg> = case.args.iter().cloned().map(RunArg::Json).collect();
            let eval =
                run_function(p, &path, &args, &case.config).expect("the evaluator runs the case");
            let eval_time = case_started.elapsed();
            // the expectation is checked against the evaluator first
            if let Some(expect) = &case.expect {
                let ok = match (&eval.value, expect.get("value"), expect.get("fault")) {
                    (Ok(v), Some(want), _) => &value_to_json(p, v, &eval.return_ty) == want,
                    (Err(f), _, Some(code)) => f.to_diagnostic(p).code.0 == code.as_str().unwrap(),
                    _ => false,
                };
                assert!(
                    ok,
                    "{name}: {} {:?}: the evaluator gives {}, the case expects {expect}",
                    case.function,
                    case.args,
                    render(p, &sm, &eval.return_ty, &eval.value)
                );
            }
            if case.config != current {
                session.reconfigure(&case.config).expect("reconfigure");
                current = case.config.clone();
            }
            let prepared = prepare_run(p, &path, &args, &|_| false).expect("prepare_run");
            let native = session
                .call(prepared.function, &prepared.arguments, &[], true)
                .unwrap_or_else(|e| {
                    panic!(
                        "{}: the driver failed: {:?}",
                        case.function,
                        e.into_diagnostics()
                    )
                });
            gate.compare(
                &session,
                p,
                &sm,
                &eval.return_ty,
                &eval.value,
                &native,
                format!("{name} {} {:?}", case.function, case.args),
            );
            if case_started.elapsed() > std::time::Duration::from_secs(2) {
                println!(
                    "  slow case {} (eval {eval_time:?}, total {:?})",
                    case.function,
                    case_started.elapsed()
                );
            }
        }
        // the module's tests and properties
        if current != EvalConfig::default() {
            session
                .reconfigure(&EvalConfig::default())
                .expect("reconfigure");
        }
        // a budget for each top-level evaluation (a test that never ends is a time fault, compared by rule 3) and a
        // bound on the shrink reruns of a failing property, the same on both engines
        let mut config = cpl_eval::TestConfig::with_seed(1);
        config.eval.time_budget = Some(std::time::Duration::from_millis(200));
        config.max_shrinks = 50;
        session.reconfigure(&config.eval).expect("reconfigure");
        for test in cpl_eval::select_tests(p, &cpl_eval::TestSelector::default()) {
            let a = cpl_eval::run_test(p, test, &config);
            let b = cpl_eval::run_test_with(p, test, &config, &mut session.clone());
            gate.compared += 1;
            if let Some(d) = cpl_cabi::compare::test_records(&a, &b, sanctioned(&a, &b)) {
                gate.divergences.push(format!(
                    "{name} test {}:\n  eval:   {}\n  native: {}",
                    a.qualified_name, d.eval, d.native
                ));
            }
        }
        sweep_module(gate, p, &sm, &mut session);
        gate.absorb(&mut session);
        assert_clean_shutdown(session, name);
        println!("  {name}: {:?}", started.elapsed());
    }
}

/// Whether either record of one test met a time or guard fault (rule 3 of the S75 normalization).
pub(crate) fn sanctioned(
    a: &cpl_diag::json::TestCaseRecord,
    b: &cpl_diag::json::TestCaseRecord,
) -> bool {
    [a, b].iter().any(|r| {
        let j = serde_json::to_string(r).unwrap_or_default();
        j.contains("\"resource\":\"time\"") || j.contains("\"depth\":")
    })
}

impl Gate {
    /// Adds another gate's observations (a worker's).
    pub fn merge(&mut self, o: Gate) {
        for (k, n) in o.nodes {
            *self.nodes.entry(k).or_default() += n;
        }
        for (k, n) in o.builtins {
            *self.builtins.entry(k).or_default() += n;
        }
        for (k, n) in o.claims {
            *self.claims.entry(k).or_default() += n;
        }
        self.fault_codes.extend(o.fault_codes);
        self.clause_pairs.extend(o.clause_pairs);
        self.resources.extend(o.resources);
        self.s77.extend(o.s77);
        self.compared += o.compared;
        self.divergences.extend(o.divergences);
        self.swept += o.swept;
        self.sweep_skipped += o.sweep_skipped;
        self.undrawable.extend(o.undrawable);
    }
}

/// The coverage gate (toolchain 12.6): what must have occurred at least once; the missing items.
fn gate_misses(gate: &Gate) -> Vec<String> {
    let mut missing = Vec::new();
    for k in cpl_emit_c::COVERAGE_KINDS {
        // ExprKind::Error never reaches emission (a program with errors is not emitted)
        if k != "ExprKind::Error" && gate.nodes.get(k).copied().unwrap_or(0) == 0 {
            missing.push(format!("node {k}"));
        }
    }
    for spec in cpl_rtspec::builtins::BUILTINS.iter().filter(|s| s.callable) {
        if gate.builtins.get(spec.id).copied().unwrap_or(0) == 0 {
            missing.push(format!("builtin {}", spec.id));
        }
    }
    for code in [
        "contract_violation",
        "assertion_failed",
        "unresolved_hole",
        "division_by_zero",
        "refinement_violation",
        "resource_exhaustion",
        "internal_error",
    ] {
        if !gate.fault_codes.contains(code) {
            missing.push(format!("fault {code}"));
        }
    }
    for (k, a) in [
        ("requires", "caller"),
        ("ensures", "body"),
        ("ensures", "binding"),
        ("invariant", "body"),
        ("decreases", "body"),
    ] {
        if !gate.clause_pairs.contains(&(k.to_string(), a.to_string())) {
            missing.push(format!("clause ({k}, {a})"));
        }
    }
    for r in ["stack", "stack_guard", "time"] {
        if !gate.resources.contains(r) {
            missing.push(format!("resource {r}"));
        }
    }
    for c in [
        "requires",
        "ensures",
        "header_decreases",
        "invariant",
        "loop_decreases",
        "where",
        "op_requires",
        "op_ensures",
        "range",
        "already_claimed",
    ] {
        if gate.claims.get(c).copied().unwrap_or(0) == 0 {
            missing.push(format!("claim {c}"));
        }
    }
    for o in scripted::S77_OUTCOMES {
        if !gate.s77.contains(o) {
            missing.push(format!("S77 {o}"));
        }
    }
    missing
}

/// D1, D2, D4 (corpus, scripted, built), and D5 into one gate.
fn run_suites(gate: &mut Gate) {
    let t = std::time::Instant::now();
    d1(gate);
    println!("D1: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    corpus(gate);
    println!("D2 and D4 corpus: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    scripted::d4_scripted(gate);
    println!("D4 scripted: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    builder::d4_builder(gate);
    println!("D4 built: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let (ran, unchecked) = harvest::d6(gate);
    println!("D6: {ran} harvested programs run, {unchecked} that do not check on their own skipped: {:?}", t.elapsed());
    println!(
        "D5: {} swept cases compared, {} skipped, {} undrawable functions {:?}",
        gate.swept,
        gate.sweep_skipped,
        gate.undrawable.len(),
        gate.undrawable
    );
    assert!(
        gate.divergences.is_empty(),
        "{} divergences:\n{}",
        gate.divergences.len(),
        gate.divergences.join("\n")
    );
    let total = gate.swept + gate.sweep_skipped;
    assert!(
        gate.sweep_skipped * 50 <= total,
        "D5 skipped {} of {total} cases, more than 2%",
        gate.sweep_skipped
    );
}

#[test]
fn differential_suites_with_the_coverage_gate() {
    if !require_cc() {
        return;
    }
    let mut gate = Gate::default();
    run_suites(&mut gate);
    println!(
        "differential: {} cases compared, {} divergences; nodes {:?}; claims {:?}; faults {:?}; resources {:?}",
        gate.compared,
        gate.divergences.len(),
        gate.nodes,
        gate.claims,
        gate.fault_codes,
        gate.resources
    );
    let missing = gate_misses(&gate);
    // CPL_DIFF_GATE=report prints the misses instead of failing (while a corpus is written)
    if std::env::var("CPL_DIFF_GATE").is_ok_and(|v| v == "report") {
        println!(
            "coverage gate: {} missing:\n{}",
            missing.len(),
            missing.join("\n")
        );
        return;
    }
    assert!(
        missing.is_empty(),
        "the coverage gate misses {}:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

/// D8 (toolchain 12.6): the same suites with the drivers built `--sanitize address,undefined` and run with
/// `ASAN_OPTIONS=detect_leaks=1:halt_on_error=1` and `UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1`: a report
/// stops the driver (the session fails) or makes it exit non-zero (rule 3 fails), and every runtime closes with zero
/// live objects.
#[test]
fn d8_sanitized_suites() {
    if !require_cc() {
        return;
    }
    let sanitizers = cpl_emit_c::cc::Compiler::discover(None, None).is_ok_and(|c| {
        c.supports(&[
            cpl_emit_c::cc::Sanitizer::Address,
            cpl_emit_c::cc::Sanitizer::Undefined,
        ])
    });
    if !sanitizers {
        if std::env::var("CPL_REQUIRE_SANITIZERS").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_SANITIZERS=1, but the compiler cannot link address,undefined");
        }
        println!("skipped: the compiler cannot link the sanitizers address,undefined (set CPL_REQUIRE_SANITIZERS=1 to fail)");
        return;
    }
    // the drivers inherit the environment of the test process
    std::env::set_var("ASAN_OPTIONS", "detect_leaks=1:halt_on_error=1");
    std::env::set_var("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1");
    let mut gate = Gate {
        sanitize: true,
        ..Gate::default()
    };
    run_suites(&mut gate);
    println!("D8: {} cases compared under ASan and UBSan", gate.compared);
}

/// D7 (toolchain 12.6): the runtime fuzzed against the evaluator (`differential/fuzz.rs`).
#[test]
fn d7_runtime_fuzz() {
    if !require_cc() {
        return;
    }
    let mut gate = Gate::default();
    let t = std::time::Instant::now();
    fuzz::d7(&mut gate);
    println!("D7: {} inputs compared: {:?}", gate.compared, t.elapsed());
    assert!(
        gate.divergences.is_empty(),
        "{} divergences:\n{}",
        gate.divergences.len(),
        gate.divergences.join("\n")
    );
}
