//! Adversarial tests of the language semantics end to end: CPL source is
//! checked by the real checker (`cpl-sema`, a dev-dependency) and executed
//! by the interpreter through the public API (`run_function`, `run_tests`,
//! `Interpreter`), using the helpers of `tests/common`.
//!
//! | Section | Rules |
//! | - | - |
//! | A. `Int` arithmetic | semantics 6.3, S37; stdlib 2.1-2.3, S8; toolchain T9 |
//! | B. `Text` and `Bytes` | semantics 6.4, 6.5; stdlib 3.3, 3.4, 6, S14, S22-S26 |
//! | C. lists and the laws of stdlib 4.3 | semantics 6.1, 6.10; stdlib 4.2-4.5, 10.7, S16, S17 |
//! | D. structs and spread | semantics 6.1, 6.2, 6.5; grammar D29 |
//! | E. enums and nested matches | semantics 3.12, 3.13, 6.2; grammar 3.13 |
//! | F. `Option`, `Result`, and `?` | semantics 3.8, 5.2 step 5, 6.2, 6.6; stdlib 5 |
//! | G. wrappers and refinements | semantics 3.3-3.5, 6.1, 6.10, S16, S23, S39, S55; toolchain T39, 7.3 |
//! | H. generics at several types | semantics 3.7, S12, S13; toolchain 7.3 |
//! | I. `while`, `invariant`, `decreases` | semantics 5.3; toolchain 1.7.1 |
//! | J. header `decreases` | semantics 5.4, S35 |
//! | K. `forall`, early `return`, shadowing, statements | semantics 2.3, 3.10, 3.11, 5.2, 6.2 |
//! | L. contract rows of toolchain 1.7.1 with exact payloads | toolchain 1.7.1, 1.7.3, T56; semantics 5.2, 5.6, 6.7, 6.10, 9.2, S34, S57, S58, S70 |
//! | M. capabilities through `Interpreter::bind_capability` | semantics 4.6, 5.3, 6.9, S31, S57; toolchain 1.7 |
//! | N. holes | semantics 6.7, 8.1, 8.3, S48 |
//! | O. the call-depth limit | semantics 6.7, S38, S72; toolchain 1.7 |
//! | P. `run_function` refusals and the standard library at the top level | semantics S39; toolchain 1.7, 3.6, 7.2, T61 |
//! | Q. several modules: imports, qualified calls, identical refinements | semantics 1.4, 2.4, S2, S44, S55 |
//! | R. the diagnostic envelope of every fault kind | semantics 6.7, 9.2, 9.5, S49, S50; toolchain 1.7.3 |
//!
//! Every expectation is the specification's answer, never the current
//! behaviour of the implementation; where the specification is silent the
//! test follows `design-doc.md` and says so in a comment.

mod common;

use cpl_diag::json::TestStatus;
use cpl_eval::{
    run_tests, CapabilityImpl, EvalConfig, Fault, Handle, HostContext, Interpreter, RunError,
    TestConfig, TestSelector, Value,
};
use cpl_hir::{DefId, Program};
use serde_json::{json, Value as Json};
use std::cell::RefCell;
use std::rc::Rc;

// ===========================================================================
// Helpers
// ===========================================================================

/// The value JSON of `function(args)` run with `--arg` JSON arguments.
fn val(p: &Program, function: &str, args: &[Json]) -> Json {
    common::value(p, &common::run_json(p, function, args))
}

/// The fault diagnostic of `function(args)`.
fn flt(p: &Program, function: &str, args: &[Json]) -> Json {
    common::fault(p, &common::run_json(p, function, args))
}

/// [`val`] with a configuration.
fn val_with(p: &Program, function: &str, args: &[Json], config: &EvalConfig) -> Json {
    let outcome = common::try_run_with(p, function, &common::json_args(args), config)
        .unwrap_or_else(|e| panic!("{function}: refused: {e}"));
    common::value(p, &outcome)
}

/// The fault diagnostic of calling `function` through `Interpreter::call`
/// directly, as an embedder would, with no calling declaration: for the
/// generator stubs, which `run_function` refuses (toolchain T61).
fn called_fault(p: &Program, function: &str, args: Vec<Value>) -> Json {
    let f = common::def(p, function);
    let mut it = Interpreter::new(p, EvalConfig::default());
    match it.call(f, args, Vec::new()) {
        Err(fault) => common::fault_json(p, &fault),
        Ok(v) => panic!("{function} returned {v:?}"),
    }
}

/// [`flt`] with a configuration.
fn flt_with(p: &Program, function: &str, args: &[Json], config: &EvalConfig) -> Json {
    let outcome = common::try_run_with(p, function, &common::json_args(args), config)
        .unwrap_or_else(|e| panic!("{function}: refused: {e}"));
    common::fault(p, &outcome)
}

/// The qualified names of a diagnostic's backtrace, innermost first.
fn bt(d: &Json) -> Vec<String> {
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

/// Unchecked mode (toolchain 1.7: `check_contracts: false`).
fn unchecked() -> EvalConfig {
    EvalConfig {
        check_contracts: false,
        ..EvalConfig::default()
    }
}

/// A configuration with a call-depth limit.
fn depth(limit: u32) -> EvalConfig {
    EvalConfig {
        max_call_depth: limit,
        ..EvalConfig::default()
    }
}

fn some(v: Json) -> Json {
    json!({"$type": "Option", "$variant": "Some", "$payload": [v]})
}

fn none() -> Json {
    json!({"$type": "Option", "$variant": "None"})
}

fn ok(v: Json) -> Json {
    json!({"$type": "Result", "$variant": "Ok", "$payload": [v]})
}

fn err(v: Json) -> Json {
    json!({"$type": "Result", "$variant": "Err", "$payload": [v]})
}

fn unit() -> Json {
    json!({"$type": "Unit"})
}

/// `Err(ConstraintError { type_name, message })` (stdlib 1.5, T30).
fn constraint(type_name: &str, message: &str) -> Json {
    err(json!({"$type": "ConstraintError", "type_name": type_name, "message": message}))
}

/// An exact decimal `Int` as JSON (serde_json `arbitrary_precision`).
fn num(digits: &str) -> Json {
    serde_json::from_str(digits).expect("a JSON number")
}

/// The records of every test and property of `p`, by run seed 1 and the
/// default runner parameters.
fn records(p: &Program) -> Vec<Json> {
    let report = run_tests(p, &TestSelector::all(), &TestConfig::with_seed(1));
    common::report_json(&report)["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The record of the test or property whose name text is `name`.
fn record(p: &Program, name: &str) -> Json {
    record_with(p, name, &TestConfig::with_seed(1))
}

fn record_with(p: &Program, name: &str, config: &TestConfig) -> Json {
    let selector = TestSelector {
        namespace: None,
        name: Some(name.to_string()),
        id: None,
    };
    let report = run_tests(p, &selector, config);
    assert_eq!(report.selected, 1, "the test {name:?} is selected once");
    common::report_json(&report)["results"][0].clone()
}

/// The fault of a test record that must have failed.
fn record_fault(rec: &Json) -> Json {
    assert_eq!(rec["status"], "failed", "{rec}");
    rec["fault"].clone()
}

/// Each named `test` and `property` of `p` must be `tested`.
fn assert_all_tested(p: &Program) {
    let report = run_tests(p, &TestSelector::all(), &TestConfig::with_seed(7));
    for r in &report.results {
        assert_eq!(
            r.status,
            TestStatus::Tested,
            "{} failed: {}",
            r.qualified_name,
            serde_json::to_string(&r.fault).unwrap_or_default()
        );
    }
    assert!(report.selected > 0);
    assert_eq!(report.tested, report.selected);
}

/// The single-line location of `needle` in the conventional file of a
/// one-module source (`common::location_of`).
fn at(source: &str, needle: &str) -> Json {
    common::location_of(source, &common::conventional_path(source), needle)
}

/// The location of `needle` at or after the line holding `anchor`.
fn at_after(source: &str, anchor: &str, needle: &str) -> Json {
    common::location_after(source, &common::conventional_path(source), anchor, needle)
}

/// A span as the JSON location of semantics S49.
fn span_json(p: &Program, span: cpl_diag::Span) -> Json {
    let mut d = cpl_diag::Diagnostic::error(
        cpl_diag::DiagCode("span_probe"),
        cpl_diag::Category::Internal,
        "span",
    );
    d.span = Some(span);
    serde_json::to_value(d.to_json(p.source_map())).expect("a diagnostic serializes")["span"]
        .clone()
}

/// The `hole` diagnostics of a check, as JSON, in report order.
fn hole_reports(result: &cpl_sema::CheckResult) -> Vec<Json> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.code.0 == "hole")
        .map(|d| serde_json::to_value(d.to_json(result.program.source_map())).unwrap())
        .collect()
}

/// A fault's `bindings` without their values: the shape the checker's
/// hole report (semantics 8.3) gives the same position.
fn without_values(bindings: &Json) -> Json {
    Json::Array(
        bindings
            .as_array()
            .expect("bindings are an array")
            .iter()
            .map(|b| {
                let mut b = b.as_object().expect("a binding object").clone();
                b.remove("value");
                Json::Object(b)
            })
            .collect(),
    )
}

// ===========================================================================
// A. Int arithmetic (semantics 6.3, S37; stdlib 2.1-2.3, S8)
// ===========================================================================

const INTS: &str = r#"module pl.ints

fn div(a: Int, b: Int) -> Int {
    a / b
}

fn rem(a: Int, b: Int) -> Int {
    a % b
}

// The identity of S37 and the sign rule of stdlib 2.2, evaluated in CPL.
fn laws(a: Int, b: Int) -> Bool
    requires b != 0
{
    (a / b) * b + a % b == a and (a % b).abs() < b.abs() and (a % b == 0 or (a % b > 0) == (a > 0))
}

fn constants() -> List<Int> {
    [
        340282366920938463463374607431768211456 * 2,
        -9223372036854775808 - 1,
        - -5,
        (-5) * (-5),
        1_000_000_000_000_000_000_000 + 1,
        -9223372036854775808 / -1,
        -9223372036854775808 % -1,
        18446744073709551615 + 1,
        0 - 18446744073709551616 - 1,
        -7 / 2 * 2,
        -(7 / 2),
    ]
}

fn members(x: Int, y: Int) -> List<Int> {
    [x.abs(), x.min(y), x.max(y), (-x).abs(), y.min(x), y.max(x)]
}

fn rendered(x: Int) -> Text {
    x.to_text()
}

fn compare(a: Int, b: Int) -> List<Bool> {
    [a < b, a <= b, a > b, a >= b, a == b, a != b]
}

fn by_zero(a: Int) -> Int {
    a / 0
}

fn rem_zero(a: Int, b: Int) -> Int {
    a % (b - b)
}

fn nested(a: Int) -> Int {
    a + by_zero(a)
}

test "dividing by zero in a test body" {
    let z = 0
    assert 10 % z == 0
}
"#;

#[test]
fn int_division_truncates_toward_zero_for_every_sign_combination_s37() {
    let p = common::check_source(INTS);
    // (a, b, a / b, a % b): stdlib 2.2's worked values and the cases where
    // floor or Euclidean division would differ (|a| < |b| with mixed signs).
    let rows: [(i64, i64, i64, i64); 16] = [
        (7, 2, 3, 1),
        (-7, 2, -3, -1),
        (7, -2, -3, 1),
        (-7, -2, 3, -1),
        (1, 2, 0, 1),
        (-1, 2, 0, -1),
        (1, -2, 0, 1),
        (-1, -2, 0, -1),
        (0, 5, 0, 0),
        (0, -5, 0, 0),
        (6, 3, 2, 0),
        (-6, 3, -2, 0),
        (6, -3, -2, 0),
        (-6, -3, 2, 0),
        (2, 7, 0, 2),
        (-2, 7, 0, -2),
    ];
    for (a, b, q, r) in rows {
        assert_eq!(
            val(&p, "pl.ints.div", &[json!(a), json!(b)]),
            json!(q),
            "{a} / {b}"
        );
        assert_eq!(
            val(&p, "pl.ints.rem", &[json!(a), json!(b)]),
            json!(r),
            "{a} % {b}"
        );
        assert_eq!(
            val(&p, "pl.ints.laws", &[json!(a), json!(b)]),
            json!(true),
            "laws at {a}, {b}"
        );
    }
}

#[test]
fn int_division_is_exact_at_magnitudes_beyond_128_bits_s37() {
    let p = common::check_source(INTS);
    // -(10^40) - 4 = 7 * -1428571428571428571428571428571428571429 - 1.
    let a = "-10000000000000000000000000000000000000004";
    let q = "1428571428571428571428571428571428571429";
    let rows = [
        (a.to_string(), "7", format!("-{q}"), "-1"),
        (a.to_string(), "-7", q.to_string(), "-1"),
        (a[1..].to_string(), "7", q.to_string(), "1"),
        (a[1..].to_string(), "-7", format!("-{q}"), "1"),
        // (2^128 + 1) and (2^64 + 1).
        (
            "340282366920938463463374607431768211457".to_string(),
            "18446744073709551617",
            "18446744073709551615".to_string(),
            "2",
        ),
        (
            "-340282366920938463463374607431768211457".to_string(),
            "18446744073709551617",
            "-18446744073709551615".to_string(),
            "-2",
        ),
        (
            "340282366920938463463374607431768211457".to_string(),
            "-18446744073709551617",
            "-18446744073709551615".to_string(),
            "2",
        ),
        (
            "-340282366920938463463374607431768211457".to_string(),
            "-18446744073709551617",
            "18446744073709551615".to_string(),
            "-2",
        ),
    ];
    for (a, b, q, r) in rows {
        let args = [num(&a), num(b)];
        assert_eq!(val(&p, "pl.ints.div", &args), num(&q), "{a} / {b}");
        assert_eq!(val(&p, "pl.ints.rem", &args), num(r), "{a} % {b}");
        assert_eq!(
            val(&p, "pl.ints.laws", &args),
            json!(true),
            "laws at {a}, {b}"
        );
    }
}

#[test]
fn int_constant_expressions_in_source_are_exact_at_machine_boundaries() {
    let p = common::check_source(INTS);
    assert_eq!(
        val(&p, "pl.ints.constants", &[]),
        json!([
            num("680564733841876926926749214863536422912"),
            num("-9223372036854775809"),
            5,
            25,
            num("1000000000000000000001"),
            // i64::MIN / -1 and i64::MIN % -1 overflow (or trap) in machine
            // arithmetic; Int has no overflow (semantics 6.1).
            num("9223372036854775808"),
            0,
            num("18446744073709551616"),
            num("-18446744073709551617"),
            // Prefix minus binds tighter than `/` (grammar 3.12): (-7) / 2 * 2.
            -6,
            -3
        ])
    );
}

#[test]
fn int_members_and_comparisons_at_huge_magnitudes() {
    let p = common::check_source(INTS);
    let x = "-123456789012345678901234567890";
    let y = "-123456789012345678901234567891";
    assert_eq!(
        val(&p, "pl.ints.members", &[num(x), num(y)]),
        json!([num(&x[1..]), num(y), num(x), num(&x[1..]), num(y), num(x)])
    );
    // `a.min(b)` is `a` when equal (stdlib 2.3); equal values are equal
    // whichever is returned, so only the order of unequal ones is observable.
    assert_eq!(val(&p, "pl.ints.rendered", &[num(x)]), json!(x));
    assert_eq!(val(&p, "pl.ints.rendered", &[json!(0)]), json!("0"));
    // Operands that differ only in the last digit of 30.
    assert_eq!(
        val(&p, "pl.ints.compare", &[num(y), num(x)]),
        json!([true, true, false, false, false, true])
    );
    assert_eq!(
        val(&p, "pl.ints.compare", &[num(x), num(x)]),
        json!([false, true, false, true, true, false])
    );
}

#[test]
fn int_division_by_zero_blames_the_enclosing_function_with_every_digit_of_its_operands() {
    let p = common::check_source(INTS);
    let huge = "-340282366920938463463374607431768211457";
    let d = flt(&p, "pl.ints.by_zero", &[num(huge)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["category"], "runtime");
    assert_eq!(d["severity"], "error");
    assert_eq!(d["symbol"], "pl.ints.by_zero");
    assert_eq!(d["qualified_name"], "pl.ints.by_zero");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["input"], json!({"a": num(huge), "b": 0}));
    assert_eq!(d["span"], at_after(INTS, "fn by_zero(", "a / 0"));
    assert!(
        d.get("clause").is_none(),
        "no clause outside a contract: {d}"
    );
    assert!(d.get("clause_kind").is_none(), "{d}");
    assert_eq!(bt(&d), ["pl.ints.by_zero"]);
    // `%` by a divisor that only evaluates to zero.
    let d = flt(&p, "pl.ints.rem_zero", &[json!(0), json!(5)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["input"], json!({"a": 0, "b": 0}));
    assert_eq!(d["span"], at(INTS, "a % (b - b)"));
    // Raised in a callee: the callee is blamed, the caller is a frame.
    let d = flt(&p, "pl.ints.nested", &[json!(3)]);
    assert_eq!(d["symbol"], "pl.ints.by_zero");
    assert_eq!(bt(&d), ["pl.ints.by_zero", "pl.ints.nested"]);
    assert_eq!(d["backtrace"][0]["span"], at(INTS, "by_zero(a)"));
}

#[test]
fn int_division_by_zero_in_a_test_body_blames_the_test() {
    let p = common::check_source(INTS);
    let rec = record(&p, "dividing by zero in a test body");
    let d = record_fault(&rec);
    // The assert's operand faults first: no assertion_failed (6.2, 7.1).
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], r#"pl.ints."dividing by zero in a test body""#);
    assert_eq!(
        d["qualified_name"],
        r#"pl.ints."dividing by zero in a test body""#
    );
    assert_eq!(d["facet"], "body");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
    assert_eq!(d["span"], at(INTS, "10 % z"));
    assert_eq!(bt(&d), [r#"pl.ints."dividing by zero in a test body""#]);
    assert!(
        d["backtrace"][0].get("span").is_none(),
        "the runner enters a test: {d}"
    );
    // A plain test's failure is no counterexample (S46).
    assert!(rec.get("counterexamples").is_none(), "{rec}");
}

// ===========================================================================
// B. Text and Bytes (semantics 6.4, 6.5; stdlib 3.3, 3.4, 6)
// ===========================================================================

const TEXT: &str = r#"module pl.text

use std.text.{trim, split, join, chars}

fn concat(a: Text, b: Text) -> List<Int> {
    let c = a ++ b
    [a.len, b.len, c.len]
}

fn empties(t: Text) -> List<Bool> {
    [t.is_empty(), (t ++ "").is_empty(), ("" ++ t) == t, t == ""]
}

fn classify(t: Text) -> Int {
    match t {
        "é" => 1
        "e\u{301}" => 2
        "a\nb" => 3
        "" => 4
        _ => 0
    }
}

fn round_trip(t: Text, sep: Text) -> Bool
    requires not sep.is_empty()
{
    join(split(t, sep), sep) == t and join(chars(t), "") == t and chars(t).len == t.len
}

fn split_empty(t: Text) -> List<Text> {
    split(t, "")
}

fn trimmed(t: Text) -> Text {
    trim(t)
}

fn utf8(t: Text) -> List<Int> {
    std.bytes.to_list(std.bytes.from_text(t))
}

fn bytes_ops(a: Bytes, b: Bytes) -> List<Int> {
    let c = a ++ b
    [a.len, b.len, c.len]
}

fn bytes_eq(a: Bytes, b: Bytes) -> List<Bool> {
    [a == b, a != b, a.is_empty(), (a ++ b) == (b ++ a)]
}

fn pack(values: List<Int>) -> Bytes {
    std.bytes.from_list(values)
}

fn concat_bytes(a: List<Int>, b: List<Int>) -> Bytes {
    std.bytes.from_list(a) ++ std.bytes.from_list(b)
}

fn decode(values: List<Int>) -> Result<Text, ConstraintError> {
    std.bytes.to_text(std.bytes.from_list(values))
}

fn unicode_ops(t: Text) -> List<Text> {
    [
        join(["\u{e9}", "\u{1f600}"], "\u{b7}"),
        std.text.to_upper(t),
        std.text.to_lower("\u{130}A"),
        std.text.from_int(-1000000000000000000000000000000),
        std.text.trim_start("\u{2003}x"),
        std.text.trim_end("x\u{2003}\u{200b}"),
    ]
}

fn unicode_preds() -> List<Bool> {
    [
        std.text.starts_with("\u{e9}", "e"),
        std.text.contains("e\u{301}", "\u{301}"),
        std.text.ends_with("a\u{1f600}", "\u{1f600}"),
        std.text.starts_with("", ""),
        std.text.contains("", "a"),
    ]
}

fn unicode_split() -> List<List<Text>> {
    [split("a\u{1f600}b\u{1f600}", "\u{1f600}"), chars("e\u{301}\u{1f600}"), split("\u{e9}", "e")]
}

fn parses(t: Text) -> Result<Int, ConstraintError> {
    std.text.parse_int(t)
}

fn wide_line(n: Int) -> Int {
    "\u{1f600}é中".len + 10 / n
}

fn decode_type(values: List<Int>) -> Text {
    match decode(values) {
        Ok(_) => "ok"
        Err(e) => e.type_name
    }
}
"#;

fn bytes_json(hex: &str) -> Json {
    json!({"$type": "Bytes", "hex": hex})
}

#[test]
fn text_concatenation_and_len_count_unicode_scalar_values() {
    let p = common::check_source(TEXT);
    let f = "pl.text.concat";
    // U+00E9 is 2 UTF-8 bytes, U+1F600 4 bytes and 2 UTF-16 units.
    assert_eq!(
        val(&p, f, &[json!("\u{e9}"), json!("\u{1f600}")]),
        json!([1, 1, 2])
    );
    // A combining mark is its own scalar value: no grapheme clustering.
    assert_eq!(
        val(&p, f, &[json!("e\u{301}"), json!("")]),
        json!([2, 0, 2])
    );
    assert_eq!(val(&p, f, &[json!(""), json!("")]), json!([0, 0, 0]));
    assert_eq!(
        val(&p, "pl.text.empties", &[json!("")]),
        json!([true, true, true, true])
    );
    // U+200B ZERO WIDTH SPACE is a scalar value like any other.
    assert_eq!(
        val(&p, "pl.text.empties", &[json!("\u{200b}")]),
        json!([false, false, true, false])
    );
}

#[test]
fn text_patterns_compare_scalar_sequences_without_normalization() {
    let p = common::check_source(TEXT);
    let f = "pl.text.classify";
    assert_eq!(val(&p, f, &[json!("\u{e9}")]), json!(1));
    // NFC would make "e\u{301}" equal to "\u{e9}"; CPL never normalizes.
    assert_eq!(val(&p, f, &[json!("e\u{301}")]), json!(2));
    assert_eq!(val(&p, f, &[json!("a\nb")]), json!(3));
    assert_eq!(val(&p, f, &[json!("")]), json!(4));
    assert_eq!(val(&p, f, &[json!("\u{c9}")]), json!(0));
    assert_eq!(val(&p, f, &[json!("a\r\nb")]), json!(0));
}

#[test]
fn text_split_join_and_chars_round_trip_on_unicode_text() {
    let p = common::check_source(TEXT);
    for (t, sep) in [
        ("a,b,,c", ","),
        ("\u{e9}\u{1f600}e\u{301}\u{1f600}", "\u{1f600}"),
        ("", ","),
        ("aaa", "aa"),
        (",", ","),
        ("e\u{301}e", "\u{301}"),
    ] {
        assert_eq!(
            val(&p, "pl.text.round_trip", &[json!(t), json!(sep)]),
            json!(true),
            "{t:?} by {sep:?}"
        );
    }
    // The trim set is stdlib S22's: U+00A0 and U+3000 are White_Space,
    // U+200B and U+FEFF are not.
    assert_eq!(
        val(&p, "pl.text.trimmed", &[json!("\u{a0}\u{3000} x \u{2029}")]),
        json!("x")
    );
    assert_eq!(
        val(&p, "pl.text.trimmed", &[json!("\u{feff}x\u{200b}")]),
        json!("\u{feff}x\u{200b}")
    );
}

#[test]
fn text_split_with_an_empty_separator_is_its_callers_precondition_fault() {
    let p = common::check_source(TEXT);
    let d = flt(&p, "pl.text.split_empty", &[json!("a\u{e9}")]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "std.text.split");
    assert_eq!(d["qualified_name"], "std.text.split");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["caller"], "pl.text.split_empty");
    assert_eq!(d["clause"], "not sep.is_empty()");
    // A free function's parameters keep their names: no `self` (6.10).
    assert_eq!(d["input"], json!({"text": "a\u{e9}", "sep": ""}));
    assert_eq!(d["status"], "failed");
    assert_eq!(d["span"]["file"], "std/text.cpl");
    assert_eq!(bt(&d), ["std.text.split", "pl.text.split_empty"]);
    assert_eq!(d["backtrace"][0]["span"], at(TEXT, r#"split(t, "")"#));
}

#[test]
fn bytes_encode_text_as_utf8_and_concatenate_and_compare_bytewise() {
    let p = common::check_source(TEXT);
    assert_eq!(
        val(&p, "pl.text.utf8", &[json!("a\u{e9}\u{4e2d}\u{1f600}")]),
        json!([97, 195, 169, 228, 184, 173, 240, 159, 152, 128])
    );
    assert_eq!(
        val(
            &p,
            "pl.text.bytes_ops",
            &[bytes_json("00ff"), bytes_json("")]
        ),
        json!([2, 0, 2])
    );
    assert_eq!(
        val(
            &p,
            "pl.text.bytes_eq",
            &[bytes_json("00ff"), bytes_json("00ff")]
        ),
        json!([true, false, false, true])
    );
    assert_eq!(
        val(
            &p,
            "pl.text.bytes_eq",
            &[bytes_json("01"), bytes_json("0100")]
        ),
        json!([false, true, false, false])
    );
    assert_eq!(
        val(&p, "pl.text.concat_bytes", &[json!([1, 2]), json!([255])]),
        bytes_json("0102ff")
    );
    assert_eq!(val(&p, "pl.text.pack", &[json!([])]), bytes_json(""));
    assert_eq!(
        val(&p, "pl.text.decode", &[json!([104, 195, 169])]),
        ok(json!("h\u{e9}"))
    );
    // Ill-formed UTF-8 is an ordinary Err naming Text (stdlib 3.4, S44).
    for bad in [
        json!([255]),
        json!([97, 237, 160, 128]),
        json!([224, 128, 128]),
        json!([195]),
    ] {
        assert_eq!(
            val(&p, "pl.text.decode_type", std::slice::from_ref(&bad)),
            json!("Text"),
            "{bad}"
        );
    }
}

#[test]
fn bytes_from_list_outside_the_byte_range_is_its_callers_precondition_fault() {
    let p = common::check_source(TEXT);
    for values in [
        json!([0, 255, 256]),
        json!([-1]),
        json!([1, num("-340282366920938463463374607431768211456")]),
    ] {
        let d = flt(&p, "pl.text.pack", std::slice::from_ref(&values));
        assert_eq!(d["code"], "contract_violation", "{values}");
        assert_eq!(d["clause_kind"], "requires");
        assert_eq!(d["symbol"], "std.bytes.from_list");
        assert_eq!(d["clause"], "forall v in values: v >= 0 and v <= 255");
        assert_eq!(d["attributed_to"], "caller");
        assert_eq!(d["caller"], "pl.text.pack");
        assert_eq!(d["input"], json!({ "values": values }));
        assert_eq!(d["span"]["file"], "std/bytes.cpl");
        assert_eq!(bt(&d), ["std.bytes.from_list", "pl.text.pack"]);
    }
}

#[test]
fn std_text_operations_work_on_scalar_values_from_source_6() {
    let p = common::check_source(TEXT);
    assert_eq!(
        val(&p, "pl.text.unicode_ops", &[json!("stra\u{df}e")]),
        json!([
            "\u{e9}\u{b7}\u{1f600}",
            // ASCII-only case mapping (stdlib S23): ß stays.
            "STRA\u{df}E",
            "\u{130}a",
            "-1000000000000000000000000000000",
            "x",
            // U+200B is not White_Space (S22), so trimming stops there.
            "x\u{2003}\u{200b}"
        ])
    );
    assert_eq!(
        val(&p, "pl.text.unicode_preds", &[]),
        json!([false, true, true, true, false])
    );
    assert_eq!(
        val(&p, "pl.text.unicode_split", &[]),
        json!([["a", "b", ""], ["e", "\u{301}", "\u{1f600}"], ["\u{e9}"]])
    );
    assert_eq!(val(&p, "pl.text.parses", &[json!("-0007")]), ok(json!(-7)));
    assert_eq!(
        val(
            &p,
            "pl.text.parses",
            &[json!("123456789012345678901234567890")]
        ),
        ok(num("123456789012345678901234567890"))
    );
    for bad in ["+", "", " 1", "1 ", "\u{661}", "0x1"] {
        let r = val(&p, "pl.text.parses", &[json!(bad)]);
        assert_eq!(r["$variant"], "Err", "{bad:?}: {r}");
        assert_eq!(r["$payload"][0]["type_name"], "Int", "{bad:?}");
    }
}

// ===========================================================================
// C. Lists and the laws of stdlib 4.3 (semantics 6.1, 6.10; stdlib 4, 10.7)
// ===========================================================================

const LISTS: &str = r#"module pl.lists

pub type Positive = Int where self > 0

fn ops(xs: List<Int>, n: Int) -> List<List<Int>>
    requires n >= 0
{
    [xs.take(n), xs.drop(n), xs.drop_last(), xs.push(n), xs ++ xs.reverse()]
}

fn accessors(xs: List<Int>, i: Int) -> List<Option<Int>> {
    [xs.get(i), xs.first(), xs.last()]
}

fn has(xss: List<List<Int>>, v: List<Int>) -> List<Bool> {
    [xss.contains(v), xss.is_empty(), flatten(xss).contains(0)]
}

fn chained() -> List<Int> {
    [1, 2, 3].drop(1).take(-1)
}

fn mixed(size: Positive) -> List<Bool> {
    let ps: List<Positive> = [size, 3]
    let is: List<Int> = [1, 3]
    [ps == [1, 3], ps == is, [size] == [1], ps.len == is.len]
}

fn all_big(xs: List<Int>) -> Bool {
    forall x in xs: 100 / x > 10
}

fn all_rows(xss: List<List<Int>>) -> Bool {
    forall row in xss: forall x in row: x > 0
}

fn snapshots(n: Int) -> List<List<Int>>
    requires n >= 0
{
    var xs: List<Int> = []
    var all: List<List<Int>> = []
    var i = 0
    while i < n
        decreases n - i
    {
        all = all.push(xs)
        xs = xs.push(i)
        i = i + 1
    }
    all.push(xs)
}
"#;

#[test]
fn list_members_from_source_at_their_boundaries() {
    let p = common::check_source(LISTS);
    let f = "pl.lists.ops";
    assert_eq!(
        val(&p, f, &[json!([1, 2, 3]), json!(0)]),
        json!([[], [1, 2, 3], [1, 2], [1, 2, 3, 0], [1, 2, 3, 3, 2, 1]])
    );
    assert_eq!(
        val(&p, f, &[json!([1, 2, 3]), json!(3)]),
        json!([[1, 2, 3], [], [1, 2], [1, 2, 3, 3], [1, 2, 3, 3, 2, 1]])
    );
    assert_eq!(
        val(&p, f, &[json!([]), json!(5)]),
        json!([[], [], [], [5], []])
    );
    // A count beyond every machine integer clamps (stdlib 4.1 note, S16).
    let huge = num("340282366920938463463374607431768211456");
    assert_eq!(
        val(&p, f, &[json!([7]), huge.clone()]),
        json!([[7], [], [], [7, huge], [7, 7]])
    );
    let f = "pl.lists.accessors";
    assert_eq!(
        val(&p, f, &[json!([4, 5]), json!(1)]),
        json!([some(json!(5)), some(json!(4)), some(json!(5))])
    );
    assert_eq!(
        val(&p, f, &[json!([4, 5]), json!(2)]),
        json!([none(), some(json!(4)), some(json!(5))])
    );
    assert_eq!(
        val(&p, f, &[json!([4, 5]), json!(-1)]),
        json!([none(), some(json!(4)), some(json!(5))])
    );
    assert_eq!(
        val(&p, f, &[json!([]), json!(0)]),
        json!([none(), none(), none()])
    );
    assert_eq!(
        val(&p, f, &[json!([4]), huge]),
        json!([none(), some(json!(4)), some(json!(4))])
    );
    let f = "pl.lists.has";
    assert_eq!(
        val(&p, f, &[json!([[1], []]), json!([])]),
        json!([true, false, false])
    );
    assert_eq!(
        val(&p, f, &[json!([[1, 0]]), json!([0, 1])]),
        json!([false, false, true])
    );
    assert_eq!(
        val(&p, f, &[json!([]), json!([])]),
        json!([false, true, false])
    );
}

#[test]
fn list_take_on_a_chained_receiver_reports_the_receiver_value_as_self() {
    let p = common::check_source(LISTS);
    let d = flt(&p, "pl.lists.chained", &[]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "std.prelude.List.take");
    assert_eq!(d["clause"], "n >= 0");
    // The receiver is evaluated before the precondition (6.2): the value
    // `take` received, not the literal the chain started from.
    assert_eq!(d["input"], json!({"self": [2, 3], "n": -1}));
    assert_eq!(d["caller"], "pl.lists.chained");
    assert_eq!(d["span"]["file"], "std/prelude.cpl");
    assert_eq!(bt(&d), ["std.prelude.List.take", "pl.lists.chained"]);
    assert_eq!(
        d["backtrace"][0]["span"],
        at(LISTS, "[1, 2, 3].drop(1).take(-1)")
    );
}

#[test]
fn list_equality_compares_erased_refined_elements_with_base_ones_s17() {
    let p = common::check_source(LISTS);
    assert_eq!(
        val(&p, "pl.lists.mixed", &[json!(1)]),
        json!([true, true, true, true])
    );
    assert_eq!(
        val(&p, "pl.lists.mixed", &[json!(2)]),
        json!([false, false, false, true])
    );
}

#[test]
fn forall_stops_at_the_first_false_before_a_faulting_element() {
    let p = common::check_source(LISTS);
    let f = "pl.lists.all_big";
    // 100 / 20 == 5 is not > 10: evaluation stops before 100 / 0.
    assert_eq!(val(&p, f, &[json!([5, 20, 0])]), json!(false));
    assert_eq!(val(&p, f, &[json!([])]), json!(true));
    let d = flt(&p, f, &[json!([5, 0, 20])]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "pl.lists.all_big");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["input"], json!({"a": 100, "b": 0}));
    let f = "pl.lists.all_rows";
    assert_eq!(val(&p, f, &[json!([[1, 2], [], [3]])]), json!(true));
    assert_eq!(val(&p, f, &[json!([[1, 2], [0], [3]])]), json!(false));
    assert_eq!(val(&p, f, &[json!([])]), json!(true));
}

#[test]
fn list_values_are_persistent_across_var_rebinding_in_a_loop() {
    let p = common::check_source(LISTS);
    assert_eq!(
        val(&p, "pl.lists.snapshots", &[json!(3)]),
        json!([[], [0], [0, 1], [0, 1, 2]])
    );
}

const LIST_LAWS: &str = r#"module laws.lists

property "L1 take ++ drop"
    for xs in gen.list(gen.int)
    for n in gen.range(0, 12)
{
    assert xs.take(n) ++ xs.drop(n) == xs
}

property "L1 at other element types"
    for xss in gen.list(gen.list(gen.text))
    for os in gen.list(gen.option(gen.bool))
    for n in gen.range(0, 6)
{
    assert xss.take(n) ++ xss.drop(n) == xss
    assert os.take(n) ++ os.drop(n) == os
    assert flatten(xss).len == flatten(xss.take(n)).len + flatten(xss.drop(n)).len
}

property "L2 take and drop of a concatenation"
    for xs in gen.list(gen.int)
    for ys in gen.list(gen.int)
{
    assert (xs ++ ys).take(xs.len) == xs
    assert (xs ++ ys).drop(xs.len) == ys
}

property "L3 concatenation identities and associativity"
    for xs in gen.list(gen.int)
    for ys in gen.list(gen.int)
    for zs in gen.list(gen.int)
{
    let empty: List<Int> = []
    assert (empty ++ xs) == xs
    assert (xs ++ empty) == xs
    assert (xs ++ ys) ++ zs == xs ++ (ys ++ zs)
}

property "L4 length of a concatenation"
    for xs in gen.list(gen.int)
    for ys in gen.list(gen.int)
{
    assert (xs ++ ys).len == xs.len + ys.len
}

property "L5 push appends one element"
    for xs in gen.list(gen.int)
    for y in gen.int
{
    assert xs.push(y) == xs ++ [y]
}

property "L6 drop_last undoes push"
    for xs in gen.list(gen.int)
    for y in gen.int
{
    assert xs.push(y).drop_last() == xs
    assert xs.push(y).len == xs.len + 1
}

property "L7 flatten"
    for xss in gen.list(gen.list(gen.int))
    for yss in gen.list(gen.list(gen.int))
    for xs in gen.list(gen.int)
{
    let none: List<List<Int>> = []
    assert flatten(none) == []
    assert flatten([xs]) == xs
    assert flatten(xss.push(xs)) == flatten(xss) ++ xs
    assert flatten(xss ++ yss) == flatten(xss) ++ flatten(yss)
}

property "L8 forall over a concatenation"
    for xs in gen.list(gen.int)
    for ys in gen.list(gen.int)
{
    assert (forall x in xs ++ ys: x > 0) == ((forall x in xs: x > 0) and (forall x in ys: x > 0))
}

property "L9 forall over a push"
    for xs in gen.list(gen.int)
    for y in gen.int
{
    assert (forall x in xs.push(y): x > 0) == ((forall x in xs: x > 0) and y > 0)
}

property "L10 forall over empty and singleton lists"
    for y in gen.int
{
    let empty: List<Int> = []
    assert (forall x in empty: x > 0) == true
    assert (forall x in [y]: x > 0) == (y > 0)
}

property "L11 take and drop never lengthen"
    for xs in gen.list(gen.int)
    for n in gen.range(0, 12)
{
    assert xs.take(n).len <= n
    assert xs.take(n).len <= xs.len
    assert xs.drop(n).len <= xs.len
}

property "L12 exact lengths"
    for xs in gen.list(gen.int)
    for n in gen.range(0, 12)
{
    assert not (n <= xs.len) or xs.take(n).len == n
    assert not (n > 0 and xs.len > n) or xs.drop(n).len < xs.len
}

property "L13 empty means equal to the empty list"
    for xs in gen.list(gen.int)
{
    assert (xs.len == 0) == (xs == [])
}

property "L14 forall survives take, drop, and drop_last"
    for xs in gen.list(gen.int)
    for n in gen.range(0, 12)
{
    assert not (forall x in xs: x > 0) or (forall x in xs.take(n): x > 0)
    assert not (forall x in xs: x > 0) or (forall x in xs.drop(n): x > 0)
    assert not (forall x in xs: x > 0) or (forall x in xs.drop_last(): x > 0)
}

property "L15 composed counts"
    for xs in gen.list(gen.int)
    for n in gen.range(0, 12)
    for m in gen.range(0, 12)
{
    assert xs.take(n).take(m) == xs.take(n.min(m))
    assert xs.drop(n).drop(m) == xs.drop(n + m)
}

property "L16 get"
    for xs in gen.list(gen.range(0, 3))
    for i in gen.range(-2, 12)
    for v in gen.range(0, 3)
{
    assert (xs.get(i) == Some(v)) == (i >= 0 and i < xs.len and xs.drop(i).take(1) == [v])
}

property "L17 and L18 reverse"
    for xs in gen.list(gen.int)
    for y in gen.int
{
    let empty: List<Int> = []
    assert xs.reverse().reverse() == xs
    assert empty.reverse() == []
    assert xs.push(y).reverse() == [y] ++ xs.reverse()
}

property "L19 contains over a concatenation"
    for xs in gen.list(gen.range(0, 5))
    for ys in gen.list(gen.range(0, 5))
    for v in gen.range(0, 5)
{
    assert (xs ++ ys).contains(v) == (xs.contains(v) or ys.contains(v))
}
"#;

#[test]
fn list_laws_l1_to_l19_hold_as_properties_at_the_default_parameters_10_7() {
    // stdlib 10.7: the laws are transcribed into properties and run with
    // `gen` at the default parameters (100 cases each).
    let p = common::check_source(LIST_LAWS);
    let all = records(&p);
    assert_eq!(all.len(), 19);
    for r in &all {
        assert_eq!(r["status"], "tested", "{r}");
        assert_eq!(r["cases"], 100, "{r}");
    }
    assert_all_tested(&p);
}

const OTHER_LAWS: &str = r#"module laws.others

use std.text.{trim, trim_start, trim_end, starts_with, ends_with, contains, split, join, to_upper, to_lower, parse_int, from_int, chars}

property "I1 and I2 the division identity"
    for a in gen.int
    for b in gen.int
{
    assert b == 0 or (a / b) * b + a % b == a
    assert b == 0 or (a % b).abs() < b.abs()
}

property "I3 parse_int of to_text"
    for n in gen.int
{
    assert parse_int(n.to_text()) == Ok(n)
}

property "I4 and I5 min, max, and abs"
    for a in gen.int
    for b in gen.int
{
    assert a.min(b) == b.min(a) and a.max(b) == b.max(a) and a.min(b) + a.max(b) == a + b
    assert a.abs() == a.max(-a)
}

property "T1 to T4 text concatenation"
    for a in gen.text
    for b in gen.text
    for c in gen.text
{
    assert (a ++ b).len == a.len + b.len
    assert ("" ++ a) == a and (a ++ "") == a
    assert (a ++ b) ++ c == a ++ (b ++ c)
    assert a.is_empty() == (a == "")
}

property "B1 to B3 bytes round trips"
    for vs in gen.list(gen.range(0, 255))
    for t in gen.text
{
    assert std.bytes.to_list(std.bytes.from_list(vs)) == vs
    let b = std.bytes.from_list(vs)
    assert std.bytes.from_list(std.bytes.to_list(b)) == b
    assert std.bytes.to_text(std.bytes.from_text(t)) == Ok(t)
}

property "O1 to O4 option and result members"
    for v in gen.int
    for d in gen.int
    for e in gen.text
{
    let none: Option<Int> = None
    let failed: Result<Int, Text> = Err(e)
    let good: Result<Int, Text> = Ok(v)
    assert Some(v).unwrap_or(d) == v
    assert none.unwrap_or(d) == d
    assert Some(v).ok_or(e) == Ok(v)
    assert none.ok_or(e) == Err(e)
    assert good.ok() == Some(v)
    assert failed.ok() == None
    assert good.err() == None
    assert failed.err() == Some(e)
    assert good.is_ok() == good.ok().is_some()
    assert failed.is_ok() == failed.ok().is_some()
}

property "X1 to X7 std.text laws"
    for t in gen.text
    for a in gen.text
    for b in gen.text
    for sep in gen.text
    for n in gen.int
{
    assert trim(trim(t)) == trim(t)
    assert trim(t) == trim_start(trim_end(t))
    assert starts_with(a ++ b, a) and ends_with(a ++ b, b) and contains(a ++ t ++ b, t)
    assert sep.is_empty() or join(split(t, sep), sep) == t
    assert sep.is_empty() or split(t, sep).len >= 1
    let s = match chars(t).get(1) {
        Some(c) => c
        None => ","
    }
    assert join(split(t, s), s) == t
    assert to_lower(to_upper(t)) == to_lower(t)
    assert parse_int(from_int(n)) == Ok(n)
    assert chars(a ++ b) == chars(a) ++ chars(b)
}
"#;

#[test]
fn int_text_bytes_option_and_text_module_laws_hold_as_properties_10_7() {
    let p = common::check_source(OTHER_LAWS);
    let all = records(&p);
    assert_eq!(all.len(), 7);
    for r in &all {
        assert_eq!(r["status"], "tested", "{r}");
        assert_eq!(r["cases"], 100, "{r}");
    }
    assert_all_tested(&p);
}

// ===========================================================================
// D. Structs and spread (semantics 6.1, 6.2, 6.5; grammar D29)
// ===========================================================================

const STRUCTS: &str = r#"module pl.structs

pub struct Point {
    x: Int
    y: Int
}

pub struct Line {
    from: Point
    to: Point
    label: Text
}

pub struct Empty {}

pub struct Box<T> {
    v: T
    n: Int
}

fn moved(l: Line, dx: Int) -> List<Line> {
    let m = Line { ..l, to: Point { ..l.to, x: l.to.x + dx }, label: "moved" }
    [l, m]
}

fn all_overridden(p: Point) -> Point {
    Point { ..p, y: 1, x: 2 }
}

fn spread_only(p: Point) -> Bool {
    Point { ..p } == p
}

fn empty() -> Empty {
    Empty {}
}

fn empties_equal() -> Bool {
    Empty {} == empty()
}

fn boom(n: Int) -> Point {
    Point { x: 1 / n, y: 0 }
}

fn base_first(n: Int) -> Point {
    Point { ..boom(n), x: 10 % n }
}

fn source_order(n: Int) -> Point {
    Point { y: 7 / n, x: 9 / n }
}

fn field_of_call() -> Int {
    all_overridden(Point { x: 0, y: 0 }).y
}

fn boxes() -> List<Bool> {
    let a = Box { v: 1, n: 0 }
    let b = Box { v: "t", n: 0 }
    let c = Box { v: [Box { v: Some(1), n: 1 }], n: 2 }
    let d = Box { ..a, v: 2 }
    [a.v + 1 == 2, b.v ++ "u" == "tu", c.v.len == 1, d.v == 2 and d.n == 0, a == Box { v: 1, n: 0 }]
}

fn boxed_text(t: Text) -> Box<Text> {
    Box { v: t, n: t.len }
}

fn same(a: Line, b: Line) -> Bool {
    a == b
}
"#;

fn point(x: i64, y: i64) -> Json {
    json!({"$type": "pl.structs.Point", "x": x, "y": y})
}

fn line(from: Json, to: Json, label: &str) -> Json {
    json!({"$type": "pl.structs.Line", "from": from, "to": to, "label": label})
}

#[test]
fn struct_spread_copies_the_base_and_replaces_only_the_listed_fields() {
    let p = common::check_source(STRUCTS);
    let l = line(point(0, 0), point(1, 2), "l");
    assert_eq!(
        val(&p, "pl.structs.moved", &[l.clone(), json!(5)]),
        json!([l, line(point(0, 0), point(6, 2), "moved")])
    );
    assert_eq!(
        val(&p, "pl.structs.all_overridden", &[point(8, 9)]),
        point(2, 1)
    );
    assert_eq!(
        val(&p, "pl.structs.spread_only", &[point(8, 9)]),
        json!(true)
    );
    assert_eq!(val(&p, "pl.structs.field_of_call", &[]), json!(1));
}

#[test]
fn struct_literal_evaluates_the_spread_base_first_then_fields_in_source_order_6_2() {
    let p = common::check_source(STRUCTS);
    // The base `boom(0)` faults before the field `10 % n` is evaluated.
    let d = flt(&p, "pl.structs.base_first", &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "pl.structs.boom");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
    assert_eq!(bt(&d), ["pl.structs.boom", "pl.structs.base_first"]);
    // Fields run in source order (y first), not declaration order (x first).
    let d = flt(&p, "pl.structs.source_order", &[json!(0)]);
    assert_eq!(d["input"], json!({"a": 7, "b": 0}));
    assert_eq!(val(&p, "pl.structs.source_order", &[json!(1)]), point(9, 7));
}

#[test]
fn struct_values_of_zero_fields_and_of_generic_structs() {
    let p = common::check_source(STRUCTS);
    assert_eq!(
        val(&p, "pl.structs.empty", &[]),
        json!({"$type": "pl.structs.Empty"})
    );
    assert_eq!(val(&p, "pl.structs.empties_equal", &[]), json!(true));
    assert_eq!(
        val(&p, "pl.structs.boxes", &[]),
        json!([true, true, true, true, true])
    );
    // A generic struct's `$type` carries no type arguments (S40, T30).
    assert_eq!(
        val(&p, "pl.structs.boxed_text", &[json!("\u{e9}t")]),
        json!({"$type": "pl.structs.Box", "v": "\u{e9}t", "n": 2})
    );
    // Equality is structural and field by field (6.5).
    let l = line(point(0, 0), point(1, 2), "l");
    let other = line(point(0, 0), point(2, 1), "l");
    assert_eq!(
        val(&p, "pl.structs.same", &[l.clone(), l.clone()]),
        json!(true)
    );
    assert_eq!(val(&p, "pl.structs.same", &[l, other]), json!(false));
}

// ===========================================================================
// E. Enums with nested matches (semantics 3.12, 3.13, 6.2)
// ===========================================================================

const ENUMS: &str = r#"module pl.enums

pub type Tag(Text)
pub type Positive = Int where self > 0

pub enum Shape {
    Circle(Int)
    Rect(Int, Int)
    Labeled(Tag, Option<Shape>)
    Dot
}

pub enum Either<L, R> {
    Left(L)
    Right(R)
}

fn area(s: Shape) -> Int {
    match s {
        Circle(0) => 0
        Circle(r) => 3 * r * r
        Rect(w, h) => w * h
        Labeled(Tag("none"), _) => -1
        Labeled(_, Some(inner)) => area(inner)
        Labeled(_, None) => -2
        Dot => 1
    }
}

fn first_match(o: Option<Option<Int>>) -> Text {
    match o {
        Some(Some(0)) => "zero"
        Some(Some(-1)) => "minus one"
        Some(Some(n)) => n.to_text()
        Some(None) => "inner none"
        None => "none"
    }
}

fn bools(a: Bool, b: Bool) -> Int {
    match a {
        true => match b {
            true => 3
            false => 2
        }
        false => if b { 1 } else { 0 }
    }
}

fn unit_match(u: Unit) -> Int {
    match u {
        Unit => 7
    }
}

fn big_literal(n: Int) -> Int {
    match n {
        123456789012345678901234567890 => 1
        -123456789012345678901234567890 => -1
        -0 => 100
        _ => 0
    }
}

fn pos_match(p: Positive) -> Text {
    match p {
        1 => "one"
        n => (n * 10).to_text()
    }
}

fn opt_pos(o: Option<Positive>) -> Int {
    match o {
        Some(1) => 100
        Some(p) => p
        None => 0
    }
}

fn early(o: Option<Int>) -> Int {
    let v = match o {
        Some(n) => n
        None => {
            return -1
        }
    }
    v * 2
}

fn sides(xs: List<Either<Int, Text>>) -> List<Int> {
    var out: List<Int> = []
    var rest = xs
    while not rest.is_empty()
        decreases rest.len
    {
        let n = match rest.first() {
            Some(Left(k)) => k
            Some(Right(t)) => t.len
            None => 0
        }
        out = out.push(n)
        rest = rest.drop(1)
    }
    out
}

fn labeled() -> Shape {
    Labeled(Tag("x"), Some(Rect(2, 3)))
}

fn dot() -> Shape {
    Dot
}

fn qualified(s: Shape) -> Int {
    match s {
        Shape.Circle(r) => r
        pl.enums.Shape.Rect(w, _) => w
        Shape.Dot => 0
        _ => -1
    }
}

fn qualified_values() -> List<Int> {
    [qualified(Shape.Circle(5)), qualified(pl.enums.Shape.Rect(3, 4)), qualified(Shape.Dot), qualified(Labeled(Tag("t"), None))]
}

fn explicit_generics() -> List<Bool> {
    [Some<Int>(1) == Some(1), Either.Left<Int, Text>(2) == Left(2), Ok<Int, Text>(3).is_ok()]
}

fn equalities() -> List<Bool> {
    [Circle(1) == Circle(1), Rect(1, 2) == Rect(2, 1), Dot == Dot, Circle(1) != Dot, Labeled(Tag("a"), None) == Labeled(Tag("a"), None)]
}
"#;

fn shape(variant: &str, payload: Option<Json>) -> Json {
    match payload {
        Some(p) => json!({"$type": "pl.enums.Shape", "$variant": variant, "$payload": p}),
        None => json!({"$type": "pl.enums.Shape", "$variant": variant}),
    }
}

fn tag(t: &str) -> Json {
    json!({"$type": "pl.enums.Tag", "$payload": [t]})
}

#[test]
fn enum_match_takes_the_first_matching_arm_through_nested_patterns() {
    let p = common::check_source(ENUMS);
    let f = "pl.enums.area";
    assert_eq!(val(&p, f, &[shape("Circle", Some(json!([0])))]), json!(0));
    assert_eq!(val(&p, f, &[shape("Circle", Some(json!([2])))]), json!(12));
    assert_eq!(val(&p, f, &[shape("Rect", Some(json!([2, 5])))]), json!(10));
    assert_eq!(val(&p, f, &[shape("Dot", None)]), json!(1));
    // A wrapper pattern with a text literal inside a variant pattern.
    assert_eq!(
        val(
            &p,
            f,
            &[shape(
                "Labeled",
                Some(json!([tag("none"), some(shape("Dot", None))]))
            )]
        ),
        json!(-1)
    );
    assert_eq!(
        val(
            &p,
            f,
            &[shape(
                "Labeled",
                Some(json!([
                    tag("big"),
                    some(shape("Rect", Some(json!([3, 4]))))
                ]))
            )]
        ),
        json!(12)
    );
    assert_eq!(
        val(
            &p,
            f,
            &[shape("Labeled", Some(json!([tag("big"), none()])))]
        ),
        json!(-2)
    );
    let f = "pl.enums.first_match";
    assert_eq!(val(&p, f, &[some(some(json!(0)))]), json!("zero"));
    assert_eq!(val(&p, f, &[some(some(json!(-1)))]), json!("minus one"));
    assert_eq!(val(&p, f, &[some(some(json!(-2)))]), json!("-2"));
    assert_eq!(val(&p, f, &[some(none())]), json!("inner none"));
    assert_eq!(val(&p, f, &[none()]), json!("none"));
}

#[test]
fn enum_matches_on_bool_unit_literals_and_refined_scrutinees() {
    let p = common::check_source(ENUMS);
    for (a, b, r) in [
        (true, true, 3),
        (true, false, 2),
        (false, true, 1),
        (false, false, 0),
    ] {
        assert_eq!(val(&p, "pl.enums.bools", &[json!(a), json!(b)]), json!(r));
    }
    assert_eq!(val(&p, "pl.enums.unit_match", &[unit()]), json!(7));
    let f = "pl.enums.big_literal";
    assert_eq!(
        val(&p, f, &[num("123456789012345678901234567890")]),
        json!(1)
    );
    assert_eq!(
        val(&p, f, &[num("-123456789012345678901234567890")]),
        json!(-1)
    );
    assert_eq!(
        val(&p, f, &[num("123456789012345678901234567891")]),
        json!(0)
    );
    // `-0` is the literal zero.
    assert_eq!(val(&p, f, &[json!(0)]), json!(100));
    assert_eq!(val(&p, "pl.enums.pos_match", &[json!(1)]), json!("one"));
    assert_eq!(val(&p, "pl.enums.pos_match", &[json!(4)]), json!("40"));
    assert_eq!(val(&p, "pl.enums.opt_pos", &[some(json!(1))]), json!(100));
    assert_eq!(val(&p, "pl.enums.opt_pos", &[some(json!(9))]), json!(9));
    assert_eq!(val(&p, "pl.enums.opt_pos", &[none()]), json!(0));
}

#[test]
fn enum_arm_blocks_return_early_and_generic_enums_match_in_loops() {
    let p = common::check_source(ENUMS);
    assert_eq!(val(&p, "pl.enums.early", &[some(json!(4))]), json!(8));
    assert_eq!(val(&p, "pl.enums.early", &[none()]), json!(-1));
    let left = |n: i64| json!({"$type": "pl.enums.Either", "$variant": "Left", "$payload": [n]});
    let right = |t: &str| json!({"$type": "pl.enums.Either", "$variant": "Right", "$payload": [t]});
    assert_eq!(
        val(
            &p,
            "pl.enums.sides",
            &[json!([left(3), right("\u{e9}\u{e9}"), left(-1)])]
        ),
        json!([3, 2, -1])
    );
    assert_eq!(val(&p, "pl.enums.sides", &[json!([])]), json!([]));
}

#[test]
fn enum_values_encode_variant_and_payload_and_compare_structurally() {
    let p = common::check_source(ENUMS);
    assert_eq!(
        val(&p, "pl.enums.labeled", &[]),
        shape(
            "Labeled",
            Some(json!([tag("x"), some(shape("Rect", Some(json!([2, 3]))))]))
        )
    );
    // A payload-less variant has no `$payload` (S40, 7.3).
    assert_eq!(
        val(&p, "pl.enums.dot", &[]),
        json!({"$type": "pl.enums.Shape", "$variant": "Dot"})
    );
    assert_eq!(
        val(&p, "pl.enums.equalities", &[]),
        json!([true, false, true, true, true])
    );
}

#[test]
fn qualified_constructors_and_explicit_type_arguments_build_the_same_values() {
    let p = common::check_source(ENUMS);
    assert_eq!(
        val(&p, "pl.enums.qualified_values", &[]),
        json!([5, 3, 0, -1])
    );
    assert_eq!(
        val(&p, "pl.enums.explicit_generics", &[]),
        json!([true, true, true])
    );
}

// ===========================================================================
// F. Option, Result, and `?` (semantics 3.8, 5.2 step 5, 6.2, 6.6; stdlib 5)
// ===========================================================================

const RESULTS: &str = r#"module pl.results

pub type Positive = Int where self > 0

fn half(n: Int) -> Result<Int, Text> {
    if n % 2 == 0 { Ok(n / 2) } else { Err("odd") }
}

fn quarter(n: Int) -> Result<Int, Text> {
    let h = half(n)?
    half(h)
}

fn chain(n: Int) -> Result<Int, Text> {
    Ok(half(half(n)?)? + 1)
}

fn pair(a: Int, b: Int) -> Int {
    a + b
}

// A `?` in an argument position exits before the later arguments run.
fn exits_early(n: Int) -> Result<Int, Text> {
    Ok(pair(half(n)?, 10 / (n - n)))
}

fn method_after(n: Int) -> Result<Int, Text> {
    Ok(half(n)?.abs())
}

fn scrutinee(n: Int) -> Result<Text, Text> {
    match half(n)? {
        0 => Ok("zero")
        h => Ok(h.to_text())
    }
}

fn unit_steps(n: Int) -> Result<Unit, Text> {
    let _ = half(n)?
    if n < 0 {
        return Err("negative")
    }
    Ok(Unit)
}

fn in_loop(xs: List<Int>) -> Result<Int, Text>
    ensures (match result {
        Ok(v) => v >= 0
        Err(_) => true
    })
{
    var total = 0
    var rest = xs
    while not rest.is_empty()
        invariant total >= 0
        decreases rest.len
    {
        let h = half(rest.get(0).unwrap_or(0))?
        total = total + h
        rest = rest.drop(1)
    }
    Ok(total - 100)
}

fn from_options(xs: List<Int>) -> Result<Int, Text> {
    let first = xs.first().ok_or("empty")?
    let last = xs.last().ok_or("no last")?
    Ok(first * 10 + last)
}

fn options(o: Option<Int>) -> List<Int> {
    [o.unwrap_or(-1), if o.is_some() { 1 } else { 0 }, if o.is_none() { 1 } else { 0 }]
}

fn results(r: Result<Int, Text>) -> List<Option<Int>> {
    [r.ok(), Some(r.unwrap_or(-1)), if r.is_err() { None } else { Some(0) }]
}

fn errs(r: Result<Int, Text>) -> Option<Text> {
    r.err()
}

fn checked(n: Int) -> Result<Positive, ConstraintError> {
    let p = check<Positive>(n)?
    Ok(p)
}

fn nested_options(o: Option<Option<Int>>) -> Int {
    match o {
        Some(inner) => inner.unwrap_or(-1)
        None => -2
    }
}
"#;

#[test]
fn question_mark_unwraps_ok_and_exits_with_err_6_2() {
    let p = common::check_source(RESULTS);
    assert_eq!(val(&p, "pl.results.quarter", &[json!(8)]), ok(json!(2)));
    assert_eq!(
        val(&p, "pl.results.quarter", &[json!(6)]),
        err(json!("odd"))
    );
    assert_eq!(
        val(&p, "pl.results.quarter", &[json!(7)]),
        err(json!("odd"))
    );
    assert_eq!(val(&p, "pl.results.chain", &[json!(8)]), ok(json!(3)));
    assert_eq!(val(&p, "pl.results.chain", &[json!(2)]), err(json!("odd")));
    assert_eq!(
        val(&p, "pl.results.method_after", &[json!(-8)]),
        ok(json!(4))
    );
    assert_eq!(
        val(&p, "pl.results.scrutinee", &[json!(0)]),
        ok(json!("zero"))
    );
    assert_eq!(
        val(&p, "pl.results.scrutinee", &[json!(10)]),
        ok(json!("5"))
    );
    assert_eq!(
        val(&p, "pl.results.scrutinee", &[json!(1)]),
        err(json!("odd"))
    );
    assert_eq!(val(&p, "pl.results.unit_steps", &[json!(2)]), ok(unit()));
    assert_eq!(
        val(&p, "pl.results.unit_steps", &[json!(-2)]),
        err(json!("negative"))
    );
    assert_eq!(
        val(&p, "pl.results.unit_steps", &[json!(-1)]),
        err(json!("odd"))
    );
}

#[test]
fn question_mark_in_an_argument_exits_before_later_arguments_are_evaluated() {
    let p = common::check_source(RESULTS);
    // The Err exits the function: `10 / 0` never runs, so there is no fault.
    assert_eq!(
        val(&p, "pl.results.exits_early", &[json!(3)]),
        err(json!("odd"))
    );
    // With an Ok the second argument runs and divides by zero.
    let d = flt(&p, "pl.results.exits_early", &[json!(4)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "pl.results.exits_early");
}

#[test]
fn question_mark_inside_a_loop_is_an_exit_checked_by_ensures_5_2() {
    let p = common::check_source(RESULTS);
    let f = "pl.results.in_loop";
    // An Err propagated from inside the loop exits the function without a
    // further invariant check; the ensures sees the Err and holds.
    assert_eq!(val(&p, f, &[json!([2, 3, 4])]), err(json!("odd")));
    assert_eq!(val(&p, f, &[json!([200])]), ok(json!(0)));
    let d = flt(&p, f, &[json!([2, 4])]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "ensures");
    // T56: the clause's blocks are joined onto one line; the parentheses
    // are kept as written.
    assert_eq!(
        d["clause"],
        "(match result { Ok(v) => v >= 0 Err(_) => true })"
    );
    assert_eq!(d["actual"], ok(json!(-97)));
    assert_eq!(d["input"], json!({"xs": [2, 4]}));
    assert_eq!(d["attributed_to"], "body");
}

#[test]
fn option_and_result_members_and_ok_or_bridges_6_10() {
    let p = common::check_source(RESULTS);
    let f = "pl.results.from_options";
    assert_eq!(val(&p, f, &[json!([1, 2, 3])]), ok(json!(13)));
    assert_eq!(val(&p, f, &[json!([4])]), ok(json!(44)));
    assert_eq!(val(&p, f, &[json!([])]), err(json!("empty")));
    assert_eq!(
        val(&p, "pl.results.options", &[some(json!(5))]),
        json!([5, 1, 0])
    );
    assert_eq!(val(&p, "pl.results.options", &[none()]), json!([-1, 0, 1]));
    assert_eq!(
        val(&p, "pl.results.results", &[ok(json!(5))]),
        json!([some(json!(5)), some(json!(5)), some(json!(0))])
    );
    assert_eq!(
        val(&p, "pl.results.results", &[err(json!("e"))]),
        json!([none(), some(json!(-1)), none()])
    );
    assert_eq!(
        val(&p, "pl.results.errs", &[err(json!("e"))]),
        some(json!("e"))
    );
    assert_eq!(val(&p, "pl.results.errs", &[ok(json!(1))]), none());
    assert_eq!(val(&p, "pl.results.checked", &[json!(3)]), ok(json!(3)));
    assert_eq!(
        val(&p, "pl.results.checked", &[json!(-3)]),
        constraint("pl.results.Positive", "-3 does not satisfy self > 0")
    );
    // Nested options stay distinguishable (S40).
    assert_eq!(
        val(&p, "pl.results.nested_options", &[some(some(json!(4)))]),
        json!(4)
    );
    assert_eq!(
        val(&p, "pl.results.nested_options", &[some(none())]),
        json!(-1)
    );
    assert_eq!(val(&p, "pl.results.nested_options", &[none()]), json!(-2));
}

// ===========================================================================
// G. Wrappers and refinements (semantics 3.3-3.5, 6.1, 6.10, S16, S23, S39,
//    S55; stdlib 1.4, 1.5, S44; toolchain T39, 7.3)
// ===========================================================================

const REFINE: &str = r#"module pl.refine

pub type PageId(Text)
pub type Meters(Int)
pub type Twice(Meters)
pub type Positive = Int where self > 0
pub type Small = Positive where self < 10
pub type Even = Small where self % 2 == 0
pub type Neg = Int where self < 0
pub type NonEmpty = List<Int> where self.len > 0
pub type Word = Text where not std.text.contains(self, " ")
pub type Line = Text where not std.text.contains(self, "\n")
pub type Truth = Bool where self
pub type Short = PageId where match self { PageId(t) => t.len < 5 }
pub type SomeInt = Option<Int> where (match self {
    Some(_) => true
    None => false
})

pub type Big = gen.Positive where self > 100
pub type Nz = Int where 10 / self > 0
pub type ShortBytes = Bytes where self.len < 3
pub type Ordered = List<Int> where (if self.len < 2 {
    true
} else {
    let a = self.get(0).unwrap_or(0)
    let b = self.get(1).unwrap_or(0)
    a <= b
})

pub struct Rect {
    w: Positive
    h: Positive
}

pub struct Box<T> {
    v: T
}

pub type Wrapped(Small)

pub enum Sized {
    Pair(Positive, Small)
    Nothing
}

pub type Square = Rect where self.w == self.h
pub type Wide = Rect where self.w / self.h > 1

fn double(p: Positive) -> Int {
    p * 2
}

fn small_id(s: Small) -> Int {
    s
}

fn even_id(e: Even) -> Int {
    e
}

fn neg_id(n: Neg) -> Int {
    n
}

// Literals proved against the parameter types at argument positions.
fn literal_args() -> List<Int> {
    [double(3), small_id(9), even_id(4), neg_id(-1), double(2 * 3 - 1), double(-(-1)), double((7))]
}

// Literals proved at assignment, annotation, and return positions; widening
// along a chain needs no proof (rule 2').
fn assigned(c: Bool) -> Positive {
    var p: Positive = 1
    p = 5
    let q: Small = 9
    if c {
        return q
    }
    p = q
    p
}

// Literals proved at field, payload, wrapper, and method-argument positions.
fn positions() -> List<Bool> {
    let r = Rect { w: 2, h: 3 }
    let w = Wrapped(5)
    let s = Pair(1, 9)
    let ps: List<Positive> = [1]
    let more = ps.push(2)
    let o: Box<Option<Small>> = Box { v: Some(3) }
    [r.w * r.h == 6, w == Wrapped(5), s == Pair(1, 9), more == [1, 2], o.v == Some(3)]
}

fn compound() -> List<Bool> {
    let xs: NonEmpty = [1, 2]
    let o: Option<Positive> = Some(2)
    let t: Truth = true
    let id: Short = PageId("abc")
    let ms: List<Meters> = [Meters(1), Meters(1)]
    [xs.len == 2, o == Some(2), t, id == PageId("abc"), ms.contains(Meters(1))]
}

fn erased(p: Positive) -> List<Bool> {
    let ps: List<Positive> = [p]
    [p == 3, p + 1 == 4, ps == [3], Some(p) == Some(3)]
}

fn unwrap_twice(t: Twice) -> Int {
    match t {
        Twice(Meters(0)) => -1
        Twice(Meters(n)) => n
    }
}

fn wrap_twice(n: Int) -> Twice {
    Twice(Meters(n))
}

fn wrappers_equal(a: PageId, b: PageId) -> Bool {
    a == b
}

fn to_even(n: Int) -> Result<Even, ConstraintError> {
    check<Even>(n)
}

fn to_even_from_small(s: Small) -> Result<Even, ConstraintError> {
    check<Even>(s)
}

fn to_word(t: Text) -> Result<Word, ConstraintError> {
    check<Word>(t)
}

fn to_line(t: Text) -> Result<Line, ConstraintError> {
    check<Line>(t)
}

fn to_nonempty(xs: List<Int>) -> Result<NonEmpty, ConstraintError> {
    check<NonEmpty>(xs)
}

fn to_square(r: Rect) -> Result<Square, ConstraintError> {
    check<Square>(r)
}

fn to_short(id: PageId) -> Result<Short, ConstraintError> {
    check<Short>(id)
}

fn to_some(o: Option<Int>) -> Result<SomeInt, ConstraintError> {
    check<SomeInt>(o)
}

fn to_truth(b: Bool) -> Result<Truth, ConstraintError> {
    check<Truth>(b)
}

fn to_gen_positive(n: Int) -> Result<gen.Positive, ConstraintError> {
    check<gen.Positive>(n)
}

fn to_big(n: Int) -> Result<Big, ConstraintError> {
    check<Big>(n)
}

fn to_short_bytes(values: List<Int>) -> Result<ShortBytes, ConstraintError> {
    check<ShortBytes>(std.bytes.from_list(values))
}

fn to_ordered(xs: List<Int>) -> Result<Ordered, ConstraintError> {
    check<Ordered>(xs)
}

fn take_box(b: Box<Small>) -> Int {
    b.v
}

fn take_deep(r: Result<List<Positive>, Text>) -> Int {
    0
}

fn take_rect(r: Rect) -> Int {
    r.w * r.h
}

fn take_wide(r: Wide) -> Int {
    r.w
}

fn take_square(s: Square) -> Int {
    s.w
}

fn take_opt(o: Option<Small>) -> Int {
    match o {
        Some(s) => s
        None => 0
    }
}

fn take_res(r: Result<Positive, Neg>) -> Int {
    match r {
        Ok(p) => p
        Err(n) => n
    }
}

fn take_nested(xss: List<List<Even>>) -> Int {
    flatten(xss).len
}

fn take_many(a: Positive, b: Neg) -> Int {
    a + b
}

fn take_word(w: Word) -> Text {
    w
}

fn take_short(s: Short) -> Int {
    0
}

fn take_truth(t: Truth) -> Bool {
    t
}

fn take_nonempty(xs: NonEmpty) -> Int {
    xs.len
}

fn guarded(p: Positive) -> Int
    requires p > 5
{
    p
}
"#;

#[test]
fn refine_literals_at_argument_assignment_and_return_positions_run_t39() {
    let p = common::check_source(REFINE);
    let expected = json!([6, 9, 4, -1, 10, 2, 14]);
    assert_eq!(val(&p, "pl.refine.literal_args", &[]), expected);
    assert_eq!(val(&p, "pl.refine.assigned", &[json!(false)]), json!(9));
    assert_eq!(val(&p, "pl.refine.assigned", &[json!(true)]), json!(9));
    assert_eq!(
        val(&p, "pl.refine.compound", &[]),
        json!([true, true, true, true, true])
    );
    // Unchecked mode trusts the proofs; the values are the same (T39).
    assert_eq!(
        val_with(&p, "pl.refine.literal_args", &[], &unchecked()),
        expected
    );
    assert_eq!(
        val_with(&p, "pl.refine.compound", &[], &unchecked()),
        json!([true, true, true, true, true])
    );
}

#[test]
fn refine_literals_at_field_payload_wrapper_and_method_argument_positions_t39() {
    let p = common::check_source(REFINE);
    let expected = json!([true, true, true, true, true]);
    assert_eq!(val(&p, "pl.refine.positions", &[]), expected);
    assert_eq!(
        val_with(&p, "pl.refine.positions", &[], &unchecked()),
        expected
    );
}

#[test]
fn refined_values_are_their_base_values_at_runtime_6_1() {
    let p = common::check_source(REFINE);
    assert_eq!(
        val(&p, "pl.refine.erased", &[json!(3)]),
        json!([true, true, true, true])
    );
    assert_eq!(
        val(&p, "pl.refine.erased", &[json!(4)]),
        json!([false, false, false, false])
    );
}

#[test]
fn wrappers_are_nominal_values_reached_only_by_matching_s23() {
    let p = common::check_source(REFINE);
    let twice = |n: i64| json!({"$type": "pl.refine.Twice", "$payload": [{"$type": "pl.refine.Meters", "$payload": [n]}]});
    assert_eq!(val(&p, "pl.refine.wrap_twice", &[json!(4)]), twice(4));
    assert_eq!(val(&p, "pl.refine.unwrap_twice", &[twice(4)]), json!(4));
    assert_eq!(val(&p, "pl.refine.unwrap_twice", &[twice(0)]), json!(-1));
    let id = |t: &str| json!({"$type": "pl.refine.PageId", "$payload": [t]});
    assert_eq!(
        val(&p, "pl.refine.wrappers_equal", &[id("a"), id("a")]),
        json!(true)
    );
    assert_eq!(
        val(&p, "pl.refine.wrappers_equal", &[id("a"), id("b")]),
        json!(false)
    );
}

#[test]
fn check_runs_a_three_level_chain_root_first_and_names_the_failing_declaration_s44() {
    let p = common::check_source(REFINE);
    let f = "pl.refine.to_even";
    assert_eq!(
        val(&p, f, &[json!(0)]),
        constraint("pl.refine.Positive", "0 does not satisfy self > 0")
    );
    assert_eq!(
        val(&p, f, &[json!(12)]),
        constraint("pl.refine.Small", "12 does not satisfy self < 10")
    );
    assert_eq!(
        val(&p, f, &[json!(3)]),
        constraint("pl.refine.Even", "3 does not satisfy self % 2 == 0")
    );
    assert_eq!(val(&p, f, &[json!(4)]), ok(json!(4)));
    // A value already of a refinement in the chain is checked from the root
    // again; the answer is the same.
    assert_eq!(
        val(&p, "pl.refine.to_even_from_small", &[json!(3)]),
        constraint("pl.refine.Even", "3 does not satisfy self % 2 == 0")
    );
    // check<gen.Positive> names the declaration the call site names (S44).
    assert_eq!(
        val(&p, "pl.refine.to_gen_positive", &[json!(0)]),
        constraint("std.gen.Positive", "0 does not satisfy self > 0")
    );
    // check is no contract: it runs in unchecked mode too (6.10, toolchain 1.7).
    assert_eq!(
        val_with(&p, f, &[json!(3)], &unchecked()),
        constraint("pl.refine.Even", "3 does not satisfy self % 2 == 0")
    );
}

#[test]
fn check_messages_render_the_value_text_and_the_canonical_predicate_6_10() {
    let p = common::check_source(REFINE);
    assert_eq!(
        val(&p, "pl.refine.to_word", &[json!("a b")]),
        constraint(
            "pl.refine.Word",
            r#""a b" does not satisfy not std.text.contains(self, " ")"#
        )
    );
    assert_eq!(
        val(&p, "pl.refine.to_word", &[json!("ab")]),
        ok(json!("ab"))
    );
    // Value text escapes the line break (S40); the predicate's text literal
    // is re-escaped canonically (toolchain 2.4 rule 10).
    assert_eq!(
        val(&p, "pl.refine.to_line", &[json!("a\nb\t\"")]),
        constraint(
            "pl.refine.Line",
            r#""a\nb\t\"" does not satisfy not std.text.contains(self, "\n")"#
        )
    );
    assert_eq!(
        val(&p, "pl.refine.to_nonempty", &[json!([])]),
        constraint("pl.refine.NonEmpty", "[] does not satisfy self.len > 0")
    );
    let rect = json!({"$type": "pl.refine.Rect", "w": 1, "h": 2});
    assert_eq!(
        val(&p, "pl.refine.to_square", &[rect]),
        constraint(
            "pl.refine.Square",
            "Rect { w: 1, h: 2 } does not satisfy self.w == self.h"
        )
    );
    let id = json!({"$type": "pl.refine.PageId", "$payload": ["toolong"]});
    assert_eq!(
        val(&p, "pl.refine.to_short", &[id]),
        constraint(
            "pl.refine.Short",
            r#"PageId("toolong") does not satisfy match self { PageId(t) => t.len < 5 }"#
        )
    );
    assert_eq!(
        val(&p, "pl.refine.to_some", &[none()]),
        constraint(
            "pl.refine.SomeInt",
            "None does not satisfy (match self { Some(_) => true None => false })"
        )
    );
    assert_eq!(
        val(&p, "pl.refine.to_some", &[some(json!(0))]),
        ok(some(json!(0)))
    );
    assert_eq!(
        val(&p, "pl.refine.to_truth", &[json!(false)]),
        constraint("pl.refine.Truth", "false does not satisfy self")
    );
}

#[test]
fn check_names_each_declaration_of_the_chain_as_the_call_site_reaches_it_s44() {
    let p = common::check_source(REFINE);
    // Big's declared base is gen.Positive: its predicate is the root one.
    assert_eq!(
        val(&p, "pl.refine.to_big", &[json!(0)]),
        constraint("std.gen.Positive", "0 does not satisfy self > 0")
    );
    assert_eq!(
        val(&p, "pl.refine.to_big", &[json!(50)]),
        constraint("pl.refine.Big", "50 does not satisfy self > 100")
    );
    assert_eq!(val(&p, "pl.refine.to_big", &[json!(101)]), ok(json!(101)));
    // Bytes render as Bytes("<hex>") in value text (S40).
    assert_eq!(
        val(&p, "pl.refine.to_short_bytes", &[json!([1, 2, 171])]),
        constraint(
            "pl.refine.ShortBytes",
            r#"Bytes("0102ab") does not satisfy self.len < 3"#
        )
    );
    // A predicate with blocks and lets is one line of clause text (T56).
    assert_eq!(
        val(&p, "pl.refine.to_ordered", &[json!([2, 1])]),
        constraint(
            "pl.refine.Ordered",
            "[2, 1] does not satisfy (if self.len < 2 { true } else { let a = self.get(0).unwrap_or(0) let b = self.get(1).unwrap_or(0) a <= b })"
        )
    );
    assert_eq!(
        val(&p, "pl.refine.to_ordered", &[json!([1, 2, 0])]),
        ok(json!([1, 2, 0]))
    );
}

#[test]
fn satisfies_refinement_evaluates_the_whole_chain_and_reports_faults_per_s34_1_7() {
    let p = common::check_source(REFINE);
    let even = common::def(&p, "pl.refine.Even");
    let nz = common::def(&p, "pl.refine.Nz");
    let mut it = Interpreter::new(&p, EvalConfig::default());
    for (n, holds) in [(4, true), (3, false), (12, false), (0, false), (-2, false)] {
        let holds_now = it
            .satisfies_refinement(even, &Value::int(n))
            .unwrap_or_else(|f| panic!("at {n}: {f}"));
        assert_eq!(holds_now, holds, "at {n}");
    }
    let fault = it.satisfies_refinement(nz, &Value::int(0)).unwrap_err();
    let d = common::fault_json(&p, &fault);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "pl.refine.Nz");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "10 / self > 0");
    assert!(it.satisfies_refinement(nz, &Value::int(5)).unwrap());
}

#[test]
fn refinement_violation_inside_generic_instantiations_and_nested_payloads_s39() {
    let p = common::check_source(REFINE);
    let d = flt(
        &p,
        "pl.refine.take_box",
        &[json!({"$type": "pl.refine.Box", "v": 12})],
    );
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["pointer"], "/v");
    assert_eq!(d["expected_type"], "Small");
    assert_eq!(d["clause"], "self < 10");
    let d = flt(&p, "pl.refine.take_deep", &[ok(json!([1, 2, -3]))]);
    assert_eq!(d["pointer"], "/$payload/0/2");
    assert_eq!(d["expected_type"], "Positive");
    assert_eq!(d["actual"], -3);
    assert_eq!(val(&p, "pl.refine.take_deep", &[err(json!("e"))]), json!(0));
    // Every argument is decoded before any refinement is checked: a later
    // argument that does not decode is the refusal (toolchain 1.7).
    match common::try_run(
        &p,
        "pl.refine.take_many",
        &common::json_args(&[json!(0), json!("x")]),
    ) {
        Err(RunError::Arg { index, .. }) => assert_eq!(index, 1),
        Err(other) => panic!("expected an argument error, got {other:?}"),
        Ok(outcome) => panic!("ran: {}", common::outcome_json(&p, &outcome)),
    }
}

#[test]
fn refinement_violation_locates_the_failing_position_inside_an_argument_s39() {
    let p = common::check_source(REFINE);
    let rect = |w: i64, h: i64| json!({"$type": "pl.refine.Rect", "w": w, "h": h});
    let cases = [
        (
            "take_rect",
            vec![rect(1, 0)],
            "r",
            "/h",
            "Positive",
            "self > 0",
            json!(0),
        ),
        (
            "take_square",
            vec![rect(1, 2)],
            "s",
            "",
            "Square",
            "self.w == self.h",
            rect(1, 2),
        ),
        (
            "take_opt",
            vec![some(json!(12))],
            "o",
            "/$payload/0",
            "Small",
            "self < 10",
            json!(12),
        ),
        (
            "take_opt",
            vec![some(json!(0))],
            "o",
            "/$payload/0",
            "Small",
            "self > 0",
            json!(0),
        ),
        (
            "take_res",
            vec![err(json!(1))],
            "r",
            "/$payload/0",
            "Neg",
            "self < 0",
            json!(1),
        ),
        (
            "take_res",
            vec![ok(json!(-1))],
            "r",
            "/$payload/0",
            "Positive",
            "self > 0",
            json!(-1),
        ),
        (
            "take_nested",
            vec![json!([[2], [4, 3]])],
            "xss",
            "/1/1",
            "Even",
            "self % 2 == 0",
            json!(3),
        ),
        (
            "take_many",
            vec![json!(0), json!(1)],
            "a",
            "",
            "Positive",
            "self > 0",
            json!(0),
        ),
        (
            "take_many",
            vec![json!(1), json!(1)],
            "b",
            "",
            "Neg",
            "self < 0",
            json!(1),
        ),
        (
            "take_word",
            vec![json!("a b")],
            "w",
            "",
            "Word",
            r#"not std.text.contains(self, " ")"#,
            json!("a b"),
        ),
        (
            "take_truth",
            vec![json!(false)],
            "t",
            "",
            "Truth",
            "self",
            json!(false),
        ),
        (
            "take_nonempty",
            vec![json!([])],
            "xs",
            "",
            "NonEmpty",
            "self.len > 0",
            json!([]),
        ),
    ];
    for (f, args, parameter, pointer, expected, clause, actual) in cases {
        let name = format!("pl.refine.{f}");
        let d = flt(&p, &name, &args);
        assert_eq!(d["code"], "refinement_violation", "{f}: {d}");
        assert_eq!(d["category"], "runtime", "{f}");
        assert_eq!(d["symbol"], name.as_str(), "{f}");
        assert_eq!(d["facet"], "body", "{f}");
        assert_eq!(d["parameter"], parameter, "{f}");
        assert_eq!(d["pointer"], pointer, "{f}");
        assert_eq!(d["expected_type"], expected, "{f}");
        assert_eq!(d["clause"], clause, "{f}");
        assert_eq!(d["actual"], actual, "{f}");
        // Raised before any activation: the backtrace is empty and omitted.
        assert!(d.get("backtrace").is_none(), "{f}: {d}");
    }
    // The span is the parameter.
    let d = flt(&p, "pl.refine.take_many", &[json!(1), json!(1)]);
    assert_eq!(d["span"], at(REFINE, "b: Neg"));
    let short = json!({"$type": "pl.refine.PageId", "$payload": ["toolong"]});
    let d = flt(&p, "pl.refine.take_short", std::slice::from_ref(&short));
    assert_eq!(d["expected_type"], "Short");
    assert_eq!(d["clause"], "match self { PageId(t) => t.len < 5 }");
    assert_eq!(d["actual"], short);
}

#[test]
fn argument_refinements_are_checked_before_the_requires_and_in_every_mode_s39() {
    let p = common::check_source(REFINE);
    let f = "pl.refine.guarded";
    let d = flt(&p, f, &[json!(0)]);
    assert_eq!(d["code"], "refinement_violation");
    let d = flt(&p, f, &[json!(3)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause"], "p > 5");
    assert!(
        d.get("caller").is_none(),
        "the command line is no caller: {d}"
    );
    assert_eq!(val(&p, f, &[json!(7)]), json!(7));
    // Boundary checks are not contracts: they run in unchecked mode (S39).
    let d = flt_with(&p, f, &[json!(0)], &unchecked());
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(val_with(&p, f, &[json!(3)], &unchecked()), json!(3));
    // A constant-expression argument is checked the same way.
    let outcome = common::run_const(&p, "pl.refine.take_opt", &["Some(10)"]);
    let d = common::fault(&p, &outcome);
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["pointer"], "/$payload/0");
}

#[test]
fn a_refinement_over_a_struct_is_checked_on_a_valid_base_value_first_s39() {
    // The specification fixes no order between a refinement's predicate and
    // the refined positions inside its base. Semantics 6.10 checks a chain
    // from its root base upward, and the design requires contract
    // expressions to be "total under their preceding assumptions": Wide's
    // predicate may assume that `h` is a Positive, as every Rect built in
    // CPL is. So the boundary check establishes the base (the field
    // refinements) before evaluating the predicate on top of it, and an
    // argument whose `h` is 0 is a refinement_violation at /h, not a
    // division by zero blamed on Wide's predicate.
    let p = common::check_source(REFINE);
    let d = flt(
        &p,
        "pl.refine.take_wide",
        &[json!({"$type": "pl.refine.Rect", "w": 5, "h": 0})],
    );
    assert_eq!(d["code"], "refinement_violation", "{d}");
    assert_eq!(d["pointer"], "/h");
    assert_eq!(d["expected_type"], "Positive");
    assert_eq!(d["clause"], "self > 0");
    // With a valid base the predicate decides.
    let d = flt(
        &p,
        "pl.refine.take_wide",
        &[json!({"$type": "pl.refine.Rect", "w": 1, "h": 1})],
    );
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["pointer"], "");
    assert_eq!(d["expected_type"], "Wide");
    assert_eq!(
        val(
            &p,
            "pl.refine.take_wide",
            &[json!({"$type": "pl.refine.Rect", "w": 4, "h": 2})]
        ),
        json!(4)
    );
}

// ===========================================================================
// H. Generics instantiated at several types (semantics 3.7, S12, S13;
//    toolchain 7.3 "generic parameter")
// ===========================================================================

const GENERICS: &str = r#"module pl.generic

pub type PageId(Text)

pub struct Pair<A, B> {
    left: A
    right: B
}

pub enum Tree<T> {
    Leaf
    Node(List<Tree<T>>, T)
}

fn first_or<T>(xs: List<T>, d: T) -> T {
    match xs.first() {
        Some(x) => x
        None => d
    }
}

fn swap<A, B>(p: Pair<A, B>) -> Pair<B, A> {
    Pair { left: p.right, right: p.left }
}

fn size<T>(t: Tree<T>) -> Int {
    match t {
        Leaf => 0
        Node(kids, _) => 1 + sizes(kids)
    }
}

fn sizes<T>(ts: List<Tree<T>>) -> Int {
    var total = 0
    var rest = ts
    while not rest.is_empty()
        decreases rest.len
    {
        total = total + match rest.first() {
            Some(t) => size(t)
            None => 0
        }
        rest = rest.drop(1)
    }
    total
}

fn count<T>(xs: List<T>) -> Int
    decreases xs.len
{
    if xs.is_empty() { 0 } else { 1 + count(xs.drop(1)) }
}

fn uses() -> List<Bool> {
    let a = first_or([1, 2], 0) == 1
    let b = first_or(["x"], "y") == "x"
    let c = first_or([[1], [2]], []) == [1]
    let d = first_or([PageId("p")], PageId("q")) == PageId("p")
    let e = first_or([Some(true)], None) == Some(true)
    let f = first_or([], Pair { left: 1, right: "r" }) == Pair { left: 1, right: "r" }
    let g = swap(swap(Pair { left: [1], right: Unit })) == Pair { left: [1], right: Unit }
    let h = size(Node([Leaf, Node([], "x"), Node([Node([], "y")], "z")], "root")) == 4
    let i = count(["a", "b", "c"]) + count([[1]]) + count([Unit]) == 5
    [a, b, c, d, e, f, g, h, i]
}

fn swapped(p: Pair<Int, Text>) -> Pair<Text, Int> {
    swap(p)
}

fn second<T>(xs: List<T>, d: T) -> T
    requires xs.len >= 2
{
    first_or(xs.drop(1), d)
}

fn bad_rev<T>(xs: List<T>) -> List<T>
    ensures result.len == xs.len
{
    xs.reverse().drop(1)
}

fn second_of_ids() -> PageId {
    second([PageId("a")], PageId("d"))
}

fn second_of_units() -> Unit {
    second([Unit], Unit)
}

fn rev_of_options() -> List<Option<Int>> {
    bad_rev([Some(1), None])
}

fn rev_of_pairs() -> List<Pair<Int, Text>> {
    bad_rev([Pair { left: 1, right: "a" }])
}

fn rev_of_errors() -> List<Result<gen.Positive, ConstraintError>> {
    bad_rev([check<gen.Positive>(0), Ok(1)])
}
"#;

#[test]
fn generic_functions_structs_and_enums_run_at_several_instantiations() {
    let p = common::check_source(GENERICS);
    assert_eq!(
        val(&p, "pl.generic.uses", &[]),
        json!([true, true, true, true, true, true, true, true, true])
    );
    assert_eq!(
        val(
            &p,
            "pl.generic.swapped",
            &[json!({"$type": "pl.generic.Pair", "left": 1, "right": "r"})]
        ),
        json!({"$type": "pl.generic.Pair", "left": "r", "right": 1})
    );
}

#[test]
fn generic_requires_input_encodes_each_instantiation_by_its_value_7_3() {
    let p = common::check_source(GENERICS);
    let d = flt(&p, "pl.generic.second_of_ids", &[]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "pl.generic.second");
    assert_eq!(d["caller"], "pl.generic.second_of_ids");
    assert_eq!(d["clause"], "xs.len >= 2");
    let id = |t: &str| json!({"$type": "pl.generic.PageId", "$payload": [t]});
    assert_eq!(d["input"], json!({"xs": [id("a")], "d": id("d")}));
    let d = flt(&p, "pl.generic.second_of_units", &[]);
    assert_eq!(d["input"], json!({"xs": [unit()], "d": unit()}));
}

#[test]
fn generic_ensures_input_and_actual_encode_each_instantiation_by_its_value_7_3() {
    let p = common::check_source(GENERICS);
    let d = flt(&p, "pl.generic.rev_of_options", &[]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["symbol"], "pl.generic.bad_rev");
    assert_eq!(d["clause"], "result.len == xs.len");
    assert_eq!(d["input"], json!({"xs": [some(json!(1)), none()]}));
    assert_eq!(d["actual"], json!([some(json!(1))]));
    assert_eq!(bt(&d), ["pl.generic.bad_rev", "pl.generic.rev_of_options"]);
    let d = flt(&p, "pl.generic.rev_of_pairs", &[]);
    assert_eq!(
        d["input"],
        json!({"xs": [{"$type": "pl.generic.Pair", "left": 1, "right": "a"}]})
    );
    assert_eq!(d["actual"], json!([]));
    let d = flt(&p, "pl.generic.rev_of_errors", &[]);
    let failed = constraint("std.gen.Positive", "0 does not satisfy self > 0");
    assert_eq!(d["input"], json!({"xs": [failed.clone(), ok(json!(1))]}));
    assert_eq!(d["actual"], json!([failed]));
}

// ===========================================================================
// I. `while` with `invariant` and `decreases` (semantics 5.3; toolchain 1.7.1)
// ===========================================================================

const LOOPS: &str = r#"module pl.loops

// The first invariant is false before the first test of the condition; the
// second, which would fault there, is never evaluated.
fn inv_entry(n: Int) -> Int {
    let before = n * 2
    var i = n
    while i > 0
        invariant i < 3
        invariant 10 / i > 0
    {
        i = i - 1
    }
    i + before
}

fn inv_after(n: Int) -> Int
    requires n >= 0
{
    var i = 0
    var acc: List<Int> = []
    while i < n
        invariant acc.len == i
        invariant i <= 2
        decreases n - i
    {
        acc = acc.push(i * i)
        i = i + 1
    }
    acc.len
}

fn measure_negative(n: Int) -> Int {
    var i = 0
    while i < n
        decreases n - i - 2
    {
        i = i + 1
    }
    i
}

fn measure_equal(n: Int) -> Int {
    var i = 0
    var m = n
    while i < n
        decreases m
    {
        i = i + 1
        if i != 2 {
            m = m - 1
        }
    }
    i
}

fn measure_up(n: Int) -> Int {
    var i = 0
    while i < n
        decreases i
    {
        i = i + 1
    }
    i
}

fn measure_faults(n: Int) -> Int {
    var i = 0
    while i < 3
        decreases 10 / (n - i)
    {
        i = i + 1
    }
    i
}

// The measure is n - i while the loop runs and divides 0 by 0 at the exit.
fn measure_after_exit(n: Int) -> Int {
    var i = 0
    while i < n
        decreases (n - i) * ((n - i) / (n - i))
    {
        i = i + 1
    }
    i
}

fn cond_faults(n: Int) -> Int {
    var i = 0
    while 10 / (n - i) > 0
        invariant i >= 0
    {
        i = i + 1
    }
    i
}

fn no_params() -> Int {
    var i = 0
    while i < 5
        invariant i < 2
    {
        i = i + 1
    }
    i
}

fn nested(n: Int) -> Int {
    var total = 0
    var i = 0
    while i < n
        decreases n - i
    {
        let row = i * 10
        var j = 0
        while j < 3
            invariant total < 25
            decreases 3 - j
        {
            total = total + row + j
            j = j + 1
        }
        i = i + 1
    }
    total
}

fn return_in_loop(xs: List<Int>) -> Int
    ensures result != 99
{
    var i = 0
    while i < xs.len
        invariant i <= 1
        decreases xs.len - i
    {
        if xs.get(i) == Some(0) {
            return 99
        }
        i = i + 1
    }
    i
}

fn scoped(n: Int) -> Int {
    var x = 0
    if n > 0 {
        let gone = 1
        x = gone
    }
    let kept = x + 1
    var i = 0
    while i < n
        invariant i + kept < 3
    {
        let inner = i
        i = inner + 1
    }
    i
}

fn match_invariant(xs: List<Int>) -> Int {
    var i = 0
    while i < xs.len
        invariant (match xs.get(i) {
            Some(v) => v >= 0
            None => true
        })
        decreases xs.len - i
    {
        i = i + 1
    }
    i
}

// The invariants run before the first test of the condition, even when the
// condition is false at once (5.3).
fn inv_no_iterations(n: Int) -> Int {
    var i = 0
    while i > n
        invariant n < 0
    {
        i = i - 1
    }
    i
}

fn in_forall(xs: List<Int>) -> Bool {
    forall x in xs: if x > 0 {
        var i = 0
        while i < x
            invariant i < 2
        {
            i = i + 1
        }
        i == x
    } else {
        true
    }
}

test "a measure in a test body" {
    var i = 0
    while i < 3
        decreases 1 - i
    {
        i = i + 1
    }
}

property "a loop in a property body"
    for n in gen.range(3, 3)
{
    var i = 0
    while i < n
        invariant i < 2
    {
        i = i + 1
    }
}

test "a loop in a test body" {
    var i = 0
    while i < 3
        invariant i != 2
    {
        i = i + 1
    }
}
"#;

fn loop_fault(name: &str, args: &[Json]) -> Json {
    let p = common::check_source(LOOPS);
    flt(&p, &format!("pl.loops.{name}"), args)
}

#[test]
fn loop_invariant_false_at_entry_reports_iteration_zero_and_stops_at_the_first_false() {
    let d = loop_fault("inv_entry", &[json!(5)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["clause"], "i < 3");
    assert_eq!(d["symbol"], "pl.loops.inv_entry");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["status"], "failed");
    assert_eq!(d["iteration"], 0);
    assert_eq!(d["input"], json!({"n": 5}));
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 5, "before": 10, "i": 5}})
    );
    assert_eq!(d["span"], at(LOOPS, "invariant i < 3"));
    assert!(d.get("caller").is_none(), "{d}");
    assert_eq!(bt(&d), ["pl.loops.inv_entry"]);
}

#[test]
fn loop_invariant_that_faults_is_that_fault_of_the_invariant_clause_s34() {
    // n = 2: the first invariant holds every time, the second faults when
    // i reaches 0 after the second body; n = 0: it faults at entry.
    for n in [2, 0] {
        let d = loop_fault("inv_entry", &[json!(n)]);
        assert_eq!(d["code"], "division_by_zero", "n = {n}");
        assert_eq!(d["facet"], "contract");
        assert_eq!(d["clause"], "10 / i > 0");
        assert_eq!(d["symbol"], "pl.loops.inv_entry");
        assert_eq!(d["input"], json!({"a": 10, "b": 0}));
        assert!(
            d.get("iteration").is_none(),
            "a fault, not a violation: {d}"
        );
        assert!(d.get("clause_kind").is_none(), "{d}");
        assert_eq!(d["span"], at(LOOPS, "10 / i"));
    }
}

#[test]
fn loop_invariant_false_after_an_iteration_reports_the_completed_count() {
    let d = loop_fault("inv_after", &[json!(5)]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["clause"], "i <= 2");
    assert_eq!(d["iteration"], 3);
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 5, "i": 3, "acc": [0, 1, 4]}})
    );
    let p = common::check_source(LOOPS);
    assert_eq!(val(&p, "pl.loops.inv_after", &[json!(2)]), json!(2));
}

#[test]
fn loop_measure_negative_reports_the_measure_without_a_previous_value() {
    let d = loop_fault("measure_negative", &[json!(1)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "n - i - 2");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["iteration"], 0);
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 1, "i": 0}, "measure": -1})
    );
    assert_eq!(d["input"], json!({"n": 1}));
    assert_eq!(d["span"], at(LOOPS, "decreases n - i - 2"));
    // 1, 0, then -1: a negative measure after decreasing ones still reports
    // the measure alone (5.3: `if v < 0` comes before the comparison).
    let d = loop_fault("measure_negative", &[json!(3)]);
    assert_eq!(d["iteration"], 2);
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 3, "i": 2}, "measure": -1})
    );
}

#[test]
fn loop_measure_equal_or_greater_reports_previous_and_measure() {
    let d = loop_fault("measure_equal", &[json!(5)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "m");
    assert_eq!(d["iteration"], 2);
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 5, "i": 2, "m": 4}, "previous": 4, "measure": 4})
    );
    let d = loop_fault("measure_up", &[json!(2)]);
    assert_eq!(d["iteration"], 1);
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 2, "i": 1}, "previous": 0, "measure": 1})
    );
    // One iteration: a single measure is compared with nothing.
    let p = common::check_source(LOOPS);
    assert_eq!(val(&p, "pl.loops.measure_up", &[json!(1)]), json!(1));
}

#[test]
fn loop_measure_that_faults_is_that_fault_and_is_never_evaluated_after_the_exit() {
    let d = loop_fault("measure_faults", &[json!(1)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "10 / (n - i)");
    assert_eq!(d["symbol"], "pl.loops.measure_faults");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
    // The measure would divide by zero when the condition fails (i == n),
    // but it is evaluated only after the condition held (5.3).
    let p = common::check_source(LOOPS);
    assert_eq!(
        val(&p, "pl.loops.measure_after_exit", &[json!(3)]),
        json!(3)
    );
}

#[test]
fn loop_condition_is_no_contract_expression_its_fault_has_the_body_facet() {
    let d = loop_fault("cond_faults", &[json!(2)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "body");
    assert!(d.get("clause").is_none(), "{d}");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
}

#[test]
fn loop_violation_input_is_empty_for_a_function_without_parameters() {
    let d = loop_fault("no_params", &[]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["iteration"], 2);
    // 5.3: `input` is `{}` for a function without parameters.
    assert_eq!(d["input"], json!({}));
    assert_eq!(d["actual"], json!({"bindings": {"i": 2}}));
}

#[test]
fn loop_violation_in_a_test_body_has_no_input_and_blames_the_test() {
    let p = common::check_source(LOOPS);
    let rec = record(&p, "a loop in a test body");
    let d = record_fault(&rec);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["symbol"], r#"pl.loops."a loop in a test body""#);
    assert_eq!(d["iteration"], 2);
    // 5.3: absent for a loop in a test or property body.
    assert!(d.get("input").is_none(), "{d}");
    assert_eq!(d["actual"], json!({"bindings": {"i": 2}}));
    // The test's own clause is no counterexample against another body (S46).
    assert!(rec.get("counterexamples").is_none(), "{rec}");
}

#[test]
fn loop_invariant_runs_before_the_first_test_even_when_the_loop_never_iterates() {
    let d = loop_fault("inv_no_iterations", &[json!(5)]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["clause"], "n < 0");
    assert_eq!(d["iteration"], 0);
    assert_eq!(d["actual"], json!({"bindings": {"n": 5, "i": 0}}));
    let p = common::check_source(LOOPS);
    assert_eq!(
        val(&p, "pl.loops.inv_no_iterations", &[json!(-2)]),
        json!(-2)
    );
}

#[test]
fn loop_bindings_inside_a_forall_predicate_include_its_binder() {
    let d = loop_fault("in_forall", &[json!([1, 5])]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["iteration"], 2);
    assert_eq!(
        d["actual"],
        json!({"bindings": {"xs": [1, 5], "x": 5, "i": 2}})
    );
    let p = common::check_source(LOOPS);
    assert_eq!(
        val(&p, "pl.loops.in_forall", &[json!([1, -3, 0])]),
        json!(true)
    );
}

#[test]
fn loop_measure_violation_in_a_test_body_has_no_input() {
    let p = common::check_source(LOOPS);
    let d = record_fault(&record(&p, "a measure in a test body"));
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["symbol"], r#"pl.loops."a measure in a test body""#);
    assert_eq!(d["iteration"], 2);
    assert!(d.get("input").is_none(), "{d}");
    assert_eq!(d["actual"], json!({"bindings": {"i": 2}, "measure": -1}));
}

#[test]
fn loop_nested_inner_iterations_restart_and_see_the_outer_bindings() {
    let d = loop_fault("nested", &[json!(2)]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["clause"], "total < 25");
    // The second execution of the inner loop: its count and its measure
    // history restart (the measure 3 is not compared with the earlier 1).
    assert_eq!(d["iteration"], 3);
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 2, "total": 36, "i": 1, "row": 10, "j": 3}})
    );
    let p = common::check_source(LOOPS);
    assert_eq!(val(&p, "pl.loops.nested", &[json!(1)]), json!(3));
}

#[test]
fn loop_return_skips_the_invariant_and_runs_the_ensures() {
    let p = common::check_source(LOOPS);
    for xs in [json!([0, 5, 5]), json!([5, 0])] {
        let d = flt(&p, "pl.loops.return_in_loop", std::slice::from_ref(&xs));
        assert_eq!(d["clause_kind"], "ensures", "{xs}");
        assert_eq!(d["clause"], "result != 99");
        assert_eq!(d["actual"], 99);
        assert_eq!(d["input"], json!({ "xs": xs }));
    }
    let d = flt(&p, "pl.loops.return_in_loop", &[json!([5, 5, 5])]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["iteration"], 2);
    assert_eq!(val(&p, "pl.loops.return_in_loop", &[json!([7])]), json!(1));
}

#[test]
fn loop_bindings_are_exactly_those_visible_at_the_loop() {
    // `gone` left scope with its block, `inner` is the body's own binding.
    let d = loop_fault("scoped", &[json!(4)]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["iteration"], 1);
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 4, "x": 1, "kept": 2, "i": 1}})
    );
}

#[test]
fn loop_clauses_do_not_run_in_unchecked_mode() {
    let p = common::check_source(LOOPS);
    let u = unchecked();
    assert_eq!(
        val_with(&p, "pl.loops.inv_entry", &[json!(5)], &u),
        json!(10)
    );
    assert_eq!(
        val_with(&p, "pl.loops.measure_equal", &[json!(5)], &u),
        json!(5)
    );
    assert_eq!(
        val_with(&p, "pl.loops.measure_faults", &[json!(1)], &u),
        json!(3)
    );
    assert_eq!(val_with(&p, "pl.loops.no_params", &[], &u), json!(5));
}

#[test]
fn loop_bindings_in_a_property_body_include_its_for_binders_and_no_input() {
    let p = common::check_source(LOOPS);
    let rec = record(&p, "a loop in a property body");
    let d = record_fault(&rec);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["symbol"], r#"pl.loops."a loop in a property body""#);
    assert_eq!(d["iteration"], 2);
    assert!(d.get("input").is_none(), "{d}");
    assert_eq!(d["actual"], json!({"bindings": {"n": 3, "i": 2}}));
    assert_eq!(rec["shrunk"]["input"], json!({"n": 3}));
}

#[test]
fn loop_and_header_clauses_with_blocks_report_one_line_of_clause_text_t56() {
    let d = loop_fault("match_invariant", &[json!([1, -2, 3])]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(
        d["clause"],
        "(match xs.get(i) { Some(v) => v >= 0 None => true })"
    );
    assert_eq!(d["iteration"], 1);
    assert_eq!(d["actual"], json!({"bindings": {"xs": [1, -2, 3], "i": 1}}));
    let p = common::check_source(RECURSION);
    let d = flt(&p, "pl.recursion.iffy", &[json!(1)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "if n > 5 { n } else { n - 10 }");
    assert_eq!(d["actual"], json!({"measure": -9}));
}

// ===========================================================================
// J. Header `decreases` (semantics 5.4, S35; toolchain 1.7.1)
// ===========================================================================

const RECURSION: &str = r#"module pl.recursion

fn count(n: Int) -> Int
    requires n >= 0
    decreases n
{
    if n == 0 { 0 } else { 1 + count(n - 1) }
}

fn stays(n: Int) -> Int
    decreases n
{
    if n <= 0 { 0 } else { stays(n) }
}

fn grows(n: Int) -> Int
    decreases n
{
    if n > 3 { n } else { grows(n + 1) }
}

fn negative(n: Int) -> Int
    decreases n
{
    if n < -5 { 0 } else { negative(n - 1) }
}

fn ping(n: Int) -> Int
    decreases n
{
    if n == 0 { 0 } else { pong(n) }
}

fn pong(n: Int) -> Int {
    ping(n)
}

fn ping_ok(n: Int) -> Int
    decreases n
{
    if n == 0 { 0 } else { pong_ok(n) }
}

fn pong_ok(n: Int) -> Int {
    ping_ok(n - 1) + 1
}

fn guarded(a: Int, b: Int) -> Int
    requires b != 0
    decreases a / b
{
    if a / b <= 0 { 0 } else { 1 + guarded(a - b, b) }
}

fn measure_faults(n: Int) -> Int
    decreases 10 / n
{
    if n == 0 { 0 } else { measure_faults(n - 1) }
}

// Not recursive (the warning unused_decreases), yet its measure is still
// evaluated on entry (S35 (a)).
fn once(n: Int) -> Int
    decreases n
{
    n
}

// The requires clause re-enters `deep` through `helper` (S33 detects only
// direct self-calls): the nearest activation has no measure yet, so no
// comparison is made (S35).
fn deep(n: Int) -> Int
    requires n == 0 or helper(n - 1) >= 0
    decreases n
{
    n
}

fn helper(m: Int) -> Int {
    deep(m)
}

fn iffy(n: Int) -> Int
    decreases if n > 5 { n } else { n - 10 }
{
    if n == 0 { 0 } else { iffy(n - 1) }
}

// The ensures of an activation calls back into the function: the activation
// is still on the stack with its measure recorded (S35).
fn settles(n: Int) -> Int
    ensures n == 0 or relay_down(n) >= 0
    decreases n
{
    n
}

fn relay_down(n: Int) -> Int {
    settles(n - 1)
}

fn loops_back(n: Int) -> Int
    ensures n == 0 or relay_same(n) >= 0
    decreases n
{
    n
}

fn relay_same(n: Int) -> Int {
    loops_back(n)
}
"#;

#[test]
fn header_measure_holds_on_a_strictly_decreasing_recursion() {
    let p = common::check_source(RECURSION);
    assert_eq!(val(&p, "pl.recursion.count", &[json!(5)]), json!(5));
    assert_eq!(val(&p, "pl.recursion.ping_ok", &[json!(3)]), json!(3));
    assert_eq!(
        val(&p, "pl.recursion.guarded", &[json!(6), json!(2)]),
        json!(3)
    );
    assert_eq!(val(&p, "pl.recursion.deep", &[json!(3)]), json!(3));
}

#[test]
fn header_measure_not_decreasing_is_reported_at_the_reentry_with_previous_s35() {
    let p = common::check_source(RECURSION);
    let d = flt(&p, "pl.recursion.stays", &[json!(2)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "n");
    assert_eq!(d["symbol"], "pl.recursion.stays");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "body");
    // A header measure carries no bindings (toolchain 1.7.1) and `input`.
    assert_eq!(d["actual"], json!({"previous": 2, "measure": 2}));
    assert_eq!(d["input"], json!({"n": 2}));
    assert!(d.get("iteration").is_none(), "{d}");
    assert_eq!(d["span"], at_after(RECURSION, "fn stays(", "decreases n"));
    assert_eq!(bt(&d), ["pl.recursion.stays", "pl.recursion.stays"]);
    assert_eq!(d["backtrace"][0]["span"], at(RECURSION, "stays(n)"));
    let d = flt(&p, "pl.recursion.grows", &[json!(2)]);
    assert_eq!(d["actual"], json!({"previous": 2, "measure": 3}));
    assert_eq!(d["input"], json!({"n": 3}));
}

#[test]
fn header_measure_negative_reports_the_measure_alone_s35() {
    let p = common::check_source(RECURSION);
    let d = flt(&p, "pl.recursion.negative", &[json!(1)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["actual"], json!({"measure": -1}));
    assert_eq!(d["input"], json!({"n": -1}));
    assert_eq!(
        bt(&d),
        [
            "pl.recursion.negative",
            "pl.recursion.negative",
            "pl.recursion.negative"
        ]
    );
    // Negative at the first entry.
    let d = flt(&p, "pl.recursion.negative", &[json!(-3)]);
    assert_eq!(d["actual"], json!({"measure": -3}));
    assert_eq!(bt(&d), ["pl.recursion.negative"]);
}

#[test]
fn header_measure_is_compared_through_frames_of_other_functions_s35() {
    let p = common::check_source(RECURSION);
    let d = flt(&p, "pl.recursion.ping", &[json!(2)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["symbol"], "pl.recursion.ping");
    assert_eq!(d["actual"], json!({"previous": 2, "measure": 2}));
    assert_eq!(
        bt(&d),
        [
            "pl.recursion.ping",
            "pl.recursion.pong",
            "pl.recursion.ping"
        ]
    );
}

#[test]
fn header_measure_runs_after_the_requires_and_its_fault_is_the_clauses_s34() {
    let p = common::check_source(RECURSION);
    // `decreases a / b` under `requires b != 0`: the requires fails first.
    let d = flt(&p, "pl.recursion.guarded", &[json!(6), json!(0)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    let d = flt(&p, "pl.recursion.measure_faults", &[json!(1)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "10 / n");
    assert_eq!(d["symbol"], "pl.recursion.measure_faults");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
    assert_eq!(
        bt(&d),
        ["pl.recursion.measure_faults", "pl.recursion.measure_faults"]
    );
    // Unchecked mode evaluates no measure.
    assert_eq!(
        val_with(&p, "pl.recursion.measure_faults", &[json!(1)], &unchecked()),
        json!(0)
    );
}

#[test]
fn header_measure_of_a_non_recursive_function_is_still_checked_on_entry_s35() {
    let p = common::check_source(RECURSION);
    assert_eq!(val(&p, "pl.recursion.once", &[json!(0)]), json!(0));
    let d = flt(&p, "pl.recursion.once", &[json!(-1)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["actual"], json!({"measure": -1}));
    assert_eq!(d["input"], json!({"n": -1}));
}

#[test]
fn header_measure_is_compared_with_an_activation_evaluating_its_ensures_s35_s70() {
    let p = common::check_source(RECURSION);
    assert_eq!(val(&p, "pl.recursion.settles", &[json!(3)]), json!(3));
    // loops_back(2)'s ensures re-enters loops_back(2): the measure 2 is not
    // below the recorded 2. The violation is raised by the inner entry and
    // claimed by the outer activation's ensures clause, whose evaluation
    // called the function (S70).
    let d = flt(&p, "pl.recursion.loops_back", &[json!(2)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "pl.recursion.loops_back");
    assert_eq!(d["clause"], "n == 0 or relay_same(n) >= 0");
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["actual"], json!({"previous": 2, "measure": 2}));
    assert_eq!(d["input"], json!({"n": 2}));
    assert_eq!(
        d["span"],
        at_after(RECURSION, "fn loops_back(", "decreases n")
    );
    assert_eq!(
        bt(&d),
        [
            "pl.recursion.loops_back",
            "pl.recursion.relay_same",
            "pl.recursion.loops_back"
        ]
    );
}

#[test]
fn an_interpreter_is_clean_after_a_fault_and_later_calls_see_no_stale_activation() {
    let p = common::check_source(RECURSION);
    let mut it = Interpreter::new(&p, EvalConfig::default());
    let stays = common::def(&p, "pl.recursion.stays");
    let count = common::def(&p, "pl.recursion.count");
    // A fault two activations deep.
    let fault = it.call(stays, vec![Value::int(2)], vec![]).unwrap_err();
    assert_eq!(bt(&common::fault_json(&p, &fault)).len(), 2);
    assert!(it.frames().is_empty(), "no activation survives a fault");
    assert_eq!(it.current_def(), None);
    // A top-level requires violation has no caller: no stale frame.
    let d = common::fault_json(
        &p,
        &it.call(count, vec![Value::int(-1)], vec![]).unwrap_err(),
    );
    assert_eq!(d["clause_kind"], "requires");
    assert!(d.get("caller").is_none(), "{d}");
    assert_eq!(bt(&d), ["pl.recursion.count"]);
    // A header measure is not compared with the faulted activations.
    assert_eq!(
        it.call(stays, vec![Value::int(0)], vec![]).unwrap(),
        Value::int(0)
    );
    assert_eq!(
        it.call(count, vec![Value::int(3)], vec![]).unwrap(),
        Value::int(3)
    );
    assert!(it.frames().is_empty());
}

// ===========================================================================
// K. `forall`, early `return`, shadowing, and statements (semantics 2.3,
//    3.10, 3.11, 5.2, 6.2; grammar D24)
// ===========================================================================

const FLOW: &str = r#"module pl.flow

use std.text.{trim}

fn helper() -> Int {
    7
}

// Locals may hide module-level and universe names (semantics 2.3).
fn hidden() -> Int {
    let trim = 3
    let helper = 4
    let flatten = 5
    let std = 6
    trim + helper + flatten + std
}

// Sibling scopes reuse a name (S8).
fn siblings(c: Bool) -> Int {
    var total = 0
    if c {
        let y = 1
        total = total + y
    } else {
        let y = 2
        total = total + y
    }
    while total < 5
        decreases 5 - total
    {
        let y = 3
        total = total + y
    }
    match Some(total) {
        Some(y) => y * 100
        None => 0
    }
}

fn find(xss: List<List<Int>>, v: Int) -> Option<Int>
    ensures (match result {
        Some(k) => k >= 0
        None => true
    })
{
    var i = 0
    while i < xss.len
        decreases xss.len - i
    {
        let row = xss.get(i).unwrap_or([])
        var j = 0
        while j < row.len
            decreases row.len - j
        {
            match row.get(j) {
                Some(x) => {
                    if x == v {
                        return Some(i * 100 + j)
                    }
                }
                None => {}
            }
            j = j + 1
        }
        i = i + 1
    }
    None
}

fn unit_return(n: Int) -> Unit
    ensures n != 13
{
    if n > 10 {
        return
    }
    let _ = n
}

fn unit_statement(n: Int) -> Int {
    unit_return(n)
    n + 1
}

// `let _ = e` still evaluates `e` (D24).
fn discard(n: Int) -> Int {
    let _ = 10 / n
    n
}

fn half(n: Int) -> Result<Int, Text> {
    if n % 2 == 0 { Ok(n / 2) } else { Err("odd") }
}

// A `?` inside a `forall` predicate exits the function.
fn halves_positive(xs: List<Int>) -> Result<Bool, Text> {
    Ok(forall x in xs: half(x)? > 0)
}

fn block_values(c: Bool) -> List<Int> {
    let v = if c {
        let a = 1
        a + 1
    } else {
        0
    }
    let w = match c {
        true => {
            let b = v * 10
            b + 1
        }
        false => -1
    }
    [v, w]
}

fn unreachable_after_return(n: Int) -> Int {
    return n
    let _ = 1 / 0
    n + 1
}

fn all_arms_return(o: Option<Int>) -> Int {
    match o {
        Some(n) => {
            return n
        }
        None => {
            return 0
        }
    }
}

@prove
fn proved(n: Int) -> Int
    requires n > 0
    ensures result > 0
{
    n - 1
}

// A user function named like a std.text function; the std one is reached
// through its qualified path.
fn split(t: Text) -> Int {
    t.len
}

fn both_splits(t: Text) -> List<Int> {
    [split(t), std.text.split(t, ",").len]
}

fn and_or(n: Int) -> List<Bool> {
    [n != 0 and 10 / n > 1, n == 0 or 10 / n > 1, not (n == 0) and 10 % n == 0]
}
"#;

#[test]
fn locals_hide_module_level_and_universe_names_2_3() {
    let p = common::check_source(FLOW);
    assert_eq!(val(&p, "pl.flow.hidden", &[]), json!(18));
    assert_eq!(val(&p, "pl.flow.helper", &[]), json!(7));
}

#[test]
fn sibling_scopes_reuse_names_without_leaking_values_s8() {
    let p = common::check_source(FLOW);
    assert_eq!(val(&p, "pl.flow.siblings", &[json!(true)]), json!(700));
    assert_eq!(val(&p, "pl.flow.siblings", &[json!(false)]), json!(500));
}

#[test]
fn early_return_from_nested_loops_and_match_arms_runs_the_ensures_5_2() {
    let p = common::check_source(FLOW);
    assert_eq!(
        val(&p, "pl.flow.find", &[json!([[1, 2], [3, 4]]), json!(4)]),
        some(json!(101))
    );
    assert_eq!(val(&p, "pl.flow.find", &[json!([[1]]), json!(9)]), none());
    assert_eq!(val(&p, "pl.flow.find", &[json!([]), json!(9)]), none());
}

#[test]
fn a_bare_return_exits_with_unit_and_the_ensures_sees_it_5_2() {
    let p = common::check_source(FLOW);
    assert_eq!(val(&p, "pl.flow.unit_return", &[json!(5)]), unit());
    assert_eq!(val(&p, "pl.flow.unit_return", &[json!(11)]), unit());
    let d = flt(&p, "pl.flow.unit_return", &[json!(13)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "n != 13");
    assert_eq!(d["actual"], unit());
    assert_eq!(d["input"], json!({"n": 13}));
    // A `Unit` expression statement is evaluated for its faults.
    let d = flt(&p, "pl.flow.unit_statement", &[json!(13)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(bt(&d), ["pl.flow.unit_return", "pl.flow.unit_statement"]);
    assert_eq!(val(&p, "pl.flow.unit_statement", &[json!(1)]), json!(2));
}

#[test]
fn a_discarded_value_is_still_evaluated_d24() {
    let p = common::check_source(FLOW);
    assert_eq!(val(&p, "pl.flow.discard", &[json!(5)]), json!(5));
    let d = flt(&p, "pl.flow.discard", &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
}

#[test]
fn a_question_mark_inside_forall_exits_and_forall_stops_at_the_first_false() {
    let p = common::check_source(FLOW);
    let f = "pl.flow.halves_positive";
    assert_eq!(val(&p, f, &[json!([2, 4])]), ok(json!(true)));
    assert_eq!(val(&p, f, &[json!([2, 3])]), err(json!("odd")));
    // half(0) is Ok(0) and 0 > 0 is false: the 3 is never examined.
    assert_eq!(val(&p, f, &[json!([0, 3])]), ok(json!(false)));
    assert_eq!(val(&p, f, &[json!([])]), ok(json!(true)));
}

#[test]
fn blocks_with_lets_are_values_of_if_and_match() {
    let p = common::check_source(FLOW);
    assert_eq!(
        val(&p, "pl.flow.block_values", &[json!(true)]),
        json!([2, 21])
    );
    assert_eq!(
        val(&p, "pl.flow.block_values", &[json!(false)]),
        json!([0, -1])
    );
}

#[test]
fn and_or_evaluate_their_right_operand_only_when_needed() {
    let p = common::check_source(FLOW);
    assert_eq!(
        val(&p, "pl.flow.and_or", &[json!(0)]),
        json!([false, true, false])
    );
    assert_eq!(
        val(&p, "pl.flow.and_or", &[json!(5)]),
        json!([true, true, true])
    );
    assert_eq!(
        val(&p, "pl.flow.and_or", &[json!(20)]),
        json!([false, false, false])
    );
}

#[test]
fn statements_after_a_return_never_run_and_diverging_arms_satisfy_the_return_type() {
    let p = common::check_source(FLOW);
    assert_eq!(
        val(&p, "pl.flow.unreachable_after_return", &[json!(4)]),
        json!(4)
    );
    assert_eq!(
        val(&p, "pl.flow.all_arms_return", &[some(json!(4))]),
        json!(4)
    );
    assert_eq!(val(&p, "pl.flow.all_arms_return", &[none()]), json!(0));
}

#[test]
fn a_prove_function_is_checked_at_runtime_like_any_other_s36() {
    let p = common::check_source(FLOW);
    assert_eq!(val(&p, "pl.flow.proved", &[json!(5)]), json!(4));
    let d = flt(&p, "pl.flow.proved", &[json!(1)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "result > 0");
    assert_eq!(d["actual"], 0);
    let d = flt(&p, "pl.flow.proved", &[json!(0)]);
    assert_eq!(d["clause_kind"], "requires");
}

#[test]
fn a_user_function_beside_a_std_function_of_the_same_name() {
    let p = common::check_source(FLOW);
    assert_eq!(
        val(&p, "pl.flow.both_splits", &[json!("a,b,c")]),
        json!([5, 3])
    );
}

// ===========================================================================
// L. The contract rows of toolchain 1.7.1 from source, with exact payloads
//    (toolchain 1.7.1, 1.7.3, T56; semantics 5.2, 5.6, 6.7, 6.10, 7.1, 9.2,
//    S34, S41, S46, S57, S58, S70)
// ===========================================================================

const CONTRACTS: &str = r#"module pl.contracts

type Even = Int where is_even(self)
type Taken = Int where [1].take(self) == [1]
type Checked = Int where bad_id(self) == self

pub struct Account {
    owner: Text
    balance: Int
}

fn is_even(n: Int) -> Bool
    requires n >= 0
{
    n % 2 == 0
}

fn bad_id(n: Int) -> Int
    ensures result == n
{
    n + 1
}

fn recip(n: Int) -> Int
    requires n != 0
{
    100 / n
}

fn calls_recip(n: Int) -> Int {
    recip(n) + 1
}

fn post(n: Int) -> Int
    ensures recip(result) > 0
{
    n
}

fn pre(n: Int) -> Int
    requires bad_id(n) == n
{
    n
}

fn to_even(n: Int) -> Result<Even, ConstraintError> {
    check<Even>(n)
}

fn to_taken(n: Int) -> Result<Taken, ConstraintError> {
    check<Taken>(n)
}

fn to_checked(n: Int) -> Result<Checked, ConstraintError> {
    check<Checked>(n)
}

fn take_even(e: Even) -> Int {
    e
}

fn clause_text(n: Int, t: Text, xs: List<Int>) -> Int
    requires   n>=0
    requires ((n)) > 0
    requires n != 1_000
    requires t != "a\u{9}b"
    requires t != "\u{e9}"
    requires t != "\u{1}\u{7f}"
    requires not(n == 5)
    requires (if n > 3 { true } else { false })
    requires xs != [
        1,
        2,
    ]
    requires (n > 6
        and n < 100)
    requires forall x in xs:x>0
{
    n
}

fn asserting(n: Int, xs: List<Int>) -> Int {
    var total = 0
    let first = xs.get(0)
    match first {
        Some(head) => {
            total = head
            assert  total+n==  (if n > 0 {
                1
            } else {
                2
            })
        }
        None => {}
    }
    total
}

fn unfinished(n: Int) -> Bool {
    ?decide
}

fn uses_unfinished(n: Int) -> Int
    requires unfinished(n)
{
    n
}

fn checked_positive(n: Int) -> Bool {
    assert n > 0
    true
}

fn uses_assert(n: Int) -> Int
    ensures checked_positive(result)
{
    n
}

fn take_n(xs: List<Int>, n: Int) -> List<Int> {
    xs.take(n)
}

fn deposit(a: Account, amount: Int) -> Account
    requires amount >= 0
    ensures result.balance == a.balance + amount
{
    Account { ..a, balance: a.balance + amount + 1 }
}

fn ratio(a: Int, b: Int) -> Int
    requires b != 0 and a / b > 1
{
    a / b
}

test "a test calls recip with zero" {
    let z = 0
    assert recip(z) > 0
}

property "a property calls recip"
    for n in gen.int
{
    assert recip(n) >= -100
}

test "a test finds a broken ensures" {
    assert bad_id(1) == 2
}

test "a test calls pre" {
    assert pre(3) == 3
}

test "a test takes minus one" {
    let xs = [1, 2]
    assert xs.take(-1) == []
}
"#;

#[test]
fn requires_from_a_function_names_it_as_caller_with_the_call_span() {
    let p = common::check_source(CONTRACTS);
    let d = flt(&p, "pl.contracts.calls_recip", &[json!(0)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["category"], "contract");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "pl.contracts.recip");
    assert_eq!(d["qualified_name"], "pl.contracts.recip");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["caller"], "pl.contracts.calls_recip");
    assert_eq!(d["clause"], "n != 0");
    assert_eq!(d["input"], json!({"n": 0}));
    assert_eq!(d["status"], "failed");
    assert!(d.get("actual").is_none(), "{d}");
    assert_eq!(d["span"], at(CONTRACTS, "requires n != 0"));
    assert_eq!(bt(&d), ["pl.contracts.recip", "pl.contracts.calls_recip"]);
    assert_eq!(
        d["backtrace"][0]["span"],
        at_after(CONTRACTS, "fn calls_recip(", "recip(n)")
    );
    assert!(d["backtrace"][1].get("span").is_none(), "{d}");
    // From the command line there is no caller (S39 (c)).
    let d = flt(&p, "pl.contracts.recip", &[json!(0)]);
    assert!(d.get("caller").is_none(), "{d}");
    assert_eq!(bt(&d), ["pl.contracts.recip"]);
}

#[test]
fn requires_from_a_test_or_a_property_names_it_as_caller() {
    let p = common::check_source(CONTRACTS);
    let test = r#"pl.contracts."a test calls recip with zero""#;
    let rec = record(&p, "a test calls recip with zero");
    let d = record_fault(&rec);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "pl.contracts.recip");
    assert_eq!(d["caller"], test);
    assert_eq!(d["input"], json!({"n": 0}));
    assert_eq!(bt(&d), ["pl.contracts.recip", test]);
    assert_eq!(d["backtrace"][0]["span"], at(CONTRACTS, "recip(z)"));
    // A requires violation is no counterexample (S46).
    assert!(rec.get("counterexamples").is_none(), "{rec}");
    let property = r#"pl.contracts."a property calls recip""#;
    let rec = record(&p, "a property calls recip");
    let d = record_fault(&rec);
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["caller"], property);
    assert_eq!(d["input"], json!({"n": 0}));
    assert_eq!(rec["shrunk"]["input"], json!({"n": 0}));
    assert_eq!(bt(&d), ["pl.contracts.recip", property]);
    assert!(rec.get("counterexamples").is_none(), "{rec}");
}

#[test]
fn requires_from_a_refinement_predicate_names_the_refinement_and_the_predicate_claims_it_s70() {
    let p = common::check_source(CONTRACTS);
    // Through check<Even>(n) in a function body.
    let d = flt(&p, "pl.contracts.to_even", &[json!(-2)]);
    assert_eq!(d["code"], "contract_violation");
    // S34/S70 rewrite symbol, facet, and clause to the predicate's...
    assert_eq!(d["symbol"], "pl.contracts.Even");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "is_even(self)");
    // ...and keep the violation as raised: kind, blame, caller, input, span.
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["caller"], "pl.contracts.Even");
    assert_eq!(d["input"], json!({"n": -2}));
    assert_eq!(d["span"], at(CONTRACTS, "requires n >= 0"));
    assert_eq!(
        bt(&d),
        [
            "pl.contracts.is_even",
            "pl.contracts.Even",
            "pl.contracts.to_even"
        ]
    );
    assert_eq!(d["backtrace"][0]["span"], at(CONTRACTS, "is_even(self)"));
    assert_eq!(d["backtrace"][1]["span"], at(CONTRACTS, "check<Even>(n)"));
    // At the run boundary: the argument's predicate runs before any call
    // of the function (S39 (a)); a fault there is reported per S34.
    let d = flt(&p, "pl.contracts.take_even", &[json!(-2)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "pl.contracts.Even");
    assert_eq!(d["clause"], "is_even(self)");
    assert_eq!(d["caller"], "pl.contracts.Even");
    assert_eq!(bt(&d), ["pl.contracts.is_even", "pl.contracts.Even"]);
    assert_eq!(val(&p, "pl.contracts.take_even", &[json!(4)]), json!(4));
}

#[test]
fn builtin_requires_in_a_test_body_names_the_test_as_caller_6_10() {
    let p = common::check_source(CONTRACTS);
    let test = r#"pl.contracts."a test takes minus one""#;
    let rec = record(&p, "a test takes minus one");
    let d = record_fault(&rec);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "std.prelude.List.take");
    assert_eq!(d["caller"], test);
    assert_eq!(d["input"], json!({"self": [1, 2], "n": -1}));
    assert_eq!(bt(&d), ["std.prelude.List.take", test]);
    assert_eq!(d["backtrace"][0]["span"], at(CONTRACTS, "xs.take(-1)"));
    assert!(rec.get("counterexamples").is_none(), "{rec}");
}

#[test]
fn builtin_requires_inside_a_refinement_predicate_names_the_refinement_as_caller_6_10() {
    let p = common::check_source(CONTRACTS);
    let d = flt(&p, "pl.contracts.to_taken", &[json!(-1)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "pl.contracts.Taken");
    assert_eq!(d["clause"], "[1].take(self) == [1]");
    assert_eq!(d["caller"], "pl.contracts.Taken");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
    assert_eq!(d["span"]["file"], "std/prelude.cpl");
    assert_eq!(
        bt(&d),
        [
            "std.prelude.List.take",
            "pl.contracts.Taken",
            "pl.contracts.to_taken"
        ]
    );
    assert_eq!(val(&p, "pl.contracts.to_taken", &[json!(1)]), ok(json!(1)));
}

#[test]
fn nested_violations_are_claimed_by_the_clause_whose_evaluation_called_the_callee_s70() {
    let p = common::check_source(CONTRACTS);
    // A callee's requires violated inside an ensures clause.
    let d = flt(&p, "pl.contracts.post", &[json!(0)]);
    assert_eq!(d["symbol"], "pl.contracts.post");
    assert_eq!(d["clause"], "recip(result) > 0");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["caller"], "pl.contracts.post");
    assert_eq!(d["input"], json!({"n": 0}));
    assert_eq!(d["span"], at(CONTRACTS, "requires n != 0"));
    assert_eq!(bt(&d), ["pl.contracts.recip", "pl.contracts.post"]);
    // A callee's ensures violated inside a requires clause.
    let d = flt(&p, "pl.contracts.pre", &[json!(3)]);
    assert_eq!(d["symbol"], "pl.contracts.pre");
    assert_eq!(d["clause"], "bad_id(n) == n");
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["input"], json!({"n": 3}));
    assert_eq!(d["actual"], json!(4));
    assert_eq!(d["span"], at(CONTRACTS, "ensures result == n"));
    // A callee's ensures violated inside a refinement predicate.
    let d = flt(&p, "pl.contracts.to_checked", &[json!(5)]);
    assert_eq!(d["symbol"], "pl.contracts.Checked");
    assert_eq!(d["clause"], "bad_id(self) == self");
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["input"], json!({"n": 5}));
    assert_eq!(d["actual"], json!(6));
    assert_eq!(
        bt(&d),
        [
            "pl.contracts.bad_id",
            "pl.contracts.Checked",
            "pl.contracts.to_checked"
        ]
    );
}

#[test]
fn a_hole_or_an_assertion_reached_from_a_clause_is_reported_per_s34() {
    let p = common::check_source(CONTRACTS);
    let d = flt(&p, "pl.contracts.uses_unfinished", &[json!(1)]);
    assert_eq!(d["code"], "unresolved_hole");
    assert_eq!(d["symbol"], "pl.contracts.uses_unfinished");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "unfinished(n)");
    // The hole's own fields describe the hole as reached.
    assert_eq!(d["hole"], "decide");
    assert_eq!(d["expected_type"], "Bool");
    assert_eq!(
        d["bindings"],
        json!([{"name": "n", "type": "Int", "kind": "parameter", "value": 1}])
    );
    assert_eq!(d["span"], at(CONTRACTS, "?decide"));
    assert_eq!(
        bt(&d),
        ["pl.contracts.unfinished", "pl.contracts.uses_unfinished"]
    );
    let d = flt(&p, "pl.contracts.uses_assert", &[json!(0)]);
    assert_eq!(d["code"], "assertion_failed");
    assert_eq!(d["symbol"], "pl.contracts.uses_assert");
    assert_eq!(d["facet"], "contract");
    // S34 sets `clause` to the contract clause's text.
    assert_eq!(d["clause"], "checked_positive(result)");
    assert_eq!(
        d["bindings"],
        json!([{"name": "n", "type": "Int", "kind": "parameter", "value": 0}])
    );
    assert_eq!(val(&p, "pl.contracts.uses_assert", &[json!(2)]), json!(2));
}

#[test]
fn clause_text_is_the_canonical_one_line_rendering_t56_9_2() {
    let p = common::check_source(CONTRACTS);
    let rows = [
        (json!(-1), "x", json!([3]), "n >= 0"),
        (json!(0), "x", json!([3]), "((n)) > 0"),
        (json!(1000), "x", json!([3]), "n != 1000"),
        (json!(1), "a\tb", json!([3]), r#"t != "a\tb""#),
        (json!(1), "\u{e9}", json!([3]), "t != \"\u{e9}\""),
        (json!(1), "\u{1}\u{7f}", json!([3]), r#"t != "\u{1}\u{7f}""#),
        (json!(5), "x", json!([3]), "not (n == 5)"),
        (
            json!(2),
            "x",
            json!([3]),
            "(if n > 3 { true } else { false })",
        ),
        (json!(7), "x", json!([1, 2]), "xs != [1, 2]"),
        (json!(4), "x", json!([3]), "(n > 6 and n < 100)"),
        (json!(7), "x", json!([0]), "forall x in xs: x > 0"),
    ];
    for (n, t, xs, clause) in rows {
        let d = flt(
            &p,
            "pl.contracts.clause_text",
            &[n.clone(), json!(t), xs.clone()],
        );
        assert_eq!(d["clause"], clause, "at n = {n}, t = {t:?}, xs = {xs}");
        assert_eq!(d["clause_kind"], "requires");
    }
    assert_eq!(
        val(
            &p,
            "pl.contracts.clause_text",
            &[json!(7), json!("x"), json!([3])]
        ),
        json!(7)
    );
    // The span of a clause runs from its keyword (toolchain 3.3).
    let d = flt(
        &p,
        "pl.contracts.clause_text",
        &[json!(-1), json!("x"), json!([3])],
    );
    assert_eq!(d["span"], at(CONTRACTS, "requires   n>=0"));
}

#[test]
fn assertion_failed_carries_the_canonical_expression_and_every_binding_innermost_first_s41() {
    let p = common::check_source(CONTRACTS);
    let d = flt(&p, "pl.contracts.asserting", &[json!(3), json!([5])]);
    assert_eq!(d["code"], "assertion_failed");
    assert_eq!(d["category"], "test");
    assert_eq!(d["symbol"], "pl.contracts.asserting");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["clause"], "total + n == (if n > 0 { 1 } else { 2 })");
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "head", "type": "Int", "kind": "pattern", "value": 5},
            {"name": "first", "type": "Option<Int>", "kind": "let", "value": some(json!(5))},
            {"name": "total", "type": "Int", "kind": "var", "mutable": true, "value": 5},
            {"name": "xs", "type": "List<Int>", "kind": "parameter", "value": [5]},
            {"name": "n", "type": "Int", "kind": "parameter", "value": 3}
        ])
    );
    // The span starts at the asserted expression.
    let start = at(CONTRACTS, "total+n==");
    assert_eq!(
        (d["span"]["line"].clone(), d["span"]["col"].clone()),
        (start["line"].clone(), start["col"].clone())
    );
    assert_eq!(bt(&d), ["pl.contracts.asserting"]);
    assert_eq!(
        val(&p, "pl.contracts.asserting", &[json!(-4), json!([6])]),
        json!(6)
    );
    assert_eq!(
        val(&p, "pl.contracts.asserting", &[json!(3), json!([])]),
        json!(0)
    );
}

#[test]
fn builtin_preconditions_are_checked_in_unchecked_mode_s58() {
    let p = common::check_source(CONTRACTS);
    let d = flt_with(
        &p,
        "pl.contracts.take_n",
        &[json!([1]), json!(-1)],
        &unchecked(),
    );
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "std.prelude.List.take");
    assert_eq!(d["caller"], "pl.contracts.take_n");
    // A user function's requires is not checked in unchecked mode.
    let d = flt_with(&p, "pl.contracts.recip", &[json!(0)], &unchecked());
    assert_eq!(d["code"], "division_by_zero");
    // check<R> is no contract and still runs its predicate, but the requires
    // of the function the predicate calls does not (toolchain 1.7).
    assert_eq!(
        val_with(&p, "pl.contracts.to_even", &[json!(-2)], &unchecked()),
        ok(json!(-2))
    );
}

#[test]
fn requires_and_ensures_report_struct_inputs_and_actuals_as_value_json() {
    let p = common::check_source(CONTRACTS);
    let acc = |b: i64| json!({"$type": "pl.contracts.Account", "owner": "o", "balance": b});
    let d = flt(&p, "pl.contracts.deposit", &[acc(5), json!(2)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "result.balance == a.balance + amount");
    assert_eq!(d["input"], json!({"a": acc(5), "amount": 2}));
    assert_eq!(d["actual"], acc(8));
    let d = flt(&p, "pl.contracts.deposit", &[acc(5), json!(-1)]);
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["input"], json!({"a": acc(5), "amount": -1}));
}

#[test]
fn a_clause_short_circuits_so_its_later_operand_cannot_fault() {
    let p = common::check_source(CONTRACTS);
    let d = flt(&p, "pl.contracts.ratio", &[json!(5), json!(0)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause"], "b != 0 and a / b > 1");
    let d = flt(&p, "pl.contracts.ratio", &[json!(1), json!(1)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(
        val(&p, "pl.contracts.ratio", &[json!(4), json!(2)]),
        json!(2)
    );
}

#[test]
fn a_counterexample_is_built_from_the_violation_as_raised_before_the_s70_claim_s46() {
    let p = common::check_source(CONTRACTS);
    let rec = record(&p, "a test calls pre");
    let d = record_fault(&rec);
    // The record's fault is claimed by pre's requires clause...
    assert_eq!(d["symbol"], "pl.contracts.pre");
    assert_eq!(d["clause"], "bad_id(n) == n");
    assert_eq!(d["clause_kind"], "ensures");
    // ...while the counterexample indicts bad_id's body, as raised.
    let cx = rec["counterexamples"].as_array().expect("a counterexample");
    assert_eq!(cx.len(), 1, "{rec}");
    assert_eq!(cx[0]["symbol"], "pl.contracts.bad_id");
    assert_eq!(cx[0]["clause"], "result == n");
    assert_eq!(cx[0]["input"], json!({"n": 3}));
    assert_eq!(cx[0]["actual"], json!(4));
    assert_eq!(cx[0]["found_by"], r#"pl.contracts."a test calls pre""#);
}

#[test]
fn an_ensures_violation_found_by_a_test_is_the_designs_counterexample_s46() {
    let p = common::check_source(CONTRACTS);
    let test = r#"pl.contracts."a test finds a broken ensures""#;
    let rec = record(&p, "a test finds a broken ensures");
    let d = record_fault(&rec);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["symbol"], "pl.contracts.bad_id");
    let cx = rec["counterexamples"]
        .as_array()
        .expect("one counterexample");
    assert_eq!(cx.len(), 1, "{rec}");
    let c = &cx[0];
    assert_eq!(c["code"], "contract_counterexample");
    assert_eq!(c["symbol"], "pl.contracts.bad_id");
    assert_eq!(c["facet"], "body");
    assert_eq!(c["clause"], "result == n");
    assert_eq!(c["clause_kind"], "ensures");
    assert_eq!(c["input"], json!({"n": 1}));
    assert_eq!(c["actual"], json!(2));
    assert_eq!(c["status"], "failed");
    assert_eq!(c["found_by"], test);
    assert_eq!(c["span"], at(CONTRACTS, "ensures result == n"));
}

// ===========================================================================
// M. Capabilities through `Interpreter::bind_capability` (semantics 4.6,
//    5.3, 6.9, 8.3, S31, S57; toolchain 1.7, 1.7.1)
// ===========================================================================

const CAPS: &str = r#"module pl.caps

pub capability Store {
    fn get(id: Int) -> Int
        requires id > 0
        requires 100 / (id - 7) != 1000
        ensures result >= id
    fn put(id: Int, v: Int) -> Unit
        requires small(v)
}

pub fn small(v: Int) -> Bool
    requires v < 1000
{
    true
}

pub fn reader(id: Int, using db: Store) -> Int
    effects { db.get }
{
    db.get(id)
}

pub fn writer(id: Int, v: Int, using db: Store) -> Unit
    effects { db.put }
{
    db.put(id, v)
}

pub fn forward(id: Int, using db: Store) -> Int
    effects { db.get }
{
    reader(id, using db) + reader(id + 1, using db)
}

pub fn two(using a: Store, using b: Store) -> Int
    effects { a.get, b.get }
{
    a.get(1) * 10 + b.get(1)
}

pub fn looped(n: Int, using db: Store) -> Int
    effects { db.get }
{
    let first = db.get(1)
    var i = 0
    while i < n
        invariant i < 2
    {
        i = i + 1
    }
    i + first
}

pub fn holed(n: Int, using db: Store) -> Int
    effects { db.get }
{
    let x = db.get(n)
    ?todo
}

pub fn asserted(n: Int, using db: Store) -> Int
    effects { db.get }
{
    let x = db.get(n)
    assert x < 3
    x
}
"#;

/// A host implementation of `Store`: `get(id)` answers `id * factor`,
/// `put` answers `Unit`; every call is logged.
struct Mem {
    factor: i64,
    log: Rc<RefCell<Vec<String>>>,
}

impl CapabilityImpl for Mem {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        let rendered: Vec<String> = args
            .iter()
            .map(|v| v.as_int().map_or("?".to_string(), |n| n.to_string()))
            .collect();
        self.log
            .borrow_mut()
            .push(format!("{op}({})", rendered.join(", ")));
        match op {
            "get" => {
                let id = args[0].as_int().expect("an Int").clone();
                Ok(Value::Int(id * self.factor))
            }
            "put" => Ok(Value::unit(ctx.program)),
            other => panic!("no operation {other}"),
        }
    }
}

struct Caps {
    p: Program,
    store: DefId,
}

fn caps() -> Caps {
    let p = common::check_source(CAPS);
    let store = common::def(&p, "pl.caps.Store");
    Caps { p, store }
}

fn bind(it: &mut Interpreter<'_>, store: DefId, factor: i64) -> (Handle, Rc<RefCell<Vec<String>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let h = it.bind_capability(
        store,
        Box::new(Mem {
            factor,
            log: Rc::clone(&log),
        }),
        "fixture:store",
    );
    (h, log)
}

fn call(
    c: &Caps,
    config: EvalConfig,
    f: &str,
    args: Vec<Value>,
    factor: i64,
) -> (
    Result<Value, Fault>,
    Vec<String>,
    Vec<cpl_eval::EffectEvent>,
) {
    let mut it = Interpreter::new(&c.p, config);
    let (h, log) = bind(&mut it, c.store, factor);
    let f = common::def(&c.p, &format!("pl.caps.{f}"));
    let out = it.call(f, args, vec![h]);
    let events = it.effects().to_vec();
    let log = log.borrow().clone();
    (out, log, events)
}

#[test]
fn capability_requires_runs_before_the_host_and_blames_the_caller_s31() {
    let c = caps();
    let (out, log, _) = call(&c, EvalConfig::default(), "reader", vec![Value::int(0)], 2);
    let d = common::fault_json(&c.p, &out.unwrap_err());
    assert!(log.is_empty(), "the host was not invoked: {log:?}");
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "pl.caps.Store.get");
    assert_eq!(d["qualified_name"], "pl.caps.Store.get");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["caller"], "pl.caps.reader");
    assert_eq!(d["clause"], "id > 0");
    assert_eq!(d["input"], json!({"id": 0}));
    assert_eq!(d["span"], at(CAPS, "requires id > 0"));
    // An operation is no activation: the backtrace is the calling function.
    assert_eq!(bt(&d), ["pl.caps.reader"]);
}

#[test]
fn capability_requires_that_calls_a_function_whose_requires_fails_is_claimed_by_the_operation_clause_s70(
) {
    let c = caps();
    let (out, log, _) = call(
        &c,
        EvalConfig::default(),
        "writer",
        vec![Value::int(1), Value::int(5000)],
        2,
    );
    let d = common::fault_json(&c.p, &out.unwrap_err());
    assert!(log.is_empty(), "{log:?}");
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "pl.caps.Store.put");
    assert_eq!(d["clause"], "small(v)");
    assert_eq!(d["facet"], "contract");
    // As raised by `small`: its requires, called while `writer` evaluates
    // the operation's clause (S31: the caller evaluates it).
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["caller"], "pl.caps.writer");
    assert_eq!(d["input"], json!({"v": 5000}));
    assert_eq!(d["span"], at(CAPS, "requires v < 1000"));
    assert_eq!(bt(&d), ["pl.caps.small", "pl.caps.writer"]);
}

#[test]
fn capability_requires_that_faults_is_that_fault_of_the_operation_clause_s34() {
    // S34: `symbol` is the declaration whose clause was being evaluated; the
    // clause belongs to the operation (semantics 1.3: an operation has its
    // own identity and its `requires` are its contract).
    let c = caps();
    let (out, log, _) = call(&c, EvalConfig::default(), "reader", vec![Value::int(7)], 2);
    let d = common::fault_json(&c.p, &out.unwrap_err());
    assert!(log.is_empty(), "{log:?}");
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "100 / (id - 7) != 1000");
    assert_eq!(d["symbol"], "pl.caps.Store.get");
    assert_eq!(d["input"], json!({"a": 100, "b": 0}));
    assert_eq!(bt(&d), ["pl.caps.reader"]);
}

#[test]
fn capability_ensures_blames_the_binding_with_the_hosts_result_s57() {
    let c = caps();
    let (out, log, _) = call(&c, EvalConfig::default(), "reader", vec![Value::int(5)], -1);
    let d = common::fault_json(&c.p, &out.unwrap_err());
    assert_eq!(log, ["get(5)"]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["symbol"], "pl.caps.Store.get");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "binding");
    assert_eq!(d["clause"], "result >= id");
    assert_eq!(d["input"], json!({"id": 5}));
    assert_eq!(d["actual"], -5);
    assert_eq!(d["span"], at(CAPS, "ensures result >= id"));
    assert_eq!(bt(&d), ["pl.caps.reader"]);
    // Unchecked mode runs neither side of the operation's contract.
    let (out, log, _) = call(&c, unchecked(), "reader", vec![Value::int(0)], -1);
    assert_eq!(out.unwrap(), Value::int(0));
    assert_eq!(log, ["get(0)"]);
}

#[test]
fn using_arguments_forward_handles_by_position_and_effects_name_the_performing_function() {
    let c = caps();
    let config = EvalConfig {
        trace_effects: true,
        ..EvalConfig::default()
    };
    let (out, log, events) = call(&c, config.clone(), "forward", vec![Value::int(2)], 3);
    assert_eq!(out.unwrap(), Value::int(15));
    assert_eq!(log, ["get(2)", "get(3)"]);
    let reader = common::def(&c.p, "pl.caps.reader");
    assert_eq!(events.len(), 2);
    // The event's span is the operation call (toolchain 1.7 EffectEvent).
    for e in &events {
        assert_eq!(span_json(&c.p, e.span), at(CAPS, "db.get(id)"));
    }
    assert!(
        events.iter().all(|e| e.def == reader && e.op == "get"),
        "{events:?}"
    );
    // Two handles of one capability bind to the `using` parameters in order.
    let mut it = Interpreter::new(&c.p, config);
    let (a, _) = bind(&mut it, c.store, 1);
    let (b, _) = bind(&mut it, c.store, 7);
    let two = common::def(&c.p, "pl.caps.two");
    assert_eq!(
        it.call(two, vec![], vec![a.clone(), b.clone()]).unwrap(),
        Value::int(17)
    );
    assert_eq!(
        it.call(two, vec![], vec![b.clone(), a.clone()]).unwrap(),
        Value::int(71)
    );
    let handles: Vec<_> = it.effects().iter().map(|e| e.handle).collect();
    assert_eq!(handles, [a.id, b.id, b.id, a.id]);
}

#[test]
fn loop_bindings_include_using_parameters_as_handle_json_5_3() {
    let c = caps();
    let (out, _, _) = call(&c, EvalConfig::default(), "looped", vec![Value::int(5)], 1);
    let d = common::fault_json(&c.p, &out.unwrap_err());
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["iteration"], 2);
    // 5.3: `bindings` holds every local binding visible at the loop, the
    // `using` parameters included; a handle's value JSON names its
    // capability (toolchain 7.3).
    assert_eq!(
        d["actual"],
        json!({"bindings": {
            "n": 5,
            "db": {"$type": "pl.caps.Store", "$handle": 0},
            "first": 1,
            "i": 2
        }})
    );
    // The `input` of a loop violation is the positional arguments.
    assert_eq!(d["input"], json!({"n": 5}));
}

#[test]
fn hole_and_assert_bindings_list_using_parameters_with_their_kind_8_3_s41() {
    let c = caps();
    let handle = json!({"$type": "pl.caps.Store", "$handle": 0});
    let (out, _, _) = call(&c, EvalConfig::default(), "holed", vec![Value::int(2)], 1);
    let d = common::fault_json(&c.p, &out.unwrap_err());
    assert_eq!(d["code"], "unresolved_hole");
    assert_eq!(d["hole"], "todo");
    assert_eq!(d["expected_type"], "Int");
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "x", "type": "Int", "kind": "let", "value": 2},
            {"name": "db", "type": "Store", "kind": "using", "value": handle},
            {"name": "n", "type": "Int", "kind": "parameter", "value": 2}
        ])
    );
    let (out, _, _) = call(
        &c,
        EvalConfig::default(),
        "asserted",
        vec![Value::int(4)],
        1,
    );
    let d = common::fault_json(&c.p, &out.unwrap_err());
    assert_eq!(d["code"], "assertion_failed");
    assert_eq!(d["clause"], "x < 3");
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "x", "type": "Int", "kind": "let", "value": 4},
            {"name": "db", "type": "Store", "kind": "using", "value": handle},
            {"name": "n", "type": "Int", "kind": "parameter", "value": 4}
        ])
    );
}

#[test]
fn effectful_functions_cannot_be_run_and_unbound_handles_are_internal_errors_6_9() {
    let c = caps();
    assert!(matches!(
        common::try_run(&c.p, "pl.caps.reader", &common::json_args(&[json!(1)])),
        Err(RunError::CannotRunEffectful(name)) if name == "pl.caps.reader"
    ));
    assert!(matches!(
        common::try_run(&c.p, "pl.caps.two", &[]),
        Err(RunError::CannotRunEffectful(_))
    ));
    let mut it = Interpreter::new(&c.p, EvalConfig::default());
    let ghost = Handle {
        id: cpl_eval::HandleId(41),
        capability: c.store,
        label: "ghost".to_string(),
    };
    let reader = common::def(&c.p, "pl.caps.reader");
    let d = common::fault_json(
        &c.p,
        &it.call(reader, vec![Value::int(1)], vec![ghost])
            .unwrap_err(),
    );
    assert_eq!(d["code"], "internal_error");
    assert_eq!(d["kind"], "unbound_capability");
    assert!(
        d.get("symbol").is_none(),
        "internal_error blames no declaration: {d}"
    );
    assert!(d.get("facet").is_none(), "{d}");
}

/// A host that answers every operation with a `Text`, whatever the
/// operation's declared return type.
struct IllTyped;

impl CapabilityImpl for IllTyped {
    fn call(
        &mut self,
        _op: &str,
        _args: Vec<Value>,
        _: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        Ok(Value::text("not an Int"))
    }
}

#[test]
fn an_internal_error_inside_a_clause_still_blames_no_declaration_6_7() {
    // semantics 6.7: a type confusion between the typed IR and a value is
    // internal_error, and "internal_error has no symbol, hence no facet"
    // (toolchain 1.7.3: "facet accompanies symbol, so internal_error, which
    // has none, has no facet either"). S34's rewrite is for faults that are
    // the clause's specification bug; an impossible state is not one.
    let c = caps();
    let mut it = Interpreter::new(&c.p, EvalConfig::default());
    let h = it.bind_capability(c.store, Box::new(IllTyped), "fixture:ill-typed");
    let reader = common::def(&c.p, "pl.caps.reader");
    // The host's Text meets the operation's `ensures result >= id`.
    let d = common::fault_json(
        &c.p,
        &it.call(reader, vec![Value::int(5)], vec![h]).unwrap_err(),
    );
    assert_eq!(d["code"], "internal_error", "{d}");
    assert_eq!(d["kind"], "type_confusion");
    assert!(d.get("symbol").is_none(), "{d}");
    assert!(d.get("facet").is_none(), "{d}");
    // A host passing an ill-typed argument: the confusion surfaces in a
    // requires clause or in a refinement predicate.
    let p = common::check_source(CONTRACTS);
    let mut it = Interpreter::new(&p, EvalConfig::default());
    let recip = common::def(&p, "pl.contracts.recip");
    let d = common::fault_json(
        &p,
        &it.call(recip, vec![Value::text("x")], vec![]).unwrap_err(),
    );
    assert_eq!(d["code"], "internal_error", "{d}");
    assert!(d.get("symbol").is_none(), "{d}");
    assert!(d.get("facet").is_none(), "{d}");
    let p = common::check_source(REFINE);
    let positive = common::def(&p, "pl.refine.Positive");
    let mut it = Interpreter::new(&p, EvalConfig::default());
    let fault = it
        .satisfies_refinement(positive, &Value::text("x"))
        .unwrap_err();
    let d = common::fault_json(&p, &fault);
    assert_eq!(d["code"], "internal_error", "{d}");
    assert!(d.get("symbol").is_none(), "{d}");
    assert!(d.get("facet").is_none(), "{d}");
}

// ===========================================================================
// N. Holes (semantics 6.7, 8.1, 8.3, S41, S48; toolchain 1.7.1)
// ===========================================================================

const HOLES: &str = r#"module pl.holes

pub type Positive = Int where self > 0

pub struct Point {
    x: Int
    y: Int
}

fn in_let(n: Int) -> Int {
    let x: Option<Positive> = ?maybe
    n
}

fn takes(p: Positive) -> Int {
    p
}

fn in_arg(n: Int) -> Int {
    takes(?size)
}

fn in_match(o: Option<Int>) -> Int {
    match o {
        Some(v) => ?some_case
        None => 0
    }
}

fn in_loop(n: Int) -> Int {
    var i = 0
    while i < n
        invariant i >= 0
        decreases n - i
    {
        let step = i + 1
        i = ?next
    }
    i
}

fn in_forall(xs: List<Int>) -> Bool {
    forall x in xs: ?pred
}

fn in_generic<T>(xs: List<T>) -> List<T> {
    let n = xs.len
    ?rest
}

fn calls_generic() -> List<Text> {
    in_generic(["a"])
}

fn in_condition(n: Int) -> Int {
    if ?cond { 1 } else { n }
}

fn in_field(n: Int) -> Point {
    Point { x: n, y: ?y }
}

fn in_list(n: Int) -> List<Positive> {
    [1, ?second]
}

fn after_return(n: Int) -> Int {
    if n > 0 {
        return n
    }
    ?negative
}

fn in_nested(xs: List<Option<Int>>, flag: Bool) -> Int {
    let outer = xs.len
    var total = 0
    if flag {
        let inner = outer * 2
        var i = 0
        while i < xs.len
            invariant i >= 0
            decreases xs.len - i
        {
            let item = xs.get(i)
            match item {
                Some(Some(v)) => {
                    let w = v + inner
                    total = ?accumulate
                }
                Some(None) => {}
                None => {}
            }
            i = i + 1
        }
    }
    total
}

test "a hole in a test" {
    let ready = true
    ?todo
}

property "a hole in a property"
    for n in gen.int
    for flag in gen.bool
{
    let m = n
    ?later
}
"#;

/// The checker's `hole` report whose span is `span`.
fn report_at(result: &cpl_sema::CheckResult, span: &Json) -> Json {
    hole_reports(result)
        .into_iter()
        .find(|r| &r["span"] == span)
        .unwrap_or_else(|| panic!("no hole report at {span}"))
}

/// The hole fault of `function(args)`, checked against the checker's hole
/// report at the same span: the same hole, symbol, expected type, and
/// bindings (names, types, kinds, `mutable`, in the same order).
fn hole_fault(result: &cpl_sema::CheckResult, p: &Program, function: &str, args: &[Json]) -> Json {
    let d = flt(p, function, args);
    assert_eq!(d["code"], "unresolved_hole", "{d}");
    assert_eq!(d["category"], "hole");
    assert_eq!(d["severity"], "error");
    let report = report_at(result, &d["span"]);
    assert_eq!(d["hole"], report["hole"]);
    assert_eq!(d["symbol"], report["symbol"]);
    assert_eq!(d["qualified_name"], report["qualified_name"]);
    assert_eq!(d["facet"], "body");
    assert_eq!(d["expected_type"], report["expected_type"]);
    assert_eq!(
        without_values(&d["bindings"]),
        report["bindings"],
        "{function}"
    );
    d
}

#[test]
fn hole_faults_agree_with_the_hole_report_at_every_expression_position_8_3() {
    let result = common::check_source_result(HOLES);
    let p = common::checked(common::check_source_result(HOLES));
    let d = hole_fault(&result, &p, "pl.holes.in_let", &[json!(1)]);
    assert_eq!(d["hole"], "maybe");
    assert_eq!(d["expected_type"], "Option<Positive>");
    assert_eq!(d["span"], at(HOLES, "?maybe"));
    let d = hole_fault(&result, &p, "pl.holes.in_arg", &[json!(1)]);
    assert_eq!(d["expected_type"], "Positive");
    let d = hole_fault(&result, &p, "pl.holes.in_match", &[some(json!(4))]);
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "v", "type": "Int", "kind": "pattern", "value": 4},
            {"name": "o", "type": "Option<Int>", "kind": "parameter", "value": some(json!(4))}
        ])
    );
    assert_eq!(val(&p, "pl.holes.in_match", &[none()]), json!(0));
    let d = hole_fault(&result, &p, "pl.holes.in_loop", &[json!(2)]);
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "step", "type": "Int", "kind": "let", "value": 1},
            {"name": "i", "type": "Int", "kind": "var", "mutable": true, "value": 0},
            {"name": "n", "type": "Int", "kind": "parameter", "value": 2}
        ])
    );
    let d = hole_fault(&result, &p, "pl.holes.in_forall", &[json!([5])]);
    assert_eq!(d["expected_type"], "Bool");
    assert_eq!(
        d["bindings"][0],
        json!({"name": "x", "type": "Int", "kind": "forall", "value": 5})
    );
    // An empty collection never reaches the predicate.
    assert_eq!(val(&p, "pl.holes.in_forall", &[json!([])]), json!(true));
    let d = hole_fault(&result, &p, "pl.holes.in_condition", &[json!(1)]);
    assert_eq!(d["expected_type"], "Bool");
    let d = hole_fault(&result, &p, "pl.holes.in_field", &[json!(1)]);
    assert_eq!(d["expected_type"], "Int");
    let d = hole_fault(&result, &p, "pl.holes.in_list", &[json!(1)]);
    assert_eq!(d["expected_type"], "Positive");
    // A hole after an early return is reached only when the return is not.
    assert_eq!(val(&p, "pl.holes.after_return", &[json!(3)]), json!(3));
    let d = hole_fault(&result, &p, "pl.holes.after_return", &[json!(-3)]);
    assert_eq!(d["hole"], "negative");
}

#[test]
fn hole_bindings_in_nested_scopes_agree_with_the_hole_report_innermost_first_8_3() {
    let result = common::check_source_result(HOLES);
    let p = common::checked(common::check_source_result(HOLES));
    let xs = json!([none(), some(json!(4))]);
    let d = hole_fault(
        &result,
        &p,
        "pl.holes.in_nested",
        &[xs.clone(), json!(true)],
    );
    assert_eq!(d["hole"], "accumulate");
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "w", "type": "Int", "kind": "let", "value": 8},
            {"name": "v", "type": "Int", "kind": "pattern", "value": 4},
            {"name": "item", "type": "Option<Option<Int>>", "kind": "let", "value": some(some(json!(4)))},
            {"name": "i", "type": "Int", "kind": "var", "mutable": true, "value": 1},
            {"name": "inner", "type": "Int", "kind": "let", "value": 4},
            {"name": "total", "type": "Int", "kind": "var", "mutable": true, "value": 0},
            {"name": "outer", "type": "Int", "kind": "let", "value": 2},
            {"name": "flag", "type": "Bool", "kind": "parameter", "value": true},
            {"name": "xs", "type": "List<Option<Int>>", "kind": "parameter", "value": xs}
        ])
    );
    assert_eq!(
        val(&p, "pl.holes.in_nested", &[json!([none()]), json!(true)]),
        json!(0)
    );
}

#[test]
fn a_hole_in_a_generic_function_names_its_type_parameters_and_encodes_values() {
    let result = common::check_source_result(HOLES);
    let p = common::checked(common::check_source_result(HOLES));
    let d = hole_fault(&result, &p, "pl.holes.calls_generic", &[]);
    assert_eq!(d["symbol"], "pl.holes.in_generic");
    assert_eq!(d["expected_type"], "List<T>");
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "n", "type": "Int", "kind": "let", "value": 1},
            {"name": "xs", "type": "List<T>", "kind": "parameter", "value": ["a"]}
        ])
    );
    assert_eq!(bt(&d), ["pl.holes.in_generic", "pl.holes.calls_generic"]);
    assert_eq!(d["backtrace"][0]["span"], at(HOLES, r#"in_generic(["a"])"#));
}

#[test]
fn a_hole_in_a_test_or_property_body_fails_it_with_the_tests_identity() {
    let result = common::check_source_result(HOLES);
    let p = common::checked(common::check_source_result(HOLES));
    let rec = record(&p, "a hole in a test");
    let d = record_fault(&rec);
    assert_eq!(d["code"], "unresolved_hole");
    assert_eq!(d["symbol"], r#"pl.holes."a hole in a test""#);
    assert_eq!(d["expected_type"], "Unit");
    assert_eq!(
        d["bindings"],
        json!([{"name": "ready", "type": "Bool", "kind": "let", "value": true}])
    );
    let report = report_at(&result, &d["span"]);
    assert_eq!(without_values(&d["bindings"]), report["bindings"]);
    assert_eq!(d["expected_type"], report["expected_type"]);
    let rec = record(&p, "a hole in a property");
    let d = record_fault(&rec);
    assert_eq!(d["code"], "unresolved_hole");
    assert_eq!(d["symbol"], r#"pl.holes."a hole in a property""#);
    // Every case fails with the same code, so the binder shrinks to 0 and
    // the reported fault is the one the shrunk tuple raised (S45).
    assert_eq!(rec["shrunk"]["input"], json!({"n": 0, "flag": false}));
    // Innermost first: the body's `let`, then the binders, the later first.
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "m", "type": "Int", "kind": "let", "value": 0},
            {"name": "flag", "type": "Bool", "kind": "for", "value": false},
            {"name": "n", "type": "Int", "kind": "for", "value": 0}
        ])
    );
    let report = report_at(&result, &d["span"]);
    assert_eq!(without_values(&d["bindings"]), report["bindings"]);
    assert_eq!(rec["failing_case"]["index"], 0);
    assert_eq!(rec["cases"], 1);
}

// ===========================================================================
// O. The call-depth limit (semantics 6.7, S38, S72; toolchain 1.7)
// ===========================================================================

const DEPTH: &str = r#"module pl.depth

type Even = Int where is_even(self)

fn down(n: Int) -> Int {
    if n == 0 { 0 } else { 1 + down(n - 1) }
}

// Builtin callees are frames but not counted activations (S72).
fn leaf() -> List<Int> {
    [1].take(1) ++ flatten([[2]]) ++ [std.text.split("a,b", ",").len, "abc".len, (-4).abs()]
}

fn calls_leaf() -> List<Int> {
    leaf()
}

fn ping(n: Int) -> Int {
    if n == 0 { 0 } else { pong(n - 1) }
}

fn pong(n: Int) -> Int {
    if n == 0 { 0 } else { ping(n - 1) }
}

fn is_even(n: Int) -> Bool {
    n % 2 == 0
}

fn to_even(n: Int) -> Result<Even, ConstraintError> {
    check<Even>(n)
}

fn guarded(n: Int) -> Int
    requires is_even(n)
{
    n
}

fn ensured(n: Int) -> Int
    ensures is_even(result)
{
    n
}

fn spin(n: Int) -> Int {
    var i = n
    while i >= 0 {
        i = i + 1
    }
    i
}

test "a test body is no activation" {
    assert down(2) == 2
}
"#;

#[test]
fn call_depth_admits_exactly_the_limit_and_blames_the_refused_call_s38() {
    let p = common::check_source(DEPTH);
    // down(2) is three activations: down(2), down(1), down(0).
    assert_eq!(
        val_with(&p, "pl.depth.down", &[json!(2)], &depth(3)),
        json!(2)
    );
    let d = flt_with(&p, "pl.depth.down", &[json!(3)], &depth(3));
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["category"], "internal");
    assert_eq!(d["resource"], "stack");
    assert_eq!(d["limit"], 3);
    // The refused call is no activation; the caller is blamed at the call.
    assert_eq!(d["symbol"], "pl.depth.down");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["span"], at(DEPTH, "down(n - 1)"));
    assert_eq!(bt(&d), ["pl.depth.down", "pl.depth.down", "pl.depth.down"]);
    assert!(
        d.get("depth").is_none(),
        "the counter, not the stack guard: {d}"
    );
}

#[test]
fn call_depth_counts_only_cpl_function_activations_s72() {
    let p = common::check_source(DEPTH);
    // S72's example: with max_call_depth = 1 a function whose body calls
    // builtins runs.
    assert_eq!(
        val_with(&p, "pl.depth.leaf", &[], &depth(1)),
        json!([1, 2, 2, 3, 4])
    );
    let d = flt_with(&p, "pl.depth.calls_leaf", &[], &depth(1));
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["limit"], 1);
    assert_eq!(d["symbol"], "pl.depth.calls_leaf");
    assert_eq!(bt(&d), ["pl.depth.calls_leaf"]);
    // Mutual recursion counts every function.
    assert_eq!(
        val_with(&p, "pl.depth.ping", &[json!(3)], &depth(4)),
        json!(0)
    );
    let d = flt_with(&p, "pl.depth.ping", &[json!(4)], &depth(4));
    assert_eq!(d["symbol"], "pl.depth.pong");
    assert_eq!(
        bt(&d),
        [
            "pl.depth.pong",
            "pl.depth.ping",
            "pl.depth.pong",
            "pl.depth.ping"
        ]
    );
}

#[test]
fn call_depth_zero_refuses_the_top_level_call_itself_s72() {
    // S72: the limit is exceeded when a call would make the number of
    // active function calls greater than max_call_depth; with 0 that is
    // the first call. No declaration is active, so none is blamed.
    let p = common::check_source(DEPTH);
    let d = flt_with(&p, "pl.depth.leaf", &[], &depth(0));
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["limit"], 0);
    assert!(d.get("symbol").is_none(), "{d}");
    assert!(d.get("backtrace").is_none(), "{d}");
}

#[test]
fn calls_made_by_predicates_and_clauses_count_and_their_exhaustion_is_reported_per_s34() {
    let p = common::check_source(DEPTH);
    // to_even [1] -> Even's predicate (a frame, not counted) -> is_even [2].
    let d = flt_with(&p, "pl.depth.to_even", &[json!(2)], &depth(1));
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["symbol"], "pl.depth.Even");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "is_even(self)");
    assert_eq!(bt(&d), ["pl.depth.Even", "pl.depth.to_even"]);
    assert_eq!(
        val_with(&p, "pl.depth.to_even", &[json!(2)], &depth(2)),
        ok(json!(2))
    );
    let d = flt_with(&p, "pl.depth.guarded", &[json!(2)], &depth(1));
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["symbol"], "pl.depth.guarded");
    assert_eq!(d["clause"], "is_even(n)");
    assert_eq!(d["facet"], "contract");
    // The ensures runs while the function is still active.
    let d = flt_with(&p, "pl.depth.ensured", &[json!(2)], &depth(1));
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["clause"], "is_even(result)");
    assert_eq!(
        val_with(&p, "pl.depth.ensured", &[json!(2)], &depth(2)),
        json!(2)
    );
}

#[test]
fn a_test_body_is_no_counted_activation_s72() {
    let p = common::check_source(DEPTH);
    let config = TestConfig {
        eval: depth(3),
        ..TestConfig::with_seed(1)
    };
    let rec = record_with(&p, "a test body is no activation", &config);
    assert_eq!(rec["status"], "tested", "{rec}");
    let config = TestConfig {
        eval: depth(2),
        ..TestConfig::with_seed(1)
    };
    let rec = record_with(&p, "a test body is no activation", &config);
    let d = record_fault(&rec);
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["limit"], 2);
    assert_eq!(
        bt(&d),
        [
            "pl.depth.down",
            "pl.depth.down",
            r#"pl.depth."a test body is no activation""#
        ]
    );
}

#[test]
fn the_time_budget_ends_a_runaway_loop_as_resource_exhaustion_time_s38() {
    let p = common::check_source(DEPTH);
    let config = EvalConfig {
        time_budget: Some(std::time::Duration::from_millis(30)),
        ..EvalConfig::default()
    };
    let d = flt_with(&p, "pl.depth.spin", &[json!(0)], &config);
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["resource"], "time");
    // `limit` is the budget (toolchain 1.7.3), in milliseconds.
    assert_eq!(d["limit"], 30);
    assert_eq!(d["symbol"], "pl.depth.spin");
    assert_eq!(bt(&d), ["pl.depth.spin"]);
}

// ===========================================================================
// P. `run_function` refusals and the standard library at the top level
//    (semantics S39; toolchain 1.7, 3.6, 7.2)
// ===========================================================================

const RUN: &str = r#"module pl.run

pub type Positive = Int where self > 0

pub capability Store {
    fn get(id: Int) -> Int
}

pub fn generic_and_effectful<T>(x: T, using db: Store) -> T
    effects { db.get }
{
    let _ = db.get(1)
    x
}

pub fn generic<T>(x: T) -> T {
    x
}

pub fn effectful(using db: Store) -> Int
    effects { db.get }
{
    db.get(1)
}

pub fn plain(n: Int) -> Int {
    n
}

pub fn nothing() -> Int {
    1
}

test "t" {
    assert true
}
"#;

fn refusal(p: &Program, function: &str, args: &[Json]) -> RunError {
    match common::try_run(p, function, &common::json_args(args)) {
        Err(e) => e,
        Ok(outcome) => panic!("{function} ran: {}", common::outcome_json(p, &outcome)),
    }
}

#[test]
fn run_refusals_come_in_the_specified_order_s39() {
    let p = common::check_source(RUN);
    // Generic comes before effectful, and both before the argument count.
    assert!(matches!(
        refusal(&p, "pl.run.generic_and_effectful", &[json!(1)]),
        RunError::Generic(name) if name == "pl.run.generic_and_effectful"
    ));
    assert!(matches!(
        refusal(&p, "pl.run.generic_and_effectful", &[]),
        RunError::Generic(_)
    ));
    assert!(matches!(
        refusal(&p, "pl.run.generic", &[json!(1), json!(2)]),
        RunError::Generic(_)
    ));
    assert!(matches!(
        refusal(&p, "pl.run.effectful", &[json!(1)]),
        RunError::CannotRunEffectful(name) if name == "pl.run.effectful"
    ));
    assert!(matches!(
        refusal(&p, "pl.run.plain", &[]),
        RunError::ArgCount {
            expected: 1,
            got: 0
        }
    ));
    assert!(matches!(
        refusal(&p, "pl.run.nothing", &[json!(1)]),
        RunError::ArgCount {
            expected: 0,
            got: 1
        }
    ));
    assert!(
        matches!(refusal(&p, "pl.run.missing", &[]), RunError::NotFound(name) if name == "pl.run.missing")
    );
    for not_a_function in ["pl.run.Positive", "pl.run.Store", r#"pl.run."t""#] {
        assert!(
            matches!(refusal(&p, not_a_function, &[]), RunError::NotAFunction(_)),
            "{not_a_function}"
        );
    }
    // A standard-library member stub with type parameters is generic too.
    assert!(matches!(
        refusal(&p, "std.prelude.List.take", &[json!([1]), json!(1)]),
        RunError::Generic(name) if name == "std.prelude.List.take"
    ));
}

#[test]
fn standard_library_functions_run_at_the_top_level_with_no_caller_s39() {
    let p = common::check_source(RUN);
    // A member stub takes its receiver as the first argument.
    assert_eq!(val(&p, "std.prelude.Int.abs", &[json!(-5)]), json!(5));
    assert_eq!(
        val(&p, "std.prelude.Int.min", &[json!(3), json!(-3)]),
        json!(-3)
    );
    assert_eq!(
        val(&p, "std.prelude.Text.len", &[json!("h\u{e9}")]),
        json!(2)
    );
    assert_eq!(
        val(&p, "std.text.split", &[json!("a,b"), json!(",")]),
        json!(["a", "b"])
    );
    let d = flt(&p, "std.text.split", &[json!("a"), json!("")]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "std.text.split");
    assert!(d.get("caller").is_none(), "{d}");
    assert_eq!(d["input"], json!({"text": "a", "sep": ""}));
    // The builtin is the only frame; the command line entered it, so the
    // frame has no call span.
    assert_eq!(bt(&d), ["std.text.split"]);
    assert!(d["backtrace"][0].get("span").is_none(), "{d}");
    let d = flt(&p, "std.bytes.from_list", &[json!([300])]);
    assert_eq!(d["symbol"], "std.bytes.from_list");
    assert!(d.get("caller").is_none(), "{d}");
}

#[test]
fn generators_called_outside_the_runner_are_internal_errors_but_range_checks_its_bounds_first() {
    // toolchain 3.6: the gen_* ids are registered but not callable; calling
    // one is internal_error (kind generator_not_a_value), and gen_range
    // checks its `requires lo <= hi` first. `run_function` refuses a
    // generator before calling it (T61, the next test), so the stubs are
    // called through `Interpreter::call`, the path `run_function` used to
    // reach them by.
    let p = common::check_source(RUN);
    let d = called_fault(&p, "std.gen.int", vec![]);
    assert_eq!(d["code"], "internal_error");
    assert_eq!(d["kind"], "generator_not_a_value");
    let d = called_fault(&p, "std.gen.range", vec![Value::int(5), Value::int(1)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["symbol"], "std.gen.range");
    assert_eq!(d["clause"], "lo <= hi");
    assert_eq!(d["input"], json!({"lo": 5, "hi": 1}));
    assert!(d.get("caller").is_none(), "{d}");
    let d = called_fault(&p, "std.gen.range", vec![Value::int(1), Value::int(5)]);
    assert_eq!(d["code"], "internal_error");
    assert_eq!(d["kind"], "generator_not_a_value");
}

#[test]
fn run_function_refuses_a_generator_as_not_a_function_before_the_generic_check_t61() {
    // T61 [D-257]: a function whose declared return type is a `Gen<T>` (the
    // `std.gen` stubs) is `RunError::NotAFunction`, before the generic and
    // effect checks and the argument count, so nothing is evaluated: not
    // even `gen.range`'s `requires lo <= hi` (S14: a generator has no value
    // outside a property's `for` clause).
    let p = common::check_source(RUN);
    for (generator, args) in [
        ("std.gen.int", vec![]),
        ("std.gen.positive_int", vec![]),
        ("std.gen.bool", vec![]),
        ("std.gen.text", vec![]),
        ("std.gen.range", vec![json!(5), json!(1)]),
        ("std.gen.range", vec![json!(1), json!(5)]),
        ("std.gen.range", vec![]),
        // Generic generators: NotAFunction comes before Generic.
        ("std.gen.list", vec![]),
        ("std.gen.option", vec![json!(1)]),
    ] {
        assert!(
            matches!(
                refusal(&p, generator, &args),
                RunError::NotAFunction(name) if name == generator
            ),
            "{generator}"
        );
    }
    // An ordinary function of the same program still runs.
    assert_eq!(val(&p, "pl.run.nothing", &[]), json!(1));
}

// ===========================================================================
// Q. Several modules: imports, qualified calls, and structurally identical
//    refinements (semantics 1.4, 2.4, S2, S44, S55; stdlib S44)
// ===========================================================================

const LIB: &str = r#"module pl.lib

pub type Positive = Int where self > 0

pub fn recip(n: Int) -> Int
    requires n != 0
{
    100 / n
}

pub fn checked(n: Int) -> Result<Positive, ConstraintError> {
    check<Positive>(n)
}

@id("fn_lib_tracked")
pub fn tracked(n: Int) -> Int
    requires n > 0
{
    recip(n - 1)
}
"#;

const APP: &str = r#"module pl.app

use pl.lib.{recip}

pub type Positive = Int where self > 0

fn call_imported(n: Int) -> Int {
    recip(n)
}

fn call_qualified(n: Int) -> Int {
    pl.lib.recip(n)
}

fn mine(n: Int) -> Result<Positive, ConstraintError> {
    check<Positive>(n)
}

fn theirs(n: Int) -> Result<pl.lib.Positive, ConstraintError> {
    check<pl.lib.Positive>(n)
}

// pl.lib.Positive and pl.app.Positive are one type (S55).
fn cross(n: Int) -> Result<Positive, ConstraintError> {
    pl.lib.checked(n)
}

fn use_lib_positive(p: pl.lib.Positive) -> Int {
    p
}

@id("fn_app_tracker")
fn tracker(n: Int) -> Int {
    pl.lib.tracked(n)
}
"#;

fn two_modules() -> Program {
    common::check_files(&[("src/pl/lib.cpl", LIB), ("src/pl/app.cpl", APP)])
}

#[test]
fn calls_across_modules_report_qualified_names_as_symbol_and_caller() {
    let p = two_modules();
    for f in ["pl.app.call_imported", "pl.app.call_qualified"] {
        let d = flt(&p, f, &[json!(0)]);
        assert_eq!(d["symbol"], "pl.lib.recip", "{f}");
        assert_eq!(d["caller"], f);
        assert_eq!(d["span"]["file"], "src/pl/lib.cpl");
        assert_eq!(bt(&d), ["pl.lib.recip", f]);
        assert_eq!(d["backtrace"][0]["span"]["file"], "src/pl/app.cpl");
    }
}

#[test]
fn an_id_is_the_symbol_while_caller_and_backtrace_use_qualified_names_s2() {
    let p = two_modules();
    // tracked's requires: the symbol is its @id, qualified_name its name.
    let d = flt(&p, "pl.app.tracker", &[json!(0)]);
    assert_eq!(d["symbol"], "fn_lib_tracked");
    assert_eq!(d["qualified_name"], "pl.lib.tracked");
    // `caller` is a qualified name (semantics 5.2, 9.3), not an identity.
    assert_eq!(d["caller"], "pl.app.tracker");
    assert_eq!(bt(&d), ["pl.lib.tracked", "pl.app.tracker"]);
    // One level deeper: recip's caller is tracked, by qualified name.
    let d = flt(&p, "pl.app.tracker", &[json!(1)]);
    assert_eq!(d["symbol"], "pl.lib.recip");
    assert_eq!(d["caller"], "pl.lib.tracked");
    assert_eq!(bt(&d), ["pl.lib.recip", "pl.lib.tracked", "pl.app.tracker"]);
}

#[test]
fn check_names_the_refinement_declaration_the_call_site_names_s44_s55() {
    let p = two_modules();
    assert_eq!(
        val(&p, "pl.app.mine", &[json!(0)]),
        constraint("pl.app.Positive", "0 does not satisfy self > 0")
    );
    assert_eq!(
        val(&p, "pl.app.theirs", &[json!(0)]),
        constraint("pl.lib.Positive", "0 does not satisfy self > 0")
    );
    assert_eq!(
        val(&p, "pl.app.cross", &[json!(0)]),
        constraint("pl.lib.Positive", "0 does not satisfy self > 0")
    );
    assert_eq!(val(&p, "pl.app.cross", &[json!(2)]), ok(json!(2)));
    let d = flt(&p, "pl.app.use_lib_positive", &[json!(0)]);
    assert_eq!(d["code"], "refinement_violation");
    assert_eq!(d["clause"], "self > 0");
    assert_eq!(d["symbol"], "pl.app.use_lib_positive");
}

// ===========================================================================
// R. The diagnostic envelope of every fault kind raised from source
//    (semantics 6.7, 9.2, 9.4, 9.5, S49, S50; toolchain 1.7.3)
// ===========================================================================

/// The keys a fault diagnostic must carry, and the rule of 9.5 that
/// `status` appears only on `contract_violation` (and the counterexample),
/// and of 9.2 that `facet` appears only together with `symbol`.
fn assert_envelope(d: &Json, required: &[&str]) {
    let keys: Vec<&str> = d
        .as_object()
        .expect("a diagnostic object")
        .keys()
        .map(String::as_str)
        .collect();
    for key in ["code", "category", "severity", "message"]
        .iter()
        .chain(required)
    {
        assert!(keys.contains(key), "`{key}` is missing from {d}");
    }
    if d["code"] != "contract_violation" {
        assert!(!keys.contains(&"status"), "status on a {}: {d}", d["code"]);
        assert!(!keys.contains(&"clause_kind"), "{d}");
        assert!(!keys.contains(&"attributed_to"), "{d}");
    }
    if !keys.contains(&"symbol") {
        assert!(!keys.contains(&"facet"), "facet without symbol: {d}");
        assert!(!keys.contains(&"qualified_name"), "{d}");
    }
    for (key, value) in d.as_object().unwrap() {
        assert!(
            !value.is_null(),
            "`{key}` is null in {d} (S49: omitted, never null)"
        );
    }
    assert_eq!(d["severity"], "error");
}

#[test]
fn every_fault_kind_carries_its_envelope_and_no_foreign_keys_s49_s50() {
    let base = ["span", "symbol", "qualified_name", "facet", "backtrace"];
    let p = common::check_source(INTS);
    let d = flt(&p, "pl.ints.by_zero", &[json!(1)]);
    assert_envelope(&d, &[&base[..], &["input"]].concat());
    let p = common::check_source(CONTRACTS);
    let d = flt(&p, "pl.contracts.calls_recip", &[json!(0)]);
    assert_envelope(
        &d,
        &[
            &base[..],
            &[
                "clause_kind",
                "clause",
                "attributed_to",
                "input",
                "caller",
                "status",
            ],
        ]
        .concat(),
    );
    let d = flt(
        &p,
        "pl.contracts.deposit",
        &[
            json!({"$type": "pl.contracts.Account", "owner": "o", "balance": 1}),
            json!(1),
        ],
    );
    assert_envelope(
        &d,
        &[
            &base[..],
            &[
                "clause_kind",
                "clause",
                "attributed_to",
                "input",
                "actual",
                "status",
            ],
        ]
        .concat(),
    );
    let d = flt(&p, "pl.contracts.uses_unfinished", &[json!(1)]);
    assert_envelope(
        &d,
        &[&base[..], &["hole", "expected_type", "bindings", "clause"]].concat(),
    );
    let d = flt(&p, "pl.contracts.asserting", &[json!(3), json!([5])]);
    assert_envelope(&d, &[&base[..], &["clause", "bindings"]].concat());
    let p = common::check_source(LOOPS);
    let d = flt(&p, "pl.loops.inv_after", &[json!(5)]);
    assert_envelope(
        &d,
        &[
            &base[..],
            &[
                "clause_kind",
                "clause",
                "attributed_to",
                "input",
                "actual",
                "iteration",
                "status",
            ],
        ]
        .concat(),
    );
    let p = common::check_source(REFINE);
    let d = flt(&p, "pl.refine.take_many", &[json!(0), json!(-1)]);
    assert_envelope(
        &d,
        &[
            "span",
            "symbol",
            "qualified_name",
            "facet",
            "expected_type",
            "actual",
            "clause",
            "parameter",
            "pointer",
        ],
    );
    let p = common::check_source(DEPTH);
    let d = flt_with(&p, "pl.depth.down", &[json!(5)], &depth(2));
    assert_envelope(&d, &[&base[..], &["resource", "limit"]].concat());
    // `run_function` refuses a generator (T61): the stub is called directly.
    let p = common::check_source(RUN);
    let d = called_fault(&p, "std.gen.int", vec![]);
    assert_envelope(&d, &["kind", "detail"]);
}

#[test]
fn fault_spans_count_columns_in_unicode_scalar_values_s49() {
    // S49: `col` is 1-based and counted in Unicode scalar values. The line
    // holds three non-ASCII scalars (4 + 2 + 3 UTF-8 bytes) before `10 / n`.
    let p = common::check_source(TEXT);
    let d = flt(&p, "pl.text.wide_line", &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["span"], at(TEXT, "10 / n"));
    assert_eq!(d["span"]["col"], 25);
    assert_eq!(d["span"]["end_col"], 31);
}
