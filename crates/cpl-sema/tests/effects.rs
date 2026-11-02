//! The effect stage (`spec/semantics.md` 4.1-4.5, S26-S30, S62;
//! `spec/toolchain.md` 3.4): the bound check through forwarding (S28's σ),
//! the handle positions of S27, `unused_capability` (S29),
//! `EffectBound::inferred`, the fields of 9.4, and the S62 rules that keep
//! a diagnosed call or clause from cascading.

mod common;

use common::*;
use cpl_diag::Severity;
use cpl_hir::{DefStatus, Program};
use cpl_sema::CheckResult;
use serde_json::{json, Value};

const HEADER: &str = "module m.e

pub capability Store {
    fn get(key: Text) -> Int
    fn put(key: Text) -> Int
}

fn read(key: Text, using s: Store) -> Int
    effects { s.get }
{
    s.get(key)
}

";

fn check(body: &str) -> (String, CheckResult) {
    let source = format!("{HEADER}{body}");
    let result = check_source(&source);
    (source, result)
}

/// The 1-based line of the first line of `source` containing `needle`.
fn line_of(source: &str, needle: &str) -> u32 {
    source
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?}")) as u32
        + 1
}

/// (line, severity, code) of every project diagnostic.
fn got(result: &CheckResult) -> Vec<(u32, Severity, String)> {
    project_triples(result)
}

fn e(line: u32, code: &str) -> (u32, Severity, String) {
    (line, Severity::Error, code.to_string())
}

fn w(line: u32, code: &str) -> (u32, Severity, String) {
    (line, Severity::Warning, code.to_string())
}

/// The JSON of every diagnostic with `code`.
fn with_code(result: &CheckResult, code: &str) -> Vec<Value> {
    json_values(result)
        .into_iter()
        .filter(|d| d["code"] == json!(code))
        .collect()
}

fn inferred(p: &Program, path: &str) -> Vec<String> {
    let d = p
        .defs()
        .find(|d| p.display_path(d.id) == path)
        .unwrap_or_else(|| panic!("no {path}"));
    let f = d.kind.as_fn().expect("a fn");
    f.effects
        .inferred
        .iter()
        .map(|e| f.display_effect(e))
        .collect()
}

fn status(p: &Program, path: &str) -> DefStatus {
    p.defs()
        .find(|d| p.display_path(d.id) == path)
        .unwrap_or_else(|| panic!("no {path}"))
        .status
}

/// An operation call and a forwarded call outside a pure function's empty
/// bound (S26, 4.4), with the payload of 9.4.
#[test]
fn undeclared_effects_direct_and_forwarded() {
    let (src, result) = check(
        "fn a(key: Text, using db: Store) -> Int {
    db.put(key) + read(key, using db)
}
",
    );
    let line = line_of(&src, "db.put(key) + read");
    assert_eq!(
        got(&result),
        [e(line, "undeclared_effect"), e(line, "undeclared_effect")],
        "{}",
        render(&result)
    );
    let d = with_code(&result, "undeclared_effect");
    assert_eq!(d[0]["category"], json!("effect"));
    assert_eq!(d[0]["symbol"], json!("m.e.a"));
    assert_eq!(d[0]["qualified_name"], json!("m.e.a"));
    assert_eq!(d[0]["facet"], json!("effects"));
    assert_eq!(d[0]["effect"], json!("db.put"));
    assert_eq!(d[0]["callee"], json!("m.e.Store.put"));
    assert_eq!(d[0]["span"]["col"], json!(5), "the operation call");
    // The forwarded effect after substitution (S28): `s.get` with `s` = `db`.
    assert_eq!(d[1]["effect"], json!("db.get"));
    assert_eq!(d[1]["callee"], json!("m.e.read"));
    assert_eq!(d[1]["span"]["col"], json!(19), "the call");
    assert_eq!(status(&result.program, "m.e.a"), DefStatus::HasErrors);
    // What the body performs, whatever the bound says.
    assert_eq!(inferred(&result.program, "m.e.a"), ["db.get", "db.put"]);
    assert_eq!(inferred(&result.program, "m.e.read"), ["s.get"]);
}

/// `using` arguments are matched by position (S28), so the substituted
/// effects follow the order of the arguments, not their names.
#[test]
fn forwarding_substitutes_by_position() {
    let (src, result) = check(
        "fn both(key: Text, using x: Store, using y: Store) -> Int
    effects { x.get, y.put }
{
    x.get(key) + y.put(key)
}

fn swapped(key: Text, using first: Store, using second: Store) -> Int
    effects { first.put, second.get }
{
    both(key, using second, using first)
}

fn straight(key: Text, using first: Store, using second: Store) -> Int
    effects { first.put, second.get }
{
    both(key, using first, using second)
}
",
    );
    let line = line_of(&src, "both(key, using first, using second)");
    assert_eq!(
        got(&result),
        [e(line, "undeclared_effect"), e(line, "undeclared_effect")],
        "{}",
        render(&result)
    );
    let effects: Vec<Value> = with_code(&result, "undeclared_effect")
        .iter()
        .map(|d| d["effect"].clone())
        .collect();
    assert_eq!(effects, [json!("first.get"), json!("second.put")]);
    assert_eq!(
        inferred(&result.program, "m.e.swapped"),
        ["first.put", "second.get"]
    );
}

/// A bound wider than the body is not diagnosed (S29); `effects {}` is
/// pure like an absent clause (S26); a contract's operation call is not
/// part of the body's effects (4.4: contracts are excluded).
#[test]
fn wide_bounds_and_inferred_sets() {
    let (src, result) = check(
        "fn wide(key: Text, using db: Store) -> Int
    effects { db.get, db.put }
{
    db.get(key)
}

fn empty(key: Text, using db: Store) -> Int
    effects {}
{
    db.get(key)
}
",
    );
    let line = line_of(&src, "effects {}") + 2;
    assert_eq!(
        got(&result),
        [e(line, "undeclared_effect")],
        "{}",
        render(&result)
    );
    assert_eq!(inferred(&result.program, "m.e.wide"), ["db.get"]);
}

/// S27: a handle may be an operation receiver or a `using` argument; any
/// other occurrence is `capability_as_value`, an operand of `==`/`!=` is
/// `capability_equality` (one per handle operand, at that operand: S27's
/// table is per occurrence, S62 makes every misplaced handle its own S27
/// diagnostic, S63 (1) spans the operand), and a `using` argument in a
/// contract expression is `capability_as_value` (5.1) with no
/// `undeclared_effect` beside it (S62).
#[test]
fn handle_positions() {
    let (src, result) = check(
        "fn h(key: Text, using db: Store, using other: Store) -> Bool
    requires read(key, using db) > 0
    effects { db.get, other.get }
{
    let n = db.get(key) + other.get(key)
    let _ = [db]
    n > 0 and db != other
}
",
    );
    let requires = line_of(&src, "requires read(key, using db)");
    let list = line_of(&src, "let _ = [db]");
    let tail = line_of(&src, "n > 0 and db != other");
    assert_eq!(
        got(&result),
        [
            e(requires, "capability_as_value"),
            e(list, "capability_as_value"),
            e(tail, "capability_equality"),
            e(tail, "capability_equality"),
        ],
        "{}",
        render(&result)
    );
    let v = with_code(&result, "capability_as_value");
    assert_eq!(v[0]["name"], json!("db"));
    assert_eq!(v[0]["type"], json!("Store"));
    assert_eq!(v[0]["facet"], json!("contract"));
    assert_eq!(v[0]["span"]["col"], json!(30), "the `using` argument");
    assert_eq!(v[1]["facet"], json!("body"));
    assert_eq!(v[1]["span"]["col"], json!(14), "the list element");
    let eqs = with_code(&result, "capability_equality");
    let eq = &eqs[0];
    assert_eq!(eq["name"], json!("db"));
    assert_eq!(eq["type"], json!("Store"));
    assert_eq!(eq["symbol"], json!("m.e.h"));
    assert_eq!(eq["span"]["col"], json!(15), "the handle `db`");
    assert_eq!(eq["span"]["end_col"], json!(17));
    assert_eq!(eqs[1]["name"], json!("other"));
    assert_eq!(eqs[1]["span"]["col"], json!(21), "the handle `other`");
    assert_eq!(eqs[1]["span"]["end_col"], json!(26));
}

/// S29: a `using` parameter that appears in no operation call and no
/// `using` argument is a warning at the parameter; the `effects` clause is
/// not a use.
#[test]
fn unused_capabilities() {
    let (src, result) = check(
        "fn idle(key: Text, using db: Store) -> Int {
    key.len
}

fn clause_only(key: Text, using db: Store) -> Int
    effects { db.get }
{
    0
}

fn forwards(key: Text, using db: Store) -> Int
    effects { db.get }
{
    read(key, using db)
}
",
    );
    let idle = line_of(&src, "fn idle");
    let clause_only = line_of(&src, "fn clause_only");
    assert_eq!(
        got(&result),
        [
            w(idle, "unused_capability"),
            w(clause_only, "unused_capability")
        ],
        "{}",
        render(&result)
    );
    let d = &with_code(&result, "unused_capability")[0];
    assert_eq!(d["name"], json!("db"));
    assert_eq!(d["category"], json!("effect"));
    assert_eq!(d["symbol"], json!("m.e.idle"));
    assert_eq!(d["span"]["col"], json!(20), "the parameter");
    assert!(!result.has_errors());
}

/// S62: a diagnosed `effects` entry, a rejected `using` argument, a
/// missing `using` argument, and a missing body cause no second
/// diagnostic.
#[test]
fn no_cascades() {
    let (src, result) = check(
        "fn typo(key: Text, using db: Store) -> Int
    effects { db.gett }
{
    db.get(key)
}

fn misnamed(key: Text, using db: Store) -> Int
    effects { dbb.get }
{
    db.get(key)
}

fn forgot(key: Text, using db: Store) -> Int
    effects { db.get }
{
    read(key)
}

fn wrong_argument(key: Text, using db: Store) -> Int {
    read(key, using key)
}

fn bodiless(key: Text, using db: Store) -> Int
",
    );
    assert_eq!(
        got(&result),
        [
            e(
                line_of(&src, "effects { db.gett }"),
                "unknown_effect_operation"
            ),
            e(
                line_of(&src, "effects { dbb.get }"),
                "effect_param_not_capability"
            ),
            e(line_of(&src, "    read(key)"), "using_arity_mismatch"),
            e(
                line_of(&src, "read(key, using key)"),
                "using_argument_not_capability"
            ),
            e(line_of(&src, "fn bodiless"), "missing_body"),
        ],
        "{}",
        render(&result)
    );
}

/// A handle written in another form than its bare name is rejected (S28)
/// and has no substitution: `using (db)` and `using db(key)`.
#[test]
fn handles_in_other_shapes_are_not_using_arguments() {
    let (src, result) = check(
        "fn paren(key: Text, using db: Store) -> Int {
    read(key, using (db))
}

fn called(key: Text, using db: Store) -> Int {
    read(key, using db(key))
}
",
    );
    assert_eq!(
        got(&result),
        [
            e(line_of(&src, "using (db)"), "using_argument_not_capability"),
            e(
                line_of(&src, "using db(key)"),
                "using_argument_not_capability"
            ),
        ],
        "{}",
        render(&result)
    );
    let d = with_code(&result, "using_argument_not_capability");
    assert_eq!(d[0]["name"], json!("(db)"));
    assert_eq!(d[0]["type"], json!("Store"));
    assert!(d[0]["message"]
        .as_str()
        .is_some_and(|m| m.contains("write `using db`")));
}

/// Tests and properties are pure (S30): a call that needs a handle is
/// `using_arity_mismatch` and nothing else.
#[test]
fn tests_are_pure() {
    let (src, result) = check(
        "test \"no handles\" {
    assert read(\"k\") == 0
}
",
    );
    assert_eq!(
        got(&result),
        [e(line_of(&src, "assert read"), "using_arity_mismatch")],
        "{}",
        render(&result)
    );
}

/// 5.1: "no `using` argument (there is no handle to pass); in a contract
/// this is the only report for a `using` argument, whatever its form":
/// each argument below is one `capability_as_value` spanning the whole
/// argument (S63 (1)), never also `using_argument_not_capability` (for
/// `(db)`, `db(key)`, or an argument that is no handle), `type_mismatch`
/// (a handle of another capability), or `hole_in_contract` (the hole's one
/// report, S64, as S28's diagnostic is outside contracts), and no call is
/// checked against the bound (S62). `name` is the handle, or the text of
/// an argument that is none; `type` its type, `?` when erroneous (S69).
#[test]
fn a_using_argument_in_a_contract_is_capability_as_value_alone() {
    let (src, result) = check(
        "pub capability Other {
    fn put(key: Text) -> Int
}

fn forms(key: Text, using db: Store, using o: Other) -> Int
    requires read(key, using (db)) > 0
    requires read(key, using db(key)) > 0
    requires read(key, using key) > 0
    requires read(key, using key.nope) > 0
    requires read(key, using ?pre) > 0
    requires read(key, using o) > 0
    ensures result == read(key, using (db))
    effects { db.get, o.put }
{
    db.get(key) + o.put(key)
}
",
    );
    let forms = [
        ("using (db)) > 0", "(db)", "db", "Store"),
        ("using db(key)) > 0", "db(key)", "db", "Store"),
        ("using key) > 0", "key", "key", "Text"),
        ("using key.nope) > 0", "key.nope", "key.nope", "?"),
        ("using ?pre) > 0", "?pre", "?pre", "?"),
        ("using o) > 0", "o", "o", "Other"),
        (
            "ensures result == read(key, using (db))",
            "(db)",
            "db",
            "Store",
        ),
    ];
    let expected: Vec<_> = forms
        .iter()
        .map(|(needle, ..)| e(line_of(&src, needle), "capability_as_value"))
        .collect();
    assert_eq!(got(&result), expected, "{}", render(&result));
    assert!(result.holes.is_empty(), "no `hole` report for `?pre`");
    let d = with_code(&result, "capability_as_value");
    for (x, (needle, arg, name, ty)) in d.iter().zip(forms) {
        let line = src
            .lines()
            .find(|l| l.contains(needle))
            .expect("the clause line");
        let col = line.find(&format!("using {arg}")).expect("the argument") + "using ".len() + 1;
        assert_eq!(x["span"]["col"], json!(col), "{needle}");
        assert_eq!(x["span"]["end_col"], json!(col + arg.len()), "{needle}");
        assert_eq!(x["name"], json!(name), "{needle}");
        assert_eq!(x["type"], json!(ty), "{needle}");
        assert_eq!(x["symbol"], json!("m.e.forms"));
        assert_eq!(x["facet"], json!("contract"));
    }
}
