//! Faults with exact payloads (toolchain 1.7.1, 1.7.3; semantics 6.7, 6.10,
//! 7.1, 8.1, S34, S41, S58): division by zero, assertions, holes, builtin
//! preconditions, `check<R>`, backtraces, on a checked program; and the
//! impossible states (`Refine` whose static proof was wrong, a `match` with
//! no matching arm, a generator as a value), which no checked program has
//! and which are therefore built by hand.

use crate::common;
use crate::support::*;
use cpl_eval::testing::{FnBuilder, ProgramBuilder};
use cpl_eval::EvalConfig;
use cpl_hir::{GenExpr, PatternKind, Program, Ty};
use cpl_syntax::ast::BinaryOp::*;
use serde_json::{json, Value as Json};

const FILE: &str = "src/checks/faults.cpl";

const SOURCE: &str = r#"module checks.faults

pub type PageId(Text)
pub type Positive = Int where self > 0
pub type Small = Positive where self < 10
pub type Nz = Int where 10 / self > 0

pub struct Page {
    id: PageId
    text: Text
}

fn div(a: Int, b: Int) -> Int {
    a / b
}

fn rem(a: Int, b: Int) -> Int {
    a % b
}

fn bottom(x: Int) -> Int {
    10 / x
}

fn middle(x: Int) -> Int {
    bottom(x) + 1
}

fn top(x: Int) -> Int {
    middle(x) * 2
}

fn checked_div(a: Int, b: Int) -> Int {
    assert b != 0
    a / b
}

fn scoped(n: Int) -> Int {
    if true {
        let gone = 1
    }
    var total = n
    let shown = total + 1
    assert shown < n
    0
}

fn summarize(page: Page) -> Text {
    ?summary
}

@id("pgn_7c2d")
pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>> {
    ?build_pages
}

fn pages_of(items: List<Int>) -> List<List<Int>> {
    paginate(items, 2)
}

fn take_n(xs: List<Int>, n: Int) -> List<Int> {
    xs.take(n)
}

fn to_positive(n: Int) -> Result<Positive, ConstraintError> {
    check<Positive>(n)
}

fn to_small(n: Int) -> Result<Small, ConstraintError> {
    check<Small>(n)
}

fn to_nz(n: Int) -> Result<Nz, ConstraintError> {
    check<Nz>(n)
}

fn double(p: Positive) -> Int {
    p * 2
}

fn doubled(n: Int) -> Result<Int, ConstraintError> {
    let p = check<Positive>(n)?
    Ok(double(p))
}
"#;

fn fixture() -> Program {
    common::check_files(&[(FILE, SOURCE)])
}

fn f(name: &str) -> String {
    format!("checks.faults.{name}")
}

fn value(p: &Program, name: &str, args: &[Json]) -> Json {
    common::value(p, &common::run_json(p, &f(name), args))
}

fn fault(p: &Program, name: &str, args: &[Json]) -> Json {
    common::fault(p, &common::run_json(p, &f(name), args))
}

/// The location of `needle` at or after the line holding `anchor`.
fn at(anchor: &str, needle: &str) -> Json {
    common::location_after(SOURCE, FILE, anchor, needle)
}

#[test]
fn division_by_zero_blames_the_enclosing_declaration() {
    let p = fixture();
    assert_eq!(value(&p, "div", &[json!(-7), json!(2)]), json!(-3));
    assert_eq!(value(&p, "rem", &[json!(7), json!(-2)]), json!(1));
    let d = fault(&p, "div", &[json!(1), json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["category"], "runtime");
    assert_eq!(d["symbol"], "checks.faults.div");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
    // The span is the operator expression.
    assert_eq!(d["span"], at("fn div(", "a / b"));
    assert_eq!(backtrace(&d), ["checks.faults.div"]);
    let d = fault(&p, "rem", &[json!(5), json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["input"], json!({"a": 5, "b": 0}));
}

#[test]
fn backtraces_are_innermost_first_with_call_spans() {
    let p = fixture();
    let d = fault(&p, "top", &[json!(0)]);
    assert_eq!(d["symbol"], "checks.faults.bottom");
    assert_eq!(
        backtrace(&d),
        [
            "checks.faults.bottom",
            "checks.faults.middle",
            "checks.faults.top"
        ]
    );
    // Each frame carries the span of the call that entered it.
    assert_eq!(d["backtrace"][0]["span"], at("fn middle(", "bottom(x)"));
    assert_eq!(d["backtrace"][1]["span"], at("fn top(", "middle(x)"));
    assert!(d["backtrace"][2].get("span").is_none());
    assert_eq!(value(&p, "top", &[json!(5)]), json!(6));
}

#[test]
fn assertion_failed_carries_the_clause_and_the_bindings() {
    let p = fixture();
    assert_eq!(value(&p, "checked_div", &[json!(6), json!(3)]), json!(2));
    let d = fault(&p, "checked_div", &[json!(1), json!(0)]);
    assert_eq!(d["code"], "assertion_failed");
    assert_eq!(d["category"], "test");
    assert_eq!(d["clause"], "b != 0");
    assert_eq!(d["symbol"], "checks.faults.checked_div");
    assert_eq!(d["facet"], "body");
    // The span is the asserted expression.
    assert_eq!(d["span"], at("fn checked_div(", "b != 0"));
    // Innermost first: b was bound after a.
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "b", "type": "Int", "kind": "parameter", "value": 0},
            {"name": "a", "type": "Int", "kind": "parameter", "value": 1}
        ])
    );
}

#[test]
fn bindings_are_those_in_scope_with_kinds() {
    let p = fixture();
    let d = fault(&p, "scoped", &[json!(4)]);
    assert_eq!(d["clause"], "shown < n");
    // `gone` went out of scope with its block; `total` is a var.
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "shown", "type": "Int", "kind": "let", "value": 5},
            {"name": "total", "type": "Int", "kind": "var", "mutable": true, "value": 4},
            {"name": "n", "type": "Int", "kind": "parameter", "value": 4}
        ])
    );
}

#[test]
fn unresolved_hole_reports_the_hole() {
    let p = fixture();
    let page = json!({"$type": "checks.faults.Page",
                      "id": {"$type": "checks.faults.PageId", "$payload": ["p"]},
                      "text": "t"});
    let d = fault(&p, "summarize", std::slice::from_ref(&page));
    assert_eq!(d["code"], "unresolved_hole");
    assert_eq!(d["category"], "hole");
    assert_eq!(d["hole"], "summary");
    assert_eq!(d["symbol"], "checks.faults.summarize");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["expected_type"], "Text");
    assert_eq!(d["span"], at("fn summarize(", "?summary"));
    assert_eq!(
        d["bindings"],
        json!([{"name": "page", "type": "Page", "kind": "parameter", "value": page}])
    );
    // Generic: the expected type names the type parameter, the bindings are
    // typed as declared, and the symbol is the @id.
    let d = fault(&p, "pages_of", &[json!([1, 2])]);
    assert_eq!(d["hole"], "build_pages");
    assert_eq!(d["symbol"], "pgn_7c2d");
    assert_eq!(d["qualified_name"], "checks.faults.paginate");
    assert_eq!(d["expected_type"], "List<List<T>>");
    assert_eq!(
        d["bindings"],
        json!([
            {"name": "size", "type": "Positive", "kind": "parameter", "value": 2},
            {"name": "items", "type": "List<T>", "kind": "parameter", "value": [1, 2]}
        ])
    );
    assert_eq!(
        backtrace(&d),
        ["checks.faults.paginate", "checks.faults.pages_of"]
    );
}

#[test]
fn a_builtin_checks_its_own_precondition_s58() {
    let p = fixture();
    assert_eq!(
        value(&p, "take_n", &[json!([1, 2, 3]), json!(5)]),
        json!([1, 2, 3])
    );
    assert_eq!(
        value(
            &p,
            "take_n",
            &[json!([1, 2]), json!(10000000000000000000000u128)]
        ),
        json!([1, 2])
    );
    let d = fault(&p, "take_n", &[json!([1]), json!(-1)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "std.prelude.List.take");
    assert_eq!(d["qualified_name"], "std.prelude.List.take");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["caller"], "checks.faults.take_n");
    assert_eq!(d["clause"], "n >= 0");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
    // The span is the stub's clause; the stub's frame has the call's span.
    assert_eq!(d["span"]["file"], "std/prelude.cpl");
    assert_eq!(
        backtrace(&d),
        ["std.prelude.List.take", "checks.faults.take_n"]
    );
    assert_eq!(d["backtrace"][0]["span"], at("fn take_n(", "xs.take(n)"));
}

#[test]
fn check_returns_a_result_and_names_the_failing_refinement() {
    let p = fixture();
    let ok = |v| json!({"$type": "Result", "$variant": "Ok", "$payload": [v]});
    let err = |t: &str, m: &str| {
        json!({"$type": "Result", "$variant": "Err", "$payload": [
            {"$type": "ConstraintError", "type_name": t, "message": m}]})
    };
    assert_eq!(value(&p, "to_positive", &[json!(3)]), ok(json!(3)));
    assert_eq!(
        value(&p, "to_positive", &[json!(0)]),
        err("checks.faults.Positive", "0 does not satisfy self > 0")
    );
    // The chain runs root first (S44).
    assert_eq!(
        value(&p, "to_small", &[json!(0)]),
        err("checks.faults.Positive", "0 does not satisfy self > 0")
    );
    assert_eq!(
        value(&p, "to_small", &[json!(20)]),
        err("checks.faults.Small", "20 does not satisfy self < 10")
    );
    assert_eq!(value(&p, "to_small", &[json!(9)]), ok(json!(9)));
    assert_eq!(value(&p, "doubled", &[json!(4)]), ok(json!(8)));
    assert_eq!(
        value(&p, "doubled", &[json!(-4)]),
        err("checks.faults.Positive", "-4 does not satisfy self > 0")
    );
}

#[test]
fn a_faulting_predicate_is_blamed_on_the_refinement() {
    let p = fixture();
    let d = fault(&p, "to_nz", &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "checks.faults.Nz");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "10 / self > 0");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
    assert_eq!(backtrace(&d), ["checks.faults.Nz", "checks.faults.to_nz"]);
    // The refinement's frame has the span of the `check` that entered it.
    assert_eq!(d["backtrace"][0]["span"], at("fn to_nz(", "check<Nz>(n)"));
}

// ---- impossible states (built by hand: no checked program has them) ----------

#[test]
fn a_wrong_static_proof_is_an_internal_error() {
    // The checker records a `Refine` node only for a constant whose
    // predicates it evaluated to true (semantics 3.5, T39), so this program
    // (`fn bad_proof() -> Positive { 0 }` with a "proof") cannot come from
    // source; the re-check is what keeps an unsound proof rule from ever
    // producing a wrong value silently.
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let positive = b.add_int_refinement(m, "Positive", Gt, 0);
    let pos_ty = b.refined(positive);
    let mut f = FnBuilder::new("bad_proof", pos_ty.clone());
    f.body(h.value(h.refine(h.int(0), positive, pos_ty)));
    let bad_proof = b.add_fn(m, f);
    let p = b.build();
    let d = fault_diag(&p, bad_proof, &[]);
    assert_eq!(d["code"], "internal_error");
    assert_eq!(d["kind"], "refinement_proof_unsound");
    assert!(d.get("symbol").is_none());
    // Unchecked mode trusts the proof.
    let unchecked = EvalConfig {
        check_contracts: false,
        ..EvalConfig::default()
    };
    assert_eq!(eval_with(&p, unchecked, bad_proof, &[]).unwrap(), json!(0));
}

#[test]
fn impossible_states_are_internal_errors() {
    // A non-exhaustive `match` is rejected by the checker (semantics 3.13)
    // and a generator outside a `for` range too (S14), so both programs are
    // built by hand.
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let mut f = FnBuilder::new("no_arm", Ty::Int);
    let n = f.param("n", Ty::Int);
    f.body(h.value(h.match_(
        int(&h, n),
        vec![h.arm(
            h.pattern(PatternKind::Int(1.into()), Ty::Int),
            h.value(h.int(1)),
        )],
        Ty::Int,
    )));
    let no_arm = b.add_fn(m, f);
    let mut f = FnBuilder::new("generator", Ty::Int);
    f.body(h.value(h.gen(GenExpr::Int, Ty::Int)));
    let generator = b.add_fn(m, f);
    let p = b.build();
    assert_eq!(crate::support::value(&p, no_arm, &[json!(1)]), json!(1));
    let d = fault_diag(&p, no_arm, &[json!(2)]);
    assert_eq!(d["code"], "internal_error");
    assert_eq!(d["category"], "internal");
    assert_eq!(d["kind"], "no_matching_arm");
    assert!(d["detail"].as_str().unwrap().contains('2'));
    let d = fault_diag(&p, generator, &[]);
    assert_eq!(d["kind"], "generator_not_a_value");
}
