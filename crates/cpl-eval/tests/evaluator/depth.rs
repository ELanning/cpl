//! Resource limits (semantics S38, S72; toolchain 1.7.1) on checked
//! programs: the explicit call-depth counter, the time budget, and the stack
//! guard behind them; and values nested deeper than a caller's stack could
//! recurse, which cross back from the evaluation thread, render, and drop
//! without recursion.

use crate::common;
use cpl_diag::json::TestStatus;
use cpl_eval::stack::on_eval_stack_sized;
use cpl_eval::test::TestConfig;
use cpl_eval::{run_function, value_text, value_to_json, EvalConfig, Interpreter, RunArg, Value};
use cpl_hir::Program;
use serde_json::{json, Value as Json};
use std::rc::Rc;
use std::time::{Duration, Instant};

const FILE: &str = "src/m.cpl";

const SOURCE: &str = r#"module m

pub type Positive = Int where self > 0

fn count_down(n: Int) -> Int
    requires n >= 0
    decreases n
{
    if n == 0 { 0 } else { count_down(n - 1) + 1 }
}

fn forever(n: Int) -> Int {
    forever(n + 1)
}

fn spin() -> Int {
    var i = 0
    while true {
        i = i + 1
    }
    i
}

pub fn deep(n: Int) -> Bool {
    if n == 0 { check<Positive>(1).is_ok() } else { deep(n - 1) }
}

// A chain of n + 1 nodes, grown in a loop.
pub struct Node {
    next: Option<Node>
}

fn chain(n: Int) -> Node {
    var acc = Node { next: None }
    var i = 0
    while i < n {
        acc = Node { next: Some(acc) }
        i = i + 1
    }
    acc
}

// Every level holds the level below twice: 2^n paths over n + 1 buffers.
pub struct Tree {
    kids: List<Tree>
}

fn doubling(n: Int) -> Tree {
    var acc = Tree { kids: [] }
    var i = 0
    while i < n {
        acc = Tree { kids: [acc, acc] }
        i = i + 1
    }
    acc
}
"#;

const TESTS: &str = r#"module tests.depth

use m.{deep}

test "deep" {
    assert deep(4)
}

test "deeper" {
    assert deep(5)
}
"#;

fn program() -> Program {
    common::check_files(&[(FILE, SOURCE), ("src/tests/depth.cpl", TESTS)])
}

fn depth(limit: u32) -> EvalConfig {
    EvalConfig {
        max_call_depth: limit,
        ..EvalConfig::default()
    }
}

/// `m.<f>(n)` under `config`: `{"value": ...}` or `{"fault": ...}`.
fn run_with(p: &Program, f: &str, n: i64, config: &EvalConfig) -> Json {
    let outcome = common::try_run_with(
        p,
        &format!("m.{f}"),
        &common::json_args(&[json!(n)]),
        config,
    )
    .expect("the run is not refused");
    common::outcome_json(p, &outcome)
}

fn backtrace_len(d: &Json) -> usize {
    d["backtrace"].as_array().map_or(0, Vec::len)
}

#[test]
fn the_default_limit_admits_exactly_ten_thousand_activations() {
    let p = program();
    let default = EvalConfig::default();
    // count_down(n) needs n + 1 activations.
    assert_eq!(
        run_with(&p, "count_down", 9999, &default),
        json!({"value": 9999})
    );
    let d = &run_with(&p, "count_down", 10000, &default)["fault"];
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["category"], "internal");
    assert_eq!(d["resource"], "stack");
    assert_eq!(d["limit"], json!(10000));
    // Blamed on the declaration that tried to make the 10 001st call, at
    // that call.
    assert_eq!(d["symbol"], "m.count_down");
    assert_eq!(
        d["span"],
        common::location_of(SOURCE, FILE, "count_down(n - 1)")
    );
    assert_eq!(backtrace_len(d), 10000);
}

#[test]
fn a_configured_limit_is_enforced_by_the_counter() {
    let p = program();
    assert_eq!(
        run_with(&p, "count_down", 4, &depth(5)),
        json!({"value": 4})
    );
    let d = &run_with(&p, "count_down", 5, &depth(5))["fault"];
    assert_eq!(d["resource"], "stack");
    assert_eq!(d["limit"], json!(5));
    assert_eq!(backtrace_len(d), 5);
    // A limit of 0 refuses the top-level call itself, which has no caller to blame.
    let d = &run_with(&p, "count_down", 0, &depth(0))["fault"];
    assert_eq!(d["resource"], "stack");
    assert_eq!(d["limit"], json!(0));
    assert!(d.get("symbol").is_none());
}

#[test]
fn runaway_recursion_is_a_fault_not_a_crash() {
    let p = program();
    let f = run_function(
        &p,
        &common::path("m.forever"),
        &[RunArg::Json(json!(0))],
        &EvalConfig::default(),
    )
    .expect("runs");
    let fault = f.value.unwrap_err();
    assert_eq!(fault.code(), "resource_exhaustion");
    assert_eq!(fault.data["limit"], json!(10000));
}

#[test]
fn the_stack_guard_backs_up_an_unbounded_limit() {
    let p = program();
    // With no effective depth limit the guard of the evaluation thread ends
    // the recursion before the host stack overflows (a 64 MiB stack keeps
    // the test's memory small; the default is 1 GiB).
    let forever = common::def(&p, "m.forever");
    let p = &p;
    let f = on_eval_stack_sized(64 << 20, move || {
        let mut it = Interpreter::new(p, depth(u32::MAX));
        it.call(forever, vec![Value::int(0)], Vec::new())
            .unwrap_err()
    });
    assert_eq!(f.code(), "resource_exhaustion");
    assert_eq!(f.data["resource"], "stack");
    assert!(f.data["depth"].as_u64().unwrap() > 1_000, "{:?}", f.data);
    assert_eq!(f.data["limit"], json!(u32::MAX));
}

#[test]
fn the_time_budget_ends_a_loop() {
    let p = program();
    let config = EvalConfig {
        time_budget: Some(Duration::from_millis(50)),
        ..EvalConfig::default()
    };
    let start = Instant::now();
    let outcome = common::try_run_with(&p, "m.spin", &[], &config).expect("runs");
    let d = common::fault(&p, &outcome);
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["resource"], "time");
    assert_eq!(d["limit"], json!(50));
    assert_eq!(d["symbol"], "m.spin");
}

#[test]
fn only_function_activations_count_toward_the_limit() {
    let p = program();
    // deep(4) is 5 function activations: within a limit of 5, with the test
    // body and the predicate of check<Positive> on top (S72).
    let mut config = TestConfig::with_seed(0);
    config.eval.max_call_depth = 5;
    let ok = common::def(&p, "tests.depth.\"deep\"");
    let over = common::def(&p, "tests.depth.\"deeper\"");
    assert_eq!(
        cpl_eval::run_test(&p, ok, &config).status,
        TestStatus::Tested
    );
    let record = cpl_eval::run_test(&p, over, &config);
    let fault = record.fault.expect("a fault");
    assert_eq!(fault.code, "resource_exhaustion");
    assert_eq!(fault.data["limit"], json!(5));
    // cpl run of the same call counts the same activations.
    assert_eq!(run_with(&p, "deep", 4, &depth(5)), json!({"value": true}));
    assert_eq!(
        run_with(&p, "deep", 5, &depth(5))["fault"]["code"],
        "resource_exhaustion"
    );
}

// ---- deep values on the caller's stack (S38) -----------------------------------

/// Run `f` on a thread with a 256 KiB stack: far less than any recursion
/// over the 200 000 value levels below would need.
fn on_small_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 10)
            .spawn_scoped(s, f)
            .expect("a thread")
            .join()
            .expect("the small-stack thread did not panic")
    })
}

/// Drop a JSON value without recursion (`serde_json`'s own drop recurses
/// with the nesting depth).
fn drop_json(j: Json) {
    let mut work = vec![j];
    while let Some(j) = work.pop() {
        match j {
            Json::Array(items) => work.extend(items),
            Json::Object(map) => work.extend(map.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

#[test]
fn a_deep_result_returns_renders_and_drops_on_the_callers_stack() {
    // A loop builds a 100 001-node chain (200 002 value levels) without
    // approaching the call-depth limit. run_function evaluates on its own
    // large stack, but the value it returns is the caller's: copied back,
    // inspected, rendered as value text and value JSON, and dropped on the
    // caller's thread, which here has a 256 KiB stack (S38: never a crash).
    let p = &common::check_source(SOURCE);
    let path = common::path("m.chain");
    on_small_stack(|| {
        let outcome = run_function(
            p,
            &path,
            &[RunArg::Json(json!(100_000))],
            &EvalConfig::default(),
        )
        .expect("runs");
        let v = outcome.value.as_ref().expect("a value");
        let mut levels = 0;
        let mut current = v;
        while let Value::Struct { fields, .. } = current {
            match &fields[0] {
                Value::Enum { payload, .. } if payload.len() == 1 => {
                    levels += 1;
                    current = &payload[0];
                }
                _ => break,
            }
        }
        assert_eq!(levels, 100_000);
        let text = value_text(p, v, &outcome.return_ty);
        let opening = "Node { next: Some(";
        let innermost = 100_000 * opening.len();
        assert!(text[..innermost] == opening.repeat(100_000));
        assert_eq!(&text[innermost..innermost + 19], "Node { next: None }");
        assert!(text[innermost + 19..] == ") }".repeat(100_000));
        let j = value_to_json(p, v, &outcome.return_ty);
        let mut levels = 0;
        let mut current = &j;
        while let Some(next) = current.get("next") {
            match next.get("$payload").and_then(|payload| payload.get(0)) {
                Some(inner) => {
                    levels += 1;
                    current = inner;
                }
                None => {
                    assert_eq!(next, &json!({"$type": "Option", "$variant": "None"}));
                    break;
                }
            }
        }
        assert_eq!(levels, 100_000);
        drop_json(j);
        drop(outcome);
    });
}

#[test]
fn a_shared_result_crosses_back_once_per_buffer() {
    // doubling(100000) has 2^100000 paths but 100 001 buffers: the copy
    // back from the evaluation thread keeps the sharing (a deep copy would
    // never finish), the two kids of every level are still one buffer, and
    // dropping the result does not recurse either.
    let p = &common::check_source(SOURCE);
    let path = common::path("m.doubling");
    on_small_stack(|| {
        let outcome = run_function(
            p,
            &path,
            &[RunArg::Json(json!(100_000))],
            &EvalConfig::default(),
        )
        .expect("runs");
        let mut levels = 0;
        let mut current = outcome.value.as_ref().expect("a value");
        loop {
            let Value::Struct { fields, .. } = current else {
                panic!("a Tree");
            };
            let Value::List(kids) = &fields[0] else {
                panic!("a list of kids");
            };
            if kids.is_empty() {
                break;
            }
            let (Value::Struct { fields: a, .. }, Value::Struct { fields: b, .. }) =
                (&kids[0], &kids[1])
            else {
                panic!("two trees");
            };
            assert!(Rc::ptr_eq(a, b), "level {levels} lost its sharing");
            levels += 1;
            current = &kids[0];
        }
        assert_eq!(levels, 100_000);
        drop(outcome);
    });
}
