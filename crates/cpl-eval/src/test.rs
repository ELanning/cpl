//! The test and property runner (`spec/toolchain.md` 1.7.2; semantics 7;
//! stdlib 8). [`TestReport`] and [`TestCaseRecord`] are the wire types of
//! `cpl_diag::json` (toolchain T3), re-exported here.
//!
//! * Selection ([`select_tests`]): every `test` and `property` whose module
//!   path starts with the selector's namespace, filtered by name and id,
//!   in qualified-name order.
//! * A plain `test` runs its body once; it is `tested` when the body
//!   completes and `failed` with the fault otherwise (semantics 7.3).
//! * A `property` gets one ChaCha20 stream keyed by [`property_seed`]
//!   (S44). Its `gen.range` bounds are evaluated once, before case 0 (a
//!   fault there fails it with zero cases; the range is a contract
//!   expression, so such a fault is reported per S34 and S71 with the
//!   range's clause text, its canonical text on one line (T56), as
//!   `clause`, while the generator's own `requires lo <= hi` keeps the
//!   shape of stdlib 12); case `i` runs at size
//!   `min(i, 100)` and draws its binders in clause order from the stream
//!   (stdlib 8.3), so `--cases N` changes no case a smaller `N` runs. A
//!   drawn value of a refinement type is checked against its predicates,
//!   as `check<R>` would.
//! * A failing case is shrunk exactly as S45 and stdlib 8.4 say: greedy,
//!   coordinate-wise over the binders in clause order, the first candidate
//!   that satisfies the binder type's refinements and fails *with the same
//!   fault code* is accepted and shrinking continues from it; a pass that
//!   accepts nothing ends it, as does `max_shrinks` reruns (then
//!   `shrink_exhausted` is true: a rerun was wanted but the budget was
//!   spent).
//! * A failure caused by an `ensures`, `invariant`, or `decreases`
//!   violation in a declaration other than the test itself also yields a
//!   `contract_counterexample` (S46), built from the violation as it was
//!   raised; a fault in the range setup is the property's fault like any
//!   other, so this holds for it too.
//!
//! The runner is generic over the engine that evaluates
//! ([`TestEngine`], toolchain 1.7.4 and T77): [`run_tests_with`] and
//! [`run_test_with`] take any engine, and [`run_tests`] and [`run_test`]
//! are them over a fresh [`Interpreter`] per test, with the Milestone 1
//! results byte for byte.
//!
//! Every body evaluation (each test, each property case, each shrink
//! rerun) is separate: it gets the time budget afresh, and nothing else
//! the runner evaluates is budgeted (the range setup, the refinement checks
//! of drawn values and shrink candidates; toolchain 7.2). The whole run
//! evaluates on the large-stack thread of [`crate::stack`]. A record's
//! `receipt` is left for the caller, which knows the revision and the
//! module hashes (Appendix C).

mod clause;
pub(crate) mod gen;

use crate::builtins;
use crate::fault::{ClauseKind, Fault, FaultKind, Frame};
use crate::interp::{Budget, EvalConfig, Interpreter};
use crate::json::{refined_positions, value_to_json};
use crate::stack::on_eval_stack;
use crate::value::Value;
use cpl_diag::json::{to_data, DiagnosticJson, FailingCase, ShrunkCase, TestKind, TestStatus};
use cpl_diag::{Category, DeclName, DiagCode, Diagnostic, Facet, FileId, StableId, RESERVED_KEYS};
use cpl_hir::{Binder, DefId, DefKind, Expr, GenExpr, LocalId, Program, PropertyDef, Ty};
use cpl_rtspec::messages::{self, render};
use cpl_syntax::TokenKind;
use gen::{Gen, Stream};
use num_bigint::BigInt;
use serde_json::{json, Map, Value as Json};
use std::time::Instant;

pub use cpl_diag::json::{TestCaseRecord, TestReport, TestSelectorSpec};

/// The code of the per-test diagnostic (semantics S46).
pub const TEST_RESULT: DiagCode = DiagCode("test_result");
/// The code of the counterexample diagnostic (semantics S46, 5.6).
pub const CONTRACT_COUNTEREXAMPLE: DiagCode = DiagCode("contract_counterexample");

/// Which tests and properties to run (Section 7.2 `cpl test`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestSelector {
    /// A module-path prefix (`tests.pagination`, `tests.pagination.*`);
    /// `None` selects every module.
    pub namespace: Option<Vec<String>>,
    /// The exact test or property name (the `TEXT` literal's value).
    pub name: Option<String>,
    pub id: Option<StableId>,
}

impl TestSelector {
    /// Every test and property.
    pub fn all() -> Self {
        TestSelector::default()
    }

    /// A module path or a namespace wildcard (`a.b` and `a.b.*` both mean
    /// the namespace, T43); anything else is `Err`, which `cpl test`
    /// reports as `usage_bad_argument` (exit 2). Segments are
    /// `LOWER_IDENT`s (a keyword is not one) written without spaces.
    pub fn parse(arg: &str) -> Result<TestSelector, String> {
        let invalid =
            || format!("`{arg}` is not a module path or a namespace wildcard (`a.b` or `a.b.*`)");
        let (tokens, diagnostics) = cpl_syntax::lex(arg, FileId(0));
        if !diagnostics.is_empty() {
            return Err(invalid());
        }
        let tokens: Vec<_> = tokens
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
            .collect();
        // The tokens must cover the whole argument with nothing between them.
        let mut at = 0;
        for t in &tokens {
            if t.span.start != at {
                return Err(invalid());
            }
            at = t.span.end;
        }
        if at as usize != arg.len() {
            return Err(invalid());
        }
        let mut segments = Vec::new();
        let mut i = 0;
        loop {
            match tokens.get(i) {
                Some(t) if t.kind == TokenKind::LowerIdent => segments.push(t.text.clone()),
                _ => return Err(invalid()),
            }
            match tokens.get(i + 1) {
                None => break,
                Some(t) if t.kind == TokenKind::Dot => {}
                Some(_) => return Err(invalid()),
            }
            if let Some(t) = tokens.get(i + 2) {
                if t.kind == TokenKind::Star {
                    if i + 3 == tokens.len() {
                        break;
                    }
                    return Err(invalid());
                }
            }
            i += 2;
        }
        Ok(TestSelector {
            namespace: Some(segments),
            name: None,
            id: None,
        })
    }
}

/// The wire form of a selector (Appendix C), for receipts.
impl From<TestSelector> for TestSelectorSpec {
    fn from(s: TestSelector) -> TestSelectorSpec {
        TestSelectorSpec {
            namespace: s.namespace,
            name: s.name,
            id: s.id,
        }
    }
}

impl From<TestSelectorSpec> for TestSelector {
    fn from(s: TestSelectorSpec) -> TestSelector {
        TestSelector {
            namespace: s.namespace,
            name: s.name,
            id: s.id,
        }
    }
}

/// Runner settings (Section 1.7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestConfig {
    /// The run seed (semantics S44).
    pub seed: u64,
    /// Cases per property; default `[test] cases = 100`.
    pub cases: u32,
    /// Body evaluations spent shrinking per failure; default
    /// `[test] max_shrinks = 1000`.
    pub max_shrinks: u32,
    /// Default false.
    pub fail_fast: bool,
    /// `time_budget` from `[test] timeout_ms`.
    pub eval: EvalConfig,
}

impl TestConfig {
    /// `[test] cases = 100` (semantics S44, stdlib 8.2).
    pub const DEFAULT_CASES: u32 = 100;
    /// `[test] max_shrinks = 1000` (semantics S45, stdlib 8.2).
    pub const DEFAULT_MAX_SHRINKS: u32 = 1000;

    /// The defaults with run seed `seed`: 100 cases, 1000 shrinks, no
    /// fail-fast, the default [`EvalConfig`] (no time budget).
    pub fn with_seed(seed: u64) -> TestConfig {
        TestConfig {
            seed,
            cases: TestConfig::DEFAULT_CASES,
            max_shrinks: TestConfig::DEFAULT_MAX_SHRINKS,
            fail_fast: false,
            eval: EvalConfig::default(),
        }
    }
}

// ===========================================================================
// The engine of the runner (toolchain 1.7.4, T77)
// ===========================================================================

/// The side of a `gen.range(lo, hi)` bound (`spec/toolchain.md` 1.7.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RangeSide {
    Lo,
    Hi,
}

/// What the runner of Section 1.7.2 needs from an engine (`spec/toolchain.md`
/// 1.7.4, T77); each method is one top-level evaluation of the engine. The
/// runner keeps everything else (selection, seeds, generation, shrinking,
/// the setup's `gen.range` precondition, the records), so that the
/// evaluator and the native engine differ in evaluation alone (semantics
/// S75). [`Interpreter`] is one engine; `cpl_cabi::NativeSession` is the
/// other.
pub trait TestEngine<'p> {
    /// The program the engine evaluates.
    fn program(&self) -> &'p Program;
    /// A test or property body, the `for` binders bound in clause order; Ok
    /// when the body completes. Under [`Budget::Fresh`]: a test body, a
    /// property case, and a shrink rerun each have their own time budget.
    fn run_body(&mut self, test: DefId, binders: &[(LocalId, Value)]) -> Result<(), Fault>;
    /// The last `ensures`, `invariant`, or `decreases` violation of the last
    /// evaluation, as raised (semantics S46): the source of a
    /// `contract_counterexample`.
    fn take_violation(&mut self) -> Option<(DefId, Fault)>;
    /// The first refinement of `chain(refined)`, root first, whose predicate
    /// is false for `value`; `None` when all hold. A faulting predicate is
    /// reported per semantics S34.
    fn failing_refinement(
        &mut self,
        refined: DefId,
        value: &Value,
        budget: Budget,
    ) -> Result<Option<DefId>, Fault>;
    /// Bound `side` of the `gen.range` of binder `binder` (clause order) of
    /// `property`, evaluated in an activation of the property with nothing
    /// bound and no time budget, a fault reported per semantics S71 with
    /// `clause_text`, the range's clause text (the runner's).
    fn eval_range_bound(
        &mut self,
        property: DefId,
        binder: usize,
        side: RangeSide,
        clause_text: &str,
    ) -> Result<Value, Fault>;
}

/// The evaluations [`run_tests`] performs, as the [`TestEngine`] of the
/// generic runner.
impl<'p> TestEngine<'p> for Interpreter<'p> {
    fn program(&self) -> &'p Program {
        Interpreter::program(self)
    }

    fn run_body(&mut self, test: DefId, binders: &[(LocalId, Value)]) -> Result<(), Fault> {
        self.eval_body(test, binders)
    }

    fn take_violation(&mut self) -> Option<(DefId, Fault)> {
        self.take_recorded_violation()
    }

    fn failing_refinement(
        &mut self,
        refined: DefId,
        value: &Value,
        budget: Budget,
    ) -> Result<Option<DefId>, Fault> {
        self.refinement_failure(refined, value, budget)
    }

    /// The bound's expression is found in the property's `for` clause: a
    /// clause has at most one `gen.range`, possibly inside `gen.list` and
    /// `gen.option`. A binder without one is `internal_error` kind
    /// `not_a_test` (a defect of the caller).
    fn eval_range_bound(
        &mut self,
        property: DefId,
        binder: usize,
        side: RangeSide,
        clause_text: &str,
    ) -> Result<Value, Fault> {
        let p = Interpreter::program(self);
        let range = p
            .defs
            .get(property.0 as usize)
            .and_then(|d| d.kind.as_property())
            .and_then(|pd| pd.binders.get(binder))
            .and_then(|b| range_of(&b.gen));
        let Some((lo, hi)) = range else {
            return Err(Fault::internal(
                "not_a_test",
                render(
                    &messages::DETAIL_NOT_A_TEST_DEF,
                    &[("def", &property.0.to_string())],
                ),
            ));
        };
        let e = match side {
            RangeSide::Lo => lo,
            RangeSide::Hi => hi,
        };
        self.eval_range_expr(property, e, clause_text)
    }
}

/// The bounds of the one `gen.range` of a `for` clause's generator.
fn range_of(g: &GenExpr) -> Option<(&Expr, &Expr)> {
    let mut g = g;
    loop {
        match g {
            GenExpr::Range(lo, hi) => return Some((lo, hi)),
            GenExpr::List(inner) | GenExpr::Option(inner) => g = inner,
            GenExpr::Int | GenExpr::PositiveInt | GenExpr::Bool | GenExpr::Text => return None,
        }
    }
}

/// Every selected `test` and `property`, sorted by qualified name.
pub fn select_tests(p: &Program, selector: &TestSelector) -> Vec<DefId> {
    let mut selected: Vec<(String, DefId)> = p
        .defs
        .iter()
        .filter(|d| d.kind.is_test())
        .filter(|d| {
            let path = &p.module(d.module).path;
            selector
                .namespace
                .as_ref()
                .is_none_or(|ns| path.starts_with(ns))
                && selector
                    .name
                    .as_ref()
                    .is_none_or(|n| matches!(&d.name, DeclName::Text(t) if t == n))
                && selector.id.as_ref().is_none_or(|id| &d.stable_id == id)
        })
        .map(|d| (p.display_path(d.id), d.id))
        .collect();
    selected.sort();
    selected.into_iter().map(|(_, d)| d).collect()
}

/// Run the selection (Section 1.7.2; semantics 7.3-7.5), in qualified-name
/// order, stopping after the first failure when `fail_fast` is set
/// (`selected` still counts the whole selection). An empty selection is
/// a report with `selected: 0`, which `cpl test` turns into
/// `empty_test_selection` (semantics S42).
pub fn run_tests(p: &Program, selector: &TestSelector, config: &TestConfig) -> TestReport {
    on_eval_stack(|| {
        run_tests_with(p, selector, config, &mut || {
            Box::new(Interpreter::new(p, config.eval.clone()))
        })
    })
}

/// The runner of Section 1.7.2 over any engine (`spec/toolchain.md` 1.7.4,
/// T77): selection, seeds, generation, shrinking, the `gen.range`
/// precondition ([`builtins::check_gen_range_at`] with the property as
/// `caller`), and the records are the runner's, and only the evaluations
/// come from the engine. `engines` yields the engine of each selected test,
/// in run order (with `fail_fast`, none after the first failing record).
/// [`run_tests`] is this over a fresh [`Interpreter`] per test, on the
/// evaluation thread; this function runs on the calling thread, so a caller
/// whose engine is an [`Interpreter`] calls it inside
/// [`crate::on_eval_stack`] (the native engine evaluates in its driver).
pub fn run_tests_with<'p>(
    p: &'p Program,
    selector: &TestSelector,
    config: &TestConfig,
    engines: &mut dyn FnMut() -> Box<dyn TestEngine<'p> + 'p>,
) -> TestReport {
    let selected = select_tests(p, selector);
    let mut results = Vec::with_capacity(selected.len());
    for &test in &selected {
        let mut engine = engines();
        let record = run_one(p, test, config, &mut *engine);
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
    TestReport {
        seed: config.seed,
        cases: config.cases,
        selected: selected.len() as u32,
        tested: results.len() as u32 - failed,
        failed,
        results,
    }
}

/// Run one test or property; the record is the payload of its
/// `test_result` diagnostic (semantics S46, [`test_result_diagnostic`]).
/// Panics when `test` is not a definition of `p`.
pub fn run_test(p: &Program, test: DefId, config: &TestConfig) -> TestCaseRecord {
    on_eval_stack(|| {
        let mut it = Interpreter::new(p, config.eval.clone());
        run_test_with(p, test, config, &mut it)
    })
}

/// [`run_test`] over `engine` (`spec/toolchain.md` 1.7.4): one record, on
/// the calling thread (see [`run_tests_with`]). Panics when `test` is not a
/// definition of `p`.
pub fn run_test_with<'p>(
    p: &'p Program,
    test: DefId,
    config: &TestConfig,
    engine: &mut dyn TestEngine<'p>,
) -> TestCaseRecord {
    run_one(p, test, config, engine)
}

/// The 32-byte `ChaCha20Rng::from_seed` key of a property's stream
/// (semantics S44, stdlib 8.2): `blake3(run_seed_le(8) ++ property_id_utf8)`,
/// where `property_id` is the property's `@id` value, else its qualified
/// name as `SymbolPath::display` renders it; reported as 64 lowercase hex
/// digits (`property_seed`, T45; [`seed_hex`]).
pub fn property_seed(run_seed: u64, property_id: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&run_seed.to_le_bytes());
    hasher.update(property_id.as_bytes());
    *hasher.finalize().as_bytes()
}

/// A property seed as the 64 lowercase hex digits records carry (T45).
pub fn seed_hex(seed: &[u8; 32]) -> String {
    seed.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `test_result` diagnostic of a record (semantics S46): `info` when
/// tested, `error` when failed; `symbol` and `qualified_name` the test's
/// identity; every other field of the record flattened into the payload.
pub fn test_result_diagnostic(record: &TestCaseRecord) -> Diagnostic {
    let (severity, message) = match record.status {
        TestStatus::Tested => (
            cpl_diag::Severity::Info,
            match record.cases {
                Some(cases) => format!("tested ({cases} cases)"),
                None => "tested".to_string(),
            },
        ),
        TestStatus::Failed => (
            cpl_diag::Severity::Error,
            match (&record.failing_case, &record.fault) {
                (Some(case), Some(fault)) => {
                    format!("failed at case {}: {}", case.index, fault.message)
                }
                (_, Some(fault)) => format!("failed: {}", fault.message),
                _ => "failed".to_string(),
            },
        ),
    };
    let mut d = Diagnostic::new(TEST_RESULT, Category::Test, severity, message).with_symbol(
        record.symbol.clone(),
        record.qualified_name.clone(),
        None,
    );
    for (key, value) in to_data(record) {
        if !RESERVED_KEYS.contains(&key.as_str()) {
            d.data.insert(key, value);
        }
    }
    d
}

// ===========================================================================
// One test or property
// ===========================================================================

fn run_one<'p>(
    p: &'p Program,
    test: DefId,
    config: &TestConfig,
    engine: &mut dyn TestEngine<'p>,
) -> TestCaseRecord {
    let start = Instant::now();
    let def = p.def(test);
    let mut record = TestCaseRecord {
        symbol: def.stable_id.clone(),
        qualified_name: p.display_path(test),
        kind: match def.kind {
            DefKind::Property(_) => TestKind::Property,
            _ => TestKind::Test,
        },
        status: TestStatus::Tested,
        duration_ms: 0,
        seed: None,
        property_seed: None,
        cases: None,
        failing_case: None,
        shrunk: None,
        fault: None,
        counterexamples: Vec::new(),
        receipt: None,
    };
    match &def.kind {
        DefKind::Test(_) => {
            if let Err(fault) = engine.run_body(test, &[]) {
                let violation = engine.take_violation();
                fail(&mut record, p, test, &fault, violation);
            }
        }
        DefKind::Property(pd) => run_property(engine, test, pd, config, &mut record),
        _ => {
            let fault = Fault::internal(
                "not_a_test",
                render(
                    &messages::DETAIL_NOT_A_TEST,
                    &[("name", &p.display_path(test))],
                ),
            );
            fail(&mut record, p, test, &fault, None);
        }
    }
    record.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    record
}

/// Mark `record` failed with `fault` (and its counterexample, if any).
fn fail(
    record: &mut TestCaseRecord,
    p: &Program,
    test: DefId,
    fault: &Fault,
    violation: Option<(DefId, Fault)>,
) {
    record.status = TestStatus::Failed;
    record.fault = Some(diagnostic_json(p, fault));
    record.counterexamples = counterexamples(p, test, fault, violation);
}

fn diagnostic_json(p: &Program, fault: &Fault) -> DiagnosticJson {
    fault.to_diagnostic(p).to_json(p.source_map())
}

fn run_property<'p>(
    it: &mut dyn TestEngine<'p>,
    test: DefId,
    pd: &'p PropertyDef,
    config: &TestConfig,
    record: &mut TestCaseRecord,
) {
    let p = it.program();
    let seed = property_seed(config.seed, p.def(test).stable_id.as_str());
    record.seed = Some(config.seed);
    record.property_seed = Some(seed_hex(&seed));

    // The ranges, once per property before case 0 (stdlib 8.2). A fault
    // there fails the property with zero cases (toolchain 1.7.2), and it is
    // the property's fault like any other: an `ensures`, `invariant`, or
    // `decreases` violation of a function a bound calls is also a
    // counterexample against that function (S46).
    let mut gens = Vec::with_capacity(pd.binders.len());
    for (index, binder) in pd.binders.iter().enumerate() {
        match compile(it, test, index, binder, &binder.gen) {
            Ok(g) => gens.push(g),
            Err(fault) => {
                let violation = it.take_violation();
                record.cases = Some(0);
                fail(record, p, test, &fault, violation);
                return;
            }
        }
    }

    let mut stream = Stream::new(seed);
    for i in 0..config.cases {
        let size = i.min(gen::MAX_SIZE);
        let values: Vec<Value> = gens
            .iter()
            .map(|g| gen::draw(&mut stream, g, size, p))
            .collect();
        let outcome = match check_draws(it, pd, &values) {
            Err(fault) => Some((fault, None, values.clone(), 0, false)),
            Ok(()) => match it.run_body(test, &bind(pd, &values)) {
                Ok(()) => None,
                Err(fault) => {
                    let violation = it.take_violation();
                    let s = shrink(
                        it,
                        test,
                        pd,
                        &gens,
                        values.clone(),
                        fault,
                        violation,
                        config,
                    );
                    Some((s.fault, s.violation, s.values, s.steps, s.exhausted))
                }
            },
        };
        if let Some((fault, violation, shrunk, steps, exhausted)) = outcome {
            record.cases = Some(i + 1);
            record.failing_case = Some(FailingCase {
                index: i,
                input: tuple_json(p, pd, &values),
            });
            record.shrunk = Some(ShrunkCase {
                input: tuple_json(p, pd, &shrunk),
                shrink_steps: steps,
                shrink_exhausted: exhausted,
            });
            fail(record, p, test, &fault, violation);
            return;
        }
    }
    record.cases = Some(config.cases);
}

/// The `for` range `g` of `binder` (the `index`th clause of the property
/// `prop`) with its `gen.range` bounds evaluated, `lo` then `hi`, in the
/// property's scope with no binders (S43) and no time budget (toolchain
/// 7.2), then the range's `requires lo <= hi` checked.
///
/// The range is a contract expression (semantics 5.1, 7.4): a fault raised
/// while a bound is evaluated is reported per S34 with `facet: "contract"`,
/// `clause` the range's clause text (its canonical text on one line, T56;
/// [`clause::range_text`]), and
/// `symbol` the property ([`Interpreter::eval_range_bound`]). The
/// generator's own precondition keeps the shape stdlib 12 fixes for
/// `gen.range(5, 1)`: the stub's `contract_violation`, `symbol
/// std.gen.range`, `clause` `lo <= hi`, `caller` the property (stdlib 8.5;
/// toolchain 1.7.2). The generator is never called: the runner builds it
/// from the evaluated bounds (toolchain T18), so its precondition is
/// checked as the builtin's own, after the contract expression's code ran.
fn compile<'p>(
    it: &mut dyn TestEngine<'p>,
    prop: DefId,
    index: usize,
    binder: &'p Binder,
    g: &'p GenExpr,
) -> Result<Gen, Fault> {
    Ok(match g {
        GenExpr::Int => Gen::Int,
        GenExpr::PositiveInt => Gen::PositiveInt,
        GenExpr::Bool => Gen::Bool,
        GenExpr::Text => Gen::Text,
        GenExpr::List(inner) => Gen::List(Box::new(compile(it, prop, index, binder, inner)?)),
        GenExpr::Option(inner) => Gen::Option(Box::new(compile(it, prop, index, binder, inner)?)),
        GenExpr::Range(lo, hi) => {
            let p = it.program();
            let text = clause::range_text(p, prop, index, binder);
            let lo = range_bound(it, prop, index, RangeSide::Lo, lo, &text)?;
            let hi = range_bound(it, prop, index, RangeSide::Hi, hi, &text)?;
            // The generator's own precondition, with the property as its
            // `caller` (toolchain 1.7.2: the pure `check_gen_range_at`, so
            // that it is the runner's for both engines).
            if let Err(mut fault) = builtins::check_gen_range_at(p, Some(prop), &lo, &hi) {
                if let Some(stub) = builtins::stub_of(it.program(), "gen_range") {
                    fault.push_frame(Frame {
                        def: stub,
                        span: Some(binder.span),
                    });
                }
                fault.push_frame(Frame {
                    def: prop,
                    span: None,
                });
                return Err(fault);
            }
            Gen::Range(lo, hi)
        }
    })
}

/// One bound of a `gen.range` (see [`compile`]): side `side` of the range
/// of binder `index`, whose expression is `e`, evaluated by the engine.
fn range_bound<'p>(
    it: &mut dyn TestEngine<'p>,
    prop: DefId,
    index: usize,
    side: RangeSide,
    e: &'p Expr,
    range_text: &str,
) -> Result<BigInt, Fault> {
    match it
        .eval_range_bound(prop, index, side, range_text)?
        .into_int()
    {
        Ok(n) => Ok(n),
        Err(other) => Err(Fault::internal(
            "type_confusion",
            render(
                &messages::DETAIL_TYPE_CONFUSION_RANGE_BOUND,
                &[("found", other.kind_name())],
            ),
        )
        .with_span(e.span)),
    }
}

/// A drawn value of a refinement type is constructed only after its
/// predicates held, as `check<R>` would (stdlib 8.5). The check is the
/// runner's, not part of the case body, so no time budget applies to it
/// (toolchain 7.2).
fn check_draws(
    it: &mut dyn TestEngine<'_>,
    pd: &PropertyDef,
    values: &[Value],
) -> Result<(), Fault> {
    let p = it.program();
    for (binder, value) in pd.binders.iter().zip(values) {
        for pos in refined_positions(p, value, &binder.elem_ty) {
            if it
                .failing_refinement(pos.refined, &pos.value, Budget::None)?
                .is_some()
            {
                return Err(Fault::internal(
                    "generator_draw_rejected",
                    render(
                        &messages::DETAIL_GENERATOR_DRAW_REJECTED,
                        &[
                            ("value", &crate::json::value_text_untyped(p, &pos.value)),
                            ("refinement", &p.display_path(pos.refined)),
                        ],
                    ),
                )
                .with_span(binder.span));
            }
        }
    }
    Ok(())
}

/// True when every refined position of `value` at type `ty` satisfies its
/// predicates; a faulting predicate counts as false (S45: such a candidate
/// is discarded). Like [`check_draws`], unbudgeted: only the rerun that
/// follows is a body evaluation (toolchain 7.2).
fn refinements_hold(it: &mut dyn TestEngine<'_>, ty: &Ty, value: &Value) -> bool {
    let p = it.program();
    refined_positions(p, value, ty).iter().all(|pos| {
        matches!(
            it.failing_refinement(pos.refined, &pos.value, Budget::None),
            Ok(None)
        )
    })
}

/// The binders of `pd` bound to `values` (clause order).
fn bind(pd: &PropertyDef, values: &[Value]) -> Vec<(LocalId, Value)> {
    pd.binders
        .iter()
        .zip(values)
        .map(|(b, v)| (b.local, v.clone()))
        .collect()
}

/// A binder tuple as the `input` object of `failing_case` and `shrunk`: the
/// value JSON of each binder by name.
fn tuple_json(p: &Program, pd: &PropertyDef, values: &[Value]) -> Map<String, Json> {
    let mut out = Map::new();
    for (binder, value) in pd.binders.iter().zip(values) {
        let name = pd
            .locals
            .get(binder.local.0 as usize)
            .map_or_else(|| format!("${}", binder.local.0), |l| l.name.clone());
        out.insert(name, value_to_json(p, value, &binder.elem_ty));
    }
    out
}

/// The result of shrinking one failure.
struct Shrunk {
    values: Vec<Value>,
    fault: Fault,
    violation: Option<(DefId, Fault)>,
    steps: u32,
    exhausted: bool,
}

/// Greedy coordinate-wise shrinking (semantics S45, stdlib 8.4, S32).
#[allow(clippy::too_many_arguments)]
fn shrink(
    it: &mut dyn TestEngine<'_>,
    test: DefId,
    pd: &PropertyDef,
    gens: &[Gen],
    values: Vec<Value>,
    fault: Fault,
    violation: Option<(DefId, Fault)>,
    config: &TestConfig,
) -> Shrunk {
    let p = it.program();
    let code = fault.code();
    let mut current = Shrunk {
        values,
        fault,
        violation,
        steps: 0,
        exhausted: false,
    };
    'passes: loop {
        let mut accepted_any = false;
        for (j, g) in gens.iter().enumerate() {
            // Continue with this binder from every accepted value.
            loop {
                let mut accepted = false;
                let base = current.values[j].clone();
                for candidate in gen::candidates(g, &base, p) {
                    if !refinements_hold(it, &pd.binders[j].elem_ty, &candidate) {
                        continue;
                    }
                    if current.steps >= config.max_shrinks {
                        current.exhausted = true;
                        break 'passes;
                    }
                    current.steps += 1;
                    let mut trial = current.values.clone();
                    trial[j] = candidate;
                    if let Err(f) = it.run_body(test, &bind(pd, &trial)) {
                        if f.code() == code {
                            current.violation = it.take_violation();
                            current.values = trial;
                            current.fault = f;
                            accepted = true;
                            accepted_any = true;
                            break;
                        }
                    }
                }
                if !accepted {
                    break;
                }
            }
        }
        if !accepted_any {
            break;
        }
    }
    current
}

/// The `contract_counterexample` diagnostics of a failure (semantics S46):
/// one when the fault is a `contract_violation` of an `ensures`,
/// `invariant`, or `decreases` clause raised by a declaration other than
/// the test itself, built from the violation as it was raised (`symbol`
/// that declaration, `facet: "body"`, its clause, its `input` and
/// `actual`, `status: "failed"`, `found_by` the test's identity). A
/// `requires` violation, and an operation's `ensures` (a binding failure,
/// not a body's), yield none.
fn counterexamples(
    p: &Program,
    test: DefId,
    fault: &Fault,
    violation: Option<(DefId, Fault)>,
) -> Vec<DiagnosticJson> {
    let FaultKind::ContractViolation(kind) = fault.kind else {
        return Vec::new();
    };
    if kind == ClauseKind::Requires {
        return Vec::new();
    }
    let Some((def, original)) = violation else {
        return Vec::new();
    };
    if def == test {
        return Vec::new();
    }
    let FaultKind::ContractViolation(original_kind) = original.kind else {
        return Vec::new();
    };
    let found_by = p.def(test).stable_id.as_str().to_string();
    let clause = original.clause().unwrap_or_default().to_string();
    let qualified = p.display_path(def);
    let mut d = Diagnostic::error(
        CONTRACT_COUNTEREXAMPLE,
        Category::Contract,
        format!(
            "{found_by} found a counterexample to {qualified}: {} clause failed: {clause}",
            original_kind.as_str()
        ),
    )
    .with_symbol(p.def(def).stable_id.clone(), qualified, Some(Facet::Body));
    if let Some(span) = original.span {
        d = d.with_span(span);
    }
    d = d
        .with_data("clause", json!(clause))
        .with_data("clause_kind", json!(original_kind.as_str()));
    for key in ["input", "actual"] {
        if let Some(v) = original.data.get(key) {
            d = d.with_data(key, v.clone());
        }
    }
    if let Some(iteration) = original.data.get("iteration") {
        d = d.with_data("iteration", iteration.clone());
    }
    d = d
        .with_data("status", json!("failed"))
        .with_data("found_by", json!(found_by));
    vec![d.to_json(p.source_map())]
}
