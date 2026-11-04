//! The verifier on small programs: proofs, counterexamples, and the reasons of `unknown`.

use cpl_diag::FileOrigin;
use cpl_hir::{DefId, Program};
use cpl_project::{load_project_from_tree, LoadOptions, VirtualTree};
use cpl_prove::{equivalent, verify, Report, Site, Status};
use serde_json::Value;

fn check(source: &str) -> Program {
    let tree = VirtualTree {
        files: [
            (
                "cpl.toml".to_string(),
                b"[project]\nname = \"t\"\nedition = \"0.1\"\n".to_vec(),
            ),
            (
                "src/m.cpl".to_string(),
                format!("module m\n{source}").into_bytes(),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let project =
        load_project_from_tree(&tree, FileOrigin::Virtual, &LoadOptions::default()).expect("loads");
    let checked = cpl_sema::check(&project);
    let errors: Vec<String> = checked
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{}: {}", d.code.0, d.message))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    checked.program
}

fn def(p: &Program, name: &str) -> DefId {
    p.defs()
        .find(|d| d.name.display() == name && p.module(d.module).path == ["m"])
        .unwrap_or_else(|| panic!("no {name}"))
        .id
}

fn report(source: &str, name: &str) -> (Program, Report) {
    let p = check(source);
    let r = verify(&p, def(&p, name)).expect("a function with a body");
    (p, r)
}

/// The statuses of a report's verdicts of one kind.
fn statuses(r: &Report, kind: fn(&Site) -> bool) -> Vec<(Status, Option<&'static str>)> {
    r.verdicts
        .iter()
        .filter(|(s, _)| kind(s))
        .map(|(_, v)| (v.status, v.reason))
        .collect()
}

fn ensures(s: &Site) -> bool {
    matches!(s, Site::Ensures(_))
}

#[test]
fn linear_postconditions_are_proved() {
    let (_, r) = report(
        "fn clamp(x: Int, lo: Int, hi: Int) -> Int\n    requires lo <= hi\n    ensures result >= lo and result <= hi\n    ensures result == x or result == lo or result == hi\n{\n    if x < lo { lo } else if x > hi { hi } else { x }\n}\n",
        "clamp",
    );
    assert_eq!(
        statuses(&r, ensures),
        vec![(Status::Proved, None), (Status::Proved, None)]
    );
    assert_eq!(r.status, Status::Proved);
}

#[test]
fn a_false_postcondition_fails_with_the_inputs_that_break_it() {
    let (_, r) = report(
        "fn next(x: Int) -> Int\n    ensures result > x\n{\n    if x > 10 { x } else { x + 1 }\n}\n",
        "next",
    );
    let (_, v) = &r.verdicts[0];
    assert_eq!(v.status, Status::Failed, "{v:?}");
    let evidence = v.evidence.as_ref().unwrap();
    assert_eq!(evidence["concrete"], Value::Bool(true));
    let x: i64 = evidence["model"]["x"].as_i64().unwrap();
    assert!(x > 10, "{evidence}");
    assert_eq!(r.status, Status::Failed);
}

#[test]
fn a_model_names_a_variant_the_enum_has() {
    // `o != None` alone does not pin the variant tag down: its range must, so that the model
    // is a value of the type (fuzzing found `{"$variant": ""}` here).
    let (_, r) = report(
        "enum Shape {\n    Dot\n    Line(Int)\n    Box(Int, Int)\n}\nfn f(o: Option<Int>, s: Shape) -> Int\n    ensures o == None or s == Dot\n{\n    0\n}\n",
        "f",
    );
    let (_, v) = &r.verdicts[0];
    assert_eq!(v.status, Status::Failed, "{v:?}");
    let model = &v.evidence.as_ref().unwrap()["model"];
    assert_eq!(model["o"]["$variant"], "Some", "{model}");
    let shape = model["s"]["$variant"].as_str().unwrap_or_default();
    assert!(["Line", "Box"].contains(&shape), "{model}");
}

#[test]
fn divisions_and_preconditions_are_decided() {
    let source = "fn risky(a: Int, b: Int) -> Int {\n    a / b\n}\nfn safe(a: Int, b: Int) -> Int\n    requires b != 0\n{\n    a / b\n}\nfn exact(a: Int, b: Int) -> Int\n    requires b != 0\n    requires a % b == 0\n    ensures result * b == a\n{\n    a / b\n}\nfn positive(x: Int) -> Int\n    requires x > 0\n{\n    x\n}\nfn caller(y: Int) -> Int {\n    positive(y)\n}\nfn guarded(y: Int) -> Int {\n    if y > 0 { positive(y) } else { 0 }\n}\n";
    let p = check(source);
    let risky = verify(&p, def(&p, "risky")).unwrap();
    assert_eq!(risky.verdicts[0].1.status, Status::Failed);
    assert_eq!(
        risky.verdicts[0].1.evidence.as_ref().unwrap()["model"]["b"],
        0
    );
    let safe = verify(&p, def(&p, "safe")).unwrap();
    assert_eq!(safe.status, Status::Proved, "{safe:?}");
    let exact = verify(&p, def(&p, "exact")).unwrap();
    assert_eq!(exact.status, Status::Proved, "{exact:?}");
    let caller = verify(&p, def(&p, "caller")).unwrap();
    assert_eq!(caller.status, Status::Failed);
    let y = caller.verdicts[0].1.evidence.as_ref().unwrap()["model"]["y"]
        .as_i64()
        .unwrap();
    assert!(y <= 0);
    let guarded = verify(&p, def(&p, "guarded")).unwrap();
    assert_eq!(guarded.status, Status::Proved, "{guarded:?}");
}

#[test]
fn loops_are_proved_from_their_invariants() {
    let source = "fn sum_to(n: Int) -> Int\n    requires n >= 0\n    ensures result >= 0\n{\n    var i = 0\n    var s = 0\n    while i < n\n        invariant i >= 0 and i <= n\n        invariant s >= 0\n        decreases n - i\n    {\n        s = s + i\n        i = i + 1\n    }\n    s\n}\nfn inferred(n: Int) -> Int\n    requires n >= 0\n    ensures result == n\n{\n    var i = 0\n    while i < n\n        invariant i >= 0\n        decreases n - i\n    {\n        i = i + 1\n    }\n    i\n}\nfn weak(n: Int) -> Int\n    requires n >= 0\n    ensures result == n\n{\n    var i = 0\n    var a = 0\n    var b = 0\n    while i < n\n        invariant i >= 0 and i <= n\n        decreases n - i\n    {\n        if i % 2 == 0 { a = a + 1 } else { b = b + 1 }\n        i = i + 1\n    }\n    a + b\n}\nfn wrong(n: Int) -> Int\n    requires n >= 0\n    ensures result == n\n{\n    var i = 0\n    while i < n\n        invariant i >= 0\n        decreases n - i\n    {\n        i = i + 2\n    }\n    i\n}\n";
    let p = check(source);
    let r = verify(&p, def(&p, "sum_to")).unwrap();
    assert_eq!(r.status, Status::Proved, "{r:?}");
    // `i <= n` is not declared: inference finds it (the condition, relaxed by the step).
    let r = verify(&p, def(&p, "inferred")).unwrap();
    assert_eq!(r.status, Status::Proved, "{r:?}");
    // `a + b == i` is beyond the inferred forms: the postcondition does not follow.
    let w = verify(&p, def(&p, "weak")).unwrap();
    assert_eq!(
        statuses(&w, ensures),
        vec![(Status::Unknown, Some("invariant_too_weak"))]
    );
    // A candidate the loop's abstraction cannot settle is run on its inputs: an odd `n` is a
    // concrete counterexample.
    let wrong = verify(&p, def(&p, "wrong")).unwrap();
    let (_, v) = &wrong.verdicts[0];
    assert_eq!(v.status, Status::Failed, "{v:?}");
    let n = v.evidence.as_ref().unwrap()["model"]["n"].as_i64().unwrap();
    assert!(n % 2 == 1, "{v:?}");
    let init: Vec<_> = statuses(&w, |s| {
        matches!(
            s,
            Site::InvariantInit(..) | Site::InvariantPreserve(..) | Site::LoopDecreases(_)
        )
    });
    assert!(init.iter().all(|(s, _)| *s == Status::Proved), "{init:?}");
}

#[test]
fn recursion_is_proved_with_its_measure_and_nonlinear_facts() {
    let source = "fn fact(n: Int) -> Int\n    requires n >= 0\n    ensures result >= 1\n    decreases n\n{\n    if n == 0 { 1 } else { n * fact(n - 1) }\n}\nfn forever(n: Int) -> Int\n    requires n >= 0\n    decreases n\n{\n    if n == 0 { 0 } else { forever(n) }\n}\nfn unmeasured(n: Int) -> Int {\n    if n <= 0 { 0 } else { unmeasured(n - 1) }\n}\n";
    let p = check(source);
    let r = verify(&p, def(&p, "fact")).unwrap();
    assert_eq!(r.status, Status::Proved, "{r:?}");
    let f = verify(&p, def(&p, "forever")).unwrap();
    let decreases = statuses(&f, |s| *s == Site::Decreases);
    assert_eq!(decreases, vec![(Status::Failed, None)]);
    let u = verify(&p, def(&p, "unmeasured")).unwrap();
    assert_eq!((u.status, u.reason), (Status::Unknown, Some("no_measure")));
}

#[test]
fn data_types_refinements_and_invariants() {
    let source = "pub type Pos = Int where self > 0\nstruct Range {\n    lo: Int\n    hi: Int\n} where self.lo <= self.hi\nenum Shape {\n    Circle(Int)\n    Square(Int)\n    Empty\n}\nfn area(s: Shape) -> Int\n    ensures result >= 0\n{\n    match s {\n        Circle(r) => r * r * 3\n        Square(w) => w * w\n        Empty => 0\n    }\n}\nfn width(r: Range) -> Int\n    ensures result >= 0\n{\n    r.hi - r.lo\n}\nfn widen(r: Range, by: Pos) -> Range {\n    Range { lo: r.lo - by, hi: r.hi + by }\n}\nfn flip(r: Range) -> Range {\n    Range { lo: r.hi, hi: r.lo }\n}\nfn unwrap_or_zero(o: Option<Int>) -> Int\n    ensures o == None or o == Some(result)\n{\n    match o {\n        Some(v) => v\n        None => 0\n    }\n}\n";
    let p = check(source);
    for name in ["area", "width", "widen", "unwrap_or_zero"] {
        let r = verify(&p, def(&p, name)).unwrap();
        assert_eq!(r.status, Status::Proved, "{name}: {r:?}");
    }
    let flip = verify(&p, def(&p, "flip")).unwrap();
    assert_eq!(flip.status, Status::Failed, "{flip:?}");
    let model = &flip.verdicts[0].1.evidence.as_ref().unwrap()["model"]["r"];
    assert_eq!(model["$type"], "m.Range");
    assert!(
        model["lo"].as_i64().unwrap() < model["hi"].as_i64().unwrap(),
        "{model}"
    );
}

#[test]
fn builtin_axioms_and_unsupported_values() {
    let source = "fn first_page(items: List<Int>, size: Int) -> List<Int>\n    requires size > 0\n{\n    items.take(size)\n}\nfn shrink(items: List<Int>, size: Int) -> Int\n    requires size > 0\n    requires items.len > size\n    ensures result < items.len\n{\n    items.drop(size).len\n}\nfn texts(t: Text) -> Bool\n    ensures result\n{\n    t.len > 3\n}\nfn twice(t: Text) -> Bool\n    ensures result\n{\n    (t ++ t).len == 2 * t.len\n}\n";
    let p = check(source);
    assert_eq!(
        verify(&p, def(&p, "first_page")).unwrap().status,
        Status::Proved
    );
    let shrink = verify(&p, def(&p, "shrink")).unwrap();
    assert_eq!(shrink.status, Status::Proved, "{shrink:?}");
    // Text concatenation is uninterpreted: a true property of it stays unknown.
    let t = verify(&p, def(&p, "twice")).unwrap();
    assert_eq!(
        statuses(&t, ensures),
        vec![(Status::Unknown, Some("unsupported_expression"))]
    );
    // The length of a text is not either, but the candidate (a short text) runs concretely.
    let t = verify(&p, def(&p, "texts")).unwrap();
    assert_eq!(statuses(&t, ensures), vec![(Status::Failed, None)]);
    let model = &t.verdicts[0].1.evidence.as_ref().unwrap()["model"]["t"];
    assert!(model.as_str().unwrap().chars().count() <= 3, "{model}");
}

#[test]
fn equivalence_is_proved_or_refuted() {
    let old = check("fn f(x: Int, y: Int) -> Int {\n    if x > y { x - y } else { y - x }\n}\nfn g(n: Int) -> Int {\n    n * 2\n}\n");
    let same = |_: &cpl_diag::StableId| true;
    let new = check("fn f(x: Int, y: Int) -> Int {\n    (x - y).abs()\n}\nfn g(n: Int) -> Int {\n    n + n\n}\n");
    let e = equivalent(&old, def(&old, "f"), &new, def(&new, "f"), &|id| {
        same(id) && !id.as_str().ends_with(".f")
    });
    assert_eq!(e.status, Status::Proved, "{e:?}");
    let e = equivalent(&old, def(&old, "g"), &new, def(&new, "g"), &|_| false);
    assert_eq!(e.status, Status::Proved, "{e:?}");
    let wrong = check(
        "fn f(x: Int, y: Int) -> Int {\n    x - y\n}\nfn g(n: Int) -> Int {\n    n * 2 + 1\n}\n",
    );
    let e = equivalent(&old, def(&old, "f"), &wrong, def(&wrong, "f"), &|_| false);
    assert_eq!(e.status, Status::Failed, "{e:?}");
    let x = e.evidence["inputs"]["x"].as_i64().unwrap();
    let y = e.evidence["inputs"]["y"].as_i64().unwrap();
    assert!(x < y, "{e:?}");
    let e = equivalent(&old, def(&old, "g"), &wrong, def(&wrong, "g"), &|_| false);
    assert_eq!(e.status, Status::Failed, "{e:?}");
}

#[test]
fn inlining_or_extracting_an_identical_helper_is_decided() {
    // `magnitude` is the same in both programs, so its calls are first one uninterpreted function
    // of both sides; the inlined version is decided by executing the helper in place.
    let old = check("fn magnitude(a: Int) -> Int {\n    if a < 0 { -a } else { a }\n}\nfn f(x: Int) -> Int {\n    magnitude(x) + 1\n}\n");
    let new = check("fn magnitude(a: Int) -> Int {\n    if a < 0 { -a } else { a }\n}\nfn f(x: Int) -> Int {\n    (if x < 0 { -x } else { x }) + 1\n}\n");
    let same = |id: &cpl_diag::StableId| id.as_str().ends_with(".magnitude");
    let e = equivalent(&old, def(&old, "f"), &new, def(&new, "f"), &same);
    assert_eq!(e.status, Status::Proved, "{e:?}");
    // extracting it back is the same query the other way around
    let e = equivalent(&new, def(&new, "f"), &old, def(&old, "f"), &same);
    assert_eq!(e.status, Status::Proved, "{e:?}");
    // a refactoring that drops the helper's negative case is refuted with inputs that show it
    let wrong = check("fn magnitude(a: Int) -> Int {\n    if a < 0 { -a } else { a }\n}\nfn f(x: Int) -> Int {\n    x + 1\n}\n");
    let e = equivalent(&old, def(&old, "f"), &wrong, def(&wrong, "f"), &same);
    assert_eq!(e.status, Status::Failed, "{e:?}");
    assert!(e.evidence["inputs"]["x"].as_i64().unwrap() < 0, "{e:?}");
}

#[test]
fn equivalence_respects_domains_faults_and_recursion() {
    // The old precondition bounds the domain; the new version may fault outside it only.
    let old = check("fn div(a: Int, b: Int) -> Int\n    requires b != 0\n{\n    a / b\n}\nfn count(n: Int) -> Int\n    requires n >= 0\n    decreases n\n{\n    if n == 0 { 0 } else { 1 + count(n - 1) }\n}\n");
    let new = check("fn div(a: Int, b: Int) -> Int {\n    if b == 0 { 0 } else { a / b }\n}\nfn count(n: Int) -> Int\n    requires n >= 0\n    decreases n\n{\n    if n <= 0 { 0 } else { count(n - 1) + 1 }\n}\n");
    let e = equivalent(&old, def(&old, "div"), &new, def(&new, "div"), &|_| false);
    assert_eq!(e.status, Status::Proved, "{e:?}");
    let e = equivalent(&old, def(&old, "count"), &new, def(&new, "count"), &|_| {
        false
    });
    assert_eq!(e.status, Status::Proved, "{e:?}");
    assert_eq!(e.evidence["partial"], true);
    // A stronger new precondition faults on inputs the old version accepted.
    let strict = check("fn div(a: Int, b: Int) -> Int\n    requires b > 0\n{\n    a / b\n}\nfn count(n: Int) -> Int {\n    n\n}\n");
    let e = equivalent(
        &old,
        def(&old, "div"),
        &strict,
        def(&strict, "div"),
        &|_| false,
    );
    assert_eq!(e.status, Status::Failed, "{e:?}");
    assert!(e.evidence["inputs"]["b"].as_i64().unwrap() < 0);
}

#[test]
fn generic_callees_are_executed_at_their_instance() {
    // `pick` returns a payload of its generic parameter; at `Bool` it is a condition.
    let source = "fn pick<T>(o: Option<T>, fallback: T) -> T {\n    match o {\n        Some(v) => v\n        None => fallback\n    }\n}\nfn decide(o: Option<Bool>) -> Int\n    ensures result == 1\n{\n    if pick(o, false) { 1 } else { 0 }\n}\nfn same(o: Option<Int>) -> Int\n    ensures result == 0 or o != None\n{\n    pick(o, 0)\n}\n";
    let p = check(source);
    // `decide(Some(false))` is 0: refuted, not proved.
    let d = verify(&p, def(&p, "decide")).unwrap();
    assert_eq!(d.status, Status::Failed, "{d:?}");
    let o = &d.verdicts[0].1.evidence.as_ref().unwrap()["model"]["o"];
    assert!(
        *o == serde_json::json!({"$type": "Option", "$variant": "None"})
            || o["$payload"][0] == false,
        "{o}"
    );
    let s = verify(&p, def(&p, "same")).unwrap();
    assert_eq!(s.status, Status::Proved, "{s:?}");
}

#[test]
fn an_unchanged_loop_is_shared_by_both_versions() {
    let body = |tail: &str| {
        format!("fn total(n: Int) -> Int\n    requires n >= 0\n{{\n    var i = 0\n    var s = 0\n    while i < n\n        invariant i >= 0\n        decreases n - i\n    {{\n        s = s + i * i\n        i = i + 1\n    }}\n    {tail}\n}}\n")
    };
    let old = check(&body("s + 1"));
    let new = check(&body("1 + s"));
    let e = equivalent(&old, def(&old, "total"), &new, def(&new, "total"), &|_| {
        false
    });
    assert_eq!(e.status, Status::Proved, "{e:?}");
    assert_eq!(e.evidence["partial"], true);
    let changed = check(&body("s + 2"));
    let e = equivalent(
        &old,
        def(&old, "total"),
        &changed,
        def(&changed, "total"),
        &|_| false,
    );
    assert_ne!(e.status, Status::Proved, "{e:?}");
}

#[test]
fn a_fault_inside_a_contract_is_behavior() {
    // `h`'s precondition divides by `y`: the old `f` faults at `y == 0`, the new one returns 1.
    let helper = "fn h(x: Int, y: Int) -> Int\n    requires x / y == x / y\n{\n    1\n}\n";
    let old = check(&format!(
        "{helper}fn f(x: Int, y: Int) -> Int {{\n    h(x, y)\n}}\n"
    ));
    let new = check(&format!(
        "{helper}fn f(x: Int, y: Int) -> Int {{\n    1\n}}\n"
    ));
    let e = equivalent(&old, def(&old, "f"), &new, def(&new, "f"), &|_| false);
    assert_eq!(e.status, Status::Failed, "{e:?}");
    assert_eq!(e.evidence["inputs"]["y"], 0);
}

#[test]
fn a_guard_protects_what_it_short_circuits() {
    let p = check("fn ratio(x: Int, y: Int) -> Bool {\n    y != 0 and x / y > 1\n}\nfn either(x: Int, y: Int) -> Bool {\n    y == 0 or x / y > 1\n}\nfn bare(x: Int, y: Int) -> Bool {\n    x / y > 1 or y == 0\n}\n");
    assert_eq!(verify(&p, def(&p, "ratio")).unwrap().status, Status::Proved);
    assert_eq!(
        verify(&p, def(&p, "either")).unwrap().status,
        Status::Proved
    );
    assert_eq!(verify(&p, def(&p, "bare")).unwrap().status, Status::Failed);
}
