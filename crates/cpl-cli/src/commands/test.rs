//! `cpl test [selector] [--json] [--jsonl] [--seed N] [--cases N] [--name TEXT] [--id ID] [--fail-fast] [--max-shrinks N] [--timeout-ms N] [--receipt PATH]`
//! (`spec/toolchain.md` 7.2, 1.7.2, 8.2; semantics 7, S44-S46, S59).
//!
//! Checks the project (errors: exit 1, nothing runs), selects the tests and
//! properties (`cpl_eval::select_tests`), runs each with `cpl_eval::run_test`
//! in qualified-name order, stopping after the first failure under
//! `--fail-fast` exactly as `cpl_eval::run_tests` does, so that every record
//! is complete, and its diagnostics emitted, as soon as it finishes (the
//! progress `--jsonl` exists for). Each record gets its S59 `receipt`
//! (`cpl_index::Index::test_receipt_core`) before its diagnostics are
//! emitted: its `contract_counterexample`s, then its `test_result`. An
//! empty selection is `empty_test_selection` (exit 1). The output carries
//! the report and the test receipt of Section 8.2
//! (`Index::complete_test_report`), which `--receipt PATH` also writes.
//!
//! The settings come from the flags, else `[test]` (`cases`, `max_shrinks`,
//! `timeout_ms`, 0 = no budget) and `[run] max_call_depth`; the run seed is
//! `--seed` or 64 random bits from the OS, always reported.
//!
//! Human output: one line per test on stdout as it completes (`ok` or
//! `FAILED`, padded to nine columns, then the qualified name; a failure
//! adds the fault's two stable lines and, for a property, the shrunk
//! input), then `test: 2 tested, 1 failed; seed 1446357982114401307`. The
//! stdout lines are the text form of the `test_result` diagnostics, which
//! stderr therefore does not repeat; every other diagnostic goes to stderr.

use super::{check_diagnostics_for_execution, Ctx};
use crate::cli::TestArgs;
use crate::codes;
use crate::output::{compact, headline, plural, pretty, print_stdout, Failure, Format, Report};
use crate::project::absolute;
use cpl_diag::json::{
    DiagnosticJson, EngineKind, EngineTestOutput, JsonMap, Status, TestCaseRecord, TestOutput,
    TestReport, TestRunConfig, TestSelectorSpec, TestStatus,
};
use cpl_diag::{Category, Diagnostic, StableId};
use cpl_eval::{
    run_test, run_test_with, select_tests, test_result_diagnostic, value_from_json, value_text,
    EvalConfig, Interpreter, TestConfig, TestSelector,
};
use cpl_hir::{DefKind, Program};
use cpl_index::Index;
use cpl_sema::CheckConfig;
use rand::Rng as _;
use serde_json::json;
use std::time::{Duration, Instant};

pub fn run(ctx: &Ctx, args: &TestArgs) -> Result<Report<EngineTestOutput>, Failure> {
    let engine = match args.engine.as_str() {
        "native" => EngineKind::Native,
        "both" => EngineKind::Both,
        _ => EngineKind::Eval,
    };
    let mut selector = match &args.selector {
        Some(text) => TestSelector::parse(text).map_err(|message| {
            ctx.session
                .refuse(codes::bad_argument(message).with_data("selector", json!(text)))
        })?,
        None => TestSelector::all(),
    };
    if let Some(id) = &args.id {
        selector.id = Some(StableId::parse(id).map_err(|e| {
            ctx.session.refuse(
                codes::bad_argument(format!("--id {id}: not a stable id: {e}"))
                    .with_data("id", json!(id)),
            )
        })?);
    }
    selector.name = args.name.clone();

    let opened = ctx.session.open(false)?;
    let project = &opened.project;
    let sm = project.source_map.clone();
    let checked = cpl_sema::check_with(project, &CheckConfig::default());
    let check_diagnostics = check_diagnostics_for_execution(&checked.diagnostics);
    if checked.has_errors() {
        return Err(Failure::new(
            Status::Unsuccessful,
            Some(opened.revision.clone()),
            &check_diagnostics,
            Some(sm),
        ));
    }
    let program = &checked.program;
    let mut diagnostics: Vec<DiagnosticJson> = Vec::new();
    let emit = |d: DiagnosticJson, all: &mut Vec<DiagnosticJson>| {
        ctx.out.stream(&d);
        all.push(d);
    };
    for d in &check_diagnostics {
        emit(d.to_json(&sm), &mut diagnostics);
    }

    let manifest = &project.manifest;
    let seed = args.seed.unwrap_or_else(|| rand::rng().next_u64());
    let timeout_ms = args.timeout_ms.unwrap_or(manifest.test.timeout_ms);
    let config = TestConfig {
        seed,
        cases: args.cases.unwrap_or(manifest.test.cases),
        max_shrinks: args.max_shrinks.unwrap_or(manifest.test.max_shrinks),
        fail_fast: args.fail_fast,
        eval: EvalConfig {
            max_call_depth: manifest.run.max_call_depth,
            time_budget: (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms)),
            ..EvalConfig::default()
        },
    };
    let spec = TestSelectorSpec::from(selector.clone());
    let run_config = TestRunConfig {
        seed,
        cases: config.cases,
        max_shrinks: config.max_shrinks,
        timeout_ms,
        selector: spec.clone(),
        check_contracts: config.eval.check_contracts,
        fail_fast: config.fail_fast,
    };
    let max_call_depth = config.eval.max_call_depth;
    let mut check_configuration = cpl_index::test_check_configuration(&run_config, max_call_depth);
    let index = Index::build(program, project, opened.revision.clone());
    // The native engine (native and both): one driver for the run, reconfigured before every test (T73).
    let mut session = None;
    if engine != EngineKind::Eval {
        let plan = checked
            .interop
            .clone()
            .unwrap_or_else(|| cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix()));
        match cpl_cabi::NativeSession::start(
            program,
            project,
            &plan,
            &config.eval,
            &super::run::driver_config(project),
        ) {
            Ok(s) => {
                if let serde_json::Value::Object(cc) = &mut check_configuration {
                    cc.insert("engine".to_string(), json!(engine.as_str()));
                    cc.insert(
                        "native".to_string(),
                        json!({
                            "runtime_digest": s.build().runtime_digest.as_str(),
                            "compiler_digest": s.build().compiler.digest.as_str(),
                        }),
                    );
                }
                session = Some(s);
            }
            Err(e) => {
                let (status, ds) = super::run::native_failure(e);
                let mut all: Vec<Diagnostic> = check_diagnostics.clone();
                all.extend(ds);
                return Err(Failure::new(
                    status,
                    Some(opened.revision.clone()),
                    &all,
                    Some(sm),
                ));
            }
        }
    }

    let started = Instant::now();
    let selected = select_tests(program, &selector);
    let mut results: Vec<TestCaseRecord> = Vec::with_capacity(selected.len());
    let mut engine_errors: Vec<Diagnostic> = Vec::new();
    for &test in &selected {
        let mut record = match (&mut session, engine) {
            (None, _) => run_test(program, test, &config),
            (Some(native), EngineKind::Native) => {
                if let Err(e) = native.reconfigure(&config.eval) {
                    engine_errors.extend(super::run::native_failure(e).1);
                    break;
                }
                let record = run_test_with(program, test, &config, native);
                if let Some(e) = native.take_error() {
                    engine_errors.extend(super::run::native_failure(e).1);
                    break;
                }
                record
            }
            (Some(native), _) => {
                let mut watched = Watch {
                    inner: Interpreter::new(program, config.eval.clone()),
                    sanctioned: false,
                };
                let eval_record = run_test_with(program, test, &config, &mut watched);
                if let Err(e) = native.reconfigure(&config.eval) {
                    engine_errors.extend(super::run::native_failure(e).1);
                    break;
                }
                let mut watched_native = Watch {
                    inner: native.clone(),
                    sanctioned: false,
                };
                let native_record = run_test_with(program, test, &config, &mut watched_native);
                if let Some(e) = native.take_error() {
                    engine_errors.extend(super::run::native_failure(e).1);
                    break;
                }
                let sanctioned = watched.sanctioned || watched_native.sanctioned;
                if let Some(div) =
                    cpl_cabi::compare::test_records(&eval_record, &native_record, sanctioned)
                {
                    let d = Diagnostic::error(
                        cpl_cabi::codes::ENGINE_DIVERGENCE,
                        Category::Internal,
                        format!("the native engine's test record differs from the evaluator's at byte {}", div.offset),
                    )
                    .with_data("eval", json!(div.eval))
                    .with_data("native", json!(div.native))
                    .with_data("test", json!(eval_record.qualified_name));
                    engine_errors.push(d);
                }
                eval_record
            }
        };
        record.receipt = Some(index.test_receipt_core(&[test], check_configuration.clone()));
        for counterexample in &record.counterexamples {
            emit(counterexample.clone(), &mut diagnostics);
        }
        emit(
            test_result_diagnostic(&record).to_json(&sm),
            &mut diagnostics,
        );
        if ctx.format() == Format::Human {
            print_stdout(&record_lines(program, &record));
        }
        let failed = record.status == TestStatus::Failed;
        results.push(record);
        if failed && config.fail_fast {
            break;
        }
    }
    let failed = results
        .iter()
        .filter(|r| r.status == TestStatus::Failed)
        .count() as u32;
    let mut report = TestReport {
        seed,
        cases: config.cases,
        selected: selected.len() as u32,
        tested: results.len() as u32 - failed,
        failed,
        results,
    };
    if selected.is_empty() {
        let d = Diagnostic::error(
            codes::EMPTY_TEST_SELECTION,
            Category::Test,
            "no test or property matches the selection",
        )
        .with_data("selector", serde_json::to_value(&spec).expect("serializes"));
        emit(d.to_json(&sm), &mut diagnostics);
    }

    let mut receipt =
        index.complete_test_report(&mut report, &selected, &run_config, max_call_depth);
    receipt.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let mut status = if report.failed > 0 || report.selected == 0 {
        Status::Unsuccessful
    } else {
        Status::Ok
    };
    if !engine_errors.is_empty() {
        for d in &engine_errors {
            emit(d.to_json(&sm), &mut diagnostics);
        }
        status = Status::Error;
    }
    if let Some(native) = session.take() {
        let _ = native.shutdown();
    }
    if let Some(path) = &args.receipt {
        let full = absolute(&ctx.session.cwd, path);
        if let Err(e) = std::fs::write(&full, format!("{}\n", pretty(&receipt))) {
            let d = codes::internal(
                "io",
                format!("cannot write the test receipt to {}: {e}", path.display()),
            )
            .with_data("path", json!(path.display().to_string()));
            emit(d.to_json(&sm), &mut diagnostics);
            status = Status::Error;
        }
    }
    let stdout = format!(
        "test: {} tested, {} failed; seed {seed}\n",
        report.tested, report.failed
    );
    let mut out = Report::new(
        "test",
        Some(opened.revision.clone()),
        status,
        diagnostics,
        EngineTestOutput {
            test: TestOutput { report, receipt },
            engine: (engine != EngineKind::Eval).then_some(engine),
        },
    )
    .with_stdout(stdout)
    .with_sources(sm);
    out.stdout_codes = &["test_result"];
    out.streamed = ctx.format() == Format::Jsonl;
    Ok(out)
}

/// The human lines of one record: `ok       <name>`, or `FAILED   <name>`
/// with the failing case and shrink steps of a property, the fault's two
/// stable lines (Section 1.4.1), and a property's shrunk input.
fn record_lines(program: &Program, record: &TestCaseRecord) -> String {
    const INDENT: &str = "         ";
    match record.status {
        TestStatus::Tested => format!("ok       {}\n", record.qualified_name),
        TestStatus::Failed => {
            let mut out = format!("FAILED   {}", record.qualified_name);
            if let (Some(case), Some(shrunk)) = (&record.failing_case, &record.shrunk) {
                out.push_str(&format!(
                    "  (case {}; shrunk in {})",
                    case.index,
                    plural(shrunk.shrink_steps.into(), "step", "steps")
                ));
            }
            out.push('\n');
            if let Some(fault) = &record.fault {
                out.push_str(&headline(fault, INDENT));
            }
            if let Some(shrunk) = &record.shrunk {
                out.push_str(&format!(
                    "{INDENT}shrunk: {}\n",
                    tuple_text(program, &record.symbol, &shrunk.input)
                ));
            }
            out
        }
    }
}

/// A binder tuple as `items = [0, 0, 0], size = 2`: the binders in clause
/// order, each value in value text (decoded from its value JSON against the
/// binder's type), compact JSON for a value that does not decode.
fn tuple_text(program: &Program, property: &StableId, input: &JsonMap) -> String {
    let binders = program
        .by_stable_id(property)
        .and_then(|def| match &program.def(def).kind {
            DefKind::Property(pd) => Some(pd),
            _ => None,
        });
    let Some(pd) = binders else {
        return input
            .iter()
            .map(|(name, value)| format!("{name} = {}", compact(value)))
            .collect::<Vec<_>>()
            .join(", ");
    };
    pd.binders
        .iter()
        .filter_map(|binder| {
            let name = pd.locals.get(binder.local.0 as usize)?.name.clone();
            let json = input.get(&name)?;
            let text = match value_from_json(program, json, &binder.elem_ty) {
                Ok(value) => value_text(program, &value, &binder.elem_ty),
                Err(_) => compact(json),
            };
            Some(format!("{name} = {text}"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A TestEngine that notes whether any evaluation produced a time fault or a stack-guard fault (rule 3 of the S75
/// normalization, toolchain 12.6).
struct Watch<E> {
    inner: E,
    sanctioned: bool,
}

impl<E> Watch<E> {
    fn note<T>(&mut self, r: &Result<T, cpl_eval::Fault>) {
        if let Err(f) = r {
            if matches!(
                f.kind,
                cpl_eval::FaultKind::ResourceExhaustion(cpl_eval::Resource::Time)
            ) || (matches!(
                f.kind,
                cpl_eval::FaultKind::ResourceExhaustion(cpl_eval::Resource::Stack)
            ) && f.data.contains_key("depth"))
            {
                self.sanctioned = true;
            }
        }
    }
}

impl<'p, E: cpl_eval::TestEngine<'p>> cpl_eval::TestEngine<'p> for Watch<E> {
    fn program(&self) -> &'p Program {
        self.inner.program()
    }
    fn run_body(
        &mut self,
        test: cpl_hir::DefId,
        binders: &[(cpl_hir::LocalId, cpl_eval::Value)],
    ) -> Result<(), cpl_eval::Fault> {
        let r = self.inner.run_body(test, binders);
        self.note(&r);
        r
    }
    fn take_violation(&mut self) -> Option<(cpl_hir::DefId, cpl_eval::Fault)> {
        self.inner.take_violation()
    }
    fn failing_refinement(
        &mut self,
        refined: cpl_hir::DefId,
        value: &cpl_eval::Value,
        budget: cpl_eval::Budget,
    ) -> Result<Option<cpl_hir::DefId>, cpl_eval::Fault> {
        let r = self.inner.failing_refinement(refined, value, budget);
        self.note(&r);
        r
    }
    fn eval_range_bound(
        &mut self,
        property: cpl_hir::DefId,
        binder: usize,
        side: cpl_eval::RangeSide,
        clause_text: &str,
    ) -> Result<cpl_eval::Value, cpl_eval::Fault> {
        let r = self
            .inner
            .eval_range_bound(property, binder, side, clause_text);
        self.note(&r);
        r
    }
}
