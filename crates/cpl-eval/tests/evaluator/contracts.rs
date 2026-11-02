//! Contract checking in the interpreter (toolchain 1.7.1; semantics 5.2-5.4,
//! S34, S35, S57, S70) on checked programs: the order of `requires`, the
//! body, and `ensures`; every exit; loop clauses; header `decreases`;
//! faulting clauses.
//!
//! The functions are those of the corpus modules `corpus.contracts` and
//! `corpus.control` (`tests/corpus/*.cpl`, read as written) plus
//! `checks.contracts` below; spans are compared with the real source
//! locations (a clause's span runs from its keyword, toolchain 3.3).

use crate::common;
use cpl_eval::EvalConfig;
use cpl_hir::Program;
use serde_json::{json, Value as Json};

const CONTRACTS: &str = "src/corpus/contracts.cpl";
const EXTRA: &str = "src/checks/contracts.cpl";

/// Functions the corpus modules do not have.
const EXTRA_SOURCE: &str = r#"module checks.contracts

fn recip(n: Int) -> Int
    requires n != 0
{
    100 / n
}

fn outer() -> Int {
    recip(0)
}

// The measure relies on the precondition (design line 118).
fn guarded_measure(a: Int, b: Int) -> Int
    requires b != 0
    decreases a / b
{
    0
}

fn faulting_clause(n: Int) -> Int
    requires 10 / n > 0
{
    n
}

// The inner clause claims the fault (S70).
fn nested_clause(n: Int) -> Int
    ensures faulting_clause(n) == result
{
    n
}

// A clause calling a function whose requires is false (S70).
fn g(x: Int) -> Int
    requires recip(x) >= 0
{
    x
}
"#;

fn corpus(name: &str) -> String {
    std::fs::read_to_string(common::corpus_dir().join(format!("{name}.cpl")))
        .expect("a corpus module")
}

/// `corpus.contracts`, `corpus.control`, and `checks.contracts`, checked.
fn fixture() -> Program {
    let contracts = corpus("contracts");
    let control = corpus("control");
    common::check_files(&[
        (CONTRACTS, contracts.as_str()),
        ("src/corpus/control.cpl", control.as_str()),
        (EXTRA, EXTRA_SOURCE),
    ])
}

fn run(p: &Program, f: &str, args: &[Json]) -> Json {
    common::outcome_json(p, &common::run_json(p, f, args))
}

fn value(p: &Program, f: &str, args: &[Json]) -> Json {
    common::value(p, &common::run_json(p, f, args))
}

fn fault(p: &Program, f: &str, args: &[Json]) -> Json {
    common::fault(p, &common::run_json(p, f, args))
}

fn backtrace(d: &Json) -> Vec<String> {
    d["backtrace"]
        .as_array()
        .map(|frames| {
            frames
                .iter()
                .map(|f| f["qualified_name"].as_str().unwrap_or("?").to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn requires_runs_before_the_body_with_the_exact_payload() {
    let p = fixture();
    let d = fault(&p, "corpus.contracts.recip", &[json!(0)]);
    // A contract violation, not the body's division by zero (5.2 step 3).
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["category"], "contract");
    assert_eq!(d["severity"], "error");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["clause"], "n != 0");
    assert_eq!(d["symbol"], "corpus.contracts.recip");
    assert_eq!(d["qualified_name"], "corpus.contracts.recip");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["input"], json!({"n": 0}));
    assert_eq!(d["status"], "failed");
    // The span is the clause, from its keyword (toolchain 3.3).
    assert_eq!(
        d["span"],
        common::location_of(&corpus("contracts"), CONTRACTS, "requires n != 0")
    );
    // No calling declaration at the top level (S39 (c)).
    assert!(d.get("caller").is_none(), "{d}");
    assert!(d.get("actual").is_none());
    assert_eq!(backtrace(&d), ["corpus.contracts.recip"]);
}

#[test]
fn requires_names_the_calling_declaration() {
    let p = fixture();
    let d = fault(&p, "checks.contracts.outer", &[]);
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "checks.contracts.recip");
    assert_eq!(d["caller"], "checks.contracts.outer");
    assert_eq!(
        backtrace(&d),
        ["checks.contracts.recip", "checks.contracts.outer"]
    );
    // The callee's frame carries the span of the call that entered it.
    assert_eq!(
        d["backtrace"][0]["span"],
        common::location_of(EXTRA_SOURCE, EXTRA, "recip(0)")
    );
    assert!(d["backtrace"][1].get("span").is_none());
}

#[test]
fn requires_clauses_run_in_order_and_stop_at_the_first_false() {
    let p = common::reader();
    let f = "reader.pages.divide_exactly";
    assert_eq!(value(&p, f, &[json!(6), json!(3)]), json!(2));
    // b != 0 fails first; a % b == 0 never runs, so no division_by_zero.
    let d = fault(&p, f, &[json!(7), json!(0)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause"], "b != 0");
    assert_eq!(d["input"], json!({"a": 7, "b": 0}));
    let d = fault(&p, f, &[json!(7), json!(2)]);
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["clause"], "a % b == 0");
}

#[test]
fn ensures_runs_on_the_tail_exit() {
    let p = fixture();
    let d = fault(&p, "corpus.contracts.half", &[json!(5)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "result * 2 == n");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "corpus.contracts.half");
    assert_eq!(d["input"], json!({"n": 5}));
    assert_eq!(d["actual"], json!(2));
    assert_eq!(
        d["span"],
        common::location_of(&corpus("contracts"), CONTRACTS, "ensures result * 2 == n")
    );
    assert_eq!(value(&p, "corpus.contracts.half", &[json!(4)]), json!(2));
}

#[test]
fn ensures_runs_on_an_early_return() {
    let p = fixture();
    let f = "corpus.control.clamp_pos";
    assert_eq!(value(&p, f, &[json!(5)]), json!(5));
    let d = fault(&p, f, &[json!(-2)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "result > 0");
    assert_eq!(d["actual"], json!(-2));
    assert_eq!(d["input"], json!({"n": -2}));
}

#[test]
fn ensures_runs_on_a_propagated_err() {
    let p = fixture();
    let f = "corpus.contracts.passthrough";
    let ok = json!({"$type": "Result", "$variant": "Ok", "$payload": [3]});
    assert_eq!(value(&p, f, std::slice::from_ref(&ok)), ok);
    let err = json!({"$type": "Result", "$variant": "Err", "$payload": ["e"]});
    let d = fault(&p, f, std::slice::from_ref(&err));
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "result.is_ok()");
    // The exit value of the `?` is the propagated Err itself.
    assert_eq!(d["actual"], err);
    assert_eq!(d["input"], json!({"r": err}));
}

#[test]
fn ensures_runs_on_a_return_inside_a_loop() {
    let p = fixture();
    let f = "corpus.control.find_index";
    assert_eq!(value(&p, f, &[json!([5, 6, 7]), json!(6)]), json!(1));
    assert_eq!(value(&p, f, &[json!([5, 6, 7]), json!(9)]), json!(-1));
}

#[test]
fn a_loop_that_satisfies_its_clauses() {
    let p = fixture();
    assert_eq!(value(&p, "corpus.control.sum_to", &[json!(4)]), json!(10));
    assert_eq!(value(&p, "corpus.control.sum_to", &[json!(0)]), json!(0));
}

#[test]
fn an_invariant_is_checked_after_every_iteration_body() {
    let p = fixture();
    let f = "corpus.contracts.bad_loop";
    assert_eq!(value(&p, f, &[json!(2)]), json!(2));
    let d = fault(&p, f, &[json!(3)]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["clause"], "i < 3");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "corpus.contracts.bad_loop");
    assert_eq!(d["iteration"], json!(3));
    assert_eq!(d["actual"], json!({"bindings": {"n": 3, "i": 3}}));
    assert_eq!(d["input"], json!({"n": 3}));
    assert_eq!(
        d["span"],
        common::location_of(&corpus("contracts"), CONTRACTS, "invariant i < 3")
    );
}

#[test]
fn a_loop_measure_is_evaluated_only_after_the_condition_held() {
    let p = fixture();
    let f = "corpus.contracts.stuck";
    // One iteration with a constant measure: evaluated once, never after the exit.
    assert_eq!(value(&p, f, &[json!(1)]), json!(1));
    assert_eq!(value(&p, f, &[json!(0)]), json!(0));
    let d = fault(&p, f, &[json!(2)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "n");
    assert_eq!(d["iteration"], json!(1));
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 2, "i": 1}, "previous": 2, "measure": 2})
    );
}

#[test]
fn a_negative_loop_measure() {
    let p = fixture();
    let f = "corpus.contracts.neg_measure";
    let d = fault(&p, f, &[json!(1)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "i - 1");
    assert_eq!(d["iteration"], json!(0));
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 1, "i": 0}, "measure": -1})
    );
    // The loop never ran, so the measure was never evaluated for n = 0.
    assert_eq!(value(&p, f, &[json!(0)]), json!(0));
}

#[test]
fn a_header_measure_compares_with_the_nearest_activation() {
    let p = fixture();
    let d = fault(&p, "corpus.contracts.bad_rec", &[json!(1)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "n");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["actual"], json!({"previous": 1, "measure": 1}));
    assert_eq!(d["input"], json!({"n": 1}));
    // Detected at the second entry: two activations, no runaway recursion.
    assert_eq!(
        backtrace(&d),
        ["corpus.contracts.bad_rec", "corpus.contracts.bad_rec"]
    );
    assert_eq!(value(&p, "corpus.contracts.bad_rec", &[json!(0)]), json!(0));
    assert_eq!(
        value(&p, "corpus.contracts.count_down", &[json!(3)]),
        json!(3)
    );
    let d = fault(&p, "corpus.contracts.count_down", &[json!(-1)]);
    // requires n >= 0 runs before the measure.
    assert_eq!(d["clause_kind"], "requires");
}

#[test]
fn a_header_measure_runs_after_the_preconditions() {
    let p = fixture();
    let f = "checks.contracts.guarded_measure";
    // decreases a / b under requires b != 0: b == 0 is the requires
    // violation, never a division by zero in the measure (design line 118).
    let d = fault(&p, f, &[json!(5), json!(0)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    // A negative measure at the first entry.
    let d = fault(&p, f, &[json!(-5), json!(1)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["actual"], json!({"measure": -5}));
    assert_eq!(d["input"], json!({"a": -5, "b": 1}));
    assert_eq!(value(&p, f, &[json!(5), json!(1)]), json!(0));
}

#[test]
fn a_faulting_clause_is_that_fault_with_the_clause_s34() {
    let p = fixture();
    let d = fault(&p, "corpus.contracts.faulting_clause", &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["category"], "runtime");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "10 / n > 0");
    assert_eq!(d["symbol"], "corpus.contracts.faulting_clause");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
    assert!(d.get("clause_kind").is_none());
    assert_eq!(backtrace(&d), ["corpus.contracts.faulting_clause"]);
    // The span is the operator that faulted, inside the clause.
    assert_eq!(
        d["span"],
        common::location_of(&corpus("contracts"), CONTRACTS, "10 / n")
    );
}

#[test]
fn nested_clauses_report_the_innermost_clause() {
    let p = fixture();
    // nested_clause's ensures calls faulting_clause(0), whose own requires
    // faults: the requires clause of faulting_clause is reported (S70).
    let d = fault(&p, "checks.contracts.nested_clause", &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "checks.contracts.faulting_clause");
    assert_eq!(d["clause"], "10 / n > 0");
    assert_eq!(d["facet"], "contract");
    assert_eq!(
        backtrace(&d),
        [
            "checks.contracts.faulting_clause",
            "checks.contracts.nested_clause"
        ]
    );
    // A clause that calls a function whose requires is false: the nested
    // violation keeps its kind and payload, and S34 names the outer clause.
    let d = fault(&p, "checks.contracts.g", &[json!(0)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "checks.contracts.g");
    assert_eq!(d["clause"], "recip(x) >= 0");
    assert_eq!(d["caller"], "checks.contracts.g");
    assert_eq!(d["input"], json!({"n": 0}));
    // The span still describes the violation as raised: recip's clause.
    assert_eq!(
        d["span"],
        common::location_of(EXTRA_SOURCE, EXTRA, "requires n != 0")
    );
}

#[test]
fn unchecked_mode_runs_no_clause() {
    let p = fixture();
    let unchecked = EvalConfig {
        check_contracts: false,
        ..EvalConfig::default()
    };
    let run_unchecked = |f: &str, n: i64| {
        let outcome = common::try_run_with(&p, f, &common::json_args(&[json!(n)]), &unchecked)
            .expect("the run is not refused");
        common::outcome_json(&p, &outcome)
    };
    // Without contracts the body runs and divides by zero.
    assert_eq!(
        run_unchecked("corpus.contracts.recip", 0)["fault"]["code"],
        "division_by_zero"
    );
    assert_eq!(
        run_unchecked("corpus.contracts.half", 5),
        json!({"value": 2})
    );
    assert_eq!(
        run_unchecked("corpus.contracts.bad_loop", 5),
        json!({"value": 5})
    );
    assert_eq!(
        run_unchecked("corpus.contracts.stuck", 4),
        json!({"value": 4})
    );
    // The same calls in checked mode fault.
    assert_eq!(
        run(&p, "corpus.contracts.bad_loop", &[json!(5)])["fault"]["clause_kind"],
        "invariant"
    );
}
