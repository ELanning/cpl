//! The contract stage and the release policy (`spec/semantics.md` 5.1,
//! 5.4, 5.5, 8.2, S32, S33, S35, S36, S47, S61, S63, S64;
//! `spec/toolchain.md` 3.4, 7.2): the rules of contract expressions, the
//! `decreases` warnings, the verification statuses, and
//! `verification_unsatisfied`, with the fields of 9.4.

mod common;

use common::*;
use cpl_diag::Severity;
use cpl_hir::{DefStatus, ExprKind, Program};
use cpl_sema::{CheckConfig, CheckResult, ProofStatus};
use serde_json::{json, Value};

fn line_of(source: &str, needle: &str) -> u32 {
    source
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?}")) as u32
        + 1
}

fn t(line: u32, severity: Severity, code: &str) -> (u32, Severity, String) {
    (line, severity, code.to_string())
}

fn with_code(result: &CheckResult, code: &str) -> Vec<Value> {
    json_values(result)
        .into_iter()
        .filter(|d| d["code"] == json!(code))
        .collect()
}

fn status(p: &Program, path: &str) -> DefStatus {
    p.defs()
        .find(|d| p.display_path(d.id) == path)
        .unwrap_or_else(|| panic!("no {path}"))
        .status
}

/// 5.1: no hole, no `?`, and only `let` statements in a contract
/// expression; S64: a contract hole is `hole_in_contract` alone, with no
/// hole report.
#[test]
fn contract_expression_rules() {
    let src = "module m.c

pub type Bounded = Int where self < ?limit

fn half(n: Int) -> Result<Int, Text> {
    Ok(n / 2)
}

fn a(n: Int) -> Result<Int, Text>
    requires half(n)? > 0
    ensures if n > 0 {
        var bound = 10
        bound = bound + 1
        n < bound
    } else {
        while false {
            assert true
        }
        true
    }
{
    half(n)
}

test \"loop clauses are contracts too\" {
    var i = 3
    while i > 0
        invariant ?inv
    {
        i = i - 1
    }
}

property \"ranges are contracts too\"
    for n in ?generator
{
    assert true
}
";
    let result = check_source(src);
    let e = Severity::Error;
    assert_eq!(
        project_triples(&result),
        [
            t(line_of(src, "where self < ?limit"), e, "hole_in_contract"),
            t(
                line_of(src, "requires half(n)?"),
                e,
                "propagate_in_contract"
            ),
            t(line_of(src, "var bound = 10"), e, "statement_in_contract"),
            t(
                line_of(src, "bound = bound + 1"),
                e,
                "statement_in_contract"
            ),
            t(line_of(src, "while false {"), e, "statement_in_contract"),
            t(line_of(src, "invariant ?inv"), e, "hole_in_contract"),
            t(line_of(src, "for n in ?generator"), e, "hole_in_contract"),
        ],
        "{}",
        render(&result)
    );
    assert!(result.holes.is_empty(), "no report for an error hole (S64)");
    let holes = with_code(&result, "hole_in_contract");
    assert_eq!(holes[0]["symbol"], json!("m.c.Bounded"));
    assert_eq!(holes[0]["facet"], json!("contract"));
    assert_eq!(holes[0]["clause"], json!("self < ?limit"));
    assert_eq!(holes[0]["hole"], json!("limit"));
    assert_eq!(holes[0]["span"]["col"], json!(37), "the hole");
    assert_eq!(holes[2]["clause"], json!("?generator"));
    let q = &with_code(&result, "propagate_in_contract")[0];
    assert_eq!(q["symbol"], json!("m.c.a"));
    assert_eq!(q["qualified_name"], json!("m.c.a"));
    assert_eq!(q["clause"], json!("half(n)? > 0"));
    assert_eq!(q["span"]["col"], json!(14), "the `?` expression");
    let s = with_code(&result, "statement_in_contract");
    assert_eq!(s[0]["facet"], json!("contract"));
    assert_eq!(s[0]["span"]["col"], json!(9), "the statement");
    // The `assert` inside the reported `while` is part of the same mistake.
    assert_eq!(s.len(), 3);
}

/// 4.5: an operation call in a contract expression is `effect_in_contract`
/// at the clause (S63 (2)), once per clause and effect.
#[test]
fn operation_calls_in_contracts() {
    let src = "module m.f

pub capability Store {
    fn get(key: Text) -> Int
}

fn a(key: Text, using db: Store) -> Int
    requires db.get(key) > 0 and db.get(key) < 10
    effects { db.get }
{
    var n = db.get(key)
    while n > 0
        invariant db.get(key) >= 0
    {
        n = n - 1
    }
    n
}
";
    let result = check_source(src);
    let e = Severity::Error;
    assert_eq!(
        project_triples(&result),
        [
            t(line_of(src, "requires db.get"), e, "effect_in_contract"),
            t(line_of(src, "invariant db.get"), e, "effect_in_contract"),
        ],
        "{}",
        render(&result)
    );
    let d = with_code(&result, "effect_in_contract");
    assert_eq!(d[0]["category"], json!("effect"));
    assert_eq!(d[0]["symbol"], json!("m.f.a"));
    assert_eq!(d[0]["facet"], json!("contract"));
    assert_eq!(
        d[0]["clause"],
        json!("db.get(key) > 0 and db.get(key) < 10")
    );
    assert_eq!(d[0]["effect"], json!("db.get"));
    assert_eq!(
        d[0]["span"]["col"],
        json!(5),
        "the clause, from its keyword"
    );
    assert_eq!(d[1]["clause"], json!("db.get(key) >= 0"));
}

/// S33: a clause may not call its own declaration (a loop clause included,
/// one diagnostic per clause); S61: a `@builtin` declaration's clauses may
/// (`std.text.trim`'s `ensures trim(result) == result`).
#[test]
fn recursive_contracts() {
    let src = "module m.r

fn a(n: Int) -> Int
    requires a(n) > 0 and a(n + 1) > 0
{
    n
}

fn b(n: Int) -> Int {
    var i = n
    while i > 0
        invariant b(i) >= 0
        decreases i
    {
        i = i - 1
    }
    i
}
";
    let result = check_source(src);
    let e = Severity::Error;
    assert_eq!(
        project_triples(&result),
        [
            t(line_of(src, "requires a(n)"), e, "recursive_contract"),
            t(line_of(src, "invariant b(i)"), e, "recursive_contract"),
        ],
        "{}",
        render(&result)
    );
    let d = &with_code(&result, "recursive_contract")[0];
    assert_eq!(d["symbol"], json!("m.r.a"));
    assert_eq!(d["clause"], json!("a(n) > 0 and a(n + 1) > 0"));
    assert_eq!(d["span"]["col"], json!(5));
    // The exemption is exercised by the standard library itself.
    let p = &result.program;
    let trim = p
        .defs()
        .find(|d| p.display_path(d.id) == "std.text.trim")
        .expect("std.text.trim");
    let f = trim.kind.as_fn().expect("a fn");
    let mut calls_itself = false;
    for c in &f.ensures {
        c.expr.walk(&mut |e| {
            calls_itself |= matches!(e.kind, ExprKind::Call { callee, .. } if callee == trim.id);
        });
    }
    assert!(calls_itself, "trim's ensures calls trim");
    assert_eq!(trim.status, DefStatus::Ok);
}

/// 5.4: `unused_decreases` at the clause, `missing_decreases` at the name
/// of a recursive `@prove` function without a measure and at every `while`
/// of a `@prove` body without one; a self-call only in the function's own
/// clauses is `recursive_contract` and does not make it recursive (S62).
#[test]
fn decreases_rules() {
    let src = "module m.d

fn measured(n: Int) -> Int
    decreases n
{
    n
}

@prove
fn ping(n: Int) -> Int
    decreases n
{
    if n <= 0 { 0 } else { pong(n - 1) }
}

@prove
fn pong(n: Int) -> Int {
    if n <= 0 { 0 } else { ping(n - 1) }
}

@prove
fn loops(n: Int) -> Int {
    var i = n
    while i > 0
        decreases i
    {
        var j = i
        while j > 0 {
            j = j - 1
        }
        i = i - 1
    }
    i
}

@prove
fn checks_itself(n: Int) -> Int
    ensures result == checks_itself(n)
{
    n
}
";
    let result = check_source(src);
    let (w, i, e) = (Severity::Warning, Severity::Info, Severity::Error);
    assert_eq!(
        project_triples(&result),
        [
            t(line_of(src, "    decreases n"), w, "unused_decreases"),
            t(line_of(src, "fn ping"), i, "verification_result"),
            t(line_of(src, "fn pong"), w, "missing_decreases"),
            t(line_of(src, "fn pong"), i, "verification_result"),
            t(line_of(src, "fn loops"), i, "verification_result"),
            t(line_of(src, "while j > 0"), w, "missing_decreases"),
            t(line_of(src, "fn checks_itself"), i, "verification_result"),
            t(
                line_of(src, "ensures result == checks_itself"),
                e,
                "recursive_contract"
            ),
        ],
        "{}",
        render(&result)
    );
    let u = &with_code(&result, "unused_decreases")[0];
    assert_eq!(u["symbol"], json!("m.d.measured"));
    assert_eq!(u["span"]["col"], json!(5), "the clause");
    let m = with_code(&result, "missing_decreases");
    assert_eq!(m[0]["symbol"], json!("m.d.pong"));
    assert_eq!(m[0]["span"]["col"], json!(4), "the name");
    assert_eq!(m[0]["span"]["end_col"], json!(8));
    assert_eq!(m[1]["symbol"], json!("m.d.loops"));
    assert_eq!(m[1]["span"]["col"], json!(9), "the loop, from `while`");
}

/// S36: one `verification_result` (info) per `@prove` declaration at its
/// name, with the verifier's status, mirrored by `CheckResult::verification`;
/// a missing measure leaves termination unknown (`no_measure`) and adds a
/// termination note.
#[test]
fn verification_statuses() {
    let src = "module m.v

@id(\"fact_proof\")
@prove
pub fn fact(n: Int) -> Int
    requires n >= 0
{
    if n <= 1 { 1 } else { n * fact(n - 1) }
}

@prove
fn id(n: Int) -> Int {
    n
}

fn plain(n: Int) -> Int {
    n
}
";
    let result = check_source(src);
    assert!(!result.has_errors(), "{}", render(&result));
    let p = &result.program;
    let statuses: Vec<(String, ProofStatus, Option<String>)> = result
        .verification
        .iter()
        .map(|v| (p.display_path(v.def), v.status, v.reason.clone()))
        .collect();
    assert_eq!(
        statuses,
        [
            (
                "m.v.fact".to_string(),
                ProofStatus::Unknown,
                Some("no_measure".to_string())
            ),
            ("m.v.id".to_string(), ProofStatus::Proved, None),
        ]
    );
    let v = with_code(&result, "verification_result");
    assert_eq!(v.len(), 2);
    assert_eq!(v[0]["category"], json!("contract"));
    assert_eq!(v[0]["severity"], json!("info"));
    assert_eq!(v[0]["symbol"], json!("fact_proof"), "the `@id`");
    assert_eq!(v[0]["qualified_name"], json!("m.v.fact"));
    assert_eq!(v[0]["facet"], json!("verification_policy"));
    assert_eq!(v[0]["status"], json!("unknown"));
    assert_eq!(v[0]["reason"], json!("no_measure"));
    assert_eq!(v[0]["message"], json!("fact: unknown (no measure)"));
    assert_eq!(v[1]["status"], json!("proved"));
    assert_eq!(v[1]["reason"], json!(null));
    assert_eq!(v[1]["message"], json!("id: proved"));
    assert_eq!(v[0]["span"]["line"], json!(line_of(src, "pub fn fact")));
    assert_eq!(v[0]["span"]["col"], json!(8), "the name");
    assert!(
        v[0]["notes"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("termination")),
        "fact is recursive without a measure"
    );
    assert!(v[1].get("notes").is_none_or(|n| n == &json!([])));
}

/// S36 (c), S47 (b): under `--release` every `@prove` declaration with
/// status `unknown` is the error `verification_unsatisfied` at its name;
/// development mode reports none. A proved one satisfies the policy.
#[test]
fn release_rejects_unproved_declarations() {
    let src = "module m.p

@prove
pub fn absolute(t: Text) -> Text
    ensures result.len >= t.len
{
    t ++ \"!\"
}

@prove
pub fn proved(n: Int) -> Int
    ensures result >= 0
{
    n.abs()
}

pub fn unproved_is_fine(n: Int) -> Int
    ensures result >= 0
{
    n.abs()
}
";
    let dev = check_source(src);
    assert!(!dev.has_errors(), "{}", render(&dev));
    assert!(with_code(&dev, "verification_unsatisfied").is_empty());
    assert_eq!(status(&dev.program, "m.p.absolute"), DefStatus::Ok);
    let release = check_source_with(
        src,
        &CheckConfig {
            release: true,
            ..CheckConfig::default()
        },
    );
    assert!(release.has_errors());
    let line = line_of(src, "pub fn absolute");
    assert_eq!(
        project_triples(&release),
        [
            t(line, Severity::Error, "verification_unsatisfied"),
            t(line, Severity::Info, "verification_result"),
            t(
                line_of(src, "pub fn proved"),
                Severity::Info,
                "verification_result"
            ),
            t(0, Severity::Warning, "project_lock_missing"),
        ],
        "{}",
        render(&release)
    );
    let d = &with_code(&release, "verification_unsatisfied")[0];
    assert_eq!(d["category"], json!("release_policy"));
    assert_eq!(d["symbol"], json!("m.p.absolute"));
    assert_eq!(d["facet"], json!("verification_policy"));
    assert_eq!(d["status"], json!("unknown"));
    assert_eq!(d["span"]["col"], json!(8));
    assert_eq!(
        status(&release.program, "m.p.absolute"),
        DefStatus::HasErrors
    );
    assert_eq!(
        status(&release.program, "m.p.unproved_is_fine"),
        DefStatus::Ok
    );
    assert_eq!(status(&release.program, "m.p.proved"), DefStatus::Ok);
}

/// S31: a capability operation's clauses are contract expressions (5.1);
/// they belong to the capability's `contract` facet (toolchain 5.1), which
/// is what the diagnostics name.
#[test]
fn operation_clauses_are_contract_expressions() {
    let src = "module m.o

pub fn half(n: Int) -> Result<Int, Text> {
    Ok(n / 2)
}

pub capability Store {
    fn get(key: Text) -> Int
        requires key.len > ?min
        ensures half(result)? >= 0
}
";
    let result = check_source(src);
    let e = Severity::Error;
    assert_eq!(
        project_triples(&result),
        [
            t(
                line_of(src, "requires key.len > ?min"),
                e,
                "hole_in_contract"
            ),
            t(
                line_of(src, "ensures half(result)?"),
                e,
                "propagate_in_contract"
            ),
        ],
        "{}",
        render(&result)
    );
    let d = with_code(&result, "hole_in_contract");
    assert_eq!(d[0]["symbol"], json!("m.o.Store"));
    assert_eq!(d[0]["facet"], json!("contract"));
    assert_eq!(d[0]["clause"], json!("key.len > ?min"));
    assert!(result.holes.is_empty());
}
