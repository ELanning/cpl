//! Adversarial tests of the effect stage, the contract stage, and the
//! release policy of `cpl-sema`, written from the specifications rather
//! than from the implementation:
//!
//! * `spec/semantics.md` 4: effect bounds (S26), the handle positions of
//!   S27, `using` arguments and forwarding with S28's positional
//!   substitution, the bound check of 4.4, `unused_capability` and
//!   `duplicate_effect` (S29), the purity of tests, properties, and
//!   contracts (S30, 4.5);
//! * `spec/semantics.md` 5: every purity code of 5.1 (`hole_in_contract`,
//!   `propagate_in_contract`, `effect_in_contract`, `capability_as_value`
//!   for a `using` argument, `statement_in_contract`, `recursive_contract`
//!   with the `@builtin` exemption of S61, `private_in_public_contract`,
//!   the S9 names), the `decreases` warnings of 5.4, and
//!   `verification_result` of S36;
//! * `spec/semantics.md` 8.2 with S47 and S64 (development and release
//!   modes), the payload fields of 9.4, the primary spans of S63, and the
//!   no-cascade rule of S62;
//! * `spec/toolchain.md` 11.2 / T37 (the release lock check) and 12.3 (the
//!   exact diagnostics of `examples/reader` and `examples/holes` in both
//!   modes, with `spec/semantics.md` Section 11).
//!
//! Every expectation encodes the specification's answer. Where the
//! specification is silent or ambiguous the test's comment says which
//! reading it takes and why.

mod common;

use common::*;
use cpl_diag::json::Digest;
use cpl_diag::Severity;
use cpl_hir::{DefStatus, Program};
use cpl_sema::{check_with, CheckConfig, CheckResult, ProofStatus};
use serde_json::{json, Value};

// ---- helpers ----------------------------------------------------------------

/// `cpl check --release` (toolchain 7.2: `CheckConfig::release`).
fn release() -> CheckConfig {
    CheckConfig {
        release: true,
        ..CheckConfig::default()
    }
}

/// The 1-based line of the `n`-th (1-based) line of `src` containing
/// `needle`.
fn line_nth(src: &str, needle: &str, n: usize) -> u32 {
    src.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .nth(n - 1)
        .map(|(i, _)| i as u32 + 1)
        .unwrap_or_else(|| panic!("no line {n} containing {needle:?}"))
}

/// The 1-based line of the first line of `src` containing `needle`.
fn line_of(src: &str, needle: &str) -> u32 {
    line_nth(src, needle, 1)
}

/// The 1-based column, in Unicode scalar values (S49), of the first
/// `needle` on the first line of `src` that contains `line_needle`.
fn col_of(src: &str, line_needle: &str, needle: &str) -> u32 {
    let line = src
        .lines()
        .find(|l| l.contains(line_needle))
        .unwrap_or_else(|| panic!("no line contains {line_needle:?}"));
    let at = line
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not on the line {line:?}"));
    line[..at].chars().count() as u32 + 1
}

fn err(line: u32, code: &str) -> Triple {
    (line, Severity::Error, code.to_string())
}

fn warn(line: u32, code: &str) -> Triple {
    (line, Severity::Warning, code.to_string())
}

fn info(line: u32, code: &str) -> Triple {
    (line, Severity::Info, code.to_string())
}

/// The project's diagnostics (std excluded; span-less ones on line 0) as a
/// sorted multiset of (line, severity, code): what toolchain 12.2 compares.
fn multiset(result: &CheckResult) -> Vec<Triple> {
    let mut v = project_triples(result);
    v.sort();
    v
}

#[track_caller]
fn assert_multiset(result: &CheckResult, expected: &[Triple]) {
    let mut want = expected.to_vec();
    want.sort();
    assert_eq!(multiset(result), want, "{}", render(result));
}

/// The S49 JSON of every diagnostic with `code`, in the result's order
/// (`Diagnostics::sorted`: span start first).
fn with_code(result: &CheckResult, code: &str) -> Vec<Value> {
    json_values(result)
        .into_iter()
        .filter(|d| d["code"] == json!(code))
        .collect()
}

/// The JSON of the one diagnostic with `code`.
#[track_caller]
fn one(result: &CheckResult, code: &str) -> Value {
    let mut v = with_code(result, code);
    assert_eq!(
        v.len(),
        1,
        "expected exactly one {code}: {}",
        render(result)
    );
    v.remove(0)
}

fn def_status(p: &Program, path: &str) -> DefStatus {
    p.defs()
        .find(|d| p.display_path(d.id) == path)
        .unwrap_or_else(|| panic!("no definition {path}"))
        .status
}

/// `EffectBound::inferred` of a function, rendered `param.op`, sorted.
fn inferred(p: &Program, path: &str) -> Vec<String> {
    let d = p
        .defs()
        .find(|d| p.display_path(d.id) == path)
        .unwrap_or_else(|| panic!("no definition {path}"));
    let f = d.kind.as_fn().expect("a fn");
    let mut v: Vec<String> = f
        .effects
        .inferred
        .iter()
        .map(|e| f.display_effect(e))
        .collect();
    v.sort();
    v
}

/// The prelude of the effect tests: two capabilities and helpers whose
/// bounds exercise S28's substitution. Every helper uses each of its
/// handles, so the prelude alone is clean (see
/// `effect_prelude_is_clean`).
const FX: &str = "module adv.fx

pub capability Store {
    fn get(key: Text) -> Int
    fn put(key: Text) -> Int
}

pub capability Log {
    fn write(msg: Text) -> Unit
}

fn read(key: Text, using s: Store) -> Int
    effects { s.get }
{
    s.get(key)
}

fn both(key: Text, using x: Store, using y: Store) -> Int
    effects { x.get, y.put }
{
    x.get(key) + y.put(key)
}

fn wide(key: Text, using s: Store) -> Int
    effects { s.get, s.put }
{
    s.get(key)
}

fn mix(key: Text, using s: Store, using l: Log) -> Int
    effects { s.get, l.write }
{
    l.write(key)
    s.get(key)
}

fn key_len(key: Text) -> Int {
    key.len
}

";

fn fx(body: &str) -> String {
    format!("{FX}{body}")
}

// ---- 4.1, 4.3, 4.4: bounds and forwarding --------------------------------------

/// The prelude checks clean, and `EffectBound::inferred` holds what each
/// body performs: a wide bound (S29) is not narrowed by it.
#[test]
fn effect_prelude_is_clean() {
    let r = check_source(FX);
    assert_multiset(&r, &[]);
    assert_eq!(inferred(&r.program, "adv.fx.wide"), ["s.get"]);
    assert_eq!(inferred(&r.program, "adv.fx.mix"), ["l.write", "s.get"]);
    assert_eq!(inferred(&r.program, "adv.fx.both"), ["x.get", "y.put"]);
}

/// 4.1, 4.4: a bound is a set of `p.op` pairs over the function's own
/// handles, so `a.get` does not cover `b.get` on a second handle of the
/// same capability. Payload of 9.4: `symbol`, `facet: "effects"`, `effect`,
/// `callee` (the operation's qualified name, 1.4); S63 (1): the call.
#[test]
fn a_bound_names_handles_not_capabilities() {
    let src = fx("fn two(key: Text, using a: Store, using b: Store) -> Int
    effects { a.get }
{
    a.get(key) + b.get(key)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "a.get(key) + b.get(key)");
    assert_multiset(&r, &[err(line, "undeclared_effect")]);
    let d = one(&r, "undeclared_effect");
    assert_eq!(d["category"], json!("effect"));
    assert_eq!(d["severity"], json!("error"));
    assert_eq!(d["symbol"], json!("adv.fx.two"));
    assert_eq!(d["qualified_name"], json!("adv.fx.two"));
    assert_eq!(d["facet"], json!("effects"));
    assert_eq!(d["effect"], json!("b.get"));
    assert_eq!(d["callee"], json!("adv.fx.Store.get"));
    let col = col_of(&src, "a.get(key) + b.get(key)", "b.get(key)");
    assert_eq!(d["span"]["line"], json!(line));
    assert_eq!(d["span"]["col"], json!(col));
    assert_eq!(d["span"]["end_col"], json!(col + 10));
    assert_eq!(def_status(&r.program, "adv.fx.two"), DefStatus::HasErrors);
}

/// 4.4: `B_g` is the callee's *declared* bound, not its body ("checking is
/// modular ... widening g's bound does" invalidate callers): `wide`
/// declares `s.put` without performing it, and a caller must cover it.
#[test]
fn forwarding_checks_the_callees_declared_bound() {
    let src = fx("fn caller(key: Text, using db: Store) -> Int
    effects { db.get }
{
    wide(key, using db)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "wide(key, using db)");
    assert_multiset(&r, &[err(line, "undeclared_effect")]);
    let d = one(&r, "undeclared_effect");
    assert_eq!(d["symbol"], json!("adv.fx.caller"));
    assert_eq!(d["facet"], json!("effects"));
    assert_eq!(d["effect"], json!("db.put"));
    assert_eq!(d["callee"], json!("adv.fx.wide"));
    assert_eq!(d["span"]["col"], json!(5), "the call (S63 (1))");
    assert_eq!(d["span"]["end_col"], json!(5 + "wide(key, using db)".len()));
}

/// S28: σ maps the callee's `using` parameters to the arguments by
/// position, so one handle passed to two parameters receives the
/// effects of both (`x.get` and `y.put` both land on `db`).
#[test]
fn one_handle_forwarded_to_two_parameters() {
    let src = fx("fn short(key: Text, using db: Store) -> Int
    effects { db.get }
{
    both(key, using db, using db)
}

fn full(key: Text, using db: Store) -> Int
    effects { db.get, db.put }
{
    both(key, using db, using db)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "both(key, using db, using db)");
    assert_multiset(&r, &[err(line, "undeclared_effect")]);
    let d = one(&r, "undeclared_effect");
    assert_eq!(d["symbol"], json!("adv.fx.short"));
    assert_eq!(d["effect"], json!("db.put"));
    assert_eq!(d["callee"], json!("adv.fx.both"));
    assert_eq!(inferred(&r.program, "adv.fx.full"), ["db.get", "db.put"]);
    assert_eq!(inferred(&r.program, "adv.fx.short"), ["db.get", "db.put"]);
}

/// S28 over two capabilities: `mix`'s `l.write` becomes `out.write` in the
/// caller, which its bound lacks.
#[test]
fn forwarding_substitutes_each_capability_separately() {
    let src = fx("fn logs(key: Text, using db: Store, using out: Log) -> Int
    effects { db.get }
{
    mix(key, using db, using out)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "mix(key, using db, using out)");
    assert_multiset(&r, &[err(line, "undeclared_effect")]);
    let d = one(&r, "undeclared_effect");
    assert_eq!(d["effect"], json!("out.write"));
    assert_eq!(d["callee"], json!("adv.fx.mix"));
}

/// S28: each `using` argument's capability must be identical to the
/// parameter's (`type_mismatch`, at the argument, S63 (1)); S62: a call
/// whose `using` arguments were rejected has no σ and is not checked
/// against the (empty) bound of the pure caller, so no `undeclared_effect`
/// accompanies the two mismatches, and both handles appear in `using`
/// arguments, so neither is `unused_capability` (S29).
#[test]
fn using_arguments_of_the_wrong_capability_have_no_substitution() {
    let src = fx(
        "fn swapped(key: Text, using db: Store, using out: Log) -> Int {
    mix(key, using out, using db)
}
",
    );
    let r = check_source(&src);
    let line = line_of(&src, "mix(key, using out, using db)");
    assert_multiset(
        &r,
        &[err(line, "type_mismatch"), err(line, "type_mismatch")],
    );
    let d = with_code(&r, "type_mismatch");
    assert_eq!(d[0]["expected"], json!("Store"));
    assert_eq!(d[0]["actual"], json!("Log"));
    assert_eq!(
        d[0]["span"]["col"],
        json!(col_of(&src, "mix(key, using out, using db)", "out"))
    );
    assert_eq!(d[1]["expected"], json!("Log"));
    assert_eq!(d[1]["actual"], json!("Store"));
    assert_eq!(
        d[1]["span"]["col"],
        json!(col_of(&src, "mix(key, using out, using db)", "using db") + 6)
    );
}

/// S26: a pure function may call a function whose bound is empty after
/// substitution, even when it passes a handle; `effects {}` is the empty
/// bound. S29: the callee's handle that appears in no operation call and
/// no `using` argument is `unused_capability` at the parameter.
#[test]
fn forwarding_to_an_empty_bound_needs_no_effect() {
    let src = fx("fn quiet(key: Text, using s: Store) -> Int
    effects {}
{
    key.len
}

fn pure_forward(key: Text, using db: Store) -> Int {
    quiet(key, using db)
}
");
    let r = check_source(&src);
    assert_multiset(&r, &[warn(line_of(&src, "fn quiet"), "unused_capability")]);
    let d = one(&r, "unused_capability");
    assert_eq!(d["name"], json!("s"));
    assert_eq!(d["category"], json!("effect"));
    assert_eq!(
        d["span"]["col"],
        json!(col_of(&src, "fn quiet", "using s")),
        "the parameter (S63 (3))"
    );
    assert!(inferred(&r.program, "adv.fx.pure_forward").is_empty());
}

/// 4.4 through a chain: every level is checked against its callee's
/// declared bound only, so the error is where the chain enters a pure
/// function, and the substituted effect is carried up (`inferred`).
#[test]
fn forwarding_is_checked_at_every_level() {
    let src = fx("fn level1(key: Text, using t: Store) -> Int
    effects { t.get }
{
    read(key, using t)
}

fn level2(key: Text, using db: Store) -> Int
    effects { db.get }
{
    level1(key, using db)
}

fn level3(key: Text, using db: Store) -> Int {
    level2(key, using db)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "level2(key, using db)");
    assert_multiset(&r, &[err(line, "undeclared_effect")]);
    let d = one(&r, "undeclared_effect");
    assert_eq!(d["symbol"], json!("adv.fx.level3"));
    assert_eq!(d["effect"], json!("db.get"));
    assert_eq!(d["callee"], json!("adv.fx.level2"));
    assert_eq!(inferred(&r.program, "adv.fx.level1"), ["t.get"]);
    assert_eq!(inferred(&r.program, "adv.fx.level2"), ["db.get"]);
}

/// 2.6: an operation the capability lacks is `unknown_member` (`name`,
/// `type`, 9.4); there is no operation, hence no effect to check (S62).
#[test]
fn an_unknown_operation_is_unknown_member_alone() {
    let src = fx("fn deleting(key: Text, using db: Store) -> Int
    effects { db.get }
{
    db.get(key) + db.delete(key)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "db.delete(key)");
    assert_multiset(&r, &[err(line, "unknown_member")]);
    let d = one(&r, "unknown_member");
    assert_eq!(d["name"], json!("delete"));
    assert_eq!(d["type"], json!("Store"));
}

/// 4.4: the same `effect_ref` twice is the warning `duplicate_effect`
/// (`effect`, 9.4) spanning the repeated reference (S63 (2)); the bound
/// still holds it.
#[test]
fn duplicate_effect_spans_the_repeated_reference() {
    let src = fx("fn dup(key: Text, using db: Store) -> Int
    effects { db.get, db.get }
{
    db.get(key)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "effects { db.get, db.get }");
    assert_multiset(&r, &[warn(line, "duplicate_effect")]);
    let d = one(&r, "duplicate_effect");
    assert_eq!(d["effect"], json!("db.get"));
    let col = col_of(&src, "effects { db.get, db.get }", ", db.get") + 2;
    assert_eq!(d["span"]["col"], json!(col));
    assert_eq!(d["span"]["end_col"], json!(col + 6));
}

/// 4.1: a `using` parameter's type must be a capability
/// (`using_param_not_capability`, `name`, `type`, at the parameter); it is
/// then no handle, so it is never `unused_capability` (S62).
#[test]
fn using_param_not_capability_payload() {
    let src = fx("fn not_cap(key: Text, using n: Int) -> Int {
    key.len
}
");
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[err(
            line_of(&src, "fn not_cap"),
            "using_param_not_capability",
        )],
    );
    let d = one(&r, "using_param_not_capability");
    assert_eq!(d["category"], json!("effect"));
    assert_eq!(d["name"], json!("n"));
    assert_eq!(d["type"], json!("Int"));
    assert_eq!(
        d["span"]["col"],
        json!(col_of(&src, "fn not_cap", "using n"))
    );
}

/// 4.3: "A capability operation has no `using` parameters, so an
/// operation call never carries `using` arguments", and S28's
/// `using_arity_mismatch` applies "also when g has none and some are
/// passed": an operation call or a prelude method call with a `using`
/// argument is `using_arity_mismatch` (`callee` the operation's or the
/// member's qualified name, `expected` 0, `actual` 1, 9.4).
#[test]
fn an_operation_call_never_carries_using_arguments() {
    let src = fx(r#"fn op_using(key: Text, using db: Store) -> Int
    effects { db.get }
{
    db.get(key, using db)
}

fn method_using(xs: List<Int>, using db: Store) -> Int
    effects { db.get }
{
    xs.take(1, using db).len + db.get("k")
}
"#);
    let r = check_source(&src);
    let op = line_of(&src, "db.get(key, using db)");
    let method = line_of(&src, "xs.take(1, using db)");
    assert_multiset(
        &r,
        &[
            err(op, "using_arity_mismatch"),
            err(method, "using_arity_mismatch"),
        ],
    );
    let d = with_code(&r, "using_arity_mismatch");
    assert_eq!(d[0]["callee"], json!("adv.fx.Store.get"));
    assert_eq!(d[0]["expected"], json!(0));
    assert_eq!(d[0]["actual"], json!(1));
    assert_eq!(d[1]["callee"], json!("std.prelude.List.take"));
    assert_eq!(d[1]["expected"], json!(0));
    assert_eq!(d[1]["actual"], json!(1));
}

/// 4.4 checks "every call in f's body, contracts excluded": a `forall` in
/// a body is an ordinary expression (only clauses, `where` predicates, and
/// `for` ranges are contract expressions, 5.1), so a call in its predicate
/// is checked against the bound.
#[test]
fn a_forall_in_a_body_is_checked_against_the_bound() {
    let src = fx("fn all_read(xs: List<Text>, using db: Store) -> Bool {
    forall x in xs: read(x, using db) > 0
}
");
    let r = check_source(&src);
    let line = line_of(&src, "forall x in xs: read(x, using db)");
    assert_multiset(&r, &[err(line, "undeclared_effect")]);
    let d = one(&r, "undeclared_effect");
    assert_eq!(d["effect"], json!("db.get"));
    assert_eq!(d["callee"], json!("adv.fx.read"));
    assert_eq!(
        d["span"]["col"],
        json!(col_of(&src, "forall x in xs", "read(")),
        "the call"
    );
}

/// 3.8: `( e )` has the type of `e`, and S27 admits a handle "as the
/// receiver of an operation call" with no bareness condition (only S28's
/// `using` arguments must be a bare name). `(db).get(key)` is therefore an
/// operation call through `db`: it performs `db.get` (undeclared in a pure
/// function, `inferred` otherwise), and `db` appears in an operation call,
/// so it is not `unused_capability` (S29). Under no reading can one call
/// both perform `db.get` through `db` and leave `db` unused.
#[test]
fn a_parenthesized_receiver_is_an_operation_call_on_its_handle() {
    let src = fx("fn paren_pure(key: Text, using db: Store) -> Int {
    (db).get(key)
}

fn paren_declared(key: Text, using db: Store) -> Int
    effects { db.get }
{
    (db).get(key)
}
");
    let r = check_source(&src);
    let pure_line = line_nth(&src, "(db).get(key)", 1);
    assert_multiset(&r, &[err(pure_line, "undeclared_effect")]);
    assert_eq!(inferred(&r.program, "adv.fx.paren_declared"), ["db.get"]);
}

/// S28 across modules: σ applies to an imported or a fully qualified
/// callee exactly as to a local one, and "identical" capability types are
/// nominal: a module's own `Store` is not `adv.lib.Store`, so passing a
/// handle of it is `type_mismatch` with no substitution and no effect
/// check (S62).
#[test]
fn forwarding_across_modules() {
    let lib = "module adv.lib

pub capability Store {
    fn get(key: Text) -> Int
}

pub fn fetch(key: Text, using s: Store) -> Int
    effects { s.get }
{
    s.get(key)
}
";
    let app = "module adv.app

use adv.lib.{fetch}

pub capability Store {
    fn get(key: Text) -> Int
}

pub fn pure_app(key: Text, using db: adv.lib.Store) -> Int {
    fetch(key, using db)
}

pub fn qualified(key: Text, using db: adv.lib.Store) -> Int
    effects { db.get }
{
    adv.lib.fetch(key, using db)
}

pub fn local_store(key: Text, using mine: Store) -> Int
    effects { mine.get }
{
    mine.get(key) + fetch(key, using mine)
}
";
    let r = check_files(&[("src/adv/lib.cpl", lib), ("src/adv/app.cpl", app)]);
    assert_multiset(
        &r,
        &[
            err(
                line_of(app, "    fetch(key, using db)"),
                "undeclared_effect",
            ),
            err(line_of(app, "fetch(key, using mine)"), "type_mismatch"),
        ],
    );
    let d = one(&r, "undeclared_effect");
    assert_eq!(d["symbol"], json!("adv.app.pure_app"));
    assert_eq!(d["effect"], json!("db.get"));
    assert_eq!(d["callee"], json!("adv.lib.fetch"));
    assert_eq!(d["span"]["file"], json!("src/adv/app.cpl"));
    assert_eq!(inferred(&r.program, "adv.app.qualified"), ["db.get"]);
}

// ---- 4.2, S27: no storage of handles -------------------------------------------

/// S27: a `var` initializer and an assignment's right-hand side are value
/// positions; each handle there is `capability_as_value` (`name`, `type`)
/// at the handle (S63 (1)), and the error-typed `var` causes nothing more
/// (S62).
#[test]
fn handles_in_var_initializers_and_assignments() {
    let src = fx("fn v(key: Text, using db: Store) -> Int
    effects { db.get }
{
    var _n = db
    _n = db
    db.get(key)
}
");
    let r = check_source(&src);
    let init = line_of(&src, "var _n = db");
    let assign = line_of(&src, "    _n = db");
    assert_multiset(
        &r,
        &[
            err(init, "capability_as_value"),
            err(assign, "capability_as_value"),
        ],
    );
    let d = with_code(&r, "capability_as_value");
    for x in &d {
        assert_eq!(x["category"], json!("effect"));
        assert_eq!(x["name"], json!("db"));
        assert_eq!(x["type"], json!("Store"));
        assert_eq!(x["symbol"], json!("adv.fx.v"));
    }
    assert_eq!(d[0]["span"]["col"], json!(14));
    assert_eq!(d[0]["span"]["end_col"], json!(16));
    assert_eq!(d[1]["span"]["col"], json!(10));
}

/// S27: storing a handle in a struct field initializer is
/// `capability_as_value`, with no `type_mismatch` against the `Int` field
/// (S62).
#[test]
fn a_handle_cannot_be_stored_in_a_struct_field() {
    let src = fx("struct Holder {
    count: Int
}

fn s(key: Text, using db: Store) -> Int
    effects { db.get }
{
    let h = Holder { count: db }
    h.count + db.get(key)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "Holder { count: db }");
    assert_multiset(&r, &[err(line, "capability_as_value")]);
    let d = one(&r, "capability_as_value");
    assert_eq!(
        d["span"]["col"],
        json!(col_of(&src, "Holder { count: db }", "db"))
    );
}

/// S27: a handle cannot be returned inside a result: a constructor
/// payload (`Ok(db)`) is `capability_as_value` alone.
#[test]
fn a_handle_cannot_be_returned_in_a_result() {
    let src = fx("fn r(key: Text, using db: Store) -> Result<Int, Text>
    effects { db.get }
{
    let _ = db.get(key)
    Ok(db)
}
");
    let r = check_source(&src);
    assert_multiset(&r, &[err(line_of(&src, "Ok(db)"), "capability_as_value")]);
    assert_eq!(one(&r, "capability_as_value")["span"]["col"], json!(8));
}

/// S27: a `forall` range is a value position; the range is
/// `capability_as_value` and not also `forall_range_not_list` (S62).
#[test]
fn a_handle_is_not_a_forall_range() {
    let src = fx("fn q(key: Text, using db: Store) -> Bool
    effects { db.get }
{
    let _ = db.get(key)
    forall x in db: x > 0
}
");
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[err(line_of(&src, "forall x in db"), "capability_as_value")],
    );
    assert_eq!(one(&r, "capability_as_value")["span"]["col"], json!(17));
}

/// S27: a wrapper constructor payload is a value position.
#[test]
fn a_handle_cannot_be_wrapped() {
    let src = fx("type Wrapped(Int)

fn w(key: Text, using db: Store) -> Int
    effects { db.get }
{
    let _ = Wrapped(db)
    db.get(key)
}
");
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[err(line_of(&src, "Wrapped(db)"), "capability_as_value")],
    );
}

/// S13 with 1.8 and 9.4 (`not_a_value_type` is unreachable in 0.1, "a
/// capability is `capability_in_value_position`"): a `let` annotated with
/// a capability type is `capability_in_value_position` at the type, and
/// the handle initializing it is still `capability_as_value` (S62: "every
/// misplaced handle is its S27 diagnostic"). A handle passed to a generic
/// function is one `capability_as_value`, never an instantiation with a
/// non-value type.
#[test]
fn handles_in_annotated_lets_and_generic_arguments() {
    let src = fx("fn ident<T>(x: T) -> T {
    x
}

fn annotated(key: Text, using db: Store) -> Int
    effects { db.get }
{
    let _x: Store = db
    let _ = ident(db)
    db.get(key)
}
");
    let r = check_source(&src);
    let ann = line_of(&src, "let _x: Store = db");
    let gen = line_of(&src, "ident(db)");
    assert_multiset(
        &r,
        &[
            err(ann, "capability_in_value_position"),
            err(ann, "capability_as_value"),
            err(gen, "capability_as_value"),
        ],
    );
    let pos = one(&r, "capability_in_value_position");
    assert_eq!(
        pos["span"]["col"],
        json!(col_of(&src, "let _x: Store = db", "Store")),
        "the offending type (S63 (3))"
    );
    let v = with_code(&r, "capability_as_value");
    assert_eq!(
        v[0]["span"]["col"],
        json!(col_of(&src, "let _x: Store = db", "db"))
    );
    assert_eq!(v[1]["span"]["col"], json!(col_of(&src, "ident(db)", "db")));
}

/// S27's table maps each *occurrence* of a handle as an operand of
/// `==`/`!=` to `capability_equality` (`name`, `type`: one handle per
/// diagnostic, 9.4), and S62 keeps every misplaced handle its own S27
/// diagnostic (the generator analogue: `gen.int == gen.int` carries two).
/// So comparing two handles is two diagnostics, each at its operand
/// (S63 (1): "the operand that failed a check").
#[test]
fn capability_equality_is_one_per_handle_operand() {
    let src = fx(
        "fn eq(key: Text, using db: Store, using other: Store) -> Bool
    effects { db.get, other.get }
{
    let _ = db.get(key) + other.get(key)
    db == other
}
",
    );
    let r = check_source(&src);
    let line = line_of(&src, "db == other");
    assert_multiset(
        &r,
        &[
            err(line, "capability_equality"),
            err(line, "capability_equality"),
        ],
    );
    let d = with_code(&r, "capability_equality");
    let names: Vec<&Value> = d.iter().map(|x| &x["name"]).collect();
    assert_eq!(names, [&json!("db"), &json!("other")]);
    assert_eq!(d[0]["span"]["col"], json!(5));
    assert_eq!(d[0]["span"]["end_col"], json!(7));
    assert_eq!(d[1]["span"]["col"], json!(11));
    assert_eq!(d[1]["span"]["end_col"], json!(16));
}

/// S17, S27, S63 (1): comparing a handle with a value is one
/// `capability_equality` at the handle operand, on either side, and no
/// `type_mismatch` (S62: the operand already failed).
#[test]
fn capability_equality_spans_the_handle_and_does_not_cascade() {
    let src = fx("fn cmp_left(key: Text, using db: Store) -> Bool
    effects { db.get }
{
    let n = db.get(key)
    n == db
}

fn cmp_right(key: Text, using db: Store) -> Bool
    effects { db.get }
{
    let _ = db.get(key)
    db != 3
}
");
    let r = check_source(&src);
    let left = line_of(&src, "n == db");
    let right = line_of(&src, "db != 3");
    assert_multiset(
        &r,
        &[
            err(left, "capability_equality"),
            err(right, "capability_equality"),
        ],
    );
    let d = with_code(&r, "capability_equality");
    assert_eq!(d[0]["name"], json!("db"));
    assert_eq!(d[0]["type"], json!("Store"));
    assert_eq!(d[0]["span"]["col"], json!(10), "the handle `db`");
    assert_eq!(d[0]["span"]["end_col"], json!(12));
    assert_eq!(d[1]["span"]["col"], json!(5));
    assert_eq!(d[1]["span"]["end_col"], json!(7));
}

/// S29 is syntactic: a `using` parameter "that appears in no operation
/// call and no `using` argument" is unused, so a handle whose only
/// occurrence is a rejected value position is both `capability_as_value`
/// (S27) and `unused_capability`: removing the stray occurrence, the fix
/// of the first, leaves the second standing (no cascade, S62).
#[test]
fn a_handle_only_stored_is_still_unused() {
    let src = fx("fn stash(key: Text, using db: Store) -> Int {
    let _ = db
    key.len
}
");
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[
            warn(line_of(&src, "fn stash"), "unused_capability"),
            err(line_of(&src, "let _ = db"), "capability_as_value"),
        ],
    );
}

/// S27: "every other occurrence is an error": a handle as an `if`
/// condition, an arithmetic or `not` operand, or a list element (also
/// inside an equality of lists) is `capability_as_value` at the handle,
/// with no `type_mismatch` (S62) and no `capability_equality` (the handles
/// are list elements, not operands of `==`).
#[test]
fn handles_as_operands_are_values() {
    let src = fx("fn cond(key: Text, using db: Store) -> Int
    effects { db.get }
{
    if db { db.get(key) } else { 0 }
}

fn arith(key: Text, using db: Store) -> Int
    effects { db.get }
{
    db + db.get(key)
}

fn negate(key: Text, using db: Store) -> Bool
    effects { db.get }
{
    let _ = db.get(key)
    not db
}

fn lists(key: Text, using db: Store) -> Bool
    effects { db.get }
{
    let _ = db.get(key)
    [db] == [db]
}
");
    let r = check_source(&src);
    let lists = line_of(&src, "[db] == [db]");
    assert_multiset(
        &r,
        &[
            err(line_of(&src, "if db {"), "capability_as_value"),
            err(line_of(&src, "db + db.get(key)"), "capability_as_value"),
            err(line_of(&src, "not db"), "capability_as_value"),
            err(lists, "capability_as_value"),
            err(lists, "capability_as_value"),
        ],
    );
}

// ---- S28: `using` arguments ----------------------------------------------------

/// S28: the `using` argument count must equal the callee's `using`
/// parameter count, in both directions; `using_arity_mismatch` carries
/// `callee` (qualified name), `expected`, and `actual` (counts, 9.4) and
/// spans the call (S63 (1)); S62: no effect check follows.
#[test]
fn using_arity_mismatch_payload_in_both_directions() {
    let src = fx("fn forgot(key: Text, using db: Store) -> Int
    effects { db.get }
{
    db.get(key) + read(key)
}

fn extra(key: Text, using db: Store) -> Int
    effects { db.get }
{
    db.get(key) + key_len(key, using db)
}
");
    let r = check_source(&src);
    let forgot = line_of(&src, "db.get(key) + read(key)");
    let extra = line_of(&src, "key_len(key, using db)");
    assert_multiset(
        &r,
        &[
            err(forgot, "using_arity_mismatch"),
            err(extra, "using_arity_mismatch"),
        ],
    );
    let d = with_code(&r, "using_arity_mismatch");
    assert_eq!(d[0]["callee"], json!("adv.fx.read"));
    assert_eq!(d[0]["expected"], json!(1));
    assert_eq!(d[0]["actual"], json!(0));
    let col = col_of(&src, "db.get(key) + read(key)", "read(key)");
    assert_eq!(d[0]["span"]["col"], json!(col));
    assert_eq!(d[0]["span"]["end_col"], json!(col + 9));
    assert_eq!(d[1]["callee"], json!("adv.fx.key_len"));
    assert_eq!(d[1]["expected"], json!(0));
    assert_eq!(d[1]["actual"], json!(1));
}

/// 5.1 ("no `using` argument (there is no handle to pass)") in every kind
/// of contract clause, the loop clauses included (5.1: `invariant`,
/// `decreases`): each is `capability_as_value` at the argument (S63 (1))
/// and, by S62, never also `undeclared_effect`.
#[test]
fn using_arguments_in_every_contract_clause_kind() {
    let src = fx("fn guarded(key: Text, using db: Store) -> Int
    requires read(key, using db) >= 0
    ensures result == read(key, using db)
    effects { db.get }
{
    var n = db.get(key)
    while n > 0
        invariant read(key, using db) >= 0
        decreases n + read(key, using db)
    {
        n = n - 1
    }
    n
}
");
    let r = check_source(&src);
    let needles = [
        "requires read(key, using db)",
        "ensures result == read(key, using db)",
        "invariant read(key, using db)",
        "decreases n + read(key, using db)",
    ];
    let expected: Vec<Triple> = needles
        .iter()
        .map(|n| err(line_of(&src, n), "capability_as_value"))
        .collect();
    assert_multiset(&r, &expected);
    let d = with_code(&r, "capability_as_value");
    for (x, needle) in d.iter().zip(needles) {
        assert_eq!(x["name"], json!("db"));
        assert_eq!(x["type"], json!("Store"));
        assert_eq!(
            x["span"]["col"],
            json!(col_of(&src, needle, "using db") + 6),
            "the `using` argument of {needle:?}"
        );
    }
}

/// 4.5 with S62: a contract that calls an effectful function without the
/// handle it needs is `using_arity_mismatch` and nothing else (no σ, so no
/// `undeclared_effect`; no operation call, so no `effect_in_contract`).
#[test]
fn a_contract_call_without_its_using_argument_is_an_arity_mismatch_alone() {
    let src = fx("fn needs(key: Text, using db: Store) -> Int
    requires read(key) >= 0
    effects { db.get }
{
    db.get(key)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "requires read(key)");
    assert_multiset(&r, &[err(line, "using_arity_mismatch")]);
    let d = one(&r, "using_arity_mismatch");
    assert_eq!(d["span"]["col"], json!(14), "the call");
}

/// 5.1: "no `using` argument (there is no handle to pass)" holds even
/// when the callee's bound is empty: the rule is about passing a handle,
/// not about the callee's effects.
#[test]
fn no_using_argument_in_a_contract_even_for_an_effect_free_callee() {
    let src = fx("fn quiet(key: Text, using s: Store) -> Int
    effects {}
{
    key.len
}

fn checks(key: Text, using db: Store) -> Int
    requires quiet(key, using db) >= 0
    effects { db.get }
{
    db.get(key)
}
");
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[
            warn(line_of(&src, "fn quiet"), "unused_capability"),
            err(
                line_of(&src, "requires quiet(key, using db)"),
                "capability_as_value",
            ),
        ],
    );
}

/// S30, 4.5: properties and refinement predicates have no handle, so a
/// call that needs one is `using_arity_mismatch` (in a `where` predicate
/// and a `for` range, which are contract expressions) and a `using`
/// argument naming a `for` binder is `using_argument_not_capability`
/// (`name`, `type`); none of them is also `undeclared_effect` (S62).
#[test]
fn properties_and_refinements_cannot_supply_handles() {
    let src = fx(r#"type Readable = Int where read("k") > self

property "no handle in a range"
    for n in gen.range(0, read("k"))
{
    assert n == n
}

property "no handle can be named"
    for n in gen.int
{
    assert read("k", using n) == n
}
"#);
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[
            err(line_of(&src, "where read(\"k\")"), "using_arity_mismatch"),
            err(line_of(&src, "gen.range(0, read"), "using_arity_mismatch"),
            err(
                line_of(&src, "using n) == n"),
                "using_argument_not_capability",
            ),
        ],
    );
    let d = one(&r, "using_argument_not_capability");
    assert_eq!(d["name"], json!("n"));
    assert_eq!(d["type"], json!("Int"));
    assert_eq!(d["span"]["col"], json!(col_of(&src, "using n) == n", "n)")));
}

/// S30 with S62: a test cannot name a handle; `using db` where nothing
/// binds `db` is `unresolved_name` alone (no `using_argument_not_capability`
/// and no effect check on top).
#[test]
fn a_test_cannot_name_a_handle() {
    let src = fx(r#"test "no handle exists in a test" {
    assert read("k", using db) == 0
}
"#);
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[err(line_of(&src, "using db) == 0"), "unresolved_name")],
    );
    assert_eq!(one(&r, "unresolved_name")["name"], json!("db"));
}

/// The grammar's `using_arg` is `"using" expr`, and S28 requires a bare
/// LOWER_IDENT naming a `using` parameter, so `using ?h` is
/// `using_argument_not_capability` at the argument (like `using 5`). The
/// hole stands in a `fn` body: S64 reports every hole exactly once per
/// check and S47 (a) makes a release check reject every hole of the linked
/// program, so whichever single report the hole gets, the program is not
/// clean and `--release` must fail.
#[test]
fn a_hole_is_not_a_using_argument() {
    let src = fx("fn hole_arg(key: Text, using db: Store) -> Int
    effects { db.get }
{
    db.get(key) + read(key, using ?h)
}
");
    let line = line_of(&src, "using ?h");
    let col = col_of(&src, "using ?h", "?h");
    let dev = check_source(&src);
    let rel = check_source_with(&src, &release());
    let at_hole = |r: &CheckResult| {
        json_values(r)
            .into_iter()
            .filter(|x| x["span"]["line"] == json!(line) && x["span"]["col"] == json!(col))
            .count()
    };
    assert!(
        rel.has_errors(),
        "a release check of a fn-body hole fails (S47 (a)): {}",
        render(&rel)
    );
    assert!(
        at_hole(&dev) >= 1,
        "the hole is reported (S64): {}",
        render(&dev)
    );
    assert!(
        at_hole(&rel) >= 1,
        "the hole is reported (S64): {}",
        render(&rel)
    );
    let d = with_code(&dev, "using_argument_not_capability");
    assert_eq!(d.len(), 1, "{}", render(&dev));
    assert_eq!(d[0]["span"]["line"], json!(line));
    assert_eq!(d[0]["span"]["col"], json!(col));
}

// ---- 4.5: operation calls in contracts -----------------------------------------

/// 4.5: an operation call in an `ensures` or a loop `decreases` is
/// `effect_in_contract` at the clause (S63 (2)) with `symbol`, `facet:
/// "contract"`, `clause` (canonical text), and `effect` (9.4), category
/// `effect`.
#[test]
fn effect_in_contract_in_ensures_and_loop_decreases() {
    let src = fx("fn e1(key: Text, using db: Store) -> Int
    ensures result >= db.get(key)
    effects { db.get }
{
    var n = db.get(key)
    while n > 0
        decreases db.get(key) + n
    {
        n = n - 1
    }
    n
}
");
    let r = check_source(&src);
    let ensures = line_of(&src, "ensures result >= db.get(key)");
    let decreases = line_of(&src, "decreases db.get(key) + n");
    assert_multiset(
        &r,
        &[
            err(ensures, "effect_in_contract"),
            err(decreases, "effect_in_contract"),
        ],
    );
    let d = with_code(&r, "effect_in_contract");
    assert_eq!(d[0]["category"], json!("effect"));
    assert_eq!(d[0]["symbol"], json!("adv.fx.e1"));
    assert_eq!(d[0]["qualified_name"], json!("adv.fx.e1"));
    assert_eq!(d[0]["facet"], json!("contract"));
    assert_eq!(d[0]["clause"], json!("result >= db.get(key)"));
    assert_eq!(d[0]["effect"], json!("db.get"));
    assert_eq!(d[0]["span"]["col"], json!(5), "the clause, from `ensures`");
    assert_eq!(d[1]["clause"], json!("db.get(key) + n"));
    assert_eq!(
        d[1]["span"]["col"],
        json!(9),
        "the clause, from `decreases`"
    );
}

/// 9.4: `effect_in_contract` carries one `effect`, so a clause calling two
/// different operations is two diagnostics at that clause.
#[test]
fn effect_in_contract_once_per_distinct_effect() {
    let src = fx("fn e2(key: Text, using db: Store) -> Int
    requires db.get(key) + db.put(key) > 0
    effects { db.get, db.put }
{
    db.get(key) + db.put(key)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "requires db.get(key) + db.put(key)");
    assert_multiset(
        &r,
        &[
            err(line, "effect_in_contract"),
            err(line, "effect_in_contract"),
        ],
    );
    let mut effects: Vec<String> = with_code(&r, "effect_in_contract")
        .iter()
        .map(|d| d["effect"].as_str().expect("an effect").to_string())
        .collect();
    effects.sort();
    assert_eq!(effects, ["db.get", "db.put"]);
}

/// 4.5: contract expressions are typed with an empty bound *regardless of
/// the enclosing function's bound*: an operation outside the bound in a
/// clause is `effect_in_contract` alone (not also `undeclared_effect`),
/// and contract calls are not part of what the body performs (4.4:
/// contracts excluded from `inferred`).
#[test]
fn effect_in_contract_ignores_the_enclosing_bound() {
    let src = fx("fn e3(key: Text, using db: Store) -> Int
    requires db.put(key) > 0
    effects { db.get }
{
    db.get(key)
}
");
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[err(
            line_of(&src, "requires db.put(key)"),
            "effect_in_contract",
        )],
    );
    assert_eq!(one(&r, "effect_in_contract")["effect"], json!("db.put"));
    assert_eq!(inferred(&r.program, "adv.fx.e3"), ["db.get"]);
}

/// 4.5 for a header `decreases` (on a recursive function, so no
/// `unused_decreases`): `effect_in_contract` alone.
#[test]
fn effect_in_contract_in_a_header_decreases() {
    let src = fx("fn e4(n: Int, key: Text, using db: Store) -> Int
    decreases db.get(key)
    effects { db.get }
{
    if n <= 0 { 0 } else { e4(n - 1, key, using db) }
}
");
    let r = check_source(&src);
    assert_multiset(
        &r,
        &[err(
            line_of(&src, "decreases db.get(key)"),
            "effect_in_contract",
        )],
    );
    let d = one(&r, "effect_in_contract");
    assert_eq!(d["clause"], json!("db.get(key)"));
}

/// 4.5 and 5.1 are independent rules: one clause with an operation call
/// and a `using` argument is `effect_in_contract` (at the clause) and
/// `capability_as_value` (at the argument).
#[test]
fn effect_in_contract_and_a_contract_using_argument_are_independent() {
    let src = fx("fn both_rules(key: Text, using db: Store) -> Int
    requires db.get(key) > read(key, using db)
    effects { db.get }
{
    db.get(key)
}
");
    let r = check_source(&src);
    let line = line_of(&src, "requires db.get(key) > read(key, using db)");
    assert_multiset(
        &r,
        &[
            err(line, "effect_in_contract"),
            err(line, "capability_as_value"),
        ],
    );
}

// ---- 5.1: holes, `?`, statements ------------------------------------------------

/// 5.1, S64: a hole in a header or loop `decreases` is `hole_in_contract`
/// alone (`symbol`, `facet: "contract"`, `clause`), spanning the hole, with
/// no `hole` diagnostic and no hole report.
#[test]
fn holes_in_decreases_clauses_are_hole_in_contract() {
    let src = "module adv.h

fn measured_hole(n: Int) -> Int
    decreases ?measure
{
    if n > 0 { measured_hole(n - 1) } else { 0 }
}

fn loop_hole(n: Int) -> Int {
    var i = n
    while i > 0
        decreases ?loop_measure
    {
        i = i - 1
    }
    i
}
";
    let r = check_source(src);
    assert_multiset(
        &r,
        &[
            err(line_of(src, "decreases ?measure"), "hole_in_contract"),
            err(line_of(src, "decreases ?loop_measure"), "hole_in_contract"),
        ],
    );
    assert!(r.holes.is_empty());
    let d = with_code(&r, "hole_in_contract");
    assert_eq!(d[0]["category"], json!("contract"));
    assert_eq!(d[0]["symbol"], json!("adv.h.measured_hole"));
    assert_eq!(d[0]["facet"], json!("contract"));
    assert_eq!(d[0]["clause"], json!("?measure"));
    assert_eq!(d[0]["span"]["col"], json!(15), "the hole");
    assert_eq!(d[0]["span"]["end_col"], json!(23));
    assert_eq!(d[1]["symbol"], json!("adv.h.loop_hole"));
    assert_eq!(d[1]["clause"], json!("?loop_measure"));
}

/// S64, S47: in both modes a contract hole is `hole_in_contract` alone,
/// never `hole_in_release`, while a body hole of the same function is
/// `hole` in development and `hole_in_release` under `--release`;
/// `CheckResult::holes` lists only the body hole, in both modes.
#[test]
fn contract_holes_are_the_same_error_in_both_modes() {
    let src = "module adv.hm

fn mixed(n: Int) -> Int
    requires n > ?bound
{
    ?mixed_body
}
";
    let requires = line_of(src, "requires n > ?bound");
    let body = line_of(src, "?mixed_body");
    let dev = check_source(src);
    assert_multiset(
        &dev,
        &[err(requires, "hole_in_contract"), info(body, "hole")],
    );
    assert_eq!(dev.holes.len(), 1);
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(requires, "hole_in_contract"),
            err(body, "hole_in_release"),
            warn(0, "project_lock_missing"),
        ],
    );
    assert_eq!(rel.holes.len(), 1);
    assert_eq!(rel.holes[0].name, "mixed_body");
}

/// 5.1: `propagate_in_contract` "also outside any function, in a `where`
/// predicate or a `for` range, where it takes precedence over
/// `propagate_outside_function`": one diagnostic each, spanning the `?`
/// expression, with `symbol`, `facet: "contract"`, and `clause`.
#[test]
fn propagate_in_where_and_for_takes_precedence() {
    let src = r#"module adv.q

fn half(n: Int) -> Result<Int, Text> {
    Ok(n / 2)
}

type Halvable = Int where half(self)? >= 0

property "halves"
    for n in gen.range(0, half(10)?)
{
    assert n >= 0
}
"#;
    let r = check_source(src);
    let where_line = line_of(src, "where half(self)?");
    let for_line = line_of(src, "for n in gen.range(0, half(10)?)");
    assert_multiset(
        &r,
        &[
            err(where_line, "propagate_in_contract"),
            err(for_line, "propagate_in_contract"),
        ],
    );
    let d = with_code(&r, "propagate_in_contract");
    assert_eq!(d[0]["category"], json!("contract"));
    assert_eq!(d[0]["symbol"], json!("adv.q.Halvable"));
    assert_eq!(d[0]["facet"], json!("contract"));
    assert_eq!(d[0]["clause"], json!("half(self)? >= 0"));
    let col = col_of(src, "where half(self)?", "half(self)?");
    assert_eq!(d[0]["span"]["col"], json!(col));
    assert_eq!(d[0]["span"]["end_col"], json!(col + 11));
    assert_eq!(d[1]["symbol"], json!("adv.q.\"halves\""));
    assert_eq!(d[1]["clause"], json!("gen.range(0, half(10)?)"));
}

/// 5.1, read with S62: a `?` in a clause of a function that does not
/// return a `Result` is `propagate_in_contract` alone. A contract
/// expression is evaluated and never exits its function, so the
/// `propagate_return_mismatch` the body rule would add has the same fix
/// (the specification states the precedence only against
/// `propagate_outside_function`; this is the reading that keeps one
/// diagnostic per mistake).
#[test]
fn propagate_in_a_contract_of_a_non_result_function_is_alone() {
    let src = "module adv.q2

fn half(n: Int) -> Result<Int, Text> {
    Ok(n / 2)
}

fn plain(n: Int) -> Int
    requires half(n)? > 0
{
    n
}
";
    let r = check_source(src);
    assert_multiset(
        &r,
        &[err(
            line_of(src, "requires half(n)?"),
            "propagate_in_contract",
        )],
    );
}

/// S32: a block inside a contract expression holds only `let` statements
/// and a tail, so a non-tail expression statement is
/// `statement_in_contract` (`symbol`, `facet: "contract"`) spanning the
/// statement, while a `let` is accepted.
#[test]
fn an_expression_statement_in_a_contract_block() {
    let src = "module adv.s

fn st(n: Int) -> Int
    requires if n > 0 {
        let limit = 10
        if n > 1 {}
        n < limit
    } else {
        true
    }
{
    n
}
";
    let r = check_source(src);
    let line = line_of(src, "if n > 1 {}");
    assert_multiset(&r, &[err(line, "statement_in_contract")]);
    let d = one(&r, "statement_in_contract");
    assert_eq!(d["category"], json!("contract"));
    assert_eq!(d["symbol"], json!("adv.s.st"));
    assert_eq!(d["facet"], json!("contract"));
    assert_eq!(d["span"]["col"], json!(9), "the statement");
    assert!(d["clause"].is_string());
}

/// 9.2: `clause` is "the formatter's rendering on one line with
/// canonical spacing, source line breaks removed" so that it "is a stable
/// key", and toolchain 5.2 repeats that "every clause ... is one line".
/// The clause of a `statement_in_contract` always holds a block; its text
/// must still be a single line (toolchain 2.4 rule 8 prints blocks
/// multi-line; see the report's spec issues).
#[test]
fn clause_text_is_one_line_even_with_blocks() {
    let src = "module adv.cl

fn st(n: Int) -> Int
    requires if n > 0 {
        var bound = 10
        n < bound
    } else {
        true
    }
{
    n
}
";
    let r = check_source(src);
    assert_multiset(
        &r,
        &[err(line_of(src, "var bound = 10"), "statement_in_contract")],
    );
    let d = one(&r, "statement_in_contract");
    let clause = d["clause"].as_str().expect("a clause");
    assert!(clause.starts_with("if n > 0 {"), "{clause:?}");
    assert!(
        !clause.contains('\n'),
        "clause {clause:?} spans several lines"
    );
}

// ---- S33, S61: recursive contracts ----------------------------------------------

/// S33: a clause that calls its own declaration through a fully qualified
/// path is still a direct self-call (`recursive_contract`, `symbol`,
/// `clause`, at the clause).
#[test]
fn recursive_contract_through_a_qualified_path() {
    let src = "module adv.r

fn self_ref(n: Int) -> Int
    ensures result == adv.r.self_ref(n)
{
    n
}
";
    let r = check_source(src);
    assert_multiset(
        &r,
        &[err(
            line_of(src, "ensures result == adv.r.self_ref"),
            "recursive_contract",
        )],
    );
    let d = one(&r, "recursive_contract");
    assert_eq!(d["category"], json!("contract"));
    assert_eq!(d["symbol"], json!("adv.r.self_ref"));
    assert_eq!(d["clause"], json!("result == adv.r.self_ref(n)"));
    assert_eq!(d["span"]["col"], json!(5));
}

/// S33: "indirect recursion through another function is not detected
/// statically": a clause calling a function that calls back is accepted.
#[test]
fn indirect_contract_recursion_is_accepted() {
    let src = "module adv.ri

fn alpha(n: Int) -> Int
    requires beta(n) >= 0
{
    n
}

fn beta(n: Int) -> Int {
    alpha(n)
}
";
    let r = check_source(src);
    assert_multiset(&r, &[]);
}

/// S61: the exemption "is keyed on `@builtin`, which only standard-library
/// files may carry (1.8), so no project declaration can claim it": a
/// project `@builtin` is `unknown_annotation` and the self-calling clause
/// is still `recursive_contract`.
#[test]
fn the_builtin_exemption_cannot_be_claimed_by_project_code() {
    let src = r#"module adv.b

@builtin("text_trim")
fn idem(n: Int) -> Int
    ensures idem(result) == result
{
    n
}
"#;
    let r = check_source(src);
    assert_multiset(
        &r,
        &[
            err(line_of(src, "@builtin"), "unknown_annotation"),
            err(line_of(src, "ensures idem(result)"), "recursive_contract"),
        ],
    );
}

/// S61, stdlib 10.4: `trim`, `to_upper`, and `to_lower` state idempotence
/// by naming themselves and the checker reports nothing for them: the
/// standard library checks without any diagnostic.
#[test]
fn std_builtins_may_name_themselves_in_clauses() {
    let src = "module adv.std

use std.text.{trim, to_upper, to_lower}

fn shout(t: Text) -> Text {
    to_upper(to_lower(trim(t)))
}
";
    let r = check_source(src);
    assert!(r.diagnostics.is_empty(), "{}", render(&r));
    for path in ["std.text.trim", "std.text.to_upper", "std.text.to_lower"] {
        assert_eq!(def_status(&r.program, path), DefStatus::Ok, "{path}");
    }
}

// ---- S4: private_in_public_contract ---------------------------------------------

/// S4: the *contract facet* of a public declaration may call only public
/// and prelude functions: an `ensures` calling a private function is
/// `private_in_public_contract` (`symbol`, `facet: "contract"`, `clause`,
/// `callee`, category `resolve` per 9.4) at the clause; a loop invariant
/// is part of the body facet (toolchain 5.2: the body "including loop
/// invariants"), a private declaration's clauses are unrestricted, and a
/// prelude function is always allowed.
#[test]
fn private_in_public_contract_covers_the_contract_facet_only() {
    let src = "module adv.p

fn small(n: Int) -> Bool {
    n < 10
}

pub fn bump(n: Int) -> Int
    ensures small(result) or true
{
    n + 1
}

pub fn loops(n: Int) -> Int {
    var i = n
    while i > 0
        invariant small(i) or i >= 10
        decreases i
    {
        i = i - 1
    }
    i
}

fn private_ok(n: Int) -> Int
    requires small(n)
{
    n
}

pub fn prelude_ok(xs: List<List<Int>>) -> Int
    requires flatten(xs).len >= 0
{
    0
}
";
    let r = check_source(src);
    let line = line_of(src, "ensures small(result)");
    assert_multiset(&r, &[err(line, "private_in_public_contract")]);
    let d = one(&r, "private_in_public_contract");
    assert_eq!(d["category"], json!("resolve"));
    assert_eq!(d["symbol"], json!("adv.p.bump"));
    assert_eq!(d["facet"], json!("contract"));
    assert_eq!(d["clause"], json!("small(result) or true"));
    assert_eq!(d["callee"], json!("adv.p.small"));
    assert_eq!(d["span"]["col"], json!(5), "the clause (S63 (2))");
}

/// S4 with 1.3: a public capability's contract facet is "every operation's
/// `requires`/`ensures` clauses", so an operation clause calling a private
/// function is `private_in_public_contract`.
#[test]
fn private_in_public_contract_on_a_public_capability_operation() {
    let src = "module adv.pc

fn valid(key: Text) -> Bool {
    key.len > 0
}

pub capability Vault {
    fn open(key: Text) -> Int
        requires valid(key)
}
";
    let r = check_source(src);
    assert_multiset(
        &r,
        &[err(
            line_of(src, "requires valid(key)"),
            "private_in_public_contract",
        )],
    );
    let d = one(&r, "private_in_public_contract");
    assert_eq!(d["callee"], json!("adv.pc.valid"));
    assert_eq!(d["clause"], json!("valid(key)"));
}

/// S4 names `decreases` among a public declaration's contract clauses;
/// a clause calling two private functions is one diagnostic per `callee`
/// (9.4 carries one `callee`).
#[test]
fn private_in_public_contract_in_decreases_and_per_callee() {
    let src = "module adv.pd

fn measure(n: Int) -> Int {
    n
}

fn small(n: Int) -> Bool {
    n < 10
}

fn tiny(n: Int) -> Bool {
    n < 2
}

pub fn shrink(n: Int) -> Int
    decreases measure(n)
{
    if n > 0 { shrink(n - 1) } else { 0 }
}

pub fn guarded(n: Int) -> Int
    requires small(n) and tiny(n)
{
    n
}
";
    let r = check_source(src);
    let dec = line_of(src, "decreases measure(n)");
    let req = line_of(src, "requires small(n) and tiny(n)");
    assert_multiset(
        &r,
        &[
            err(dec, "private_in_public_contract"),
            err(req, "private_in_public_contract"),
            err(req, "private_in_public_contract"),
        ],
    );
    let d = with_code(&r, "private_in_public_contract");
    assert_eq!(d[0]["callee"], json!("adv.pd.measure"));
    assert_eq!(d[0]["clause"], json!("measure(n)"));
    assert_eq!(d[0]["symbol"], json!("adv.pd.shrink"));
    let mut callees: Vec<&str> = d[1..]
        .iter()
        .map(|x| x["callee"].as_str().expect("a callee"))
        .collect();
    callees.sort_unstable();
    assert_eq!(callees, ["adv.pd.small", "adv.pd.tiny"]);
}

// ---- S9: `result` and `self` ----------------------------------------------------

/// S9 (5.1: "`result` only in `ensures`; `self` only in `where`"):
/// `result` in a `requires` or a header `decreases` is
/// `result_outside_ensures` (`name`) spanning the clause (S63 (2)); `self`
/// in a loop invariant is `self_outside_where` (`name`). The erroneous
/// names cause no clause `type_mismatch` (S62).
#[test]
fn result_and_self_outside_their_clauses() {
    let src = "module adv.n

fn r1(n: Int) -> Int
    requires result > 0
{
    n
}

fn r2(n: Int) -> Int
    decreases result
{
    if n > 0 { r2(n - 1) } else { 0 }
}

fn r3(n: Int) -> Int {
    var i = n
    while i > 0
        invariant self >= 0
    {
        i = i - 1
    }
    i
}
";
    let r = check_source(src);
    assert_multiset(
        &r,
        &[
            err(
                line_of(src, "requires result > 0"),
                "result_outside_ensures",
            ),
            err(line_of(src, "decreases result"), "result_outside_ensures"),
            err(line_of(src, "invariant self >= 0"), "self_outside_where"),
        ],
    );
    let d = with_code(&r, "result_outside_ensures");
    assert_eq!(d[0]["name"], json!("result"));
    assert_eq!(d[0]["span"]["col"], json!(5), "the clause, from `requires`");
    assert_eq!(
        d[1]["span"]["col"],
        json!(5),
        "the clause, from `decreases`"
    );
    assert_eq!(one(&r, "self_outside_where")["name"], json!("self"));
}

/// 5.1: `decreases` needs `strip` type `Int`, so a refinement of `Int` is
/// an acceptable measure on a recursive function.
#[test]
fn a_refined_measure_is_accepted() {
    let src = "module adv.m

pub type Positive = Int where self > 0

fn r4(size: Positive, n: Int) -> Int
    decreases size
{
    if n > 0 { r4(size, n - 1) } else { 0 }
}
";
    let r = check_source(src);
    assert_multiset(&r, &[]);
}

// ---- 5.4: `decreases` warnings --------------------------------------------------

/// 5.4: `unused_decreases` (W, `symbol`) spans the clause from its keyword
/// (S63 (2)); a loop `decreases` is never `unused_decreases` (the rule is
/// about "a `decreases` clause on a function"); a self-call through a
/// qualified path makes the function recursive.
#[test]
fn unused_decreases_only_for_header_clauses_of_non_recursive_functions() {
    let src = "module adv.d

fn flat(n: Int) -> Int
    decreases n
{
    n
}

fn measured_loop(n: Int) -> Int {
    var i = n
    while i > 0
        decreases i
    {
        i = i - 1
    }
    i
}

fn countdown(n: Int) -> Int
    decreases n
{
    if n <= 0 { 0 } else { adv.d.countdown(n - 1) }
}
";
    let r = check_source(src);
    let line = line_of(src, "    decreases n");
    assert_multiset(&r, &[warn(line, "unused_decreases")]);
    let d = one(&r, "unused_decreases");
    assert_eq!(d["category"], json!("contract"));
    assert_eq!(d["symbol"], json!("adv.d.flat"));
    assert_eq!(d["qualified_name"], json!("adv.d.flat"));
    assert_eq!(d["span"]["col"], json!(5));
    assert_eq!(d["span"]["end_col"], json!(5 + "decreases n".len()));
}

/// 5.4: recursion "through other functions" counts: on the three-function
/// cycle `ra -> rb -> rc -> ra`, `ra`'s `decreases` is used, and the
/// `@prove` function `rb` without one is `missing_decreases` at its name
/// beside its `verification_result`.
#[test]
fn recursion_through_a_three_function_cycle() {
    let src = "module adv.cy

fn ra(n: Int) -> Int
    decreases n
{
    if n <= 0 { 0 } else { rb(n - 1) }
}

@prove
fn rb(n: Int) -> Int {
    if n <= 0 { 0 } else { rc(n - 1) }
}

fn rc(n: Int) -> Int {
    if n <= 0 { 0 } else { ra(n - 1) }
}
";
    let r = check_source(src);
    let rb = line_of(src, "fn rb");
    assert_multiset(
        &r,
        &[
            warn(rb, "missing_decreases"),
            info(rb, "verification_result"),
        ],
    );
    let d = one(&r, "missing_decreases");
    assert_eq!(d["symbol"], json!("adv.cy.rb"));
    assert_eq!(d["span"]["col"], json!(4), "the name");
    assert_eq!(d["span"]["end_col"], json!(6));
}

/// 5.4: a `@prove` function that is recursive with a measure and one whose
/// loops all have measures have no `missing_decreases`.
#[test]
fn measured_prove_functions_have_no_termination_warning() {
    let src = "module adv.ok

@prove
fn sum_to(n: Int) -> Int
    requires n >= 0
    decreases n
{
    if n == 0 { 0 } else { n + sum_to(n - 1) }
}

@prove
fn loop_ok(n: Int) -> Int {
    var i = n
    while i > 0
        decreases i
    {
        i = i - 1
    }
    i
}
";
    let r = check_source(src);
    assert_multiset(
        &r,
        &[
            info(line_of(src, "fn sum_to"), "verification_result"),
            info(line_of(src, "fn loop_ok"), "verification_result"),
        ],
    );
}

/// 5.4: a recursive `@prove` function without a measure *and* with an
/// unmeasured loop has both warnings: at the name and at the loop (from
/// `while`, S63 (5)).
#[test]
fn a_recursive_prove_function_with_an_unmeasured_loop_has_both_warnings() {
    let src = "module adv.bm

@prove
fn both_missing(n: Int) -> Int {
    var i = n
    while i > 0 {
        i = i - 1
    }
    if n > 0 { both_missing(n - 1) } else { i }
}
";
    let r = check_source(src);
    let name = line_of(src, "fn both_missing");
    let lp = line_of(src, "while i > 0 {");
    assert_multiset(
        &r,
        &[
            warn(name, "missing_decreases"),
            warn(lp, "missing_decreases"),
            info(name, "verification_result"),
        ],
    );
    let d = with_code(&r, "missing_decreases");
    assert_eq!(d[0]["span"]["col"], json!(4), "the name");
    assert_eq!(d[0]["span"]["end_col"], json!(4 + "both_missing".len()));
    assert_eq!(d[1]["span"]["line"], json!(lp));
    assert_eq!(d[1]["span"]["col"], json!(5), "the loop, from `while`");
    for x in &d {
        assert_eq!(x["symbol"], json!("adv.bm.both_missing"));
    }
}

/// 5.4: "every `while` loop without a `decreases` clause in the body of a
/// `@prove` function", nested loops included: one warning per loop.
#[test]
fn every_unmeasured_loop_of_a_prove_body_is_reported() {
    let src = "module adv.nl

@prove
fn nested(n: Int) -> Int {
    var i = n
    while i > 0 {
        var j = i
        while j > 0 {
            j = j - 1
        }
        i = i - 1
    }
    i
}
";
    let r = check_source(src);
    assert_multiset(
        &r,
        &[
            warn(line_of(src, "while i > 0 {"), "missing_decreases"),
            warn(line_of(src, "while j > 0 {"), "missing_decreases"),
            info(line_of(src, "fn nested"), "verification_result"),
        ],
    );
}

/// 5.4: the measure obligation belongs to `@prove` bodies only: loops in a
/// plain function or a test need no `decreases`.
#[test]
fn loops_outside_prove_bodies_need_no_measure() {
    let src = "module adv.lp

fn spin(n: Int) -> Int {
    var i = n
    while i > 0 {
        i = i - 1
    }
    i
}

test \"loops in tests need no measure\" {
    var i = 3
    while i > 0 {
        i = i - 1
    }
    assert i == 0
}
";
    let r = check_source(src);
    assert_multiset(&r, &[]);
}

// ---- 5.5, S36: verification_result ---------------------------------------------

/// S36: `@prove` is on `fn` only (1.8); misplaced it is
/// `annotation_misplaced` at the annotation (S63 (3)) and records no
/// verification policy: no `verification_result`, no
/// `CheckResult::verification` entry, and no `verification_unsatisfied`
/// under `--release`.
#[test]
fn prove_on_a_non_function_has_no_verification_status() {
    let src = "module adv.g

@prove
type Small = Int where self < 10

@prove
test \"proved test\" {
    assert true
}
";
    let first = line_nth(src, "@prove", 1);
    let second = line_nth(src, "@prove", 2);
    let dev = check_source(src);
    assert_multiset(
        &dev,
        &[
            err(first, "annotation_misplaced"),
            err(second, "annotation_misplaced"),
        ],
    );
    assert!(dev.verification.is_empty());
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(first, "annotation_misplaced"),
            err(second, "annotation_misplaced"),
            warn(0, "project_lock_missing"),
        ],
    );
    assert!(rel.verification.is_empty());
}

/// S36: one `verification_result` per `@prove` *declaration*: a repeated
/// `@prove` is `duplicate_annotation` at the later annotation (S63 (3),
/// (4)) and the declaration still has exactly one status, and under
/// `--release` exactly one `verification_unsatisfied`.
#[test]
fn a_duplicated_prove_yields_one_status() {
    let src = "module adv.dp

@prove
@prove
fn twice(n: Int) -> Int {
    n
}
";
    let dup = line_nth(src, "@prove", 2);
    let name = line_of(src, "fn twice");
    let dev = check_source(src);
    assert_multiset(
        &dev,
        &[
            err(dup, "duplicate_annotation"),
            info(name, "verification_result"),
        ],
    );
    assert_eq!(dev.verification.len(), 1);
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(dup, "duplicate_annotation"),
            info(name, "verification_result"),
            err(name, "verification_unsatisfied"),
            warn(0, "project_lock_missing"),
        ],
    );
}

/// S36: the status is the policy's, whatever else is wrong with the
/// declaration: a `@prove` function with a body error still has its
/// `verification_result` (and, under `--release`, its
/// `verification_unsatisfied`).
#[test]
fn a_prove_function_with_errors_still_has_a_status() {
    let src = "module adv.be

@prove
fn broken(n: Int) -> Int {
    let _x: Text = n
    n
}
";
    let name = line_of(src, "fn broken");
    let bad = line_of(src, "let _x: Text = n");
    let dev = check_source(src);
    assert_multiset(
        &dev,
        &[err(bad, "type_mismatch"), info(name, "verification_result")],
    );
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(bad, "type_mismatch"),
            info(name, "verification_result"),
            err(name, "verification_unsatisfied"),
            warn(0, "project_lock_missing"),
        ],
    );
}

/// S36, S47, S64: a private `@prove` function with a hole body has its
/// status and its hole report; under `--release` it fails for both
/// reasons (the hole is `hole_in_release`, the status
/// `verification_unsatisfied`), and the `verification_result` info is
/// still reported ("nothing else differs between the modes").
#[test]
fn a_private_prove_function_with_a_hole_fails_release_twice() {
    let src = "module adv.later

@prove
fn later(n: Int) -> Int
    ensures result >= n
{
    ?later_body
}
";
    let name = line_of(src, "fn later");
    let hole = line_of(src, "?later_body");
    let dev = check_source(src);
    assert_multiset(
        &dev,
        &[info(name, "verification_result"), info(hole, "hole")],
    );
    assert!(!dev.has_errors());
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            info(name, "verification_result"),
            err(name, "verification_unsatisfied"),
            err(hole, "hole_in_release"),
            warn(0, "project_lock_missing"),
        ],
    );
    let d = one(&rel, "hole_in_release");
    assert_eq!(d["hole"], json!("later_body"));
    assert_eq!(d["expected_type"], json!("Int"));
    assert_eq!(d["symbol"], json!("adv.later.later"));
}

// ---- 8.2, S47, S64: release mode ------------------------------------------------

/// S47: every `@prove` declaration with status `unknown` fails a release
/// check, private ones included; `verification_unsatisfied` carries
/// `symbol` (the `@id` when present), `qualified_name`, `facet:
/// "verification_policy"`, and `status` (9.4), category `release_policy`,
/// at the name (S63 (3)); the definition has errors (toolchain 1.6).
/// Both postconditions are about text, which the verifier does not decide.
#[test]
fn release_rejects_every_unproved_declaration() {
    let src = "module adv.proofs

@prove
fn private_proof(t: Text) -> Text
    ensures result.len > 0
{
    t
}

@id(\"pub_proof_1\")
@prove
pub fn public_proof(t: Text) -> Text
    ensures result.len > t.len
{
    t ++ \"!\"
}
";
    let private = line_of(src, "fn private_proof");
    let public = line_of(src, "pub fn public_proof");
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(private, "verification_unsatisfied"),
            info(private, "verification_result"),
            err(public, "verification_unsatisfied"),
            info(public, "verification_result"),
            warn(0, "project_lock_missing"),
        ],
    );
    let d = with_code(&rel, "verification_unsatisfied");
    assert_eq!(d[0]["symbol"], json!("adv.proofs.private_proof"));
    assert_eq!(d[0]["span"]["col"], json!(4));
    assert_eq!(d[1]["category"], json!("release_policy"));
    assert_eq!(d[1]["severity"], json!("error"));
    assert_eq!(d[1]["symbol"], json!("pub_proof_1"));
    assert_eq!(d[1]["qualified_name"], json!("adv.proofs.public_proof"));
    assert_eq!(d[1]["facet"], json!("verification_policy"));
    assert_eq!(d[1]["status"], json!("unknown"));
    assert_eq!(d[1]["span"]["col"], json!(8));
    assert_eq!(d[1]["span"]["end_col"], json!(8 + "public_proof".len()));
    assert_eq!(rel.verification.len(), 2);
    assert!(rel
        .verification
        .iter()
        .all(|v| v.status == ProofStatus::Unknown
            && v.reason.as_deref() == Some("unsupported_expression")));
    assert_eq!(
        def_status(&rel.program, "adv.proofs.public_proof"),
        DefStatus::HasErrors
    );
    let dev = check_source(src);
    assert!(with_code(&dev, "verification_unsatisfied").is_empty());
    assert_eq!(
        def_status(&dev.program, "adv.proofs.public_proof"),
        DefStatus::Ok
    );
}

/// S47 (a), S64: every `fn`-body hole of the linked program, in a `while`
/// condition or a nested block of a private function, is
/// `hole_in_release` (`hole`, `symbol`, `qualified_name`,
/// `expected_type`; category `release_policy`) in place of `hole`;
/// `CheckResult::holes` lists both in both modes, and the function has
/// errors only under `--release`.
#[test]
fn release_rejects_holes_anywhere_in_a_function_body() {
    let src = "module adv.rel

fn scan(n: Int) -> Int {
    var i = n
    while ?keep_going {
        i = i - 1
    }
    if i > 0 {
        ?positive
    } else {
        0
    }
}
";
    let cond = line_of(src, "while ?keep_going");
    let nested = line_of(src, "?positive");
    let dev = check_source(src);
    assert_multiset(&dev, &[info(cond, "hole"), info(nested, "hole")]);
    assert_eq!(dev.holes.len(), 2);
    assert_eq!(def_status(&dev.program, "adv.rel.scan"), DefStatus::Ok);
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(cond, "hole_in_release"),
            err(nested, "hole_in_release"),
            warn(0, "project_lock_missing"),
        ],
    );
    assert_eq!(rel.holes.len(), 2);
    let d = with_code(&rel, "hole_in_release");
    assert_eq!(d[0]["category"], json!("release_policy"));
    assert_eq!(d[0]["hole"], json!("keep_going"));
    assert_eq!(d[0]["expected_type"], json!("Bool"));
    assert_eq!(d[0]["symbol"], json!("adv.rel.scan"));
    assert_eq!(d[0]["qualified_name"], json!("adv.rel.scan"));
    assert_eq!(d[0]["span"]["col"], json!(11), "the hole");
    assert_eq!(d[0]["span"]["end_col"], json!(22));
    assert_eq!(d[1]["hole"], json!("positive"));
    assert_eq!(d[1]["expected_type"], json!("Int"));
    assert_eq!(
        def_status(&rel.program, "adv.rel.scan"),
        DefStatus::HasErrors
    );
}

/// S64: a hole with no expected type is `hole_type_unknown` only, in both
/// modes: no `hole_in_release` beside it and no report.
#[test]
fn hole_type_unknown_is_not_also_hole_in_release() {
    let src = "module adv.ht

fn unknown_hole() -> Int {
    let _x = ?mystery
    0
}
";
    let line = line_of(src, "?mystery");
    let dev = check_source(src);
    assert_multiset(&dev, &[err(line, "hole_type_unknown")]);
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(line, "hole_type_unknown"),
            warn(0, "project_lock_missing"),
        ],
    );
    assert!(rel.holes.is_empty());
}

/// S47: "Nothing else differs between the modes": a project without holes
/// or `@prove` reports exactly the same diagnostic objects under
/// `--release`, plus the lock warning (T37), which is span-less with
/// category `project`.
#[test]
fn release_mode_changes_nothing_else() {
    let src = "module adv.same

pub capability Store {
    fn get(key: Text) -> Int
}

fn flat(n: Int) -> Int
    decreases n
{
    n
}

fn dup(key: Text, using db: Store) -> Int
    effects { db.get, db.get }
{
    db.get(key)
}

fn idle(key: Text, using db: Store) -> Int {
    key.len
}

test \"uses flat\" {
    assert flat(1) == 1
}
";
    let dev = check_source(src);
    assert_multiset(
        &dev,
        &[
            warn(line_of(src, "    decreases n"), "unused_decreases"),
            warn(
                line_of(src, "effects { db.get, db.get }"),
                "duplicate_effect",
            ),
            warn(line_of(src, "fn idle"), "unused_capability"),
        ],
    );
    let rel = check_source_with(src, &release());
    let lock = one(&rel, "project_lock_missing");
    assert_eq!(lock["severity"], json!("warning"));
    assert_eq!(lock["category"], json!("project"));
    assert!(lock.get("span").is_none(), "span-less (S63 (6))");
    let rest: Vec<Value> = json_values(&rel)
        .into_iter()
        .filter(|d| d["code"] != json!("project_lock_missing"))
        .collect();
    assert_eq!(json_values(&dev), rest);
    assert!(!rel.has_errors());
    assert!(with_code(&dev, "project_lock_missing").is_empty());
}

/// S64: "every hole is reported exactly once per check", and the only
/// holes without a `hole` report are the error holes `hole_type_unknown`
/// (no expected type, or one with unresolved variables, S56) and
/// `hole_in_contract`. A hole whose expected type comes from another
/// declaration's erroneous signature has an expected type (the error type,
/// S62), so it is still reported: `hole` in development and, being a hole
/// of a `fn` body, `hole_in_release` under `--release` (S47 (a): "every
/// hole in the linked program").
#[test]
fn holes_with_an_erroneous_expected_type_are_reported_once() {
    let src = "module adv.eh

fn takes(x: Nope) -> Int {
    1
}

fn caller(n: Int) -> Int {
    takes(?h) + n
}
";
    let bad = line_of(src, "fn takes(x: Nope)");
    let hole = line_of(src, "takes(?h)");
    let dev = check_source(src);
    assert_multiset(&dev, &[err(bad, "unresolved_type"), info(hole, "hole")]);
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(bad, "unresolved_type"),
            err(hole, "hole_in_release"),
            warn(0, "project_lock_missing"),
        ],
    );
    assert_eq!(
        def_status(&rel.program, "adv.eh.caller"),
        DefStatus::HasErrors
    );
}

/// S47: test and property bodies are not linked, so their holes are
/// warnings under `--release` and do not fail the check; with
/// `--deny-warnings` (toolchain 7.2, `CheckConfig::warnings_as_errors`)
/// the warnings do.
#[test]
fn test_body_holes_do_not_fail_a_release_check() {
    let src = "module adv.th

test \"unfinished\" {
    assert ?claim
}
";
    let line = line_of(src, "?claim");
    let rel = check_source_with(src, &release());
    assert_multiset(&rel, &[warn(line, "hole"), warn(0, "project_lock_missing")]);
    assert!(!rel.has_errors());
    assert_eq!(rel.holes.len(), 1);
    let strict = check_source_with(
        src,
        &CheckConfig {
            release: true,
            warnings_as_errors: true,
            ..CheckConfig::default()
        },
    );
    assert!(strict.has_errors());
}

/// 8.1: "Several holes may share a name within a declaration ... they are
/// distinguished by span": under `--release` each is its own
/// `hole_in_release`, and each has its own report.
#[test]
fn same_named_holes_are_rejected_separately() {
    let src = "module adv.sn

fn pick(b: Bool) -> Int {
    if b {
        ?same
    } else {
        ?same
    }
}
";
    let rel = check_source_with(src, &release());
    assert_multiset(
        &rel,
        &[
            err(line_nth(src, "?same", 1), "hole_in_release"),
            err(line_nth(src, "?same", 2), "hole_in_release"),
            warn(0, "project_lock_missing"),
        ],
    );
    assert_eq!(rel.holes.len(), 2);
    for d in with_code(&rel, "hole_in_release") {
        assert_eq!(d["hole"], json!("same"));
        assert_eq!(d["expected_type"], json!("Int"));
    }
}

/// The lock `cpl check --lock` writes for `files` (toolchain 11.2):
/// `toolchain` (`cpl`, the edition, the std digest), the blake3 digest of
/// the bytes of every source-set file by path (Section 0: `cpl.toml` and
/// the `.cpl` files, never the lock), and the manifest's `[verification]`
/// defaults (11.1: 10000, []). The digests are the loader's
/// (`Project::manifest_digest`, `SourceEntry::digest`), which
/// `crates/cpl-project/tests` check against blake3 itself, so this crate
/// needs no blake3 dependency (toolchain 1.3).
fn lock_for(files: &[(&str, &str)], std: &Digest, cpl: &str) -> String {
    let project = load_tree(&tree(files));
    let mut sources: Vec<(String, String)> = files
        .iter()
        .map(|(path, _)| {
            let digest = if *path == "cpl.toml" {
                project.manifest_digest.clone()
            } else {
                let file = project
                    .source_map
                    .find(path)
                    .unwrap_or_else(|| panic!("{path} is a member of the source set"));
                project.files[file.0 as usize].digest.clone()
            };
            (path.to_string(), digest.as_str().to_string())
        })
        .collect();
    sources.sort();
    let mut lock = format!(
        "# Generated by cpl. Do not edit.\nversion = 1\n\n[toolchain]\ncpl = \"{cpl}\"\nedition = \"{}\"\nstd = \"{}\"\n\n[sources]\n",
        cpl_diag::version::EDITION,
        std.as_str()
    );
    for (path, digest) in sources {
        lock.push_str(&format!("\"{path}\" = \"{digest}\"\n"));
    }
    lock.push_str("\n[verification]\nproof_timeout_ms = 10000\nadmit = []\n");
    lock
}

const LOCKED: &str = "module adv.lock

pub fn one() -> Int {
    1
}
";

/// T37, 11.2: a present lock that matches the tree is neither
/// `project_lock_missing` nor `project_lock_stale`, in either mode.
#[test]
fn a_fresh_lock_satisfies_the_release_lock_check() {
    let m = manifest("locked");
    let files = [("cpl.toml", m.as_str()), ("src/adv/lock.cpl", LOCKED)];
    let std = load_tree(&tree(&files)).std_digest();
    let lock = lock_for(&files, &std, cpl_diag::version::TOOLCHAIN);
    let mut all = files.to_vec();
    all.push(("cpl.lock", lock.as_str()));
    let project = load_tree(&tree(&all));
    assert!(project.lock.is_some(), "the lock parses");
    let rel = check_with(&project, &release());
    assert_multiset(&rel, &[]);
    assert!(!rel.has_errors());
    let dev = check_with(&project, &CheckConfig::default());
    assert_multiset(&dev, &[]);
}

/// T37, 11.2: a present lock whose `[sources]` differ from the tree is the
/// span-less error `project_lock_stale` (category `project`) under
/// `--release`, and nothing without it.
#[test]
fn a_stale_lock_is_an_error_only_under_release() {
    let m = manifest("locked");
    let files = [("cpl.toml", m.as_str()), ("src/adv/lock.cpl", LOCKED)];
    let std = load_tree(&tree(&files)).std_digest();
    let lock = lock_for(&files, &std, cpl_diag::version::TOOLCHAIN);
    let edited = LOCKED.replace("    1\n", "    2\n");
    let all = [
        ("cpl.toml", m.as_str()),
        ("src/adv/lock.cpl", edited.as_str()),
        ("cpl.lock", lock.as_str()),
    ];
    let project = load_tree(&tree(&all));
    let rel = check_with(&project, &release());
    assert_multiset(&rel, &[err(0, "project_lock_stale")]);
    let d = one(&rel, "project_lock_stale");
    assert_eq!(d["category"], json!("project"));
    assert!(d.get("span").is_none());
    assert!(rel.has_errors());
    let dev = check_with(&project, &CheckConfig::default());
    assert_multiset(&dev, &[]);
}

/// 11.2: a lock written by another toolchain version is the warning
/// `project_lock_toolchain_mismatch` in every mode ("every project command
/// parses an existing lock"), and is not stale: staleness compares only
/// `[sources]`, `[verification]`, and `toolchain.std`.
#[test]
fn a_lock_from_another_toolchain_warns_in_both_modes() {
    let m = manifest("locked");
    let files = [("cpl.toml", m.as_str()), ("src/adv/lock.cpl", LOCKED)];
    let std = load_tree(&tree(&files)).std_digest();
    let lock = lock_for(&files, &std, "0.0.0-other");
    let mut all = files.to_vec();
    all.push(("cpl.lock", lock.as_str()));
    let project = load_tree(&tree(&all));
    let dev = check_with(&project, &CheckConfig::default());
    assert_multiset(&dev, &[warn(0, "project_lock_toolchain_mismatch")]);
    let rel = check_with(&project, &release());
    assert_multiset(&rel, &[warn(0, "project_lock_toolchain_mismatch")]);
}

// ---- toolchain 12.3, semantics Section 11: the examples --------------------------

fn example_check(name: &str, release_mode: bool) -> CheckResult {
    let config = if release_mode {
        release()
    } else {
        CheckConfig::default()
    };
    check_with(&load_dir(&example(name)), &config)
}

/// (file, line, severity, code) of a diagnostic; `("", 0)` when span-less.
type Located = (String, u32, Severity, String);

fn located(result: &CheckResult) -> Vec<Located> {
    let map = result.program.source_map();
    let mut v: Vec<Located> = result
        .diagnostics
        .iter()
        .map(|d| {
            let (file, line) = d.span.map_or((String::new(), 0), |s| {
                (
                    map.file(s.file).path.clone(),
                    map.line_col(s.file, s.start).line,
                )
            });
            (file, line, d.severity, d.code.0.to_string())
        })
        .collect();
    v.sort();
    v
}

fn at(file: &str, line: u32, severity: Severity, code: &str) -> Located {
    (file.to_string(), line, severity, code.to_string())
}

#[track_caller]
fn assert_located(result: &CheckResult, expected: &[Located]) {
    let mut want = expected.to_vec();
    want.sort();
    assert_eq!(located(result), want, "{}", render(result));
}

const READER_PAGES: &str = "src/reader/pages.cpl";
const HOLES_PAGES: &str = "src/holes/pages.cpl";

/// The `verification_result` of a `paginate` declared at `line` of `file`
/// (toolchain 7.2's example object; S36; 9.4).
#[track_caller]
fn assert_paginate_status(d: &Value, file: &str, line: u32, qualified: &str) {
    // Reader's postconditions compare lists, which the verifier does not decide; the holes
    // version is unfinished.
    let (reason, message) = if qualified.starts_with("holes.") {
        ("unresolved_hole", "paginate: unknown (unresolved hole)")
    } else {
        (
            "unsupported_expression",
            "paginate: unknown (unsupported expression)",
        )
    };
    assert_eq!(d["category"], json!("contract"));
    assert_eq!(d["severity"], json!("info"));
    assert_eq!(d["symbol"], json!("pgn_7c2d"));
    assert_eq!(d["qualified_name"], json!(qualified));
    assert_eq!(d["facet"], json!("verification_policy"));
    assert_eq!(d["status"], json!("unknown"));
    assert_eq!(d["reason"], json!(reason));
    assert_eq!(d["message"], json!(message));
    assert_eq!(
        d["span"],
        json!({ "file": file, "line": line, "col": 8, "end_line": line, "end_col": 16 })
    );
    assert!(
        d.get("notes").is_none(),
        "paginate is not recursive and its loop has a measure: no termination note"
    );
}

/// Toolchain 12.3 and Section 11, development mode: `examples/reader`
/// checks with exactly one diagnostic, the `verification_result` (info,
/// `unknown`) of `paginate` at its name, mirrored by
/// `CheckResult::verification`; no hole, no warning.
#[test]
fn reader_development_diagnostics_are_exactly_one_status() {
    let r = example_check("reader", false);
    assert_located(
        &r,
        &[at(READER_PAGES, 45, Severity::Info, "verification_result")],
    );
    assert!(!r.has_errors());
    assert_paginate_status(
        &one(&r, "verification_result"),
        READER_PAGES,
        45,
        "reader.pages.paginate",
    );
    let p = &r.program;
    let statuses: Vec<(String, ProofStatus, Option<String>)> = r
        .verification
        .iter()
        .map(|v| (p.display_path(v.def), v.status, v.reason.clone()))
        .collect();
    assert_eq!(
        statuses,
        [(
            "reader.pages.paginate".to_string(),
            ProofStatus::Unknown,
            Some("unsupported_expression".to_string())
        )]
    );
    assert!(r.holes.is_empty());
    assert_eq!(def_status(p, "reader.pages.paginate"), DefStatus::Ok);
}

/// Toolchain 12.3, Section 11, `--release`: `examples/reader` fails with
/// exactly one `verification_unsatisfied` (for `reader.pages.paginate`),
/// keeps the `verification_result` info (S36 (b), S47 "nothing else
/// differs"), and warns `project_lock_missing` (no `cpl.lock`, T37).
#[test]
fn reader_release_diagnostics_are_exact() {
    let r = example_check("reader", true);
    assert_located(
        &r,
        &[
            at(
                READER_PAGES,
                45,
                Severity::Error,
                "verification_unsatisfied",
            ),
            at(READER_PAGES, 45, Severity::Info, "verification_result"),
            at("", 0, Severity::Warning, "project_lock_missing"),
        ],
    );
    assert!(r.has_errors());
    let d = one(&r, "verification_unsatisfied");
    assert_eq!(d["category"], json!("release_policy"));
    assert_eq!(d["symbol"], json!("pgn_7c2d"));
    assert_eq!(d["qualified_name"], json!("reader.pages.paginate"));
    assert_eq!(d["facet"], json!("verification_policy"));
    assert_eq!(d["status"], json!("unknown"));
    assert_eq!(
        d["span"],
        json!({ "file": READER_PAGES, "line": 45, "col": 8, "end_line": 45, "end_col": 16 })
    );
    assert_paginate_status(
        &one(&r, "verification_result"),
        READER_PAGES,
        45,
        "reader.pages.paginate",
    );
    let lock = one(&r, "project_lock_missing");
    assert_eq!(lock["category"], json!("project"));
    assert!(lock.get("span").is_none());
    assert_eq!(r.verification.len(), 1);
    assert_eq!(
        def_status(&r.program, "reader.pages.paginate"),
        DefStatus::HasErrors
    );
    assert_eq!(
        def_status(&r.program, "reader.pages.load_text"),
        DefStatus::Ok
    );
}

/// Toolchain 12.3, Section 11, development mode: `examples/holes` checks
/// with exactly two `hole` infos (`build_pages`: `List<List<T>>`;
/// `summary`: `Text`) and the `verification_result` of `paginate`
/// (toolchain 7.2's example: line 15, columns 8-16; the hole at line 20,
/// columns 5-17).
#[test]
fn holes_development_diagnostics_are_exact() {
    let r = example_check("holes", false);
    assert_located(
        &r,
        &[
            at(HOLES_PAGES, 15, Severity::Info, "verification_result"),
            at(HOLES_PAGES, 20, Severity::Info, "hole"),
            at(HOLES_PAGES, 24, Severity::Info, "hole"),
        ],
    );
    assert!(!r.has_errors());
    assert_paginate_status(
        &one(&r, "verification_result"),
        HOLES_PAGES,
        15,
        "holes.pages.paginate",
    );
    let h = with_code(&r, "hole");
    assert_eq!(h[0]["category"], json!("hole"));
    assert_eq!(h[0]["hole"], json!("build_pages"));
    assert_eq!(h[0]["symbol"], json!("pgn_7c2d"));
    assert_eq!(h[0]["qualified_name"], json!("holes.pages.paginate"));
    assert_eq!(h[0]["facet"], json!("body"));
    assert_eq!(h[0]["expected_type"], json!("List<List<T>>"));
    assert_eq!(
        h[0]["span"],
        json!({ "file": HOLES_PAGES, "line": 20, "col": 5, "end_line": 20, "end_col": 17 })
    );
    assert_eq!(h[1]["hole"], json!("summary"));
    assert_eq!(h[1]["symbol"], json!("holes.pages.summarize"));
    assert_eq!(h[1]["expected_type"], json!("Text"));
    assert_eq!(
        h[1]["span"],
        json!({ "file": HOLES_PAGES, "line": 24, "col": 5, "end_line": 24, "end_col": 13 })
    );
    assert_eq!(r.holes.len(), 2);
    assert_eq!(r.verification.len(), 1);
}

/// Toolchain 12.3, Section 11, `--release`: `examples/holes` fails with
/// exactly two `hole_in_release` (in place of the two `hole` infos, S64)
/// and one `verification_unsatisfied`, keeps the `verification_result`
/// info, and warns `project_lock_missing`; `CheckResult::holes` still
/// lists both holes (S64).
#[test]
fn holes_release_diagnostics_are_exact() {
    let r = example_check("holes", true);
    assert_located(
        &r,
        &[
            at(HOLES_PAGES, 15, Severity::Error, "verification_unsatisfied"),
            at(HOLES_PAGES, 15, Severity::Info, "verification_result"),
            at(HOLES_PAGES, 20, Severity::Error, "hole_in_release"),
            at(HOLES_PAGES, 24, Severity::Error, "hole_in_release"),
            at("", 0, Severity::Warning, "project_lock_missing"),
        ],
    );
    assert!(r.has_errors());
    let h = with_code(&r, "hole_in_release");
    assert_eq!(h[0]["category"], json!("release_policy"));
    assert_eq!(h[0]["hole"], json!("build_pages"));
    assert_eq!(h[0]["symbol"], json!("pgn_7c2d"));
    assert_eq!(h[0]["qualified_name"], json!("holes.pages.paginate"));
    assert_eq!(h[0]["expected_type"], json!("List<List<T>>"));
    assert_eq!(
        h[0]["span"],
        json!({ "file": HOLES_PAGES, "line": 20, "col": 5, "end_line": 20, "end_col": 17 })
    );
    assert_eq!(h[1]["hole"], json!("summary"));
    assert_eq!(h[1]["symbol"], json!("holes.pages.summarize"));
    assert_eq!(h[1]["qualified_name"], json!("holes.pages.summarize"));
    assert_eq!(h[1]["expected_type"], json!("Text"));
    let v = one(&r, "verification_unsatisfied");
    assert_eq!(v["symbol"], json!("pgn_7c2d"));
    assert_eq!(v["status"], json!("unknown"));
    assert_eq!(v["facet"], json!("verification_policy"));
    assert_paginate_status(
        &one(&r, "verification_result"),
        HOLES_PAGES,
        15,
        "holes.pages.paginate",
    );
    let names: Vec<&str> = r.holes.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["build_pages", "summary"]);
    assert_eq!(r.verification.len(), 1);
    for path in ["holes.pages.paginate", "holes.pages.summarize"] {
        assert_eq!(def_status(&r.program, path), DefStatus::HasErrors, "{path}");
    }
    assert_eq!(def_status(&r.program, "holes.pages.Page"), DefStatus::Ok);
}

/// Toolchain 1.6: "Loading from a tree and from a directory produce
/// identical Projects", so both examples report the same diagnostic
/// objects, in both modes, when loaded as a `VirtualTree`.
#[test]
fn examples_check_alike_from_a_tree_and_a_directory() {
    for name in ["reader", "holes"] {
        for release_mode in [false, true] {
            let config = if release_mode {
                release()
            } else {
                CheckConfig::default()
            };
            let from_dir = check_with(&load_dir(&example(name)), &config);
            let from_tree = check_with(&load_tree(&tree_of_dir(&example(name))), &config);
            assert_eq!(
                json_values(&from_dir),
                json_values(&from_tree),
                "{name}, release = {release_mode}"
            );
        }
    }
}
