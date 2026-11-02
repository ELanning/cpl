//! Dynamic semantics (semantics 6.1-6.5) on a checked program: strict
//! left-to-right evaluation, short-circuiting `and`/`or`/`forall`,
//! persistent values, `var` rebinding, struct spread, patterns, `if` as an
//! expression, `?`, structural equality.

use crate::common;
use cpl_hir::Program;
use serde_json::{json, Value as Json};

const SOURCE: &str = r#"module corpus.sem

pub type PageId(Text)
pub type Meters(Int)

pub struct Point {
    x: Int
    y: Int
}

pub struct Page {
    id: PageId
    text: Text
}

pub enum StoreError {
    Missing(PageId)
    Unavailable
}

fn and_div(c: Bool) -> Bool {
    c and 1 / 0 == 0
}

fn or_div(c: Bool) -> Bool {
    c or 1 / 0 == 0
}

fn boom() -> Int {
    assert false
    0
}

// The left operand faults first.
fn left_first() -> Int {
    (1 / 0) + boom()
}

fn pair(a: Int, b: Int) -> Int {
    a + b * 0
}

fn args_left_first() -> Int {
    pair(boom(), 1 / 0)
}

fn all_divide(xs: List<Int>) -> Bool {
    forall x in xs: 10 / x > 0
}

fn push_keeps_original(xs: List<Int>, v: Int) -> List<List<Int>> {
    let ys = xs.push(v)
    [xs, ys]
}

fn rebind(xs: List<Int>) -> List<List<Int>> {
    var ys = xs
    ys = ys.push(1)
    [xs, ys]
}

fn moved(p: Point, dx: Int) -> List<Point> {
    let q = Point { ..p, x: p.x + dx }
    [p, q]
}

fn describe(r: Result<Option<Int>, Text>) -> Text {
    match r {
        Ok(Some(n)) => n.to_text()
        Ok(None) => "none"
        Err(e) => e
    }
}

fn word(n: Int) -> Text {
    match n {
        0 => "zero"
        -1 => "minus one"
        _ => "other"
    }
}

fn unwrap(m: Meters) -> Int {
    match m {
        Meters(n) => n
    }
}

fn label(result: Result<Page, StoreError>) -> Text {
    match result {
        Ok(page) => page.text
        Err(Missing(_)) => "Missing page"
        Err(Unavailable) => "Storage unavailable"
    }
}

fn bump(c: Bool, n: Int) -> Int {
    n + if c { 1 } else { 0 }
}

fn require_positive(n: Int) -> Result<Int, Text> {
    if n > 0 { Ok(n) } else { Err("not positive") }
}

fn early(a: Int, b: Int) -> Result<Int, Text> {
    let x = require_positive(a)?
    Ok(x / b)
}

fn equalities() -> List<Bool> {
    let none: Option<Int> = None
    [
        [[1], [2]] == [[1], [2]],
        [[1], [2]] == [[2], [1]],
        Point { y: 2, x: 1 } == Point { y: 2, x: 1 },
        Point { y: 2, x: 1 } != Point { y: 2, x: 3 },
        PageId("a") == PageId("a"),
        Unit == Unit,
        "\u{e9}" == "e\u{301}",
        none == none,
        Some(1) == none,
    ]
}

fn noop() -> Unit {
    let _ = 1
}

fn concat_len(a: Text, b: Text) -> Int {
    (a ++ b).len
}
"#;

struct Fixture {
    p: Program,
}

fn fixture() -> Fixture {
    Fixture {
        p: common::check_source(SOURCE),
    }
}

fn value(p: &Program, f: &str, args: &[Json]) -> Json {
    common::value(p, &common::run_json(p, &format!("corpus.sem.{f}"), args))
}

/// The diagnostic object of the fault a call raises.
fn fault_diag(p: &Program, f: &str, args: &[Json]) -> Json {
    common::fault(p, &common::run_json(p, &format!("corpus.sem.{f}"), args))
}

/// The code of the fault a call raises.
fn code(p: &Program, f: &str, args: &[Json]) -> Json {
    fault_diag(p, f, args)["code"].clone()
}

#[test]
fn and_or_evaluate_the_right_operand_only_when_needed() {
    let fx = fixture();
    assert_eq!(value(&fx.p, "and_div", &[json!(false)]), json!(false));
    assert_eq!(code(&fx.p, "and_div", &[json!(true)]), "division_by_zero");
    assert_eq!(value(&fx.p, "or_div", &[json!(true)]), json!(true));
    assert_eq!(code(&fx.p, "or_div", &[json!(false)]), "division_by_zero");
}

#[test]
fn operands_and_arguments_evaluate_left_to_right() {
    let fx = fixture();
    assert_eq!(code(&fx.p, "left_first", &[]), "division_by_zero");
    assert_eq!(code(&fx.p, "args_left_first", &[]), "assertion_failed");
}

#[test]
fn forall_stops_at_the_first_false() {
    let fx = fixture();
    assert_eq!(value(&fx.p, "all_divide", &[json!([])]), json!(true));
    assert_eq!(value(&fx.p, "all_divide", &[json!([1, 2])]), json!(true));
    assert_eq!(value(&fx.p, "all_divide", &[json!([-1, 0])]), json!(false));
    let d = fault_diag(&fx.p, "all_divide", &[json!([1, 0])]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "corpus.sem.all_divide");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
}

#[test]
fn values_are_persistent_and_var_rebinds_a_name() {
    let fx = fixture();
    assert_eq!(
        value(&fx.p, "push_keeps_original", &[json!([1]), json!(2)]),
        json!([[1], [1, 2]])
    );
    assert_eq!(value(&fx.p, "rebind", &[json!([])]), json!([[], [1]]));
    let p1 = json!({"$type": "corpus.sem.Point", "x": 1, "y": 2});
    assert_eq!(
        value(&fx.p, "moved", &[p1.clone(), json!(5)]),
        json!([p1, {"$type": "corpus.sem.Point", "x": 6, "y": 2}])
    );
}

#[test]
fn match_tries_arms_in_order_with_nested_patterns() {
    let fx = fixture();
    let ok_some = json!({"$type": "Result", "$variant": "Ok", "$payload": [
        {"$type": "Option", "$variant": "Some", "$payload": [3]}]});
    assert_eq!(value(&fx.p, "describe", &[ok_some]), json!("3"));
    let ok_none = json!({"$type": "Result", "$variant": "Ok", "$payload": [
        {"$type": "Option", "$variant": "None"}]});
    assert_eq!(value(&fx.p, "describe", &[ok_none]), json!("none"));
    let err = json!({"$type": "Result", "$variant": "Err", "$payload": ["boom"]});
    assert_eq!(value(&fx.p, "describe", &[err]), json!("boom"));
    assert_eq!(value(&fx.p, "word", &[json!(-1)]), json!("minus one"));
    assert_eq!(value(&fx.p, "word", &[json!(0)]), json!("zero"));
    assert_eq!(value(&fx.p, "word", &[json!(7)]), json!("other"));
    assert_eq!(
        value(
            &fx.p,
            "unwrap",
            &[json!({"$type": "corpus.sem.Meters", "$payload": [7]})]
        ),
        json!(7)
    );
    let missing = json!({"$type": "Result", "$variant": "Err", "$payload": [
        {"$type": "corpus.sem.StoreError", "$variant": "Missing", "$payload": [
            {"$type": "corpus.sem.PageId", "$payload": ["p"]}]}]});
    assert_eq!(value(&fx.p, "label", &[missing]), json!("Missing page"));
    let unavailable = json!({"$type": "Result", "$variant": "Err", "$payload": [
        {"$type": "corpus.sem.StoreError", "$variant": "Unavailable"}]});
    assert_eq!(
        value(&fx.p, "label", &[unavailable]),
        json!("Storage unavailable")
    );
}

#[test]
fn if_is_an_expression_and_question_mark_propagates() {
    let fx = fixture();
    assert_eq!(value(&fx.p, "bump", &[json!(true), json!(1)]), json!(2));
    assert_eq!(value(&fx.p, "bump", &[json!(false), json!(1)]), json!(1));
    // `?` on Err exits with that Err; the division is never reached.
    assert_eq!(
        value(&fx.p, "early", &[json!(-1), json!(0)]),
        json!({"$type": "Result", "$variant": "Err", "$payload": ["not positive"]})
    );
    // `?` on Ok continues with the payload.
    assert_eq!(
        code(&fx.p, "early", &[json!(1), json!(0)]),
        "division_by_zero"
    );
    assert_eq!(
        value(&fx.p, "early", &[json!(6), json!(3)]),
        json!({"$type": "Result", "$variant": "Ok", "$payload": [2]})
    );
}

#[test]
fn equality_is_structural() {
    let fx = fixture();
    assert_eq!(
        value(&fx.p, "equalities", &[]),
        json!([true, false, true, true, true, true, false, true, false])
    );
}

#[test]
fn blocks_without_a_tail_are_unit_and_text_counts_scalars() {
    let fx = fixture();
    assert_eq!(value(&fx.p, "noop", &[]), json!({"$type": "Unit"}));
    assert_eq!(
        value(&fx.p, "concat_len", &[json!("\u{e9}"), json!("\u{1f600}")]),
        json!(2)
    );
}
