//! The evaluator hooks of Milestone 2 (`spec/toolchain.md` 1.7.4, T77; the "evaluator hooks" row of toolchain 12):
//!
//! * the switch onto `cpl-rtspec` (T70): the registry's ids, the limits, and the one division;
//! * `bind_capability_with` and its validation in every outcome of semantics S77, and `foreign_failure` (stdlib S45,
//!   semantics S80);
//! * the runner over `TestEngine`: over `Interpreter` it gives the reports of the Milestone 1 runner, byte for byte,
//!   on `examples/reader` and on a failing property with shrinking, for seeds 1..=8 (goldens written by the Milestone 1
//!   runner, `tests/m2_hooks/`);
//! * `prepare_run`, `run_function_with_hosts` with its two-step hosts, and `RunEngine` over `Interpreter`;
//! * `requires_spec`, `requires_violation_at`, `check_gen_range_at`, `operation_identity`;
//! * `scope_at` equal to the interpreter's environment at every binding site of every program of the interpreter
//!   corpus (`tests/corpus/*.cpl`) and of the programs below.

mod common;
#[path = "m2_hooks/runner_cases.rs"]
mod runner_cases;

use cpl_diag::{Diagnostic, Span, SymbolPath};
use cpl_eval::builtins::{self, RequiresFacts};
use cpl_eval::test::TestReport;
use cpl_eval::testing::corpus::parse_cases;
use cpl_eval::types::operation_identity;
use cpl_eval::{
    boundary, json, on_eval_stack, prepare_run, run_function, run_function_with_hosts, run_test,
    run_test_with, run_tests, run_tests_with, scope_at, BindOptions, Budget, CapabilityImpl,
    EvalConfig, Fault, Handle, HostContext, HostSource, Interpreter, RangeSide, RunArg, RunEngine,
    RunError, RunHosts, TestConfig, TestEngine, TestSelector, Value,
};
use cpl_hir::{DefId, LocalId, ParamDef, Program, Ty};
use num_bigint::BigInt;
use serde_json::{json, Value as Json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

// ===========================================================================
// The switch onto cpl-rtspec (toolchain 1.6, 1.7.4, 1.11; T70; stdlib S47)
// ===========================================================================

#[test]
fn the_registry_and_the_checkers_ids_are_cpl_rtspecs() {
    let registry: Vec<&str> = builtins::ids().collect();
    assert_eq!(registry, cpl_rtspec::builtins::IDS.to_vec());
    assert_eq!(cpl_sema::codes::BUILTIN_IDS, &cpl_rtspec::builtins::IDS[..]);
    for id in cpl_rtspec::builtins::IDS {
        assert!(
            builtins::lookup(id).is_some(),
            "the evaluator implements `{id}`"
        );
    }
}

#[test]
fn the_limits_are_cpl_rtspecs() {
    assert_eq!(
        cpl_eval::stack::EVAL_STACK_SIZE,
        cpl_rtspec::limits::EVAL_STACK_SIZE
    );
    assert_eq!(cpl_eval::stack::EVAL_STACK_SIZE, 1 << 30);
    assert_eq!(
        cpl_eval::stack::GUARD_MARGIN,
        cpl_rtspec::limits::GUARD_MARGIN
    );
    assert_eq!(cpl_eval::stack::GUARD_MARGIN, 64 << 20);
    assert_eq!(
        EvalConfig::DEFAULT_MAX_CALL_DEPTH,
        cpl_rtspec::limits::DEFAULT_MAX_CALL_DEPTH
    );
    assert_eq!(EvalConfig::default().max_call_depth, 10_000);
}

#[test]
fn the_one_division_is_cpl_rtspecs() {
    for a in -9i64..=9 {
        for b in -4i64..=4 {
            let (a, b) = (BigInt::from(a), BigInt::from(b));
            match (
                cpl_eval::arith::div_rem(&a, &b),
                cpl_rtspec::arith::div_rem(&a, &b),
            ) {
                (Ok(ours), Some(theirs)) => assert_eq!(ours, theirs, "{a} / {b}"),
                (Err(fault), None) => {
                    assert_eq!(fault.code().0, "division_by_zero");
                    assert_eq!(fault.message, "division by zero");
                    assert_eq!(
                        fault.data["input"],
                        json!({"a": a.to_string().parse::<i64>().unwrap(), "b": 0})
                    );
                }
                (ours, theirs) => panic!("{a} / {b}: {ours:?} against {theirs:?}"),
            }
        }
    }
}

#[test]
fn white_space_is_the_set_of_stdlib_s22() {
    let p = common::check_source(
        "module ws.main\n\nuse std.text.{trim}\n\npub fn t(s: Text) -> Text {\n    trim(s)\n}\n",
    );
    // Every White_Space scalar is trimmed, and nothing else (U+200B and U+FEFF are not White_Space).
    let t = common::def(&p, "ws.main.t");
    let trimmed = on_eval_stack(|| {
        let mut it = Interpreter::new(&p, EvalConfig::default());
        let mut trimmed = Vec::new();
        for c in (0u32..0x3100)
            .chain([0xFEFF, 0x200B])
            .filter_map(char::from_u32)
        {
            let text: String = [c, 'x', c].iter().collect();
            let v = it.call(t, vec![Value::text(&text)], vec![]).unwrap();
            if v.as_text() == Some("x") {
                trimmed.push(c as u32);
            } else {
                assert_eq!(v.as_text(), Some(text.as_str()));
            }
        }
        trimmed
    });
    let expected: Vec<u32> = cpl_rtspec::whitespace::RANGES
        .iter()
        .flat_map(|&(lo, hi)| lo..=hi)
        .collect();
    assert_eq!(trimmed, expected);
    assert_eq!(expected.len(), 25);
}

// ===========================================================================
// The runner over TestEngine (toolchain 1.7.2, 1.7.4)
// ===========================================================================

/// A TestEngine that is not the Interpreter type: it delegates to one and counts the evaluations, so the generic
/// runner is exercised through a trait object of another type.
struct Counting<'p> {
    inner: Interpreter<'p>,
    counts: Rc<RefCell<BTreeMap<&'static str, u32>>>,
}

impl<'p> Counting<'p> {
    fn note(&self, what: &'static str) {
        *self.counts.borrow_mut().entry(what).or_default() += 1;
    }
}

impl<'p> TestEngine<'p> for Counting<'p> {
    fn program(&self) -> &'p Program {
        self.inner.program()
    }
    fn run_body(&mut self, test: DefId, binders: &[(LocalId, Value)]) -> Result<(), Fault> {
        self.note("run_body");
        TestEngine::run_body(&mut self.inner, test, binders)
    }
    fn take_violation(&mut self) -> Option<(DefId, Fault)> {
        self.note("take_violation");
        TestEngine::take_violation(&mut self.inner)
    }
    fn failing_refinement(
        &mut self,
        refined: DefId,
        value: &Value,
        budget: Budget,
    ) -> Result<Option<DefId>, Fault> {
        self.note("failing_refinement");
        TestEngine::failing_refinement(&mut self.inner, refined, value, budget)
    }
    fn eval_range_bound(
        &mut self,
        property: DefId,
        binder: usize,
        side: RangeSide,
        clause_text: &str,
    ) -> Result<Value, Fault> {
        self.note("eval_range_bound");
        TestEngine::eval_range_bound(&mut self.inner, property, binder, side, clause_text)
    }
}

/// The reports of one program's cases by three routes: `run_tests`, `run_tests_with` over a fresh Interpreter per
/// test, and `run_tests_with` over the counting engine; plus the evaluations the counting engine saw.
/// Named reports, in case order.
type Reports = Vec<(String, Json)>;

fn three_routes(
    p: &Program,
    cases: &[runner_cases::Case],
) -> (Reports, Reports, Reports, BTreeMap<&'static str, u32>) {
    let mut plain = Vec::new();
    let mut generic = Vec::new();
    let mut counted = Vec::new();
    let mut counts = BTreeMap::new();
    for c in cases {
        let report = run_tests(p, &c.selector, &c.config);
        plain.push((c.name.clone(), runner_cases::stable_report(&report)));
        let report = on_eval_stack(|| {
            run_tests_with(p, &c.selector, &c.config, &mut || {
                Box::new(Interpreter::new(p, c.config.eval.clone()))
            })
        });
        generic.push((c.name.clone(), runner_cases::stable_report(&report)));
        let (report, seen) = on_eval_stack(|| {
            let seen = Rc::new(RefCell::new(BTreeMap::new()));
            let report = run_tests_with(p, &c.selector, &c.config, &mut || {
                Box::new(Counting {
                    inner: Interpreter::new(p, c.config.eval.clone()),
                    counts: Rc::clone(&seen),
                })
            });
            let seen = seen.borrow().clone();
            (report, seen)
        });
        counted.push((c.name.clone(), runner_cases::stable_report(&report)));
        for (k, v) in seen {
            *counts.entry(k).or_default() += v;
        }
    }
    (plain, generic, counted, counts)
}

fn assert_golden(file: &str, reports: Vec<(String, Json)>, route: &str) {
    let expected = std::fs::read_to_string(runner_cases::golden_path(file))
        .unwrap_or_else(|e| panic!("the golden {file}: {e}"));
    let actual = runner_cases::golden_text(reports);
    if actual != expected {
        let at = actual
            .bytes()
            .zip(expected.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(actual.len().min(expected.len()));
        let from = at.saturating_sub(200);
        panic!(
            "{route} differs from the Milestone 1 runner's {file} at byte {at}:\n--- golden\n{}\n--- actual\n{}",
            &expected[from..(at + 200).min(expected.len())],
            &actual[from..(at + 200).min(actual.len())]
        );
    }
}

#[test]
fn the_generic_runner_gives_the_milestone_1_reports_on_the_reader_example() {
    let p = runner_cases::reader();
    let (plain, generic, counted, counts) = three_routes(&p, &runner_cases::seeds());
    assert_golden("runner_reader.json", plain, "run_tests");
    assert_golden(
        "runner_reader.json",
        generic,
        "run_tests_with over Interpreter",
    );
    assert_golden(
        "runner_reader.json",
        counted,
        "run_tests_with over a counting engine",
    );
    // 8 seeds x (2 tests + 100 cases of the property), and a refinement check per drawn Positive.
    assert_eq!(counts["run_body"], 8 * 102);
    assert_eq!(counts["failing_refinement"], 8 * 100);
    assert!(!counts.contains_key("eval_range_bound"));
}

#[test]
fn the_generic_runner_gives_the_milestone_1_reports_on_a_failing_property_that_shrinks() {
    let p = runner_cases::wrong_reader();
    let (plain, generic, counted, counts) = three_routes(&p, &runner_cases::seeds());
    // The property fails and shrinks under every seed, with a contract_counterexample against paginate.
    for (name, report) in &plain {
        let record = report["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "property")
            .expect("the property's record");
        assert_eq!(record["status"], "failed", "{name}");
        assert!(
            record["shrunk"]["shrink_steps"].as_u64().unwrap() > 0,
            "{name}"
        );
        assert_eq!(
            record["counterexamples"][0]["code"], "contract_counterexample",
            "{name}"
        );
    }
    assert_golden("runner_wrong_reader.json", plain, "run_tests");
    assert_golden(
        "runner_wrong_reader.json",
        generic,
        "run_tests_with over Interpreter",
    );
    assert_golden(
        "runner_wrong_reader.json",
        counted,
        "run_tests_with over a counting engine",
    );
    assert!(counts["take_violation"] >= 8);
}

#[test]
fn the_generic_runner_gives_the_milestone_1_reports_on_every_path_of_the_runner() {
    let p = runner_cases::props();
    let cases = runner_cases::props_cases();
    let (plain, generic, counted, counts) = three_routes(&p, &cases);
    assert_golden("runner_props.json", plain, "run_tests");
    assert_golden(
        "runner_props.json",
        generic,
        "run_tests_with over Interpreter",
    );
    assert_golden(
        "runner_props.json",
        counted,
        "run_tests_with over a counting engine",
    );
    // The range bounds come from the engine, side by side.
    assert!(counts["eval_range_bound"] > 0);
}

#[test]
fn run_test_with_gives_run_tests_records() {
    let p = runner_cases::props();
    let config = TestConfig::with_seed(4);
    let report: TestReport = run_tests(&p, &TestSelector::all(), &config);
    for record in &report.results {
        let path = SymbolPath::parse(&record.qualified_name).expect("a qualified name");
        let test = p.lookup(&path).expect("the test");
        let mut one = run_test(&p, test, &config);
        let mut with = on_eval_stack(|| {
            let mut it = Interpreter::new(&p, config.eval.clone());
            run_test_with(&p, test, &config, &mut it)
        });
        let mut expected = record.clone();
        expected.duration_ms = 0;
        one.duration_ms = 0;
        with.duration_ms = 0;
        assert_eq!(one, expected, "{}", record.qualified_name);
        assert_eq!(with, expected, "{}", record.qualified_name);
    }
}

#[test]
fn an_engine_is_asked_for_each_test_in_run_order_and_none_after_a_fail_fast_stop() {
    let p = runner_cases::props();
    let mut config = TestConfig::with_seed(1);
    config.fail_fast = true;
    let made = on_eval_stack(|| {
        let made = Rc::new(RefCell::new(0u32));
        let counter = Rc::clone(&made);
        let report = run_tests_with(&p, &TestSelector::all(), &config, &mut || {
            *counter.borrow_mut() += 1;
            Box::new(Interpreter::new(&p, config.eval.clone()))
        });
        let n = *made.borrow();
        (n, report.results.len() as u32, report.selected)
    });
    assert_eq!(made.0, made.1, "one engine per record");
    assert!(made.1 < made.2, "fail_fast stopped the run");
}

#[test]
fn the_range_bounds_an_engine_evaluates_are_the_sides_of_the_clause() {
    let p = runner_cases::props();
    let prop = common::def(&p, "tests.ranges.\"computed bounds\"");
    let mut it = Interpreter::new(&p, EvalConfig::default());
    let lo = TestEngine::eval_range_bound(&mut it, prop, 0, RangeSide::Lo, "x").unwrap();
    let hi = TestEngine::eval_range_bound(&mut it, prop, 0, RangeSide::Hi, "x").unwrap();
    let nested_hi = TestEngine::eval_range_bound(&mut it, prop, 1, RangeSide::Hi, "x").unwrap();
    assert_eq!(lo.as_int(), Some(&BigInt::from(2)));
    assert_eq!(hi.as_int(), Some(&BigInt::from(7)));
    assert_eq!(nested_hi.as_int(), Some(&BigInt::from(7)));
    // A fault in a bound is claimed with the clause text the runner passes (semantics S71).
    let faulting = common::def(&p, "tests.ranges.\"division\"");
    let fault =
        TestEngine::eval_range_bound(&mut it, faulting, 0, RangeSide::Lo, "gen.range(1 / 0, 5)")
            .unwrap_err();
    let d = common::fault_json(&p, &fault);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["clause"], "gen.range(1 / 0, 5)");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "tests.ranges.\"division\"");
    // A binder without a range is a defect of the caller.
    let plain = common::def(&p, "tests.props.\"always\"");
    let fault = TestEngine::eval_range_bound(&mut it, plain, 0, RangeSide::Lo, "x").unwrap_err();
    assert_eq!(fault.data["kind"], "not_a_test");
}

// ===========================================================================
// bind_capability_with and the validation of semantics S77
// ===========================================================================

const CODEC: &str = r#"module app.codec

use std.foreign.{ForeignError}

pub type Positive = Int where self > 0
pub type Small = Positive where self < 10
pub type Faulty = Int where 10 / self > 0
pub type Name(Text)

pub struct Dimensions {
    width: Positive
    height: Positive
}

pub type Wide = Dimensions where self.width / self.height > 1

pub enum Shape {
    Square(Small)
    Line(List<Small>)
    Empty
}

pub enum Chain {
    End(Positive)
    Link(Option<Chain>)
}

pub capability Codec {
    fn dimensions(image: Bytes) -> Result<Dimensions, ForeignError>
    fn size() -> Positive
    fn small() -> Small
    fn shape() -> Shape
    fn wide() -> Wide
    fn wide_or_error() -> Result<Wide, ForeignError>
    fn name() -> Name
    fn names() -> List<Name>
    fn maybe() -> Option<Positive>
    fn faulty() -> Faulty
    fn chain() -> Chain
    fn sized(n: Int) -> Int
        ensures result >= n
    fn checked() -> Result<Small, ForeignError>
        ensures result.is_ok()
}

pub fn get_dimensions(image: Bytes, using c: Codec) -> Result<Dimensions, ForeignError>
    effects { c.dimensions }
{
    c.dimensions(image)
}

pub fn get_size(using c: Codec) -> Positive
    effects { c.size }
{
    c.size()
}

pub fn get_small(using c: Codec) -> Small
    effects { c.small }
{
    c.small()
}

pub fn get_shape(using c: Codec) -> Shape
    effects { c.shape }
{
    c.shape()
}

pub fn get_wide(using c: Codec) -> Wide
    effects { c.wide }
{
    c.wide()
}

pub fn get_wide_or_error(using c: Codec) -> Result<Wide, ForeignError>
    effects { c.wide_or_error }
{
    c.wide_or_error()
}

pub fn get_name(using c: Codec) -> Name
    effects { c.name }
{
    c.name()
}

pub fn get_names(using c: Codec) -> List<Name>
    effects { c.names }
{
    c.names()
}

pub fn get_maybe(using c: Codec) -> Option<Positive>
    effects { c.maybe }
{
    c.maybe()
}

pub fn get_faulty(using c: Codec) -> Faulty
    effects { c.faulty }
{
    c.faulty()
}

pub fn get_sized(n: Int, using c: Codec) -> Int
    effects { c.sized }
{
    c.sized(n)
}

pub fn get_checked(using c: Codec) -> Result<Small, ForeignError>
    effects { c.checked }
{
    c.checked()
}

pub fn get_chain(using c: Codec) -> Chain
    effects { c.chain }
{
    c.chain()
}

pub fn twice(using c: Codec) -> Int
    effects { c.size }
{
    let a = c.size()
    let b = c.size()
    a + b
}

pub fn deep(n: Int, using c: Codec) -> Int
    requires n >= 0
    effects { c.size }
{
    if n == 0 { c.size() } else { deep(n - 1, using c) }
}

pub fn positive_arg(n: Positive, using c: Codec) -> Int
    effects { c.size }
{
    n + c.size()
}

pub fn two(using a: Codec, using b: Codec) -> Int
    effects { a.size, b.size }
{
    a.size() * 10 + b.size()
}
"#;

struct Codec {
    p: Program,
    cap: DefId,
}

fn codec() -> Codec {
    let p = common::check_source(CODEC);
    let cap = common::def(&p, "app.codec.Codec");
    Codec { p, cap }
}

impl Codec {
    fn def(&self, name: &str) -> DefId {
        common::def(&self.p, &format!("app.codec.{name}"))
    }
    fn op(&self, name: &str) -> u32 {
        let ops = &self.p.def(self.cap).kind.as_capability().unwrap().ops;
        ops.iter()
            .position(|o| o.name == name)
            .expect("an operation") as u32
    }
    fn int(&self, n: i64) -> Value {
        Value::int(n)
    }
    fn dims(&self, w: Value, h: Value) -> Value {
        Value::new_struct(self.def("Dimensions"), vec![w, h])
    }
    fn name(&self, t: &str) -> Value {
        Value::wrap(self.def("Name"), Value::text(t))
    }
    fn shape(&self, variant: &str, payload: Vec<Value>) -> Value {
        Value::variant(&self.p, self.def("Shape"), variant, payload).expect("a variant")
    }
}

/// A host whose every operation answers the same value, logging the calls.
struct Answer {
    value: Value,
    log: Rc<RefCell<Vec<String>>>,
}

impl CapabilityImpl for Answer {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        let args: Vec<String> = args
            .iter()
            .map(|v| json::value_text_untyped(ctx.program, v))
            .collect();
        self.log
            .borrow_mut()
            .push(format!("{op}({})", args.join(", ")));
        Ok(self.value.clone())
    }
}

/// Call `function` with `args` and one Codec handle answering `answer`, bound as `imgcodec` with `validate_results`
/// as given: the value JSON of the result, or the diagnostic object of the fault.
fn call_with(
    c: &Codec,
    function: &str,
    args: Vec<Json>,
    answer: impl Fn(&Codec) -> Value + Sync,
    validate: bool,
    config: EvalConfig,
) -> Result<Json, Json> {
    let f = c.def(function);
    on_eval_stack(|| {
        let fd = c.p.def(f).kind.as_fn().unwrap();
        let values = args
            .iter()
            .zip(&fd.params)
            .map(|(a, param)| cpl_eval::value_from_json(&c.p, a, &param.ty).ok().unwrap())
            .collect();
        let mut it = Interpreter::new(&c.p, config);
        let h = it.bind_capability_with(
            c.cap,
            Box::new(Answer {
                value: answer(c),
                log: Rc::new(RefCell::new(Vec::new())),
            }),
            BindOptions {
                label: "imgcodec".to_string(),
                validate_results: validate,
            },
        );
        assert_eq!(h.label, "imgcodec");
        it.call(f, values, vec![h])
            .map(|v| cpl_eval::value_to_json(&c.p, &v, &fd.ret))
            .map_err(|fault| common::fault_json(&c.p, &fault))
    })
}

/// The value JSON of a validated call (the value returned to CPL), or panic with its fault.
fn value_of(
    c: &Codec,
    function: &str,
    args: Vec<Json>,
    answer: impl Fn(&Codec) -> Value + Sync,
) -> Json {
    match call_with(c, function, args, answer, true, EvalConfig::default()) {
        Ok(v) => v,
        Err(d) => panic!("{function}: expected a value, got {d}"),
    }
}

/// The diagnostic of a validated call's fault, or panic with its value.
fn fault_of(
    c: &Codec,
    function: &str,
    args: Vec<Json>,
    answer: impl Fn(&Codec) -> Value + Sync,
) -> Json {
    fault_with(c, function, args, answer, true, EvalConfig::default())
}

fn fault_with(
    c: &Codec,
    function: &str,
    args: Vec<Json>,
    answer: impl Fn(&Codec) -> Value + Sync,
    validate: bool,
    config: EvalConfig,
) -> Json {
    match call_with(c, function, args, answer, validate, config) {
        Err(d) => d,
        Ok(v) => panic!("{function}: expected a fault, got the value {v}"),
    }
}

/// The location of the call `c.<op>(` in CODEC, inside `function`.
fn op_call(function: &str, call: &str) -> Json {
    common::location_after(
        CODEC,
        "src/app/codec.cpl",
        &format!("pub fn {function}("),
        call,
    )
}

fn invalid_result(message: &str, pointer: &str, expected_type: &str, op: &str) -> Json {
    json!({
        "$type": "Result",
        "$variant": "Err",
        "$payload": [{
            "$type": "std.foreign.ForeignError",
            "kind": {"$type": "std.foreign.ForeignErrorKind", "$variant": "InvalidResult"},
            "message": message,
            "details": [
                {"$type": "std.foreign.ForeignDetail", "key": "adapter", "value": "imgcodec"},
                {"$type": "std.foreign.ForeignDetail", "key": "capability", "value": "app.codec.Codec"},
                {"$type": "std.foreign.ForeignDetail", "key": "operation", "value": format!("app.codec.Codec.{op}")},
                {"$type": "std.foreign.ForeignDetail", "key": "pointer", "value": pointer},
                {"$type": "std.foreign.ForeignDetail", "key": "expected_type", "value": expected_type},
            ],
        }],
    })
}

fn bytes(text: &str) -> Json {
    let hex: String = text.bytes().map(|b| format!("{b:02x}")).collect();
    json!({"$type": "Bytes", "hex": hex})
}

#[test]
fn a_valid_result_passes_unchanged() {
    let c = codec();
    assert_eq!(
        value_of(&c, "get_dimensions", vec![bytes("img")], |c| Value::ok(
            &c.p,
            c.dims(c.int(4), c.int(3))
        )),
        json!({"$type": "Result", "$variant": "Ok", "$payload": [{"$type": "app.codec.Dimensions", "width": 4, "height": 3}]})
    );
    assert_eq!(value_of(&c, "get_size", vec![], |c| c.int(7)), json!(7));
    assert_eq!(value_of(&c, "get_small", vec![], |c| c.int(9)), json!(9));
    assert_eq!(
        value_of(&c, "get_shape", vec![], |c| c
            .shape("Line", vec![Value::list(vec![c.int(1), c.int(2)])])),
        json!({"$type": "app.codec.Shape", "$variant": "Line", "$payload": [[1, 2]]})
    );
    assert_eq!(
        value_of(&c, "get_names", vec![], |c| Value::list(vec![
            c.name("a"),
            c.name("b")
        ])),
        json!([{"$type": "app.codec.Name", "$payload": ["a"]}, {"$type": "app.codec.Name", "$payload": ["b"]}])
    );
    assert_eq!(
        value_of(&c, "get_maybe", vec![], |c| Value::none(&c.p)),
        json!({"$type": "Option", "$variant": "None"})
    );
    assert_eq!(
        value_of(&c, "get_wide", vec![], |c| c.dims(c.int(5), c.int(2)))["width"],
        5
    );
}

#[test]
fn a_shape_failure_with_a_foreign_error_return_type_is_an_invalid_result() {
    let c = codec();
    // The whole result: ` at <pointer>` is left out of the message (stdlib S45), and `pointer` is "".
    assert_eq!(
        value_of(&c, "get_dimensions", vec![bytes("")], |c| c.int(3)),
        invalid_result(
            "the result of app.codec.Codec.dimensions is not a value of Result<Dimensions, ForeignError>",
            "",
            "Result<Dimensions, ForeignError>",
            "dimensions"
        )
    );
    // A position inside: the declared type there.
    assert_eq!(
        value_of(&c, "get_dimensions", vec![bytes("")], |c| Value::ok(&c.p, c.dims(Value::text("x"), c.int(3)))),
        invalid_result(
            "the result of app.codec.Codec.dimensions is not a value of Positive at /$payload/0/width",
            "/$payload/0/width",
            "Positive",
            "dimensions"
        )
    );
}

#[test]
fn a_refinement_failure_with_a_foreign_error_return_type_is_an_invalid_result() {
    let c = codec();
    // Value order: width before height.
    assert_eq!(
        value_of(&c, "get_dimensions", vec![bytes("")], |c| Value::ok(&c.p, c.dims(c.int(0), c.int(0)))),
        invalid_result(
            "the result of app.codec.Codec.dimensions does not satisfy Positive at /$payload/0/width: self > 0",
            "/$payload/0/width",
            "Positive",
            "dimensions"
        )
    );
    assert_eq!(
        value_of(&c, "get_dimensions", vec![bytes("")], |c| Value::ok(&c.p, c.dims(c.int(5), c.int(-1)))),
        invalid_result(
            "the result of app.codec.Codec.dimensions does not satisfy Positive at /$payload/0/height: self > 0",
            "/$payload/0/height",
            "Positive",
            "dimensions"
        )
    );
    // A refinement over a struct: its fields' positions first (S74), then its own predicate.
    assert_eq!(
        value_of(&c, "get_wide_or_error", vec![], |c| Value::ok(&c.p, c.dims(c.int(2), c.int(2)))),
        invalid_result(
            "the result of app.codec.Codec.wide_or_error does not satisfy Wide at /$payload/0: self.width / self.height > 1",
            "/$payload/0",
            "Wide",
            "wide_or_error"
        )
    );
}

#[test]
fn a_refinement_failure_otherwise_is_a_refinement_violation_blamed_on_the_binding() {
    let c = codec();
    let d = fault_of(&c, "get_size", vec![], |c| c.int(0));
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["category"], "runtime");
    assert_eq!(
        d["message"],
        "the value does not satisfy Positive: self > 0"
    );
    assert_eq!(d["symbol"], "app.codec.Codec.size");
    assert_eq!(d["qualified_name"], "app.codec.Codec.size");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["span"], op_call("get_size", "c.size()"));
    assert_eq!(d["expected_type"], "Positive");
    assert_eq!(d["actual"], 0);
    assert_eq!(d["clause"], "self > 0");
    assert_eq!(d["pointer"], "");
    assert_eq!(d["attributed_to"], "binding");
    assert_eq!(d["binding"], "imgcodec");
    assert!(d.get("parameter").is_none(), "a result has no parameter");
    assert_eq!(
        frames(&d),
        ["app.codec.get_size"],
        "the operation adds no frame"
    );

    // A chain, root first: Small's own predicate only after Positive's held.
    let d = fault_of(&c, "get_small", vec![], |c| c.int(0));
    assert_eq!(
        (d["expected_type"].clone(), d["clause"].clone()),
        (json!("Small"), json!("self > 0"))
    );
    let d = fault_of(&c, "get_small", vec![], |c| c.int(12));
    assert_eq!(
        (d["expected_type"].clone(), d["clause"].clone()),
        (json!("Small"), json!("self < 10"))
    );
    assert_eq!(d["message"], "the value does not satisfy Small: self < 10");

    // Nested positions.
    let d = fault_of(&c, "get_shape", vec![], |c| {
        c.shape("Line", vec![Value::list(vec![c.int(1), c.int(0)])])
    });
    assert_eq!(
        (d["pointer"].clone(), d["actual"].clone()),
        (json!("/$payload/0/1"), json!(0))
    );
    let d = fault_of(&c, "get_maybe", vec![], |c| Value::some(&c.p, c.int(-3)));
    assert_eq!(
        (d["pointer"].clone(), d["expected_type"].clone()),
        (json!("/$payload/0"), json!("Positive"))
    );

    // S74: the fields of a refinement's base before its own predicate, which would divide by zero.
    let d = fault_of(&c, "get_wide", vec![], |c| c.dims(c.int(4), c.int(0)));
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(
        (d["pointer"].clone(), d["expected_type"].clone()),
        (json!("/height"), json!("Positive"))
    );
    let d = fault_of(&c, "get_wide", vec![], |c| c.dims(c.int(1), c.int(1)));
    assert_eq!(
        (
            d["pointer"].clone(),
            d["expected_type"].clone(),
            d["actual"].clone()
        ),
        (
            json!(""),
            json!("Wide"),
            json!({"$type": "app.codec.Dimensions", "width": 1, "height": 1})
        )
    );
}

fn frames(d: &Json) -> Vec<String> {
    d["backtrace"]
        .as_array()
        .map(|fs| {
            fs.iter()
                .map(|f| f["qualified_name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_shape_failure_otherwise_is_the_internal_error_binding_result_ill_typed() {
    let c = codec();
    let d = fault_of(&c, "get_size", vec![], |_| Value::text("seven"));
    assert_eq!(d["code"], "internal_error");
    assert_eq!(d["kind"], "binding_result_ill_typed");
    assert_eq!(
        d["detail"],
        "the result of app.codec.Codec.size is not a value of Positive"
    );
    assert_eq!(
        d["message"],
        "internal error (binding_result_ill_typed): the result of app.codec.Codec.size is not a value of Positive"
    );
    assert_eq!(d["operation"], "app.codec.Codec.size");
    assert_eq!(d["binding"], "imgcodec");
    assert_eq!(d["pointer"], "");
    assert_eq!(d["span"], op_call("get_size", "c.size()"));
    for key in ["symbol", "qualified_name", "facet", "clause"] {
        assert!(d.get(key).is_none(), "an internal_error has no {key}");
    }
    assert_eq!(frames(&d), ["app.codec.get_size"]);

    type Make = fn(&Codec) -> Value;
    let cases: Vec<(&str, Make, &str, &str)> = vec![
        ("get_name", |_| Value::text("x"), "", "Name"),
        (
            "get_name",
            |c| Value::wrap(c.def("Name"), c.int(1)),
            "/$payload/0",
            "Text",
        ),
        (
            "get_names",
            |c| Value::list(vec![c.name("a"), Value::text("b")]),
            "/1",
            "Name",
        ),
        ("get_shape", |c| c.dims(c.int(1), c.int(1)), "", "Shape"),
        (
            "get_shape",
            |c| Value::new_enum(c.def("Shape"), 7, vec![]),
            "",
            "Shape",
        ),
        (
            "get_shape",
            |c| Value::new_enum(c.def("Shape"), 0, vec![]),
            "",
            "Shape",
        ),
        (
            "get_shape",
            |c| c.shape("Square", vec![Value::Bool(true)]),
            "/$payload/0",
            "Small",
        ),
        (
            "get_wide",
            |c| Value::new_struct(c.def("Dimensions"), vec![c.int(1)]),
            "",
            "Wide",
        ),
        ("get_maybe", |_| Value::Bool(false), "", "Option<Positive>"),
        (
            "get_maybe",
            |c| Value::some(&c.p, Value::text("1")),
            "/$payload/0",
            "Positive",
        ),
    ];
    for (function, answer, pointer, expected) in cases {
        let d = fault_of(&c, function, vec![], answer);
        assert_eq!(d["kind"], "binding_result_ill_typed", "{function}");
        assert_eq!(d["pointer"], pointer, "{function}");
        let detail = d["detail"].as_str().unwrap();
        assert!(
            detail.contains(&format!("is not a value of {expected}")),
            "{function}: {detail}"
        );
    }
}

#[test]
fn a_faulting_predicate_is_reported_per_s34() {
    let c = codec();
    let d = fault_of(&c, "get_faulty", vec![], |c| c.int(0));
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "app.codec.Faulty");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "10 / self > 0");
    assert_eq!(
        frames(&d),
        ["app.codec.Faulty", "app.codec.get_faulty"],
        "the predicate is an activation entered through the call"
    );
    assert_eq!(
        d["backtrace"][0]["span"],
        op_call("get_faulty", "c.faulty()")
    );
}

#[test]
fn validation_runs_before_the_ensures_clauses_which_see_the_replacing_value() {
    let c = codec();
    // A result of the wrong shape never reaches `ensures result >= n`.
    let d = fault_of(&c, "get_sized", vec![json!(3)], |_| Value::text("x"));
    assert_eq!(d["kind"], "binding_result_ill_typed");
    // A valid result that breaks the ensures clause: the binding's fault (S31, S57).
    let d = fault_of(&c, "get_sized", vec![json!(3)], |c| c.int(1));
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["attributed_to"], "binding");
    assert_eq!(d["symbol"], "app.codec.Codec.sized");
    // `ensures result.is_ok()` sees the Err(ForeignError) that replaced an invalid result.
    let d = fault_of(&c, "get_checked", vec![], |c| Value::ok(&c.p, c.int(42)));
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause"], "result.is_ok()");
    assert_eq!(d["actual"]["$variant"], "Err");
    assert_eq!(
        d["actual"]["$payload"][0]["message"],
        "the result of app.codec.Codec.checked does not satisfy Small at /$payload/0: self < 10"
    );
}

#[test]
fn validation_runs_in_every_contract_mode() {
    let c = codec();
    let unchecked = EvalConfig {
        check_contracts: false,
        ..EvalConfig::default()
    };
    let d = fault_with(
        &c,
        "get_size",
        vec![],
        |c| c.int(0),
        true,
        unchecked.clone(),
    );
    assert_eq!(d["code"], "refinement_violation");
    let d = fault_with(
        &c,
        "get_size",
        vec![],
        |_| Value::text("x"),
        true,
        unchecked,
    );
    assert_eq!(d["kind"], "binding_result_ill_typed");
}

#[test]
fn bind_capability_stays_trusted() {
    let c = codec();
    // Without validation, a 0 for Positive is not caught at the boundary ...
    let v = call_with(
        &c,
        "get_size",
        vec![],
        |c| c.int(0),
        false,
        EvalConfig::default(),
    );
    assert_eq!(v, Ok(json!(0)));
    // ... and a Text for an Int meets type_confusion where it is used (semantics 6.9, as in Milestone 1).
    let d = fault_with(
        &c,
        "twice",
        vec![],
        |_| Value::text("x"),
        false,
        EvalConfig::default(),
    );
    assert_eq!(d["kind"], "type_confusion");
}

#[test]
fn validate_result_can_be_called_on_its_own() {
    let c = codec();
    let mut it = Interpreter::new(&c.p, EvalConfig::default());
    let span = Span::new(cpl_diag::FileId(0), 1, 2);
    let ok = boundary::validate_result(&mut it, c.cap, c.op("size"), "b", span, c.int(3)).unwrap();
    assert_eq!(ok.as_int(), Some(&BigInt::from(3)));
    let fault =
        boundary::validate_result(&mut it, c.cap, c.op("size"), "b", span, c.int(0)).unwrap_err();
    assert_eq!(fault.span, Some(span));
    assert_eq!(fault.data["binding"], "b");
    let fault = boundary::validate_result(&mut it, c.cap, 99, "b", span, c.int(0)).unwrap_err();
    assert_eq!(fault.data["kind"], "unknown_operation");
    let ops = &c.p.def(c.cap).kind.as_capability().unwrap().ops;
    assert!(boundary::is_foreign_error_return(
        &c.p,
        &ops[c.op("dimensions") as usize].ret
    ));
    assert!(!boundary::is_foreign_error_return(
        &c.p,
        &ops[c.op("size") as usize].ret
    ));
    assert!(!boundary::is_foreign_error_return(&c.p, &Ty::Int));
}

#[test]
fn a_long_or_deep_result_is_validated_without_recursion() {
    let c = codec();
    // A value nested 200 000 deep, as a host may return one (the walk is a work list).
    let deep = |end: i64| {
        move |c: &Codec| {
            let mut v = Value::variant(&c.p, c.def("Chain"), "End", vec![Value::int(end)]).unwrap();
            for _ in 0..200_000 {
                v = Value::variant(&c.p, c.def("Chain"), "Link", vec![Value::some(&c.p, v)])
                    .unwrap();
            }
            v
        }
    };
    let d = on_eval_stack(|| {
        let r = call_with(
            &c,
            "get_chain",
            vec![],
            deep(0),
            true,
            EvalConfig::default(),
        );
        let d = r.expect_err("a refinement violation");
        (
            d["code"].clone(),
            d["pointer"].as_str().unwrap().len(),
            d["actual"].clone(),
        )
    });
    assert_eq!(d.0, "refinement_violation");
    assert_eq!(d.1, "/$payload/0".len() * 400_001);
    assert_eq!(d.2, 0);
    let ok = on_eval_stack(|| {
        call_with(
            &c,
            "get_chain",
            vec![],
            deep(1),
            true,
            EvalConfig::default(),
        )
        .map(|_| ())
        .map_err(|d| d.to_string())
    });
    assert_eq!(ok, Ok(()));
    // A long list of positions.
    let v = value_of(&c, "get_names", vec![], |c| {
        Value::list((0..100_000).map(|_| c.name("n")).collect())
    });
    assert_eq!(v.as_array().unwrap().len(), 100_000);
    let d = fault_of(&c, "get_names", vec![], |c| {
        let mut items: Vec<Value> = (0..50_000).map(|_| c.name("n")).collect();
        items.push(Value::int(1));
        Value::list(items)
    });
    assert_eq!(d["pointer"], "/50000");
}

// ---- foreign_failure (stdlib S45, semantics S80) ----------------------------------

#[test]
fn a_reported_failure_is_a_foreign_error_value_with_the_runtimes_details_first() {
    let c = codec();
    let details = vec![
        ("foreign_type".to_string(), "decode_png".to_string()),
        ("foreign_code".to_string(), "-3".to_string()),
        ("retry_safe".to_string(), "true".to_string()),
    ];
    for (i, kind) in cpl_rtspec::foreign::KINDS.iter().enumerate() {
        let v = boundary::foreign_failure(
            &c.p,
            c.cap,
            c.op("dimensions"),
            "imgcodec",
            kind,
            "bad \"png\"",
            &details,
        )
        .unwrap();
        let j = json::value_to_json_untyped(&c.p, &v);
        assert_eq!(j["$variant"], "Err");
        let e = &j["$payload"][0];
        assert_eq!(e["kind"]["$variant"], *kind);
        assert_eq!(e["message"], "bad \"png\"");
        let keys: Vec<&str> = e["details"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "adapter",
                "capability",
                "operation",
                "foreign_type",
                "foreign_code",
                "retry_safe"
            ]
        );
        assert_eq!(e["details"][0]["value"], "imgcodec");
        assert_eq!(e["details"][1]["value"], "app.codec.Codec");
        assert_eq!(e["details"][2]["value"], "app.codec.Codec.dimensions");
        assert_eq!(e["details"][4]["value"], "-3");
        // The variant index is the declaration order of cpl_rtspec::foreign::KINDS.
        let Value::Enum { payload, .. } = &v else {
            panic!("an Err")
        };
        let Value::Struct { fields, .. } = &payload[0] else {
            panic!("a ForeignError")
        };
        let variant = fields.iter().find_map(|f| match f {
            Value::Enum { variant, .. } => Some(*variant),
            _ => None,
        });
        assert_eq!(variant, Some(i as u32));
    }
}

/// A host that reports the failure `Unavailable: gone` for every operation.
struct Failing;

impl CapabilityImpl for Failing {
    fn call(
        &mut self,
        op: &str,
        _args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        let p = ctx.program;
        let cap = p
            .lookup(&SymbolPath::parse("app.codec.Codec").unwrap())
            .unwrap();
        let index = p
            .def(cap)
            .kind
            .as_capability()
            .unwrap()
            .ops
            .iter()
            .position(|o| o.name == op)
            .unwrap();
        boundary::foreign_failure(p, cap, index as u32, "imgcodec", "Unavailable", "gone", &[])
    }
}

#[test]
fn a_reported_failure_that_the_return_type_cannot_carry_is_binding_error_unrepresentable() {
    let c = codec();
    let fault = boundary::foreign_failure(
        &c.p,
        c.cap,
        c.op("size"),
        "imgcodec",
        "Exception",
        "line 1\nline 2",
        &[],
    )
    .unwrap_err();
    assert!(fault.span.is_none());
    let d = common::fault_json(&c.p, &fault);
    assert_eq!(d["kind"], "binding_error_unrepresentable");
    assert_eq!(
        d["detail"],
        "imgcodec reported Exception for app.codec.Codec.size: \"line 1\\nline 2\""
    );
    assert_eq!(d["operation"], "app.codec.Codec.size");
    assert_eq!(d["binding"], "imgcodec");

    // Returned by a host, the fault takes the operation call's span and the caller's frames.
    let d = on_eval_stack(|| {
        let mut it = Interpreter::new(&c.p, EvalConfig::default());
        let options = BindOptions {
            label: "imgcodec".into(),
            validate_results: true,
        };
        let h = it.bind_capability_with(c.cap, Box::new(Failing), options);
        let fault = it.call(c.def("get_size"), vec![], vec![h]).unwrap_err();
        common::fault_json(&c.p, &fault)
    });
    assert_eq!(d["kind"], "binding_error_unrepresentable");
    assert_eq!(d["span"], op_call("get_size", "c.size()"));
    assert_eq!(frames(&d), ["app.codec.get_size"]);
}

#[test]
fn a_reported_failure_is_not_validated_further() {
    // A failure that a validating binding reports as Err(ForeignError) is a value of the return type and passes.
    let c = codec();
    let d = on_eval_stack(|| {
        let mut it = Interpreter::new(&c.p, EvalConfig::default());
        let options = BindOptions {
            label: "imgcodec".into(),
            validate_results: true,
        };
        let h = it.bind_capability_with(c.cap, Box::new(Failing), options);
        let v = it
            .call(c.def("get_dimensions"), vec![Value::bytes(b"")], vec![h])
            .unwrap();
        json::value_to_json_untyped(&c.p, &v)
    });
    assert_eq!(d["$payload"][0]["kind"]["$variant"], "Unavailable");
    assert_eq!(d["$payload"][0]["message"], "gone");
}

// ===========================================================================
// prepare_run, run_function_with_hosts, RunEngine (toolchain 1.7.4; semantics S78)
// ===========================================================================

fn refusal<T>(r: &Result<T, RunError>) -> String {
    match r {
        Ok(_) => "ok".to_string(),
        Err(e) => match e {
            RunError::ProgramHasErrors => "ProgramHasErrors".to_string(),
            RunError::NotFound(_) => "NotFound".to_string(),
            RunError::NotAFunction(_) => "NotAFunction".to_string(),
            RunError::Generic(_) => "Generic".to_string(),
            RunError::CannotRunEffectful(_) => "CannotRunEffectful".to_string(),
            RunError::ArgCount { .. } => "ArgCount".to_string(),
            RunError::Arg { index, .. } => format!("Arg {index}"),
            RunError::ArgNotConstant { index, .. } => format!("ArgNotConstant {index}"),
            RunError::Bind(_) => "Bind".to_string(),
        },
    }
}

#[test]
fn prepare_run_refuses_and_decodes_as_run_function() {
    let reader = common::reader();
    let c = codec();
    let cases: Vec<(&Program, &str, Vec<RunArg>)> = vec![
        (&reader, "reader.pages.nope", vec![]),
        (&reader, "reader.pages.Page", vec![]),
        (&reader, "std.gen.list", vec![]),
        (&reader, "reader.pages.paginate", vec![]),
        (&reader, "reader.pages.load_page", vec![]),
        (&reader, "reader.pages.paginate_ints", vec![]),
        (
            &reader,
            "reader.pages.paginate_ints",
            common::json_args(&[json!([1]), json!("x")]),
        ),
        (
            &reader,
            "reader.pages.paginate_ints",
            common::const_args(&["[1, 2, 3]", "nope"]),
        ),
        (
            &reader,
            "reader.pages.paginate_ints",
            common::const_args(&["[1, 2, 3]", "2"]),
        ),
        (
            &reader,
            "reader.pages.positive_size",
            common::json_args(&[json!(" 7 ")]),
        ),
        (&c.p, "app.codec.get_size", vec![]),
    ];
    for (p, function, args) in &cases {
        let path = common::path(function);
        let ours = prepare_run(p, &path, args, &|_| false);
        let theirs = run_function(p, &path, args, &EvalConfig::default());
        assert_eq!(refusal(&ours), refusal(&theirs), "{function}");
        if let (Ok(prepared), Ok(outcome)) = (ours, theirs) {
            let fd = p.def(prepared.function).kind.as_fn().unwrap();
            assert_eq!(prepared.return_ty, outcome.return_ty);
            assert_eq!(prepared.arguments.len(), outcome.arguments.len());
            for ((a, b), param) in prepared
                .arguments
                .iter()
                .zip(&outcome.arguments)
                .zip(&fd.params)
            {
                assert_eq!(
                    cpl_eval::value_to_json(p, a, &param.ty),
                    cpl_eval::value_to_json(p, b, &param.ty)
                );
            }
        }
    }
    // An empty program has errors.
    let empty = Program::empty(std::sync::Arc::new(cpl_diag::SourceMap::new()));
    assert_eq!(
        refusal(&prepare_run(&empty, &common::path("a.b"), &[], &|_| true)),
        "ProgramHasErrors"
    );
}

#[test]
fn prepare_run_asks_for_hosts_only_for_using_parameters_and_runs_nothing() {
    let c = codec();
    let asked = RefCell::new(Vec::new());
    let has = |param: &ParamDef| {
        asked.borrow_mut().push(param.name.clone());
        param.name == "a"
    };
    // `two(using a, using b)`: b has no host, so the run is refused.
    let r = prepare_run(&c.p, &common::path("app.codec.two"), &[], &has);
    assert_eq!(refusal(&r), "CannotRunEffectful");
    assert_eq!(*asked.borrow(), ["a", "b"]);
    asked.borrow_mut().clear();
    // Every using parameter has a host: decoding follows.
    let r = prepare_run(
        &c.p,
        &common::path("app.codec.positive_arg"),
        &common::json_args(&[json!(0)]),
        &|_| true,
    );
    let prepared = r.expect("prepared: the refinement is checked by the call, not here");
    assert_eq!(prepared.arguments[0].as_int(), Some(&BigInt::from(0)));
    assert_eq!(prepared.function, c.def("positive_arg"));
    let r = prepare_run(
        &c.p,
        &common::path("app.codec.positive_arg"),
        &common::json_args(&[json!("x")]),
        &|_| true,
    );
    assert_eq!(refusal(&r), "Arg 0");
    // A function without using parameters never asks.
    let r = prepare_run(&c.p, &common::path("app.codec.get_size"), &[], &has);
    assert_eq!(refusal(&r), "CannotRunEffectful");
    let reader = common::reader();
    let r = prepare_run(
        &reader,
        &common::path("reader.pages.paginate_ints"),
        &common::const_args(&["[1]", "1"]),
        &has,
    );
    assert!(r.is_ok());
    assert_eq!(*asked.borrow(), ["c"]);
}

/// The hosts of a run: each `create` logs and answers an `Answer` host with `value`, unless `refuse` names the
/// parameter. The log is shared with the hosts' operation calls (an `Rc`: the hosts are not `Send`).
struct TestHosts {
    value: Value,
    refuse: Option<String>,
    validate: bool,
    log: Rc<RefCell<Vec<String>>>,
    thread: std::thread::ThreadId,
}

impl RunHosts for TestHosts {
    fn available(&self, param: &ParamDef) -> bool {
        self.log
            .borrow_mut()
            .push(format!("available {}", param.name));
        true
    }
    fn create(
        &mut self,
        param: &ParamDef,
    ) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>> {
        assert_eq!(
            std::thread::current().id(),
            self.thread,
            "hosts are created on the calling thread"
        );
        self.log.borrow_mut().push(format!("create {}", param.name));
        if self.refuse.as_deref() == Some(param.name.as_str()) {
            let d = Diagnostic::error(
                cpl_diag::DiagCode("interop_bind_failed"),
                cpl_diag::Category::Internal,
                "refused",
            );
            return Err(vec![d]);
        }
        Ok((
            Box::new(ThreadCheck {
                inner: Answer {
                    value: self.value.clone(),
                    log: Rc::clone(&self.log),
                },
                thread: self.thread,
            }),
            BindOptions {
                label: format!("binding-{}", param.name),
                validate_results: self.validate,
            },
        ))
    }
}

struct ThreadCheck {
    inner: Answer,
    thread: std::thread::ThreadId,
}

impl CapabilityImpl for ThreadCheck {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        assert_eq!(
            std::thread::current().id(),
            self.thread,
            "a host is called on the calling thread"
        );
        self.inner.call(op, args, ctx)
    }
}

fn hosts(value: Value, refuse: Option<&str>) -> TestHosts {
    TestHosts {
        value,
        refuse: refuse.map(str::to_string),
        validate: true,
        log: Rc::new(RefCell::new(Vec::new())),
        thread: std::thread::current().id(),
    }
}

#[test]
fn run_function_with_hosts_binds_in_parameter_order_and_calls_the_hosts() {
    let c = codec();
    let mut h = hosts(Value::int(4), None);
    let outcome = run_function_with_hosts(
        &c.p,
        &common::path("app.codec.two"),
        &[],
        &EvalConfig::default(),
        &mut h,
    )
    .unwrap();
    assert_eq!(common::value(&c.p, &outcome), json!(44));
    assert_eq!(
        *h.log.borrow(),
        [
            "available a",
            "available b",
            "create a",
            "create b",
            "size()",
            "size()"
        ]
    );

    // The handles are numbered from 0 in parameter order, and effects are traced.
    let config = EvalConfig {
        trace_effects: true,
        ..EvalConfig::default()
    };
    let mut h = hosts(Value::int(4), None);
    let outcome =
        run_function_with_hosts(&c.p, &common::path("app.codec.two"), &[], &config, &mut h)
            .unwrap();
    let handles: Vec<u32> = outcome.effects.iter().map(|e| e.handle.0).collect();
    assert_eq!(handles, [0, 1]);

    // Results are validated with the binding name the host gave.
    let mut h = hosts(Value::int(0), None);
    let outcome = run_function_with_hosts(
        &c.p,
        &common::path("app.codec.get_size"),
        &[],
        &EvalConfig::default(),
        &mut h,
    )
    .unwrap();
    let d = common::fault(&c.p, &outcome);
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["binding"], "binding-c");
}

#[test]
fn hosts_are_created_after_decoding_and_before_the_arguments_are_checked() {
    let c = codec();
    // A decoding error: no host is created.
    let mut h = hosts(Value::int(1), None);
    let r = run_function_with_hosts(
        &c.p,
        &common::path("app.codec.positive_arg"),
        &common::json_args(&[json!("x")]),
        &EvalConfig::default(),
        &mut h,
    );
    assert_eq!(refusal(&r), "Arg 0");
    assert_eq!(*h.log.borrow(), ["available c"]);
    // A refinement violation of an argument: the host was created (granted, S78), but never called.
    let mut h = hosts(Value::int(1), None);
    let outcome = run_function_with_hosts(
        &c.p,
        &common::path("app.codec.positive_arg"),
        &common::json_args(&[json!(0)]),
        &EvalConfig::default(),
        &mut h,
    )
    .unwrap();
    let d = common::fault(&c.p, &outcome);
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["parameter"], "n");
    assert_eq!(*h.log.borrow(), ["available c", "create c"]);
    assert_eq!(
        common::outcome_json(&c.p, &outcome)["fault"]["code"],
        "refinement_violation"
    );
    // A valid argument runs.
    let mut h = hosts(Value::int(1), None);
    let outcome = run_function_with_hosts(
        &c.p,
        &common::path("app.codec.positive_arg"),
        &common::json_args(&[json!(2)]),
        &EvalConfig::default(),
        &mut h,
    )
    .unwrap();
    assert_eq!(common::value(&c.p, &outcome), json!(3));
}

#[test]
fn a_host_that_cannot_be_created_is_run_error_bind_and_nothing_runs() {
    let c = codec();
    let mut h = hosts(Value::int(1), Some("a"));
    let r = run_function_with_hosts(
        &c.p,
        &common::path("app.codec.two"),
        &[],
        &EvalConfig::default(),
        &mut h,
    );
    let Err(RunError::Bind(diagnostics)) = r else {
        panic!("RunError::Bind expected")
    };
    assert_eq!(diagnostics[0].code.0, "interop_bind_failed");
    assert_eq!(*h.log.borrow(), ["available a", "available b", "create a"]);
    let mut h = hosts(Value::int(1), Some("b"));
    let r = run_function_with_hosts(
        &c.p,
        &common::path("app.codec.two"),
        &[],
        &EvalConfig::default(),
        &mut h,
    );
    assert_eq!(refusal(&r), "Bind");
    assert_eq!(
        *h.log.borrow(),
        ["available a", "available b", "create a", "create b"]
    );
}

#[test]
fn run_function_still_refuses_using_parameters() {
    let c = codec();
    let r = run_function(
        &c.p,
        &common::path("app.codec.get_size"),
        &[],
        &EvalConfig::default(),
    );
    assert_eq!(refusal(&r), "CannotRunEffectful");
}

#[test]
fn a_run_with_hosts_evaluates_on_the_evaluation_thread() {
    // 5000 nested activations need the evaluation thread's stack; the host is called from it through the calling
    // thread.
    let c = codec();
    let mut h = hosts(Value::int(6), None);
    let outcome = run_function_with_hosts(
        &c.p,
        &common::path("app.codec.deep"),
        &common::json_args(&[json!(5000)]),
        &EvalConfig::default(),
        &mut h,
    )
    .unwrap();
    assert_eq!(common::value(&c.p, &outcome), json!(6));
}

#[test]
fn the_interpreter_is_a_run_engine() {
    let c = codec();
    let f = c.def("positive_arg");
    let fd = c.p.def(f).kind.as_fn().unwrap();
    let param = &fd.using[0];
    let d = on_eval_stack(|| {
        let mut it = Interpreter::new(&c.p, EvalConfig::default());
        let log = Rc::new(RefCell::new(Vec::new()));
        let source = HostSource::Impl(
            Box::new(Answer {
                value: Value::int(5),
                log: Rc::clone(&log),
            }),
            BindOptions {
                label: "x".into(),
                validate_results: true,
            },
        );
        let h: Handle = RunEngine::bind(&mut it, param, source).unwrap();
        assert_eq!(h.id.0, 0);
        assert_eq!(h.label, "x");
        let refused =
            RunEngine::bind(&mut it, param, HostSource::Binding("imgcodec".into())).unwrap_err();
        assert_eq!(refused[0].code.0, "internal_error");
        assert_eq!(refused[0].data["kind"], "call_mismatch");
        let ok = it
            .call_checked(f, vec![Value::int(2)], vec![h.clone()])
            .unwrap();
        assert_eq!(ok.as_int(), Some(&BigInt::from(7)));
        // S39 (a): the arguments' refined positions are checked first.
        let fault = it
            .call_checked(f, vec![Value::int(0)], vec![h])
            .unwrap_err();
        assert_eq!(log.borrow().len(), 1, "the refused call reached no host");
        common::fault_json(&c.p, &fault)
    });
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["parameter"], "n");
}

// ===========================================================================
// requires_spec, requires_violation_at, check_gen_range_at, operation_identity
// ===========================================================================

#[test]
fn requires_spec_takes_the_stubs_facts() {
    let p = common::reader();
    let facts: RequiresFacts = builtins::requires_spec(&p, "list_take", 0);
    assert_eq!(facts.symbol.as_str(), "std.prelude.List.take");
    assert_eq!(facts.clause, "n >= 0");
    assert_eq!(facts.inputs, ["self", "n"]);
    assert!(facts.span.is_some());
    for spec in cpl_rtspec::builtins::BUILTINS.iter() {
        for (i, r) in spec.requires.iter().enumerate() {
            let facts = builtins::requires_spec(&p, spec.id, i);
            assert_eq!(facts.clause, r.clause, "{}", spec.id);
            assert_eq!(facts.inputs, r.inputs, "{}", spec.id);
            assert_eq!(facts.symbol.as_str(), spec.fallback_symbol, "{}", spec.id);
            let stub = builtins::stub_of(&p, spec.id).expect("a stub");
            assert_eq!(
                facts.span,
                Some(p.def(stub).kind.as_fn().unwrap().requires[i].span)
            );
        }
    }
}

#[test]
fn requires_spec_falls_back_to_cpl_rtspec_without_stubs() {
    let p = Program::empty(std::sync::Arc::new(cpl_diag::SourceMap::new()));
    for spec in cpl_rtspec::builtins::BUILTINS.iter() {
        for (i, r) in spec.requires.iter().enumerate() {
            let facts = builtins::requires_spec(&p, spec.id, i);
            assert_eq!(facts.symbol.as_str(), spec.fallback_symbol);
            assert_eq!(facts.clause, r.clause);
            assert_eq!(facts.inputs, r.inputs);
            assert_eq!(facts.span, None);
        }
    }
}

#[test]
fn requires_violation_at_is_the_fault_an_evaluation_raises() {
    let p = common::check_source(
        "module rq.main\n\npub fn f(xs: List<Int>) -> List<Int> {\n    xs.take(-1)\n}\n",
    );
    let outcome = common::run_json(&p, "rq.main.f", &[json!([1, 2])]);
    let raised = common::fault(&p, &outcome);
    let f = common::def(&p, "rq.main.f");
    let built = builtins::requires_violation_at(
        &p,
        Some(f),
        "list_take",
        0,
        &[
            Value::list(vec![Value::int(1), Value::int(2)]),
            Value::int(-1),
        ],
    );
    let mut built = common::fault_json(&p, &built);
    // The evaluation adds the frames as the fault leaves the stub and the function.
    built["backtrace"] = raised["backtrace"].clone();
    assert_eq!(built, raised);
    let top = builtins::requires_violation_at(
        &p,
        None,
        "list_take",
        0,
        &[Value::empty_list(), Value::int(-1)],
    );
    assert!(common::fault_json(&p, &top).get("caller").is_none());
}

#[test]
fn check_gen_range_at_is_check_gen_range_with_the_caller_given() {
    let p = runner_cases::props();
    let prop = common::def(&p, "tests.props.\"empty range\"");
    let (lo, hi) = (BigInt::from(5), BigInt::from(1));
    assert!(builtins::check_gen_range_at(&p, Some(prop), &hi, &lo).is_ok());
    assert!(builtins::check_gen_range_at(&p, None, &lo, &lo).is_ok());
    let at = builtins::check_gen_range_at(&p, Some(prop), &lo, &hi).unwrap_err();
    let mut it = Interpreter::new(&p, EvalConfig::default());
    it.push_frame(prop, None);
    let with = builtins::check_gen_range(&it, &lo, &hi).unwrap_err();
    it.pop_frame();
    let d = common::fault_json(&p, &at);
    assert_eq!(d, common::fault_json(&p, &with));
    assert_eq!(d["symbol"], "std.gen.range");
    assert_eq!(d["clause"], "lo <= hi");
    assert_eq!(d["input"], json!({"lo": 5, "hi": 1}));
    assert_eq!(d["caller"], "tests.props.\"empty range\"");
    let none = builtins::check_gen_range_at(&p, None, &lo, &hi).unwrap_err();
    assert!(common::fault_json(&p, &none).get("caller").is_none());
}

#[test]
fn operation_identity_names_the_operation() {
    let p = common::reader();
    let read = common::def(&p, "reader.pages.PageRead");
    let image_codec = common::def(&p, "reader.images.ImageCodec");
    assert_eq!(
        operation_identity(&p, read, "get").as_str(),
        "reader.pages.PageRead.get"
    );
    assert_eq!(
        operation_identity(&p, image_codec, "dimensions").as_str(),
        "reader.images.ImageCodec.dimensions"
    );
    // It is the symbol of an operation's clause fault.
    let c = codec();
    let d = fault_of(&c, "get_sized", vec![json!(3)], |c| c.int(1));
    assert_eq!(
        d["symbol"],
        operation_identity(&c.p, c.cap, "sized").as_str()
    );
}

// ===========================================================================
// scope_at (toolchain 1.7.4; semantics S41; spec/cabi.md 3.2)
// ===========================================================================

/// Every binding site an evaluation reached: (declaration, span, locals outermost first).
type Seen = Rc<RefCell<Vec<(DefId, Span, Vec<LocalId>)>>>;

fn probe(it: &mut Interpreter<'_>, seen: &Seen) {
    let seen = Rc::clone(seen);
    it.set_site_probe(Box::new(move |def, span, scope| {
        seen.borrow_mut().push((def, span, scope.to_vec()));
    }));
}

/// Every reached site equals `scope_at`'s entry for it; returns the sites of `scope_at` that were reached.
fn check_seen(p: &Program, seen: &Seen, reached: &mut BTreeMap<(u32, u32), bool>) {
    for (def, span, scope) in seen.borrow().iter() {
        let sites = scope_at(p, *def);
        let entry = sites.iter().find(|(s, _)| s == span);
        let Some((_, locals)) = entry else {
            panic!("{}: the interpreter reached a site at {span:?} that scope_at does not list ({sites:?})", p.display_path(*def));
        };
        assert_eq!(
            locals,
            scope,
            "{}: the locals at {span:?}",
            p.display_path(*def)
        );
        reached.insert((def.0, span.start), true);
    }
}

/// Every site of every definition of the program's non-std modules, unreached.
fn all_sites(p: &Program) -> BTreeMap<(u32, u32), bool> {
    let mut out = BTreeMap::new();
    for d in p.defs() {
        if p.module(d.module).path.first().map(String::as_str) == Some("std") {
            continue;
        }
        for (span, _) in scope_at(p, d.id) {
            out.insert((d.id.0, span.start), false);
        }
    }
    out
}

fn corpus_program(name: &str) -> (Program, String) {
    let source = std::fs::read_to_string(common::corpus_dir().join(format!("{name}.cpl"))).unwrap();
    let path = format!("src/corpus/{name}.cpl");
    (
        common::check_files(&[(path.as_str(), source.as_str())]),
        source,
    )
}

/// Run a function with JSON arguments on an Interpreter that observes its binding sites: the fault's span and
/// diagnostic, if it faulted.
fn observe_call(p: &Program, f: DefId, args: &[Json], seen: &Seen) -> Option<(Option<Span>, Json)> {
    let fd = p.def(f).kind.as_fn().unwrap();
    let found = on_eval_stack(|| {
        let values: Vec<Value> = args
            .iter()
            .zip(&fd.params)
            .map(|(a, param)| {
                cpl_eval::value_from_json(p, a, &param.ty)
                    .unwrap_or_else(|e| panic!("{}: {}", e.pointer, e.message))
            })
            .collect();
        let local = Rc::new(RefCell::new(Vec::new()));
        let mut it = Interpreter::new(p, EvalConfig::default());
        probe(&mut it, &local);
        let out = it.call(f, values, vec![]);
        let fault = out
            .err()
            .map(|fault| (fault.span, common::fault_json(p, &fault)));
        let local = local.borrow().clone();
        (local, fault)
    });
    seen.borrow_mut().extend(found.0);
    found.1
}

#[test]
fn scope_at_is_the_interpreters_environment_at_every_site_of_the_interpreter_corpus() {
    let mut names: Vec<String> = std::fs::read_dir(common::corpus_dir())
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            (path.extension()? == "cpl")
                .then(|| path.file_stem().unwrap().to_string_lossy().to_string())
        })
        .collect();
    names.sort();
    assert!(names.len() >= 12);
    let mut total = 0;
    let mut total_reached_cases = 0;
    let mut checked_faults = 0;
    for name in names {
        let (p, _) = corpus_program(&name);
        let cases_text =
            std::fs::read_to_string(common::corpus_dir().join(format!("{name}.run.json"))).unwrap();
        let cases = parse_cases(&cases_text).unwrap();
        let seen: Seen = Rc::new(RefCell::new(Vec::new()));
        let mut reached = all_sites(&p);
        total += reached.len();
        total_reached_cases += cases.len();
        // The cases of the module, and the calls that reach the sites no case reaches: `paginate` of
        // `corpus.holes` is generic, so no `cpl run` case can call it (its .cpl says so), but the interpreter can.
        let mut calls: Vec<(String, Vec<Json>)> = cases
            .iter()
            .map(|c| (c.function.clone(), c.args.clone()))
            .collect();
        if name == "holes" {
            calls.push((
                "corpus.holes.paginate".to_string(),
                vec![json!([]), json!(2)],
            ));
        }
        for (function, args) in &calls {
            let f = p.lookup(&SymbolPath::parse(function).unwrap()).unwrap();
            let fault = observe_call(&p, f, args, &seen);
            // A fault that lists bindings lists the locals of its site, innermost first.
            if let Some((span, d)) = fault {
                if let Some(bindings) = d.get("bindings").and_then(Json::as_array) {
                    let names: Vec<&str> = bindings
                        .iter()
                        .map(|b| b["name"].as_str().unwrap())
                        .collect();
                    let site = seen
                        .borrow()
                        .iter()
                        .rev()
                        .find(|(_, s, _)| Some(*s) == span)
                        .cloned()
                        .expect("the faulting site was observed");
                    let locals = p.def(site.0).kind.locals();
                    let expected: Vec<&str> = site
                        .2
                        .iter()
                        .rev()
                        .map(|l| locals[l.0 as usize].name.as_str())
                        .collect();
                    assert_eq!(names, expected, "{function}: the bindings of the fault");
                    checked_faults += 1;
                }
            }
        }
        check_seen(&p, &seen, &mut reached);
        let missed: Vec<_> = reached.iter().filter(|(_, r)| !**r).collect();
        assert!(
            missed.is_empty(),
            "corpus.{name}: sites that no case reaches: {missed:?}"
        );
    }
    assert!(total >= 9, "the corpus has binding sites ({total})");
    assert!(total_reached_cases >= 74);
    assert!(
        checked_faults >= 2,
        "faults that list bindings were compared ({checked_faults})"
    );
}

const SCOPES: &str = r#"module scopes.main

pub capability Db {
    fn get(id: Int) -> Int
}

pub enum Pair {
    Two(Int, Option<Int>)
    One(Int)
}

pub fn shapes(n: Int, xs: List<Int>, using db: Db) -> Int
    effects { db.get }
{
    let a = db.get(n)
    var total = 0
    var i = 0
    while i < xs.len
        invariant i >= 0
    {
        let x = xs.get(i)
        match x {
            Some(v) => {
                let doubled = v * 2
                total = total + doubled
                assert doubled == v + v
            }
            None => {}
        }
        i = i + 1
    }
    assert forall y in xs: y == y
    if total >= 0 {
        let big = total
        assert big >= 0
    } else {
        let small = total
        assert small < 0
    }
    total + a
}

pub fn pairs(p: Pair) -> Int {
    match p {
        Two(a, Some(b)) => {
            assert a == a
            var s = a + b
            while s < 0
                invariant s <= 0
            {
                assert s == s
                s = s + 1
            }
            s
        }
        Two(a, None) => a
        One(c) => {
            let d = c
            assert d == c
            d
        }
    }
}

pub fn hole_in_arm(p: Pair) -> Int {
    let k = 1
    match p {
        One(c) => ?inside
        _ => k
    }
}

pub fn hole_in_forall(xs: List<Int>) -> Bool {
    let t = 0
    forall z in xs: ?pred
}

test "a test" {
    let x = 1
    assert x == 1
    let y = x + 1
    while y < 0 {
        let w = y
        assert w < 0
    }
    assert y == 2
}

property "a property"
    for a in gen.int
    for b in gen.bool
{
    let c = a
    assert c == a or b
}
"#;

struct Fixed(i64);

impl CapabilityImpl for Fixed {
    fn call(
        &mut self,
        _op: &str,
        _args: Vec<Value>,
        _ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        Ok(Value::int(self.0))
    }
}

#[test]
fn scope_at_is_the_interpreters_environment_in_blocks_arms_loops_tests_and_properties() {
    let p = common::check_source(SCOPES);
    let seen: Seen = Rc::new(RefCell::new(Vec::new()));
    let mut reached = all_sites(&p);
    let db = common::def(&p, "scopes.main.Db");
    let shapes = common::def(&p, "scopes.main.shapes");
    // shapes: the using parameter after the positional ones (cabi 3.2: `db` is listed before `id`, innermost first).
    for (n, xs) in [(1, json!([1, 2, 3])), (2, json!([-1]))] {
        let local = on_eval_stack(|| {
            let values = vec![
                Value::int(n),
                cpl_eval::value_from_json(&p, &xs, &Ty::List(Box::new(Ty::Int)))
                    .ok()
                    .unwrap(),
            ];
            let local: Seen = Rc::new(RefCell::new(Vec::new()));
            let mut it = Interpreter::new(&p, EvalConfig::default());
            probe(&mut it, &local);
            let h = it.bind_capability(db, Box::new(Fixed(n)), "db");
            it.call(shapes, values, vec![h]).unwrap();
            let v = local.borrow().clone();
            v
        });
        seen.borrow_mut().extend(local);
    }
    let pair = |j: Json| vec![j];
    for arg in [
        json!({"$type": "scopes.main.Pair", "$variant": "Two", "$payload": [3, {"$type": "Option", "$variant": "Some", "$payload": [-5]}]}),
        json!({"$type": "scopes.main.Pair", "$variant": "Two", "$payload": [3, {"$type": "Option", "$variant": "Some", "$payload": [4]}]}),
        json!({"$type": "scopes.main.Pair", "$variant": "One", "$payload": [9]}),
    ] {
        let _ = observe_call(&p, common::def(&p, "scopes.main.pairs"), &pair(arg), &seen);
    }
    let (_, d) = observe_call(
        &p,
        common::def(&p, "scopes.main.hole_in_arm"),
        &pair(json!({"$type": "scopes.main.Pair", "$variant": "One", "$payload": [9]})),
        &seen,
    )
    .unwrap();
    let names: Vec<&str> = d["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["c", "k", "p"]);
    let (_, d) = observe_call(
        &p,
        common::def(&p, "scopes.main.hole_in_forall"),
        &[json!([1])],
        &seen,
    )
    .unwrap();
    let names: Vec<&str> = d["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["z", "t", "xs"]);
    // The test and the property, through the runner's engine.
    for name in ["scopes.main.\"a test\"", "scopes.main.\"a property\""] {
        let test = common::def(&p, name);
        let local = on_eval_stack(|| {
            let local: Seen = Rc::new(RefCell::new(Vec::new()));
            let mut it = Interpreter::new(&p, EvalConfig::default());
            probe(&mut it, &local);
            let record = run_test_with(&p, test, &TestConfig::with_seed(1), &mut it);
            assert_eq!(record.status, cpl_diag::json::TestStatus::Tested, "{name}");
            let v = local.borrow().clone();
            v
        });
        seen.borrow_mut().extend(local);
    }
    check_seen(&p, &seen, &mut reached);
    // Every site but the loop bodies that no input enters is reached.
    let missed: Vec<String> = reached
        .iter()
        .filter(|(_, r)| !**r)
        .map(|((def, start), _)| format!("{}@{start}", p.display_path(DefId(*def))))
        .collect();
    assert_eq!(
        missed.len(),
        1,
        "the loop body of the test, which no run enters: {missed:?}"
    );
    // The listed order: source order, outermost first for a shared start.
    let shapes_sites = scope_at(&p, shapes);
    let starts: Vec<u32> = shapes_sites.iter().map(|(s, _)| s.start).collect();
    let mut sorted = starts.clone();
    sorted.sort();
    assert_eq!(starts, sorted);
    let names = |locals: &[LocalId]| -> Vec<String> {
        let table = p.def(shapes).kind.locals();
        locals
            .iter()
            .map(|l| table[l.0 as usize].name.clone())
            .collect()
    };
    let listed: Vec<Vec<String>> = shapes_sites.iter().map(|(_, l)| names(l)).collect();
    assert_eq!(
        listed,
        [
            vec!["n", "xs", "db", "a", "total", "i"],
            vec!["n", "xs", "db", "a", "total", "i", "x", "v", "doubled"],
            vec!["n", "xs", "db", "a", "total", "i"],
            vec!["n", "xs", "db", "a", "total", "i", "big"],
            vec!["n", "xs", "db", "a", "total", "i", "small"],
        ]
    );
    // A declaration without a body has no site.
    assert!(scope_at(&p, db).is_empty());
    assert!(scope_at(&p, DefId(u32::MAX)).is_empty());
}
