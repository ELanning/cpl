//! Adversarial tests of the body typer against `spec/semantics.md` Section
//! 3 in full, 2.5, 8.1, and 8.3, with the 9.4 payloads of the codes these
//! sections raise and the primary spans of S63.
//!
//! Every expectation is the specification's answer, not the
//! implementation's current behaviour: a failing test is a finding. The
//! sections are: assignability and the `Widen`/`Refine` nodes of
//! `spec/toolchain.md` T39 (3.4, S15, S20); the compile-time proof rule
//! (3.5, S16, S40, S55, S65, `check<R>`); generic inference (3.7, S19,
//! S62); members (3.9, S22, S23, D43); constructor resolution (2.5, S10);
//! struct literals and spread; empty lists (3.6, 3.7); `if` and `match`
//! with divergence (3.10, 3.11, S24, S68); exhaustiveness and
//! reachability (3.13, S25); `?` (3.8, 5.1); generators (S14, T18); and
//! holes with their reports (8.1, 8.3, S48, S56, S64).

mod common;

use common::*;
use cpl_diag::Severity;
use cpl_hir::{
    BindingKind, Block, Def, DefKind, Expr, ExprKind, FnBody, GenExpr, PatternKind, Program,
    RefinementProof, Stmt, Ty,
};
use cpl_sema::CheckResult;
use serde_json::{json, Value};
use std::collections::BTreeMap;

// ---- helpers -------------------------------------------------------------------

/// The 1-based line that ends with the marker comment `// #name`.
#[track_caller]
fn mark(src: &str, name: &str) -> u32 {
    let tag = format!("// #{name}");
    let hits: Vec<usize> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim_end().ends_with(&tag))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits.len(), 1, "marker {tag:?} must be on exactly one line");
    hits[0] as u32 + 1
}

/// The 1-based line of the only line of `src` that contains `needle`.
#[track_caller]
fn line(src: &str, needle: &str) -> u32 {
    let hits: Vec<usize> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits.len(), 1, "{needle:?} must be on exactly one line");
    hits[0] as u32 + 1
}

/// The 1-based (line, column) of the only occurrence of `needle`.
#[track_caller]
fn pos(src: &str, needle: &str) -> (u64, u64) {
    assert_eq!(
        src.matches(needle).count(),
        1,
        "{needle:?} must occur exactly once"
    );
    let before = &src[..src.find(needle).expect("present")];
    let line = before.matches('\n').count() as u64 + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() as u64 + 1;
    (line, col)
}

/// The diagnostics in project files as sorted (line, code) pairs.
fn got(result: &CheckResult) -> Vec<(u32, String)> {
    let mut v: Vec<(u32, String)> = project_triples(result)
        .into_iter()
        .map(|t| (t.0, t.2))
        .collect();
    v.sort();
    v
}

/// The exact multiset of (line, code) of the project diagnostics.
#[track_caller]
fn expect(result: &CheckResult, want: &[(u32, &str)]) {
    let mut want: Vec<(u32, String)> = want.iter().map(|(l, c)| (*l, c.to_string())).collect();
    want.sort();
    assert_eq!(got(result), want, "\n{}", render(result));
}

/// The JSON of every diagnostic with `code`, in result order.
fn with_code(result: &CheckResult, code: &str) -> Vec<Value> {
    json_values(result)
        .into_iter()
        .filter(|d| d["code"] == code)
        .collect()
}

/// The JSON of the only `code` diagnostic whose primary span starts on
/// `line`.
#[track_caller]
fn diag_at(result: &CheckResult, code: &str, line: u32) -> Value {
    let hits: Vec<Value> = with_code(result, code)
        .into_iter()
        .filter(|d| d["span"]["line"].as_u64() == Some(u64::from(line)))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "one {code} on line {line}:\n{}",
        render(result)
    );
    hits.into_iter().next().expect("one")
}

/// The start (line, column) of a diagnostic's primary span.
fn start(d: &Value) -> (u64, u64) {
    (
        d["span"]["line"].as_u64().expect("a span line"),
        d["span"]["col"].as_u64().expect("a span column"),
    )
}

/// The end (line, column) of a diagnostic's primary span.
fn end(d: &Value) -> (u64, u64) {
    (
        d["span"]["end_line"].as_u64().expect("a span end line"),
        d["span"]["end_col"].as_u64().expect("a span end column"),
    )
}

/// The messages of a diagnostic's notes.
fn notes(d: &Value) -> Vec<String> {
    d["notes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|n| n["message"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `expected` and `actual` of a diagnostic, as JSON.
fn ea(d: &Value) -> (Value, Value) {
    (d["expected"].clone(), d["actual"].clone())
}

/// A sorted list of strings.
fn sorted(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .expect("an array")
        .iter()
        .map(|x| x.as_str().expect("a string").to_string())
        .collect();
    out.sort();
    out
}

fn def<'p>(p: &'p Program, path: &str) -> &'p Def {
    p.defs()
        .find(|d| p.display_path(d.id) == path)
        .unwrap_or_else(|| panic!("no definition {path}"))
}

fn body<'p>(p: &'p Program, path: &str) -> &'p Block {
    match &def(p, path).kind {
        DefKind::Fn(f) => match &f.body {
            FnBody::Block(b) => b,
            FnBody::Builtin(_) => panic!("{path} is a builtin"),
        },
        DefKind::Test(t) => &t.body,
        DefKind::Property(pr) => &pr.body,
        _ => panic!("{path} has no body"),
    }
}

fn tail(b: &Block) -> &Expr {
    b.tail.as_deref().expect("a tail expression")
}

/// The `from` refinements of the `Widen` nodes around `e`, in the order of
/// the steps down the chain (innermost node first), and the widened
/// expression.
fn widens<'e>(p: &Program, e: &'e Expr) -> (Vec<String>, &'e Expr) {
    let mut steps = Vec::new();
    let mut e = e;
    while let ExprKind::Widen { expr, from } = &e.kind {
        steps.push(p.display_path(*from));
        e = expr;
    }
    steps.reverse();
    (steps, e)
}

/// Every `Refine` node of a block in body order: (refinement, node type).
fn refines(p: &Program, b: &Block) -> Vec<(String, String)> {
    let mut out = Vec::new();
    b.walk(&mut |e| {
        if let ExprKind::Refine { refined, proof, .. } = &e.kind {
            assert_eq!(*proof, RefinementProof::Constant);
            out.push((p.display_path(*refined), p.display_ty(&e.ty)));
        }
    });
    out
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// The type of the local `name` of definition `path`, with its generics.
fn local(p: &Program, path: &str, name: &str) -> String {
    let d = def(p, path);
    let l = d
        .kind
        .locals()
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no local {name} in {path}"));
    p.display_ty_in(&l.ty, d.kind.generics())
}

/// The first arm pattern of the `match` that is the tail of `b`.
fn first_arm_pattern(b: &Block) -> &PatternKind {
    let ExprKind::Match { arms, .. } = &tail(b).kind else {
        panic!("a match tail")
    };
    &arms[0].pattern.kind
}

/// Declarations shared by the assignability tests.
const REFS: &str = r#"
type Positive = Int where self > 0
type Big = Positive where self > 100
type P2 = Int where self > 0
type Small = Positive where self < 10
type PageId(Text)

struct Dims {
    width: Positive
    height: Int
}

fn takes_int(n: Int) -> Int {
    n
}

fn takes_positive(p: Positive) -> Int {
    p
}

fn takes_p2(p: P2) -> Int {
    p
}

fn takes_big(b: Big) -> Int {
    b
}
"#;

/// `module <name>`, [`REFS`], then `rest`.
fn with_refs(name: &str, rest: &str) -> String {
    format!("module {name}\n{REFS}{rest}")
}

// ---- 3.4: assignability, Widen and Refine (T39) ----------------------------------------

/// 3.4 rule 2 (refinement to base, top level only) in every check-mode
/// position of S56: a `Big` reaches `Int` through one `Widen` per
/// refinement of its chain; rule 2' stops at the expected refinement, also
/// when that is a structurally identical refinement declared under another
/// name (S55); rule 1 (identity) inserts no node.
#[test]
fn widening_inserts_one_node_per_chain_step() {
    let src = with_refs(
        "t.wa",
        r#"
fn arg(b: Big) -> Int {
    takes_int(b)
}

fn ret(b: Big) -> Int {
    return b
}

fn element(b: Big) -> List<Int> {
    [b]
}

fn payload(b: Big) -> Option<Int> {
    Some(b)
}

fn field(b: Big) -> Dims {
    Dims { width: b, height: b }
}

fn assign(b: Big) -> Int {
    var n = 0
    n = b
    n
}

fn operand(b: Big) -> Int {
    b + 1
}

fn middle(b: Big) -> Positive {
    b
}

fn same_class(b: Big) -> P2 {
    b
}

fn identical(p: P2) -> Positive {
    p
}
"#,
    );
    let result = check_source(&src);
    expect(&result, &[]);
    let p = &result.program;
    let down = |e: &Expr| widens(p, e).0;
    let to_int = ["t.wa.Big", "t.wa.Positive"];

    let ExprKind::Call { args, .. } = &tail(body(p, "t.wa.arg")).kind else {
        panic!("a call")
    };
    assert_eq!(down(&args[0]), to_int);
    assert!(matches!(widens(p, &args[0]).1.kind, ExprKind::Local(_)));
    assert_eq!(args[0].ty, Ty::Int);

    let Stmt::Return(Some(r)) = &body(p, "t.wa.ret").stmts[0] else {
        panic!("a return")
    };
    assert_eq!(down(r), to_int);

    let ExprKind::List(items) = &tail(body(p, "t.wa.element")).kind else {
        panic!("a list")
    };
    assert_eq!(down(&items[0]), to_int);

    let ExprKind::Ctor { args, .. } = &tail(body(p, "t.wa.payload")).kind else {
        panic!("Some(b)")
    };
    assert_eq!(down(&args[0]), to_int);

    let ExprKind::StructLit { fields, .. } = &tail(body(p, "t.wa.field")).kind else {
        panic!("a struct literal")
    };
    let field = |i: u32| &fields.iter().find(|(j, _)| *j == i).expect("a field").1;
    // `width: Positive` takes one step (rule 2'), `height: Int` two (rule 2).
    assert_eq!(down(field(0)), ["t.wa.Big"]);
    assert_eq!(p.display_ty(&field(0).ty), "Positive");
    assert_eq!(down(field(1)), to_int);

    let Stmt::Assign { value, .. } = &body(p, "t.wa.assign").stmts[1] else {
        panic!("an assignment")
    };
    assert_eq!(down(value), to_int);

    let ExprKind::Binary { lhs, .. } = &tail(body(p, "t.wa.operand")).kind else {
        panic!("+")
    };
    assert_eq!(down(lhs), to_int);

    assert_eq!(down(tail(body(p, "t.wa.middle"))), ["t.wa.Big"]);
    assert_eq!(down(tail(body(p, "t.wa.same_class"))), ["t.wa.Big"]);
    assert!(matches!(
        tail(body(p, "t.wa.identical")).kind,
        ExprKind::Local(_)
    ));
}

/// 3.5 and T39: a constant proved against a refinement is one `Refine`
/// node around the whole constant (`2 * 3 - 1`; parentheses leave no node),
/// naming the refinement written at the use site (S55: `P2` and `Positive`
/// are one type, named as written; a chain is one node); the literal forms
/// prove their parts (S19) and an `if` is proved branch by branch, never as
/// a whole.
#[test]
fn constants_are_proved_at_the_use_site_refinement() {
    let src = with_refs(
        "t.rn",
        r#"
fn a() -> Int {
    takes_positive(5) + takes_p2(6) + takes_big(150)
}

fn b() -> Positive {
    2 * 3 - 1
}

fn c() -> Positive {
    (4)
}

fn d(flag: Bool) -> Positive {
    if flag { 1 } else { 2 }
}

fn e() -> List<Positive> {
    [1, 2]
}

fn f() -> Option<Positive> {
    Some(3)
}

fn g() -> Dims {
    Dims { width: 7, height: 0 }
}

fn h() -> Positive {
    var p: Positive = 1
    p = 2
    p
}

fn i() -> Small {
    5
}
"#,
    );
    let result = check_source(&src);
    expect(&result, &[]);
    let p = &result.program;
    let r = |path: &str| refines(p, body(p, path));
    assert_eq!(
        r("t.rn.a"),
        pairs(&[
            ("t.rn.Positive", "Positive"),
            ("t.rn.P2", "P2"),
            ("t.rn.Big", "Big")
        ])
    );
    let ExprKind::Refine { expr, .. } = &tail(body(p, "t.rn.b")).kind else {
        panic!("a Refine around the whole constant")
    };
    assert!(matches!(expr.kind, ExprKind::Binary { .. }));
    let ExprKind::Refine { expr, .. } = &tail(body(p, "t.rn.c")).kind else {
        panic!("a Refine")
    };
    assert!(matches!(expr.kind, ExprKind::Lit(_)));
    let ExprKind::If {
        then_block,
        else_block,
        ..
    } = &tail(body(p, "t.rn.d")).kind
    else {
        panic!("an if, with no Refine around it")
    };
    assert!(matches!(tail(then_block).kind, ExprKind::Refine { .. }));
    let els = else_block.as_ref().expect("an else block");
    assert!(matches!(tail(els).kind, ExprKind::Refine { .. }));
    let ExprKind::List(items) = &tail(body(p, "t.rn.e")).kind else {
        panic!("a list, with no Refine around it")
    };
    assert!(items
        .iter()
        .all(|x| matches!(x.kind, ExprKind::Refine { .. })));
    let ExprKind::Ctor { args, .. } = &tail(body(p, "t.rn.f")).kind else {
        panic!("Some(3)")
    };
    assert!(matches!(args[0].kind, ExprKind::Refine { .. }));
    assert_eq!(r("t.rn.g"), pairs(&[("t.rn.Positive", "Positive")]));
    assert_eq!(
        r("t.rn.h"),
        pairs(&[("t.rn.Positive", "Positive"), ("t.rn.Positive", "Positive")])
    );
    assert!(matches!(tail(body(p, "t.rn.h")).kind, ExprKind::Local(_)));
    assert_eq!(r("t.rn.i"), pairs(&[("t.rn.Small", "Small")]));
}

/// S15 and 3.4 rule 4: type constructors are invariant; the mismatch
/// carries `expected` and `actual` as written and, exactly when the erased
/// types agree, the note that refinements convert only at the top level.
#[test]
fn refinements_convert_only_at_the_top_level() {
    let src = with_refs(
        "t.tl",
        r#"
fn a(ps: List<Positive>) -> List<Int> {
    ps // #a
}

fn b(ns: List<Int>) -> List<Positive> {
    ns // #b
}

fn c(r: Result<Positive, Text>) -> Result<Int, Text> {
    r // #c
}

fn d(xs: List<Int>) -> List<Text> {
    xs // #d
}

fn e(id: PageId) -> Text {
    id // #e
}
"#,
    );
    let result = check_source(&src);
    let m = |n: &str| mark(&src, n);
    expect(
        &result,
        &[
            (m("a"), "type_mismatch"),
            (m("b"), "type_mismatch"),
            (m("c"), "type_mismatch"),
            (m("d"), "type_mismatch"),
            (m("e"), "type_mismatch"),
        ],
    );
    let check = |n: &str, expected: &str, actual: &str, note: bool| {
        let d = diag_at(&result, "type_mismatch", m(n));
        assert_eq!(ea(&d), (json!(expected), json!(actual)), "#{n}");
        let has = notes(&d).iter().any(|x| x.contains("top level"));
        assert_eq!(has, note, "#{n}: {:?}", notes(&d));
    };
    check("a", "List<Int>", "List<Positive>", true);
    check("b", "List<Positive>", "List<Int>", true);
    check("c", "Result<Int, Text>", "Result<Positive, Text>", true);
    check("d", "List<Text>", "List<Int>", false);
    check("e", "Text", "PageId", false);
}

/// S20: without an expected type the first list element and the first
/// non-diverging branch or arm set the form's type and later parts check
/// against it (`[size, 3]` is `List<Positive>` with the 3 proved,
/// `[3, size]` is `List<Int>` with the size widened).
#[test]
fn the_first_part_sets_the_type_of_a_form() {
    let src = with_refs(
        "t.ff",
        r#"
fn a(size: Positive) -> List<Positive> {
    let xs = [size, 3]
    xs
}

fn b(size: Positive) -> List<Positive> {
    let xs = [3, size]
    xs // #b
}

fn c(size: Positive, flag: Bool) -> Positive {
    let x = if flag { size } else { 1 }
    x
}

fn d(size: Positive, flag: Bool) -> Positive {
    let y = if flag { 1 } else { size }
    y // #d
}

fn e(o: Option<Positive>) -> Positive {
    let z = match o {
        Some(v) => v
        None => 1
    }
    z
}

fn f(size: Positive, n: Int, flag: Bool) -> Int {
    let w = if flag { size } else { n } // #f
    w
}
"#,
    );
    let result = check_source(&src);
    let m = |n: &str| mark(&src, n);
    expect(
        &result,
        &[
            (m("b"), "type_mismatch"),
            (m("d"), "cannot_prove_refinement"),
            (m("f"), "cannot_prove_refinement"),
        ],
    );
    let p = &result.program;
    assert_eq!(local(p, "t.ff.a", "xs"), "List<Positive>");
    assert_eq!(local(p, "t.ff.b", "xs"), "List<Int>");
    assert_eq!(local(p, "t.ff.c", "x"), "Positive");
    assert_eq!(local(p, "t.ff.d", "y"), "Int");
    assert_eq!(local(p, "t.ff.e", "z"), "Positive");
    assert_eq!(local(p, "t.ff.f", "w"), "Positive");
    let Stmt::Let { value, .. } = &body(p, "t.ff.a").stmts[0] else {
        panic!("a let")
    };
    let ExprKind::List(items) = &value.kind else {
        panic!("a list")
    };
    assert!(matches!(items[0].kind, ExprKind::Local(_)));
    assert!(matches!(items[1].kind, ExprKind::Refine { .. }));
    let Stmt::Let { value, .. } = &body(p, "t.ff.b").stmts[0] else {
        panic!("a let")
    };
    let ExprKind::List(items) = &value.kind else {
        panic!("a list")
    };
    assert_eq!(widens(p, &items[1]).0, ["t.ff.Positive"]);
    let b = diag_at(&result, "type_mismatch", m("b"));
    assert_eq!(ea(&b), (json!("List<Positive>"), json!("List<Int>")));
    let f = diag_at(&result, "cannot_prove_refinement", m("f"));
    assert_eq!(start(&f), pos(&src, "n } // #f"));
    assert_eq!(ea(&f), (json!("Positive"), json!("Int")));
}

/// 3.6, S17: the right operand of `==`/`!=` is checked against the erased
/// left type, so a refined operand is neither proved nor rejected
/// (`p == 0`, `ps == [0, -1]`, `o == Some(0)` are accepted); a left
/// operand with unresolved variables is unified with the inferred right one
/// (`[] == xs`, `None == o`).
#[test]
fn equality_compares_erased_types() {
    let src = with_refs(
        "t.eq",
        r#"
fn a(p: Positive) -> Bool {
    p == 0
}

fn b(ps: List<Positive>) -> Bool {
    ps == [0, -1]
}

fn c(p: Positive, n: Int) -> Bool {
    n != p
}

fn d(ps: List<Positive>, ns: List<Int>) -> Bool {
    ns == ps
}

fn e(o: Option<Positive>) -> Bool {
    o == Some(0)
}

fn f(xs: List<Int>) -> Bool {
    [] == xs
}

fn g(o: Option<Text>) -> Bool {
    None == o
}

fn h(b: Big, s: Small) -> Bool {
    b == s
}
"#,
    );
    let result = check_source(&src);
    expect(&result, &[]);
    let p = &result.program;
    let ExprKind::Binary { rhs, .. } = &tail(body(p, "t.eq.a")).kind else {
        panic!("==")
    };
    assert!(
        !matches!(rhs.kind, ExprKind::Refine { .. }),
        "`==` proves nothing: {rhs:?}"
    );
}

/// `x = e` (3.11): `e ⇐ type(x)`, where an unannotated `var` has its
/// initializer's refined type (3.7).
#[test]
fn assignments_check_against_the_binding_type() {
    let src = with_refs(
        "t.asg",
        r#"
fn a(size: Positive, n: Int) -> Positive {
    var p = size
    p = 3
    p = 0 // #zero
    p = n // #name
    p
}
"#,
    );
    let result = check_source(&src);
    expect(
        &result,
        &[
            (mark(&src, "zero"), "refinement_false"),
            (mark(&src, "name"), "cannot_prove_refinement"),
        ],
    );
    assert_eq!(local(&result.program, "t.asg.a", "p"), "Positive");
}

// ---- 3.5: the proof rule (S16, S40, S55, S65, check<R>) ------------------------------

/// S16 step 2: the predicates of chain(E) are evaluated from strip(E)
/// upward; `refinement_false` names E as `expected`, the failing predicate
/// as `clause`, and the value as value JSON (S40).
#[test]
fn chain_predicates_are_evaluated_from_the_root_base() {
    let src = with_refs(
        "t.co",
        r#"
fn a() -> Small {
    0 // #a
}

fn b() -> Small {
    50 // #b
}

fn c() -> Big {
    -7 // #c
}
"#,
    );
    let result = check_source(&src);
    let m = |n: &str| mark(&src, n);
    expect(
        &result,
        &[
            (m("a"), "refinement_false"),
            (m("b"), "refinement_false"),
            (m("c"), "refinement_false"),
        ],
    );
    let fields = |n: &str| {
        let d = diag_at(&result, "refinement_false", m(n));
        (
            d["expected"].clone(),
            d["actual"].clone(),
            d["clause"].clone(),
        )
    };
    assert_eq!(fields("a"), (json!("Small"), json!(0), json!("self > 0")));
    assert_eq!(fields("b"), (json!("Small"), json!(50), json!("self < 10")));
    assert_eq!(fields("c"), (json!("Big"), json!(-7), json!("self > 0")));
}

/// S40 value JSON in `refinement_false`'s `actual`: a wrapper, the prelude's
/// and a user enum's variants with and without payload, `Unit`, a list of
/// structs, a text with escapes, and a `Bool`. The predicates'
/// constructors resolve against the erased left operand (3.6, S10 step 1).
#[test]
fn refinement_false_reports_value_json() {
    let src = r#"module t.vj

type PageId(Text)
type Nonblank = PageId where self != PageId("")
type NotNone = Option<Int> where self != None
type NotZero = Option<Int> where self != Some(0)
type Warm = Color where self == Red
type NoUnit = Unit where false
type Few = List<Dims> where self.len < 2
type Short = Text where self.len < 3
type Yes = Bool where self

enum Color {
    Red
    Green
}

struct Dims {
    w: Int
    h: Int
}

fn a() -> Nonblank {
    PageId("") // #a
}

fn b() -> NotNone {
    None // #b
}

fn c() -> NotZero {
    Some(0) // #c
}

fn d() -> Warm {
    Green // #d
}

fn e() -> NoUnit {
    Unit // #e
}

fn f() -> Few {
    [Dims { w: 1, h: 2 }, Dims { w: 3, h: 4 }] // #f
}

fn g() -> Short {
    "a\"b\\c" // #g
}

fn h() -> Yes {
    false // #h
}

fn i() -> Nonblank {
    PageId("x")
}

fn j() -> Warm {
    Red
}

fn k() -> NotZero {
    Some(1)
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let want: Vec<(u32, &str)> = names.iter().map(|n| (m(n), "refinement_false")).collect();
    expect(&result, &want);
    let actual = |n: &str| diag_at(&result, "refinement_false", m(n))["actual"].clone();
    let clause = |n: &str| diag_at(&result, "refinement_false", m(n))["clause"].clone();
    assert_eq!(
        actual("a"),
        json!({ "$type": "t.vj.PageId", "$payload": [""] })
    );
    assert_eq!(clause("a"), json!("self != PageId(\"\")"));
    assert_eq!(
        actual("b"),
        json!({ "$type": "Option", "$variant": "None" })
    );
    assert_eq!(clause("b"), json!("self != None"));
    assert_eq!(
        actual("c"),
        json!({ "$type": "Option", "$variant": "Some", "$payload": [0] })
    );
    assert_eq!(
        actual("d"),
        json!({ "$type": "t.vj.Color", "$variant": "Green" })
    );
    assert_eq!(actual("e"), json!({ "$type": "Unit" }));
    assert_eq!(clause("e"), json!("false"));
    assert_eq!(
        actual("f"),
        json!([
            { "$type": "t.vj.Dims", "w": 1, "h": 2 },
            { "$type": "t.vj.Dims", "w": 3, "h": 4 }
        ])
    );
    assert_eq!(clause("f"), json!("self.len < 2"));
    assert_eq!(actual("g"), json!("a\"b\\c"));
    assert_eq!(actual("h"), json!(false));
    assert_eq!(clause("h"), json!("self"));
    let p = &result.program;
    for f in ["t.vj.i", "t.vj.j", "t.vj.k"] {
        assert!(
            matches!(tail(body(p, f)).kind, ExprKind::Refine { .. }),
            "{f}"
        );
    }
}

/// S16 step 2: a predicate that reads a field or calls a function is
/// outside the compile-time evaluable subset, so even a constant is
/// `cannot_prove_refinement`, with a note pointing at `check`; rule 2'
/// needs no proof; a refinement of a struct exposes the struct's fields
/// through a widened receiver (3.9, T19).
#[test]
fn predicates_outside_the_evaluable_subset() {
    let src = r#"module t.ev

struct Area {
    width: Int
    height: Int
}

type Wide = Area where self.width > 10
type Odd = Int where is_odd(self)
type OddBig = Odd where self > 100

fn is_odd(n: Int) -> Bool {
    n % 2 == 1
}

fn a() -> Wide {
    Area { width: 20, height: 1 } // #a
}

fn b() -> OddBig {
    101 // #b
}

fn c(x: OddBig) -> Odd {
    x
}

fn d(w: Wide) -> Int {
    w.width
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "cannot_prove_refinement"),
            (mark(src, "b"), "cannot_prove_refinement"),
        ],
    );
    let a = diag_at(&result, "cannot_prove_refinement", mark(src, "a"));
    assert_eq!(ea(&a), (json!("Wide"), json!("Area")));
    assert!(
        notes(&a).iter().any(|n| n.contains("check<Wide>")),
        "{:?}",
        notes(&a)
    );
    let b = diag_at(&result, "cannot_prove_refinement", mark(src, "b"));
    assert_eq!(ea(&b), (json!("OddBig"), json!("Int")));
    assert!(
        notes(&b)
            .iter()
            .any(|n| n.contains("is_odd") && n.contains("check<")),
        "{:?}",
        notes(&b)
    );
    let p = &result.program;
    assert_eq!(widens(p, tail(body(p, "t.ev.c"))).0, ["t.ev.OddBig"]);
    let ExprKind::Field { base, index, .. } = &tail(body(p, "t.ev.d")).kind else {
        panic!("a field read")
    };
    assert_eq!(*index, 0);
    assert_eq!(widens(p, base).0, ["t.ev.Wide"]);
}

/// S65 and S55: refinements of one root base whose resolved predicates
/// differ are distinct types; passing one for the other is
/// `cannot_prove_refinement`, never `type_mismatch`, rendered with
/// qualified names when the bare names coincide and with a note quoting
/// both predicates; a constant is still proved against either.
#[test]
fn distinct_refinements_of_one_base() {
    let a = r#"module t.a

pub type Valid = Text where ok(self)

pub fn ok(t: Text) -> Bool {
    t.len > 0
}
"#;
    let b = r#"module t.b

pub type Valid = Text where ok(self)

type Pos = Int where self >= 1
type Positive = Int where self > 0
type Small = Positive where self < 10

pub fn ok(t: Text) -> Bool {
    t.len > 1
}

fn convert(v: t.a.Valid) -> Valid {
    v // #convert
}

fn other(p: Positive) -> Pos {
    p // #other
}

fn chained(s: Small) -> Pos {
    s // #chained
}

fn constant() -> Pos {
    1
}
"#;
    let result = check_files(&[("src/t/a.cpl", a), ("src/t/b.cpl", b)]);
    expect(
        &result,
        &[
            (mark(b, "convert"), "cannot_prove_refinement"),
            (mark(b, "other"), "cannot_prove_refinement"),
            (mark(b, "chained"), "cannot_prove_refinement"),
        ],
    );
    let convert = diag_at(&result, "cannot_prove_refinement", mark(b, "convert"));
    assert_eq!(ea(&convert), (json!("t.b.Valid"), json!("t.a.Valid")));
    let other = diag_at(&result, "cannot_prove_refinement", mark(b, "other"));
    assert_eq!(ea(&other), (json!("Pos"), json!("Positive")));
    let note = notes(&other).join(" | ");
    assert!(
        note.contains("self >= 1") && note.contains("self > 0") && note.contains("check<Pos>(p)"),
        "{note}"
    );
    let chained = diag_at(&result, "cannot_prove_refinement", mark(b, "chained"));
    assert_eq!(ea(&chained), (json!("Pos"), json!("Small")));
}

/// S55: identity is the resolved predicate with grouping parentheses
/// removed and literals compared by value, so each pair is one type and
/// rule 1 inserts no node.
#[test]
fn refinement_identity_is_structural() {
    let src = r#"module t.id

type A = Text where self != "\u{41}"
type B = Text where self != "A"
type C = Int where self > 0
type D = Int where (self > (0))

fn ab(a: A) -> B {
    a
}

fn cd(c: C) -> D {
    c
}

fn dc(d: D) -> C {
    d
}
"#;
    let result = check_source(src);
    expect(&result, &[]);
    let p = &result.program;
    for f in ["t.id.ab", "t.id.cd", "t.id.dc"] {
        assert!(
            matches!(tail(body(p, f)).kind, ExprKind::Local(_)),
            "{f}: {:?}",
            tail(body(p, f)).kind
        );
    }
}

/// 3.7 `check<R>`: the parameter type is strip(R), so a value of any
/// refinement of the root base is accepted through `Widen` nodes and the
/// result is `Result<R, ConstraintError>`; a wrong argument type is
/// `type_mismatch` against the root base, a non-refinement type argument
/// `check_requires_refinement` with `type`, and a wrong count
/// `type_argument_count_mismatch` with the counts.
#[test]
fn check_takes_the_root_base() {
    let src = with_refs(
        "t.ck",
        r#"
fn a(b: Big) -> Result<Small, ConstraintError> {
    check<Small>(b)
}

fn b(p: P2) -> Result<Big, ConstraintError> {
    check<Big>(p)
}

fn c(t: Text) -> Result<Positive, ConstraintError> {
    check<Positive>(t) // #c
}

fn d(n: Int) -> Result<Positive, ConstraintError> {
    check<Positive, Int>(n) // #d
}

fn e(id: PageId) -> Result<PageId, ConstraintError> {
    check<PageId>(id) // #e
}

fn f<T>(x: T) -> Result<T, ConstraintError> {
    check<T>(x) // #f
}
"#,
    );
    let result = check_source(&src);
    let m = |n: &str| mark(&src, n);
    expect(
        &result,
        &[
            (m("c"), "type_mismatch"),
            (m("d"), "type_argument_count_mismatch"),
            (m("e"), "check_requires_refinement"),
            (m("f"), "check_requires_refinement"),
        ],
    );
    let c = diag_at(&result, "type_mismatch", m("c"));
    assert_eq!(ea(&c), (json!("Int"), json!("Text")));
    assert_eq!(start(&c), pos(&src, "t) // #c"));
    let d = diag_at(&result, "type_argument_count_mismatch", m("d"));
    assert_eq!(ea(&d), (json!(1), json!(2)));
    assert_eq!(
        diag_at(&result, "check_requires_refinement", m("e"))["type"],
        "PageId"
    );
    assert_eq!(
        diag_at(&result, "check_requires_refinement", m("f"))["type"],
        "T"
    );
    let p = &result.program;
    let t = tail(body(p, "t.ck.a"));
    let ExprKind::Check { refined, arg } = &t.kind else {
        panic!("a Check")
    };
    assert_eq!(p.display_path(*refined), "t.ck.Small");
    assert_eq!(widens(p, arg).0, ["t.ck.Big", "t.ck.Positive"]);
    assert_eq!(p.display_ty(&t.ty), "Result<Small, ConstraintError>");
    let ExprKind::Check { arg, .. } = &tail(body(p, "t.ck.b")).kind else {
        panic!("a Check")
    };
    assert_eq!(widens(p, arg).0, ["t.ck.P2"]);
}

/// S19 with 3.5: the literal forms prove their parts, so the failure is
/// reported at the failing part (S63 (1)), per branch for an `if` or a
/// `match`.
#[test]
fn literal_forms_report_the_failing_part() {
    let src = with_refs(
        "t.lf",
        r#"
fn a() -> List<Positive> {
    [1, 0, 2] // #a
}

fn b() -> Dims {
    Dims { width: 0, height: 0 } // #b
}

fn c(n: Int) -> Option<Positive> {
    Some(n) // #c
}

fn d(flag: Bool) -> Positive {
    if flag { 1 } else { 0 } // #d
}

fn e(o: Option<Int>) -> Positive {
    match o {
        Some(_) => 3
        None => -3 // #e
    }
}
"#,
    );
    let result = check_source(&src);
    let m = |n: &str| mark(&src, n);
    expect(
        &result,
        &[
            (m("a"), "refinement_false"),
            (m("b"), "refinement_false"),
            (m("c"), "cannot_prove_refinement"),
            (m("d"), "refinement_false"),
            (m("e"), "refinement_false"),
        ],
    );
    let at = |code: &str, n: &str| start(&diag_at(&result, code, m(n)));
    assert_eq!(at("refinement_false", "a"), pos(&src, "0, 2] // #a"));
    assert_eq!(
        at("refinement_false", "b"),
        pos(&src, "0, height: 0 } // #b")
    );
    assert_eq!(at("cannot_prove_refinement", "c"), pos(&src, "n) // #c"));
    assert_eq!(at("refinement_false", "d"), pos(&src, "0 } // #d"));
    assert_eq!(
        diag_at(&result, "refinement_false", m("e"))["actual"],
        json!(-3)
    );
    let c = diag_at(&result, "cannot_prove_refinement", m("c"));
    assert_eq!(ea(&c), (json!("Positive"), json!("Int")));
}

/// S19 (calls take their types from their arguments) with 3.5: a call is
/// never a constant expression, so `id(5)` at `Positive` instantiates
/// `T := Int` and is `cannot_prove_refinement`, while `id(size)`
/// instantiates `T := Positive`.
#[test]
fn calls_are_not_constant_expressions() {
    let src = with_refs(
        "t.gc",
        r#"
fn id<T>(x: T) -> T {
    x
}

fn a() -> Positive {
    id(5) // #a
}

fn b(size: Positive) -> Positive {
    id(size)
}
"#,
    );
    let result = check_source(&src);
    expect(&result, &[(mark(&src, "a"), "cannot_prove_refinement")]);
    let a = diag_at(&result, "cannot_prove_refinement", mark(&src, "a"));
    assert_eq!(ea(&a), (json!("Positive"), json!("Int")));
    let p = &result.program;
    let ExprKind::Call { generic_args, .. } = &tail(body(p, "t.gc.b")).kind else {
        panic!("a call")
    };
    let shown: Vec<String> = generic_args.iter().map(|t| p.display_ty(t)).collect();
    assert_eq!(shown, ["Positive"]);
}

// ---- 3.7: generics and inference -----------------------------------------------------------

/// 3.7 steps 2-4 and S19: arguments are checked left to right against the
/// instantiated parameters and the expected type applies to the result only
/// afterwards, so `pick(1, size)` is `Int`, `pick(1, "a")` rejects the
/// second argument, and `pick(None, Some(2))` at `Option<Positive>` has
/// already instantiated `T := Option<Int>` (invariance, S15).
#[test]
fn generic_arguments_are_inferred_left_to_right() {
    let src = with_refs(
        "t.lr",
        r#"
fn pick<T>(a: T, b: T) -> T {
    a
}

fn a(size: Positive) -> Positive {
    pick(size, 1)
}

fn b(size: Positive) -> Positive {
    pick(1, size) // #b
}

fn c() -> Int {
    pick(1, "a") // #c
}

fn d(size: Positive) -> List<Positive> {
    pick([], [size])
}

fn e() -> Option<Positive> {
    pick(None, None)
}

fn f() -> Option<Positive> {
    pick(None, Some(2)) // #f
}
"#,
    );
    let result = check_source(&src);
    let m = |n: &str| mark(&src, n);
    expect(
        &result,
        &[
            (m("b"), "cannot_prove_refinement"),
            (m("c"), "type_mismatch"),
            (m("f"), "type_mismatch"),
        ],
    );
    let b = diag_at(&result, "cannot_prove_refinement", m("b"));
    assert_eq!(ea(&b), (json!("Positive"), json!("Int")));
    let c = diag_at(&result, "type_mismatch", m("c"));
    assert_eq!(ea(&c), (json!("Int"), json!("Text")));
    assert_eq!(start(&c), pos(&src, "\"a\") // #c"));
    let f = diag_at(&result, "type_mismatch", m("f"));
    assert_eq!(ea(&f), (json!("Option<Positive>"), json!("Option<Int>")));
    let p = &result.program;
    let ExprKind::Call {
        generic_args, args, ..
    } = &tail(body(p, "t.lr.a")).kind
    else {
        panic!("a call")
    };
    let shown: Vec<String> = generic_args.iter().map(|t| p.display_ty(t)).collect();
    assert_eq!(shown, ["Positive"]);
    assert!(matches!(args[1].kind, ExprKind::Refine { .. }));
}

/// 3.7: a variable still unbound when its statement ends is
/// `cannot_infer_type` at the expression that created it (S63 (1)): the
/// generic call, the nullary constructor, or the inner empty list, with
/// `type` rendering the unknown parts as `?` and the note "add a type
/// annotation"; an expected type or explicit arguments resolve it.
#[test]
fn unresolved_variables_are_reported_where_they_were_created() {
    let src = r#"module t.ci

fn empty<T>() -> List<T> {
    []
}

fn none<A, B>(a: A) -> Option<B> {
    None
}

fn a() -> Int {
    let _x = empty() // #a
    1
}

fn b() -> Int {
    let _y = none(1) // #b
    1
}

fn c() -> Int {
    let _z = Some(None) // #c
    1
}

fn d() -> Int {
    let _w = flatten([]) // #d
    1
}

fn e() -> Int {
    let _v = [[]] // #e
    1
}

fn f() -> List<Text> {
    empty()
}

fn g() -> Int {
    empty<Int>().len
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "cannot_infer_type"),
            (m("b"), "cannot_infer_type"),
            (m("c"), "cannot_infer_type"),
            (m("d"), "cannot_infer_type"),
            (m("e"), "cannot_infer_type"),
        ],
    );
    let check = |n: &str, at: &str, ty: &str| {
        let d = diag_at(&result, "cannot_infer_type", m(n));
        assert_eq!(start(&d), pos(src, at), "#{n}");
        assert_eq!(d["type"], json!(ty), "#{n}");
        assert!(
            notes(&d)
                .iter()
                .any(|x| x.contains("add a type annotation")),
            "#{n}: {:?}",
            notes(&d)
        );
    };
    check("a", "empty() // #a", "List<?>");
    check("b", "none(1) // #b", "Option<?>");
    check("c", "None) // #c", "Option<?>");
    check("d", "flatten([]) // #d", "List<?>");
    check("e", "[]] // #e", "List<?>");
    let p = &result.program;
    let ExprKind::Call { generic_args, .. } = &tail(body(p, "t.ci.f")).kind else {
        panic!("a call")
    };
    assert_eq!(generic_args, &[Ty::Text]);
}

/// 3.7 step 1 and 3.8: explicit type arguments must match the parameter
/// count of the function, the constructor's enum, or the wrapper (which has
/// none), reported with `expected`/`actual` counts. S62: the call they
/// belong to has a diagnostic, so its unknowns are not also
/// `cannot_infer_type`.
#[test]
fn explicit_type_argument_counts() {
    let src = r#"module t.ta

use std.text.{trim}

type Positive = Int where self > 0
type PageId(Text)

fn empty<T>() -> List<T> {
    []
}

fn a() -> Int {
    let _x = empty<Int, Int>() // #a
    1
}

fn b(t: Text) -> Text {
    trim<Int>(t) // #b
}

fn c() -> Option<Int> {
    Some<Int, Int>(1) // #c
}

fn d() -> PageId {
    PageId<Text>("x") // #d
}

fn e() -> List<Positive> {
    empty<Positive>()
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "type_argument_count_mismatch"),
            (m("b"), "type_argument_count_mismatch"),
            (m("c"), "type_argument_count_mismatch"),
            (m("d"), "type_argument_count_mismatch"),
        ],
    );
    let counts = |n: &str| ea(&diag_at(&result, "type_argument_count_mismatch", m(n)));
    assert_eq!(counts("a"), (json!(1), json!(2)));
    assert_eq!(counts("b"), (json!(0), json!(1)));
    assert_eq!(counts("c"), (json!(1), json!(2)));
    assert_eq!(counts("d"), (json!(0), json!(1)));
}

/// 3.3 and S13: a type parameter is identical only to itself and opaque
/// inside its declaration; nothing widens into or out of it.
#[test]
fn type_parameters_are_rigid() {
    let src = r#"module t.tp

type Positive = Int where self > 0

fn a<T>(x: T) -> Int {
    x // #a
}

fn b<T, U>(x: T, y: U) -> U {
    x // #b
}

fn c<T>(x: T, p: Positive) -> T {
    p // #c
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "type_mismatch"),
            (m("b"), "type_mismatch"),
            (m("c"), "type_mismatch"),
        ],
    );
    let pair = |n: &str| ea(&diag_at(&result, "type_mismatch", m(n)));
    assert_eq!(pair("a"), (json!("Int"), json!("T")));
    assert_eq!(pair("b"), (json!("U"), json!("T")));
    assert_eq!(pair("c"), (json!("T"), json!("Positive")));
}

/// S12 and S19: a generic struct literal unifies with the expected type
/// first and proves its fields against it; without one it infers from the
/// fields (and is then invariant, S15); the spread is checked first and
/// fixes the arguments for the fields after it.
#[test]
fn generic_struct_literals() {
    let src = r#"module t.gs

type Positive = Int where self > 0

struct Pair<A, B> {
    left: A
    right: B
}

fn a() -> Pair<Positive, Text> {
    Pair { left: 1, right: "a" }
}

fn b() -> Pair<Positive, Text> {
    let p = Pair { left: 1, right: "a" }
    p // #b
}

fn c(p: Pair<Int, Text>) -> Int {
    let _q = Pair { ..p, left: "x" } // #c
    1
}

fn d(p: Pair<Int, Text>) -> Pair<Int, Text> {
    Pair { ..p, right: "y" }
}

fn e() -> Int {
    let _r = Pair { left: [], right: 1 } // #e
    1
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("b"), "type_mismatch"),
            (m("c"), "type_mismatch"),
            (m("e"), "cannot_infer_type"),
        ],
    );
    let b = diag_at(&result, "type_mismatch", m("b"));
    assert_eq!(
        ea(&b),
        (json!("Pair<Positive, Text>"), json!("Pair<Int, Text>"))
    );
    let c = diag_at(&result, "type_mismatch", m("c"));
    assert_eq!(ea(&c), (json!("Int"), json!("Text")));
    assert_eq!(start(&c), pos(src, "\"x\" } // #c"));
    let e = diag_at(&result, "cannot_infer_type", m("e"));
    assert_eq!(start(&e), pos(src, "[], right: 1 } // #e"));
    assert_eq!(e["type"], json!("List<?>"));
    let p = &result.program;
    assert_eq!(local(p, "t.gs.b", "p"), "Pair<Int, Text>");
    assert_eq!(refines(p, body(p, "t.gs.a")).len(), 1);
}

/// 3.7: an unannotated binding takes its initializer's type, refinements
/// included (`let s = size` is `Positive`, `var rest = items` is
/// `List<Positive>`), and a variable binds to a refinement (`Some(size)`
/// is `Option<Positive>`, which is then not an `Option<Int>`).
#[test]
fn bindings_keep_refined_types() {
    let src = r#"module t.lk

type Positive = Int where self > 0

fn takes_positive(p: Positive) -> Int {
    p
}

fn a(size: Positive, items: List<Positive>) -> Int {
    let s = size
    var rest = items
    rest = rest.drop(1)
    let o = Some(size)
    let nested = [items]
    takes_positive(s) + rest.len + nested.len + o.unwrap_or(1)
}

fn b(size: Positive) -> Option<Int> {
    let o = Some(size)
    o // #b
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "b"), "type_mismatch")]);
    let p = &result.program;
    assert_eq!(local(p, "t.lk.a", "s"), "Positive");
    assert_eq!(local(p, "t.lk.a", "rest"), "List<Positive>");
    assert_eq!(local(p, "t.lk.a", "o"), "Option<Positive>");
    assert_eq!(local(p, "t.lk.a", "nested"), "List<List<Positive>>");
}

/// S62: a failed callee, receiver, or call causes no second diagnostic,
/// while an independent mistake inside the arguments is still reported.
#[test]
fn failed_parts_do_not_cascade() {
    let src = r#"module t.cs

fn two(a: Int, b: Int) -> Int {
    a + b
}

fn a() -> Int {
    undefined_fn(1 + "x") // #a
}

fn b(xs: List<Int>) -> Int {
    xs.nope().len // #b
}

fn c() -> Text {
    two(1).len // #c
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "unresolved_name"),
            (m("a"), "type_mismatch"),
            (m("b"), "unknown_member"),
            (m("c"), "arity_mismatch"),
        ],
    );
}

// ---- 3.9: members ------------------------------------------------------------------------

/// 3.9, S22, S23, D43: member misuse carries `name` and the receiver's
/// `type` as written; wrappers, enums, structs, `Int`, and type parameters
/// have no undeclared members.
#[test]
fn member_diagnostics_carry_name_and_type() {
    let src = r#"module t.mb

type PageId(Text)

struct Page {
    id: PageId
    text: Text
}

fn a(xs: List<Int>) -> Int {
    xs.len() // #a
}

fn b(xs: List<Int>) -> Bool {
    xs.is_empty // #b
}

fn c(page: Page) -> Text {
    page.title // #c
}

fn d(id: PageId) -> Int {
    id.len // #d
}

fn e<T>(x: T) -> Int {
    x.len // #e
}

fn f(o: Option<Int>) -> Int {
    o.len // #f
}

fn g(page: Page) -> Int {
    page.len // #g
}

fn h(n: Int) -> Int {
    n.double() // #h
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "property_called"),
            (m("b"), "method_not_called"),
            (m("c"), "unknown_member"),
            (m("d"), "unknown_member"),
            (m("e"), "unknown_member"),
            (m("f"), "unknown_member"),
            (m("g"), "unknown_member"),
            (m("h"), "unknown_member"),
        ],
    );
    let check = |code: &str, n: &str, name: &str, ty: &str| {
        let d = diag_at(&result, code, m(n));
        assert_eq!(
            (d["name"].clone(), d["type"].clone()),
            (json!(name), json!(ty)),
            "#{n}"
        );
    };
    check("property_called", "a", "len", "List<Int>");
    check("method_not_called", "b", "is_empty", "List<Int>");
    check("unknown_member", "c", "title", "Page");
    check("unknown_member", "d", "len", "PageId");
    check("unknown_member", "e", "len", "T");
    check("unknown_member", "f", "len", "Option<Int>");
    check("unknown_member", "g", "len", "Page");
    check("unknown_member", "h", "double", "Int");
}

/// S62: after `property_called` or `method_not_called` the expression has
/// the member's own type, so a mismatch with the expected type is an
/// independent mistake and is reported, and none is reported when the type
/// fits.
#[test]
fn misused_members_keep_their_type() {
    let src = r#"module t.mt

fn a(xs: List<Int>) -> Bool {
    let b: Bool = xs.len() // #a
    b
}

fn b(xs: List<Int>) -> Int {
    let n: Int = xs.len() // #b
    n
}

fn c(xs: List<Int>) -> Int {
    let m: Int = xs.is_empty // #c
    m
}

fn d(xs: List<Int>) -> Bool {
    let e: Bool = xs.is_empty // #d
    e
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "property_called"),
            (m("a"), "type_mismatch"),
            (m("b"), "property_called"),
            (m("c"), "method_not_called"),
            (m("c"), "type_mismatch"),
            (m("d"), "method_not_called"),
        ],
    );
    let a = diag_at(&result, "type_mismatch", m("a"));
    assert_eq!(ea(&a), (json!("Bool"), json!("Int")));
    let c = diag_at(&result, "type_mismatch", m("c"));
    assert_eq!(ea(&c), (json!("Int"), json!("Bool")));
}

/// 3.9 and T19: member lookup uses strip(T): a refinement of a list has the
/// list's members and a refinement of a struct its fields, and the HIR
/// widens the receiver to its root base.
#[test]
fn members_of_refined_receivers() {
    let src = r#"module t.mr

type NonEmpty = List<Int> where self.len > 0

struct Area {
    width: Int
    height: Int
}

type Wide = Area where self.width > 10

fn a(xs: NonEmpty) -> Int {
    xs.len
}

fn b(w: Wide) -> Int {
    w.height
}

fn c(xs: NonEmpty) -> Option<Int> {
    xs.first()
}
"#;
    let result = check_source(src);
    expect(&result, &[]);
    let p = &result.program;
    let ExprKind::Call { callee, args, .. } = &tail(body(p, "t.mr.a")).kind else {
        panic!("a call of the len stub")
    };
    assert_eq!(p.display_path(*callee), "std.prelude.List.len");
    assert_eq!(widens(p, &args[0]).0, ["t.mr.NonEmpty"]);
    let ExprKind::Field { base, index, .. } = &tail(body(p, "t.mr.b")).kind else {
        panic!("a field")
    };
    assert_eq!(*index, 1);
    assert_eq!(widens(p, base).0, ["t.mr.Wide"]);
    let ExprKind::Call { callee, args, .. } = &tail(body(p, "t.mr.c")).kind else {
        panic!("a call of the first stub")
    };
    assert_eq!(p.display_path(*callee), "std.prelude.List.first");
    assert_eq!(widens(p, &args[0]).0, ["t.mr.NonEmpty"]);
}

/// 3.8 method calls with S19: a member stub is instantiated at the
/// receiver's type, so its parameters can be refinements against which
/// literal arguments are proved (`ps.contains(0)` is `refinement_false`);
/// `arity_mismatch` names the stub as `callee` with the counts.
#[test]
fn member_parameters_are_instantiated_at_the_receiver() {
    let src = r#"module t.mp

type Positive = Int where self > 0

fn a(ps: List<Positive>) -> List<Positive> {
    ps.push(5)
}

fn b(ps: List<Positive>) -> List<Positive> {
    ps.push(0) // #b
}

fn c(ps: List<Positive>) -> Bool {
    ps.contains(0) // #c
}

fn d(ps: List<Positive>) -> Positive {
    ps.first().unwrap_or(1)
}

fn e(xs: List<Int>) -> List<Int> {
    xs.take() // #e
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("b"), "refinement_false"),
            (m("c"), "refinement_false"),
            (m("e"), "arity_mismatch"),
        ],
    );
    let e = diag_at(&result, "arity_mismatch", m("e"));
    assert_eq!(e["callee"], json!("std.prelude.List.take"));
    assert_eq!(ea(&e), (json!(1), json!(0)));
    let p = &result.program;
    let ExprKind::Call { args, .. } = &tail(body(p, "t.mp.a")).kind else {
        panic!("push")
    };
    assert!(matches!(args[1].kind, ExprKind::Refine { .. }));
}

// ---- 2.5: constructor resolution (S10) -------------------------------------------------

/// S10 step 1: an unqualified constructor resolves against strip(E) in
/// every check-mode position (S56): a return, an argument, an annotated
/// binding, a struct field, list elements, the payload of an expected-first
/// constructor (S19), branches, an assignment, the right operand of `==`
/// (3.6), and a pattern (against the scrutinee).
#[test]
fn constructors_resolve_by_expected_type() {
    let src = r#"module t.cr

enum A {
    Missing(Int)
    Gone
}

enum B {
    Missing(Int)
    Gone
}

struct Holder {
    a: A
}

fn takes_a(a: A) -> Bool {
    a == Gone
}

fn ret() -> A {
    Missing(1)
}

fn arg() -> Bool {
    takes_a(Missing(1))
}

fn annotated() -> B {
    let b: B = Missing(2)
    b
}

fn returned() -> B {
    return Gone
}

fn field() -> Holder {
    Holder { a: Gone }
}

fn listed() -> List<A> {
    [Missing(1), Gone]
}

fn nested() -> Option<B> {
    Some(Missing(1))
}

fn branches(flag: Bool) -> A {
    if flag { Missing(1) } else { Gone }
}

fn assigned() -> A {
    var x: A = Gone
    x = Missing(3)
    x
}

fn compared(a: A) -> Bool {
    a == Missing(1)
}

fn matched(b: B) -> Int {
    match b {
        Missing(n) => n
        Gone => 0
    }
}
"#;
    let result = check_source(src);
    expect(&result, &[]);
    let p = &result.program;
    let a = def(p, "t.cr.A").id;
    let b = def(p, "t.cr.B").id;
    assert!(matches!(tail(body(p, "t.cr.ret")).kind, ExprKind::Ctor { def, .. } if def == a));
    let Stmt::Return(Some(r)) = &body(p, "t.cr.returned").stmts[0] else {
        panic!("a return")
    };
    assert!(matches!(r.kind, ExprKind::Ctor { def, .. } if def == b));
    let ExprKind::Ctor { args, .. } = &tail(body(p, "t.cr.nested")).kind else {
        panic!("Some(..)")
    };
    assert!(matches!(args[0].kind, ExprKind::Ctor { def, .. } if def == b));
    let ExprKind::Binary { rhs, .. } = &tail(body(p, "t.cr.compared")).kind else {
        panic!("==")
    };
    assert!(matches!(rhs.kind, ExprKind::Ctor { def, .. } if def == a));
    assert!(matches!(
        first_arm_pattern(body(p, "t.cr.matched")),
        PatternKind::Ctor { def, .. } if *def == b
    ));
}

/// S10 step 2: where no expected type is known (an unannotated binding, the
/// left operand of `==`, a list's first element, an argument whose
/// parameter type is a still-unknown `T`), two lexical candidates are
/// `ambiguous_constructor` with `name` and the candidates' qualified names;
/// a qualified constructor skips steps 1-2.
#[test]
fn constructors_without_expected_type_are_ambiguous() {
    let src = r#"module t.am

enum A {
    Missing(Int)
    Gone
}

enum B {
    Missing(Int)
    Gone
}

fn id<T>(x: T) -> T {
    x
}

fn a() -> Int {
    let _x = Missing(1) // #a
    1
}

fn b() -> Int {
    let _g = Gone // #b
    1
}

fn c(a: A) -> Bool {
    Missing(1) == a // #c
}

fn d() -> A {
    id(Missing(1)) // #d
}

fn e() -> Int {
    let _xs = [Gone] // #e
    1
}

fn f() -> B {
    A.Missing(1) // #f
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "ambiguous_constructor"),
            (m("b"), "ambiguous_constructor"),
            (m("c"), "ambiguous_constructor"),
            (m("d"), "ambiguous_constructor"),
            (m("e"), "ambiguous_constructor"),
            (m("f"), "type_mismatch"),
        ],
    );
    let a = diag_at(&result, "ambiguous_constructor", m("a"));
    assert_eq!(a["name"], json!("Missing"));
    assert_eq!(
        sorted(&a["candidates"]),
        ["t.am.A.Missing", "t.am.B.Missing"]
    );
    let b = diag_at(&result, "ambiguous_constructor", m("b"));
    assert_eq!(b["name"], json!("Gone"));
    assert_eq!(sorted(&b["candidates"]), ["t.am.A.Gone", "t.am.B.Gone"]);
    let f = diag_at(&result, "type_mismatch", m("f"));
    assert_eq!(ea(&f), (json!("B"), json!("A")));
}

/// S10: a nominal wrapper and an enum variant of one name are both lexical
/// candidates; step 1 picks the wrapper for an expected wrapper and the
/// variant for an expected enum, in expressions and in patterns.
#[test]
fn wrappers_and_variants_of_one_name() {
    let src = r#"module t.wv

type Tag(Text)

enum Label {
    Tag(Text)
    Plain
}

fn a() -> Tag {
    Tag("x")
}

fn b() -> Label {
    Tag("x")
}

fn c() -> Int {
    let _t = Tag("x") // #c
    1
}

fn d(t: Tag) -> Text {
    match t {
        Tag(s) => s
    }
}

fn e(l: Label) -> Text {
    match l {
        Tag(s) => s
        Plain => ""
    }
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "c"), "ambiguous_constructor")]);
    let c = diag_at(&result, "ambiguous_constructor", mark(src, "c"));
    assert_eq!(sorted(&c["candidates"]), ["t.wv.Label.Tag", "t.wv.Tag"]);
    let p = &result.program;
    let tag = def(p, "t.wv.Tag").id;
    let label = def(p, "t.wv.Label").id;
    assert!(matches!(tail(body(p, "t.wv.a")).kind, ExprKind::Wrap { def, .. } if def == tag));
    assert!(matches!(tail(body(p, "t.wv.b")).kind, ExprKind::Ctor { def, .. } if def == label));
    assert!(matches!(
        first_arm_pattern(body(p, "t.wv.d")),
        PatternKind::Unwrap { def, .. } if *def == tag
    ));
    assert!(matches!(
        first_arm_pattern(body(p, "t.wv.e")),
        PatternKind::Ctor { def, .. } if *def == label
    ));
}

/// S10 step 1 consults strip(E): a refinement of `Option` resolves `None` to
/// the prelude's constructor even beside a user variant named `None`, also
/// inside the refinement's own predicate (the right operand of `!=`, 3.6);
/// without an expected type the two are ambiguous.
#[test]
fn constructor_resolution_strips_refined_expected_types() {
    let src = r#"module t.pn

enum Maybe {
    None
    Just(Int)
}

type SomeInt = Option<Int> where self != None

fn a() -> SomeInt {
    Some(1)
}

fn b() -> SomeInt {
    None // #b
}

fn c() -> Maybe {
    None
}

fn d() -> Int {
    let _n = None // #d
    1
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "b"), "refinement_false"),
            (mark(src, "d"), "ambiguous_constructor"),
        ],
    );
    let b = diag_at(&result, "refinement_false", mark(src, "b"));
    assert_eq!(
        b["actual"],
        json!({ "$type": "Option", "$variant": "None" })
    );
    assert_eq!(b["clause"], json!("self != None"));
    let d = diag_at(&result, "ambiguous_constructor", mark(src, "d"));
    assert_eq!(
        sorted(&d["candidates"]),
        ["std.prelude.Option.None", "t.pn.Maybe.None"]
    );
    let p = &result.program;
    let option = p.std_defs.option;
    let maybe = def(p, "t.pn.Maybe").id;
    let ExprKind::Refine { expr, refined, .. } = &tail(body(p, "t.pn.a")).kind else {
        panic!("a proved Some(1)")
    };
    assert_eq!(p.display_path(*refined), "t.pn.SomeInt");
    assert!(matches!(expr.kind, ExprKind::Ctor { def, .. } if def == option));
    assert!(matches!(tail(body(p, "t.pn.c")).kind, ExprKind::Ctor { def, .. } if def == maybe));
}

/// S10 step 2: the lexical candidates come from the current module and the
/// file's imports only, so a variant of an enum that is neither resolves
/// through an expected type and is `unresolved_name` without one.
#[test]
fn constructors_of_unimported_enums() {
    let lib = r#"module t.lib

pub enum StoreError {
    Missing(Text)
    Unavailable
}

pub fn describe(e: StoreError) -> Int {
    match e {
        Missing(_) => 1
        Unavailable => 2
    }
}
"#;
    let user = r#"module t.user

use t.lib.{describe}

fn a() -> Int {
    describe(Unavailable)
}

fn b() -> Int {
    let _e = Unavailable // #b
    1
}
"#;
    let result = check_files(&[("src/t/lib.cpl", lib), ("src/t/user.cpl", user)]);
    expect(&result, &[(mark(user, "b"), "unresolved_name")]);
    assert_eq!(
        diag_at(&result, "unresolved_name", mark(user, "b"))["name"],
        json!("Unavailable")
    );
}

/// S10 step 3: a refinement, a type parameter, or a struct name applied like
/// a constructor is `not_a_constructor` with `name`.
#[test]
fn types_that_are_not_constructors() {
    let src = r#"module t.nc

type Positive = Int where self > 0

struct Page {
    text: Text
}

fn a() -> Int {
    let _p = Positive(3) // #a
    1
}

fn b<T>(x: T) -> T {
    let _t = T(x) // #b
    x
}

fn c() -> Int {
    let _q = Page("x") // #c
    1
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "not_a_constructor"),
            (m("b"), "not_a_constructor"),
            (m("c"), "not_a_constructor"),
        ],
    );
    assert_eq!(
        diag_at(&result, "not_a_constructor", m("a"))["name"],
        json!("Positive")
    );
    assert_eq!(
        diag_at(&result, "not_a_constructor", m("b"))["name"],
        json!("T")
    );
    assert_eq!(
        diag_at(&result, "not_a_constructor", m("c"))["name"],
        json!("Page")
    );
}

// ---- 3.8: struct literals --------------------------------------------------------------

/// 3.8 struct literals: `missing_field` lists the missing names in
/// declaration order, `unknown_field` and `duplicate_field` name the field,
/// all with the struct as `type`; the duplicate spans the later occurrence
/// with a note at the earlier (S63 (4)); a spread supplies the remaining
/// fields.
#[test]
fn struct_literal_field_rules() {
    let src = r#"module t.sl

struct S {
    a: Int
    b: Int
    c: Int
}

fn missing() -> S {
    S { b: 1 } // #missing
}

fn unknown() -> S {
    S { a: 1, b: 2, c: 3, d: 4 } // #unknown
}

fn duplicate() -> S {
    S { a: 1, b: 2, a: 3, c: 4 } // #duplicate
}

fn spread(s: S) -> S {
    S { ..s, b: 5 }
}

fn spread_unknown(s: S) -> S {
    S { ..s, z: 5 } // #spread_unknown
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("missing"), "missing_field"),
            (m("unknown"), "unknown_field"),
            (m("duplicate"), "duplicate_field"),
            (m("spread_unknown"), "unknown_field"),
        ],
    );
    let missing = diag_at(&result, "missing_field", m("missing"));
    assert_eq!(missing["missing"], json!(["a", "c"]));
    assert_eq!(missing["type"], json!("S"));
    let unknown = diag_at(&result, "unknown_field", m("unknown"));
    assert_eq!(
        (unknown["name"].clone(), unknown["type"].clone()),
        (json!("d"), json!("S"))
    );
    let dup = diag_at(&result, "duplicate_field", m("duplicate"));
    assert_eq!(dup["name"], json!("a"));
    assert_eq!(dup["type"], json!("S"));
    assert_eq!(start(&dup), pos(src, "a: 3, c: 4 } // #duplicate"));
    let first = &dup["notes"][0]["span"];
    let (fl, fc) = pos(src, "a: 1, b: 2, a: 3");
    assert_eq!(
        (first["line"].as_u64(), first["col"].as_u64()),
        (Some(fl), Some(fc)),
        "the note carries the earlier occurrence: {dup}"
    );
    assert_eq!(
        diag_at(&result, "unknown_field", m("spread_unknown"))["name"],
        json!("z")
    );
}

/// 3.8: a struct literal naming a private struct of another module is
/// `private_item` with `symbol` and `qualified_name`.
#[test]
fn private_struct_literals() {
    let a = r#"module t.pa

struct Secret {
    v: Int
}

fn make() -> Secret {
    Secret { v: 1 }
}
"#;
    let b = r#"module t.pb

fn f() -> Int {
    let _s = t.pa.Secret { v: 1 } // #f
    1
}
"#;
    let result = check_files(&[("src/t/pa.cpl", a), ("src/t/pb.cpl", b)]);
    expect(&result, &[(mark(b, "f"), "private_item")]);
    let d = diag_at(&result, "private_item", mark(b, "f"));
    assert_eq!(d["qualified_name"], json!("t.pa.Secret"));
    assert_eq!(d["symbol"], json!("t.pa.Secret"));
}

// ---- 3.6, 3.7: empty lists -------------------------------------------------------------

/// 3.6 and 3.7: an empty list takes its element type from the expected
/// type, from the other operand of `==` on either side, or from `++`'s left
/// operand; a refinement of a list is proved on the whole constant form
/// (`[1] ⇐ NonEmpty`, S19); only when nothing binds it is it
/// `cannot_infer_type`.
#[test]
fn empty_lists_take_their_type_from_context() {
    let src = r#"module t.el

type NonEmpty = List<Int> where self.len > 0

fn a(xs: List<Int>) -> Bool {
    [] == xs
}

fn b(xs: List<Int>) -> Bool {
    xs == []
}

fn c(o: Option<Text>) -> Bool {
    None == o
}

fn d(xs: List<Int>) -> List<Int> {
    xs ++ []
}

fn e() -> List<List<Int>> {
    [[], [1]]
}

fn f() -> NonEmpty {
    [1]
}

fn g() -> NonEmpty {
    [1] ++ []
}

fn h() -> NonEmpty {
    [] // #h
}

fn i() -> Bool {
    [] == [] // #i
}
"#;
    let result = check_source(src);
    let (lh, li) = (mark(src, "h"), mark(src, "i"));
    let all = got(&result);
    assert!(
        all.contains(&(lh, "refinement_false".to_string())),
        "{}",
        render(&result)
    );
    let rest: Vec<&(u32, String)> = all.iter().filter(|(l, _)| *l != lh).collect();
    assert!(
        !rest.is_empty()
            && rest
                .iter()
                .all(|(l, c)| *l == li && c == "cannot_infer_type"),
        "only `[] == []` is cannot_infer_type:\n{}",
        render(&result)
    );
    let h = diag_at(&result, "refinement_false", lh);
    assert_eq!(h["actual"], json!([]));
    assert_eq!(h["clause"], json!("self.len > 0"));
    let p = &result.program;
    let ExprKind::Refine { expr, .. } = &tail(body(p, "t.el.f")).kind else {
        panic!("a proved list")
    };
    assert!(matches!(expr.kind, ExprKind::List(_)));
    let ExprKind::Refine { expr, .. } = &tail(body(p, "t.el.g")).kind else {
        panic!("a proved concatenation")
    };
    assert!(matches!(expr.kind, ExprKind::Binary { .. }));
}

// ---- 3.10, 3.11: if, match, and divergence (S24, S68) --------------------------------

/// S24: a block diverges when it ends in a `return` or in an `if`/`else`
/// all of whose branches diverge; a diverging form satisfies any expected
/// type, also as an operand; without an expected type the first
/// non-diverging branch or arm sets the type (S20).
#[test]
fn diverging_forms_satisfy_any_type() {
    let src = r#"module t.dv

fn a(flag: Bool) -> Int {
    if flag { return 1 } else { return 2 }
}

fn b(flag: Bool) -> Int {
    1 + if flag { return 1 } else { return 2 }
}

fn c(flag: Bool) -> Int {
    let x = if flag { return 0 } else { "s" }
    let _y: Int = x // #c
    0
}

fn d(o: Option<Text>) -> Int {
    let x = match o {
        None => { return 0 }
        Some(t) => t
    }
    let _z: Int = x // #d
    0
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "c"), "type_mismatch"),
            (mark(src, "d"), "type_mismatch"),
        ],
    );
    for n in ["c", "d"] {
        let d = diag_at(&result, "type_mismatch", mark(src, n));
        assert_eq!(ea(&d), (json!("Int"), json!("Text")), "#{n}");
    }
    let p = &result.program;
    assert_eq!(local(p, "t.dv.c", "x"), "Text");
    assert_eq!(local(p, "t.dv.d", "x"), "Text");
}

/// S24: `while` and `assert false` never diverge, so a body ending in one
/// has no tail: `type_mismatch` with the note "missing tail expression",
/// spanning the whole block (S63 (1)); an `else`-less `if` as the tail has
/// type `Unit`, with the note of 3.8, at the `if`.
#[test]
fn non_diverging_endings() {
    let src = r#"module t.nd

fn a() -> Int {
    while true {
        return 1
    }
}

fn b() -> Int {
    assert false
}

fn c(flag: Bool) -> Int {
    if flag { return 1 } // #c
}
"#;
    let result = check_source(src);
    let (la, lb, lc) = (
        line(src, "fn a() -> Int {"),
        line(src, "fn b() -> Int {"),
        mark(src, "c"),
    );
    expect(
        &result,
        &[
            (la, "type_mismatch"),
            (lb, "type_mismatch"),
            (lc, "type_mismatch"),
        ],
    );
    let a = diag_at(&result, "type_mismatch", la);
    assert_eq!(start(&a), pos(src, "{\n    while true"));
    assert_eq!(end(&a), (u64::from(la) + 4, 2));
    assert_eq!(notes(&a), ["missing tail expression"]);
    assert_eq!(ea(&a), (json!("Int"), json!("Unit")));
    let b = diag_at(&result, "type_mismatch", lb);
    assert_eq!(start(&b), pos(src, "{\n    assert false"));
    assert_eq!(end(&b), (u64::from(lb) + 2, 2));
    assert_eq!(notes(&b), ["missing tail expression"]);
    let c = diag_at(&result, "type_mismatch", lc);
    assert_eq!(start(&c), pos(src, "if flag { return 1 } // #c"));
    assert_eq!(ea(&c), (json!("Int"), json!("Unit")));
    assert!(
        notes(&c).iter().any(|n| n.contains("without `else`")),
        "{:?}",
        notes(&c)
    );
}

/// S68: an `if` or `match` statement passes `Unit` into every branch and
/// arm, so each non-`Unit` branch tail is `type_mismatch` there (never
/// `unused_value`); an `else`-less `if` carries exactly one diagnostic in
/// value position too (its `A ⇐ Unit` premise).
#[test]
fn branching_statements_check_their_branches() {
    let src = r#"module t.st

fn a(o: Option<Int>) -> Int {
    match o {
        Some(v) => v // #a1
        None => 0 // #a2
    }
    1
}

fn b(flag: Bool) -> Int {
    if flag { 1 } else { } // #b
    2
}

fn c(flag: Bool) -> Int {
    let _x: Int = if flag {
        1 // #c
    }
    2
}

fn d(flag: Bool) -> Int {
    let _x: Int = if flag { } // #d
    2
}

fn e(o: Option<Int>) -> Int {
    match o {
        Some(_) => {}
        None => {}
    }
    1
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a1"), "type_mismatch"),
            (m("a2"), "type_mismatch"),
            (m("b"), "type_mismatch"),
            (m("c"), "type_mismatch"),
            (m("d"), "type_mismatch"),
        ],
    );
    for n in ["a1", "a2", "b", "c"] {
        let d = diag_at(&result, "type_mismatch", m(n));
        assert_eq!(ea(&d), (json!("Unit"), json!("Int")), "#{n}");
    }
    assert_eq!(
        start(&diag_at(&result, "type_mismatch", m("b"))),
        pos(src, "1 } else { } // #b")
    );
    let d = diag_at(&result, "type_mismatch", m("d"));
    assert_eq!(ea(&d), (json!("Int"), json!("Unit")));
    assert!(
        notes(&d).iter().any(|n| n.contains("without `else`")),
        "{:?}",
        notes(&d)
    );
}

/// S24: the first statement after a diverging one is `unreachable_statement`
/// (warning), once per block; an arm-less `match` on an enum with no
/// variants diverges (Σ = ∅).
#[test]
fn unreachable_statements_once_per_block() {
    let src = r#"module t.ur

enum Never {
}

fn a(n: Int) -> Int {
    return n
    let _a = 1 // #a
    let _b = 2
    n
}

fn b(n: Never) -> Int {
    match n {
    }
    let _c = 1 // #b
    2
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "unreachable_statement"),
            (mark(src, "b"), "unreachable_statement"),
        ],
    );
    let a = diag_at(&result, "unreachable_statement", mark(src, "a"));
    assert_eq!(a["severity"], json!("warning"));
    assert_eq!(start(&a), pos(src, "let _a = 1"));
}

/// 3.11: a bare `return` is allowed exactly where the return type strips to
/// `Unit` (so also for a refinement of `Unit`), else `return_value_missing`
/// with `expected`; a returned value checks against the declared type.
#[test]
fn return_statements() {
    let src = r#"module t.rt

type Done = Unit where true
type Positive = Int where self > 0

fn a() -> Done {
    return
}

fn b(n: Int) -> Int {
    if n > 0 {
        return // #b
    }
    n
}

fn c() -> Unit {
    return 1 // #c
}

fn d() -> Positive {
    return 0 // #d
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("b"), "return_value_missing"),
            (m("c"), "type_mismatch"),
            (m("d"), "refinement_false"),
        ],
    );
    assert_eq!(
        diag_at(&result, "return_value_missing", m("b"))["expected"],
        json!("Int")
    );
    assert_eq!(
        ea(&diag_at(&result, "type_mismatch", m("c"))),
        (json!("Unit"), json!("Int"))
    );
}

/// 3.11 statements in loops and S68: a `while` body is `⇐ Unit`, so its
/// non-`Unit` tail is `type_mismatch`; a non-tail expression statement of
/// another type is `unused_value` with `type` (refinements as written) and
/// the note pointing at `let _ = ...`; a hole statement is `Unit`.
#[test]
fn expression_statements_and_loop_bodies() {
    let src = r#"module t.uv

type Positive = Int where self > 0

fn a(xs: List<Int>) -> Int {
    xs.push(1) // #a
    xs.len
}

fn b(size: Positive) -> Int {
    size // #b
    1
}

fn c(flag: Bool) -> Int {
    var i = 0
    while flag {
        i // #c
    }
    i
}

fn d(n: Int) -> Int {
    ?log // #d
    n
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "unused_value"),
            (m("b"), "unused_value"),
            (m("c"), "type_mismatch"),
            (m("d"), "hole"),
        ],
    );
    let a = diag_at(&result, "unused_value", m("a"));
    assert_eq!(a["type"], json!("List<Int>"));
    assert_eq!(start(&a), pos(src, "xs.push(1) // #a"));
    assert!(
        notes(&a).iter().any(|n| n.contains("let _ =")),
        "{:?}",
        notes(&a)
    );
    assert_eq!(
        diag_at(&result, "unused_value", m("b"))["type"],
        json!("Positive")
    );
    assert_eq!(
        ea(&diag_at(&result, "type_mismatch", m("c"))),
        (json!("Unit"), json!("Int"))
    );
    assert_eq!(
        diag_at(&result, "hole", m("d"))["expected_type"],
        json!("Unit")
    );
}

// ---- 3.12, 3.13: patterns, exhaustiveness, reachability -----------------------------

/// 3.13, S25: `missing` lists the witnesses of the usefulness search in the
/// declaration order of the missing constructors, depth-first, each a
/// constructor applied to `_` per payload (`_` for an infinite type); a
/// match with no arms misses every constructor of Σ(T); Σ is computed on
/// strip(T); `type` is the scrutinee type as written and the diagnostic
/// spans the `match` (S63 (5)).
#[test]
fn exhaustiveness_witnesses() {
    let src = r#"module t.ex

enum Color {
    Red
    Green
    Blue
}

enum Shape {
    Both(Bool, Bool)
}

type PageId(Text)
type Yes = Bool where self

fn a(c: Color) -> Int {
    match c { // #a
        Green => 1
    }
}

fn b(o: Option<Color>) -> Int {
    match o { // #b
        Some(Red) => 1
        None => 0
    }
}

fn c(r: Result<Bool, Bool>) -> Int {
    match r { // #c
        Ok(true) => 1
        Err(false) => 0
    }
}

fn d(s: Shape) -> Int {
    match s { // #d
        Both(true, _) => 1
        Both(false, true) => 2
    }
}

fn e(o: Option<Int>) -> Int {
    match o { // #e
        Some(1) => 1
        None => 0
    }
}

fn f(o: Option<Int>) -> Int {
    match o { // #f
    }
}

fn g(flag: Bool) -> Int {
    match flag { // #g
    }
}

fn h(u: Unit) -> Int {
    match u { // #h
    }
}

fn i(n: Int) -> Int {
    match n { // #i
    }
}

fn j(id: PageId) -> Int {
    match id { // #j
        PageId("a") => 1
    }
}

fn k(y: Yes) -> Int {
    match y { // #k
        true => 1
    }
}

fn l(o: Option<Option<Color>>) -> Int {
    match o { // #l
        Some(Some(Red)) => 1
        Some(None) => 2
        None => 3
    }
}

fn m(t: Text) -> Int {
    match t { // #m
        "a" => 1
        "b" => 2
    }
}

fn n(x: Int) -> Int {
    match x { // #n
        -1 => 0
        0 => 1
    }
}

fn o(r: Result<Option<Bool>, Unit>) -> Int {
    match r { // #o
        Ok(Some(true)) => 1
        Ok(None) => 2
        Err(Unit) => 3
    }
}
"#;
    let cases: &[(&str, &[&str], &str)] = &[
        ("a", &["Red", "Blue"], "Color"),
        ("b", &["Some(Green)", "Some(Blue)"], "Option<Color>"),
        ("c", &["Ok(false)", "Err(true)"], "Result<Bool, Bool>"),
        ("d", &["Both(false, false)"], "Shape"),
        ("e", &["Some(_)"], "Option<Int>"),
        ("f", &["Some(_)", "None"], "Option<Int>"),
        ("g", &["true", "false"], "Bool"),
        ("h", &["Unit"], "Unit"),
        ("i", &["_"], "Int"),
        ("j", &["PageId(_)"], "PageId"),
        ("k", &["false"], "Yes"),
        (
            "l",
            &["Some(Some(Green))", "Some(Some(Blue))"],
            "Option<Option<Color>>",
        ),
        ("m", &["_"], "Text"),
        ("n", &["_"], "Int"),
        ("o", &["Ok(Some(false))"], "Result<Option<Bool>, Unit>"),
    ];
    let result = check_source(src);
    let want: Vec<(u32, &str)> = cases
        .iter()
        .map(|(n, _, _)| (mark(src, n), "non_exhaustive_match"))
        .collect();
    expect(&result, &want);
    for (n, missing, ty) in cases {
        let d = diag_at(&result, "non_exhaustive_match", mark(src, n));
        assert_eq!(d["missing"], json!(missing), "#{n}");
        assert_eq!(d["type"], json!(ty), "#{n}");
        let (l, _) = pos(src, &format!("{{ // #{n}\n"));
        assert_eq!(start(&d), (l, 5), "#{n}: the span starts at `match`");
    }
}

/// 3.13: arm i is unreachable when it is useless after arms 1..i-1
/// (bindings and `_` cover everything, literals repeat); the warning spans
/// the arm (S63 (5)) and coexists with a non-exhaustive report.
#[test]
fn unreachable_arms() {
    let src = r#"module t.ua

fn a(o: Option<Bool>) -> Int {
    match o {
        Some(_) => 1
        None => 2
        Some(true) => 3 // #a
    }
}

fn b(n: Int) -> Int {
    match n {
        x => x
        0 => 0 // #b
    }
}

fn c(t: Text) -> Int {
    match t {
        "a" => 1
        "a" => 2 // #c
        _ => 3
    }
}

fn d(o: Option<Bool>) -> Int {
    match o { // #d
        Some(true) => 1
        Some(true) => 2 // #d2
    }
}

fn e(r: Result<Int, Int>) -> Int {
    match r {
        Ok(_) => 1
        Err(_) => 2
        _ => 3 // #e
    }
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "unreachable_arm"),
            (m("b"), "unreachable_arm"),
            (m("c"), "unreachable_arm"),
            (m("d"), "non_exhaustive_match"),
            (m("d2"), "unreachable_arm"),
            (m("e"), "unreachable_arm"),
        ],
    );
    let a = diag_at(&result, "unreachable_arm", m("a"));
    assert_eq!(a["severity"], json!("warning"));
    assert_eq!(start(&a), pos(src, "Some(true) => 3"));
    assert_eq!(
        diag_at(&result, "non_exhaustive_match", m("d"))["missing"],
        json!(["None"])
    );
}

/// 3.12: patterns bind the unstripped type (a `Positive` payload binds a
/// `Positive`, a wrapper's payload its type), literal patterns are checked
/// against strip(T) and never proved, and the mismatch codes carry
/// `expected`/`actual`; an erroneous arm takes no part in the
/// exhaustiveness check (S62).
#[test]
fn pattern_typing() {
    let src = r#"module t.pt

type Positive = Int where self > 0
type PageId(Text)

fn takes_positive(p: Positive) -> Int {
    p
}

fn a(o: Option<Positive>) -> Int {
    match o {
        Some(p) => takes_positive(p)
        None => 0
    }
}

fn b(size: Positive) -> Int {
    match size {
        1 => 1
        s => takes_positive(s)
    }
}

fn c(o: Option<Positive>) -> Int {
    match o {
        Some(0) => 0
        _ => 1
    }
}

fn d(id: PageId) -> Int {
    match id {
        PageId(t) => t.len
    }
}

fn e(t: Text) -> Int {
    match t {
        0 => 0 // #e
        _ => 1
    }
}

fn f(o: Option<Int>) -> Int {
    match o {
        Some(_, _) => 1 // #f
        None => 0
    }
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "e"), "pattern_type_mismatch"),
            (mark(src, "f"), "pattern_arity_mismatch"),
        ],
    );
    assert_eq!(
        ea(&diag_at(&result, "pattern_type_mismatch", mark(src, "e"))),
        (json!("Text"), json!("Int"))
    );
    assert_eq!(
        ea(&diag_at(&result, "pattern_arity_mismatch", mark(src, "f"))),
        (json!(1), json!(2))
    );
    let p = &result.program;
    assert_eq!(local(p, "t.pt.a", "p"), "Positive");
    assert_eq!(local(p, "t.pt.b", "s"), "Positive");
    assert_eq!(local(p, "t.pt.d", "t"), "Text");
}

/// S20 for `match` and `if`: the first non-diverging arm or branch sets the
/// type and later ones check against it, binding its variables (`Ok(1)`
/// then `Err("x")` is `Result<Int, Text>`).
#[test]
fn branches_bind_each_others_variables() {
    let src = r#"module t.bb

fn a(flag: Bool) -> Int {
    let r = match flag {
        true => Ok(1)
        false => Err("x")
    }
    r.unwrap_or(0)
}

fn b(flag: Bool) -> Int {
    let o = if flag { None } else { Some(1) }
    o.unwrap_or(0)
}
"#;
    let result = check_source(src);
    expect(&result, &[]);
    let p = &result.program;
    assert_eq!(local(p, "t.bb.a", "r"), "Result<Int, Text>");
    assert_eq!(local(p, "t.bb.b", "o"), "Option<Int>");
}

// ---- 3.8: `?` ------------------------------------------------------------------------------

/// 3.8 `e?`: the operand must strip to a `Result`, the enclosing function's
/// return type must strip to one, and the error types must be identical
/// (no widening, no erasure); the payloads carry `expected`/`actual`. A
/// function whose declared return type is itself a refinement of a
/// `Result` (`h`) is `cannot_prove_refinement` at the `?` (S73 [D-251]; an
/// earlier reading of the row accepted it, unsoundly).
#[test]
fn propagation_codes() {
    let src = r#"module t.pr

type Positive = Int where self > 0
type Res = Result<Int, Text> where self != Err("")

fn a(o: Option<Int>) -> Result<Int, Text> {
    let v = o? // #a
    Ok(v)
}

fn b(r: Result<Int, Text>) -> Int {
    r? // #b
}

fn c(r: Result<Int, Text>) -> Result<Int, Bool> {
    Ok(r?) // #c
}

fn d(r: Result<Int, Positive>) -> Result<Int, Int> {
    Ok(r?) // #d
}

fn e(r: Result<Int, List<Positive>>) -> Result<Int, List<Int>> {
    Ok(r?) // #e
}

fn f(r: Result<Result<Int, Text>, Text>) -> Result<Int, Text> {
    Ok(r??)
}

fn g(r: Result<Positive, Text>) -> Result<Positive, Text> {
    let p: Positive = r?
    Ok(p)
}

fn h(r: Result<Int, Text>) -> Res {
    let _v = r? // #h
    Ok(1)
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "propagate_not_result"),
            (m("b"), "propagate_return_mismatch"),
            (m("c"), "propagate_error_mismatch"),
            (m("d"), "propagate_error_mismatch"),
            (m("e"), "propagate_error_mismatch"),
            (m("h"), "cannot_prove_refinement"),
        ],
    );
    assert_eq!(
        diag_at(&result, "propagate_not_result", m("a"))["actual"],
        json!("Option<Int>")
    );
    assert_eq!(
        diag_at(&result, "propagate_return_mismatch", m("b"))["actual"],
        json!("Int")
    );
    let pair = |n: &str| ea(&diag_at(&result, "propagate_error_mismatch", m(n)));
    assert_eq!(pair("c"), (json!("Bool"), json!("Text")));
    assert_eq!(pair("d"), (json!("Int"), json!("Positive")));
    assert_eq!(pair("e"), (json!("List<Int>"), json!("List<Positive>")));
    assert_eq!(
        ea(&diag_at(&result, "cannot_prove_refinement", m("h"))),
        (json!("Res"), json!("Result<Int, Text>"))
    );
}

/// S73 [D-251]: `e?` in a function whose declared return type `R` is a
/// refinement (`strip(R)` a `Result`, so the `e?` row's premise holds) is
/// `cannot_prove_refinement` spanning the `?` expression, with `expected`
/// = `R`, `actual` = `strip(R)` (the root type, through a chain of
/// refinements too), and a note naming the checked spelling. The `?`
/// keeps its type and the success path is checked against `R` as always:
/// a constant tail is proved (`a`), a tail that is not a constant is a
/// second `cannot_prove_refinement` of its own (`b`). A `?` whose error
/// type differs is `propagate_error_mismatch` alone (`c`: the row's
/// premise fails, one diagnostic per `?`). A function returning the base
/// type (`d`), or a `Result` with a refined payload (`e`), propagates as
/// before. The checked program has no `Refine` node for a propagated `Err`
/// (toolchain T39), which is why the checker must refuse it.
#[test]
fn propagation_into_a_refinement_return_type_s73() {
    let src = r#"module t.ps

type Res = Result<Int, Text> where self != Err("")
type Res2 = Res where self != Err("x")
type Positive = Int where self > 0

fn a(r: Result<Int, Text>) -> Res2 {
    let _v = r? // #a
    Ok(1)
}

fn b(r: Result<Int, Text>) -> Res {
    Ok(r?) // #b
}

fn c(r: Result<Int, Bool>) -> Res {
    let _v = r? // #c
    Ok(1)
}

fn d(r: Result<Int, Text>) -> Result<Int, Text> {
    Ok(r? + 1)
}

fn e(r: Result<Int, Text>) -> Result<Positive, Text> {
    let _v = r?
    Ok(1)
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "cannot_prove_refinement"),
            (m("b"), "cannot_prove_refinement"),
            (m("b"), "cannot_prove_refinement"),
            (m("c"), "propagate_error_mismatch"),
        ],
    );
    let a = diag_at(&result, "cannot_prove_refinement", m("a"));
    assert_eq!(ea(&a), (json!("Res2"), json!("Result<Int, Text>")));
    let (line, col) = pos(src, "r? // #a");
    assert_eq!(start(&a), (line, col));
    assert_eq!(end(&a), (line, col + 2));
    let note = notes(&a).join("\n");
    assert!(note.contains("check<Res2>("), "{note}");
    assert!(note.contains("match"), "{note}");
    // `b`: the `?` (inside) and the tail `Ok(r?)` (not a constant), both
    // at `Res`: the propagation's `actual` is the root type, the tail's
    // the type of `Ok(r?)`.
    let at_b: Vec<Value> = with_code(&result, "cannot_prove_refinement")
        .into_iter()
        .filter(|d| d["span"]["line"].as_u64() == Some(u64::from(m("b"))))
        .collect();
    let (line, col) = pos(src, "Ok(r?) // #b");
    let mut starts: Vec<(u64, u64)> = at_b.iter().map(start).collect();
    starts.sort();
    assert_eq!(starts, [(line, col), (line, col + 3)]);
    for d in &at_b {
        assert_eq!(d["expected"], json!("Res"));
        assert_eq!(d["actual"], json!("Result<Int, Text>"));
    }
    assert_eq!(
        ea(&diag_at(&result, "propagate_error_mismatch", m("c"))),
        (json!("Text"), json!("Bool"))
    );
}

/// 3.8 and 5.1: `?` in a test or property body is
/// `propagate_outside_function` alone; in a contract expression, a `where`
/// predicate outside any function included, it is `propagate_in_contract`,
/// which takes precedence, with `symbol`, `facet: "contract"`, and
/// `clause`.
#[test]
fn propagation_outside_function_bodies() {
    let src = r#"module t.po

type Positive = Int where self > 0
type Q = Int where check<Positive>(self)? > 0 // #where

fn f(r: Result<Int, Text>) -> Result<Int, Text>
    requires r? > 0 // #requires
{
    r
}

test "in a test" {
    let _v = check<Positive>(1)? // #test
}

property "in a property"
    for n in gen.int
{
    let _v = check<Positive>(n)? // #property
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("where"), "propagate_in_contract"),
            (m("requires"), "propagate_in_contract"),
            (m("test"), "propagate_outside_function"),
            (m("property"), "propagate_outside_function"),
        ],
    );
    let r = diag_at(&result, "propagate_in_contract", m("requires"));
    assert_eq!(r["symbol"], json!("t.po.f"));
    assert_eq!(r["facet"], json!("contract"));
    assert_eq!(r["clause"], json!("r? > 0"));
    let w = diag_at(&result, "propagate_in_contract", m("where"));
    assert_eq!(w["symbol"], json!("t.po.Q"));
    assert_eq!(w["clause"], json!("check<Positive>(self)? > 0"));
}

// ---- S14, T18: generators --------------------------------------------------------------

/// S14 and T18: a `Gen<T>` expression is allowed only as a `for` range or a
/// `std.gen` argument; elsewhere it is `generator_outside_for` with
/// `actual`; inside a `std.gen` call an argument is an ordinary argument
/// (`gen.range(gen.int, 1)` and `gen.list(5)` are `type_mismatch` alone,
/// S62); nested generators type their binders.
#[test]
fn generator_placement() {
    let src = r#"module t.gp

fn takes(n: Int) -> Int {
    n
}

fn a() -> Int {
    let _g = gen.int // #a
    1
}

fn b() -> Int {
    takes(gen.positive_int) // #b
}

fn c() -> Int {
    match gen.bool { // #c
        _ => 1
    }
}

property "nested generators"
    for xs in gen.list(gen.option(gen.text))
    for n in gen.range(0, 10)
    for o in gen.option(gen.positive_int)
{
    assert xs.len >= 0 and n >= 0 and o.unwrap_or(1) > 0
}

property "a generator in a body"
    for m in gen.int
{
    let _g = gen.bool // #body
    assert m == m
}

property "a generator where an Int is expected"
    for k in gen.range(gen.int, 1) // #range
{
    assert k == k
}

property "a non-generator where a generator is expected"
    for ys in gen.list(5) // #list
{
    assert ys == ys
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "generator_outside_for"),
            (m("b"), "generator_outside_for"),
            (m("c"), "generator_outside_for"),
            (m("body"), "generator_outside_for"),
            (m("range"), "type_mismatch"),
            (m("list"), "type_mismatch"),
        ],
    );
    let actual = |n: &str| diag_at(&result, "generator_outside_for", m(n))["actual"].clone();
    assert_eq!(actual("a"), json!("Gen<Int>"));
    assert_eq!(actual("b"), json!("Gen<Positive>"));
    assert_eq!(actual("c"), json!("Gen<Bool>"));
    assert_eq!(
        ea(&diag_at(&result, "type_mismatch", m("range"))),
        (json!("Int"), json!("Gen<Int>"))
    );
    let p = &result.program;
    let DefKind::Property(prop) = &def(p, "t.gp.\"nested generators\"").kind else {
        panic!("a property")
    };
    let elems: Vec<String> = prop
        .binders
        .iter()
        .map(|b| p.display_ty(&b.elem_ty))
        .collect();
    assert_eq!(elems, ["List<Option<Text>>", "Int", "Option<Positive>"]);
    assert_eq!(
        prop.binders[0].gen,
        GenExpr::List(Box::new(GenExpr::Option(Box::new(GenExpr::Text))))
    );
    assert!(matches!(prop.binders[1].gen, GenExpr::Range(..)));
    assert_eq!(
        prop.binders[2].gen,
        GenExpr::Option(Box::new(GenExpr::PositiveInt))
    );
}

/// 1.6 and 2.2: importing a `std.gen` item is `generator_import` (and, S62,
/// not also `unused_import`); `gen` alone is `not_a_value`; a `for` range
/// that is not a `Gen<T>` is `for_range_not_generator` with `actual`
/// (3.14).
#[test]
fn generator_names_and_ranges() {
    let src = r#"module t.gi

use std.gen.{int} // #import

fn a() -> Int {
    let _g = gen // #gen
    1
}

property "p"
    for x in [1, 2] // #range
{
    assert true
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("import"), "generator_import"),
            (m("gen"), "not_a_value"),
            (m("range"), "for_range_not_generator"),
        ],
    );
    assert_eq!(
        diag_at(&result, "generator_import", m("import"))["name"],
        json!("int")
    );
    assert_eq!(
        diag_at(&result, "not_a_value", m("gen"))["name"],
        json!("gen")
    );
    assert_eq!(
        diag_at(&result, "for_range_not_generator", m("range"))["actual"],
        json!("List<Int>")
    );
}

// ---- 8.1, 8.3: holes and their reports (S48, S56, S64) ------------------------------

/// S56: a hole in a check-mode position takes that position's type: a body
/// tail, `return`, an assignment, an annotation, the conditions of
/// `assert`, `while`, `if`, the fixed operands, the right operand of `==`
/// (the *erased* left type) and of `++` (strip of the left type), every
/// argument (after the statement resolved its variables), a constructor
/// payload, a field, list elements, branches, arms, an expression
/// statement (`Unit`), a `forall` predicate, and a type parameter by name.
#[test]
fn holes_take_the_type_of_their_position() {
    let src = r#"module t.hx

type Positive = Int where self > 0

struct Dims {
    width: Positive
    height: Int
}

pub capability Store {
    fn get(key: Text) -> Int
}

fn takes_positive(p: Positive) -> Int {
    p
}

fn pick<T>(a: T, b: T) -> T {
    a
}

fn tail() -> Positive {
    ?tail
}

fn returned(n: Int) -> List<Text> {
    if n > 0 {
        return ?returned
    }
    []
}

fn assigned() -> Bool {
    var flag = false
    flag = ?assigned
    flag
}

fn annotated() -> Int {
    let n: Option<Int> = ?annotated
    n.unwrap_or(0)
}

fn conditions(n: Int) -> Int {
    assert ?asserted
    var i = n
    while ?looping {
        i = i - 1
    }
    if ?branching { i } else { 0 }
}

fn operators(n: Int) -> Bool {
    let a = not ?negated
    let b = -?minus + n
    let c = ?added * 2
    let d = ?anded and a
    let e = ?ordered < n
    a and b > c and d and e
}

fn equality(ps: List<Positive>, t: Text) -> Bool {
    let a = 1 == ?eq_int
    let b = ps == ?eq_erased
    let c = t != ?ne_text
    a and b and c
}

fn concat(ps: List<Positive>) -> Int {
    let a = "x" ++ ?cat_text
    let b = ps ++ ?cat_list
    a.len + b.len
}

fn calls(xs: List<Int>, ps: List<Positive>, using db: Store) -> Int
    effects { db.get }
{
    let a = takes_positive(?arg)
    let b = pick(?generic, 1)
    let c = xs.take(?method).len
    let d = ps.push(?pushed).len
    let e = db.get(?op_arg)
    let f = check<Positive>(?checked)
    a + b + c + d + e + f.unwrap_or(1)
}

fn literals() -> Int {
    let o: Option<Positive> = Some(?payload)
    let d = Dims { width: ?field, height: 1 }
    let xs: List<Text> = [?element]
    let ys = [1, ?later_element]
    o.unwrap_or(1) + d.height + xs.len + ys.len
}

fn branches(flag: Bool, o: Option<Int>) -> Int {
    let t: Text = if flag { ?then_branch } else { "x" }
    let u = if flag { "y" } else { ?else_branch }
    let v = match o {
        Some(n) => n
        None => ?arm
    }
    t.len + u.len + v
}

fn statement(n: Int) -> Int {
    ?statement
    n
}

fn quantified(xs: List<Int>) -> Bool {
    forall x in xs: ?predicate
}

fn type_parameter<T>(x: T) -> T {
    ?generic_tail
}
"#;
    let want: BTreeMap<&str, &str> = [
        ("tail", "Positive"),
        ("returned", "List<Text>"),
        ("assigned", "Bool"),
        ("annotated", "Option<Int>"),
        ("asserted", "Bool"),
        ("looping", "Bool"),
        ("branching", "Bool"),
        ("negated", "Bool"),
        ("minus", "Int"),
        ("added", "Int"),
        ("anded", "Bool"),
        ("ordered", "Int"),
        ("eq_int", "Int"),
        ("eq_erased", "List<Int>"),
        ("ne_text", "Text"),
        ("cat_text", "Text"),
        ("cat_list", "List<Positive>"),
        ("arg", "Positive"),
        ("generic", "Int"),
        ("method", "Int"),
        ("pushed", "Positive"),
        ("op_arg", "Text"),
        ("checked", "Int"),
        ("payload", "Positive"),
        ("field", "Positive"),
        ("element", "Text"),
        ("later_element", "Int"),
        ("then_branch", "Text"),
        ("else_branch", "Text"),
        ("arm", "Int"),
        ("statement", "Unit"),
        ("predicate", "Bool"),
        ("generic_tail", "T"),
    ]
    .into_iter()
    .collect();
    let result = check_source(src);
    let non_holes: Vec<(u32, String)> = got(&result)
        .into_iter()
        .filter(|(_, c)| c != "hole")
        .collect();
    assert!(non_holes.is_empty(), "\n{}", render(&result));
    let holes = with_code(&result, "hole");
    let found: BTreeMap<String, String> = holes
        .iter()
        .map(|d| {
            (
                d["hole"].as_str().expect("a hole name").to_string(),
                d["expected_type"].as_str().expect("a type").to_string(),
            )
        })
        .collect();
    let want: BTreeMap<String, String> = want
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    assert_eq!(found, want);
    assert_eq!(holes.len(), want.len());
    assert_eq!(result.holes.len(), want.len());
    assert!(holes.iter().all(|d| d["severity"] == "info"));
}

/// S56 and S64: a hole in an infer-only position (an unannotated
/// initializer, a scrutinee, a `forall` range, the left operand of `++` and
/// of `==`, a receiver, a list's first element, the first branch of an `if`
/// without an expected type, or an argument whose type the statement never
/// resolves) is `hole_type_unknown` with `hole` and `symbol`, and gets no
/// `hole` diagnostic and no report.
#[test]
fn holes_in_infer_only_positions() {
    let src = r#"module t.hu

fn id<T>(x: T) -> T {
    x
}

fn a() -> Int {
    let _x = ?unannotated // #a
    1
}

fn b() -> Int {
    match ?scrutinee { // #b
        _ => 1
    }
}

fn c() -> Bool {
    let _ok = forall x in ?range: x > 0 // #c
    true
}

fn d() -> Text {
    ?left ++ "x" // #d
}

fn e() -> Bool {
    ?eq_left == 1 // #e
}

fn f() -> Int {
    (?receiver).len // #f
}

fn g() -> Int {
    let _xs = [?first_element, 1] // #g
    1
}

fn h(flag: Bool) -> Int {
    let _y = if flag { ?first_branch } else { 1 } // #h
    1
}

fn i() -> Int {
    let _z = id(?never_resolved) // #i
    1
}
"#;
    let cases = [
        ("a", "unannotated"),
        ("b", "scrutinee"),
        ("c", "range"),
        ("d", "left"),
        ("e", "eq_left"),
        ("f", "receiver"),
        ("g", "first_element"),
        ("h", "first_branch"),
        ("i", "never_resolved"),
    ];
    let result = check_source(src);
    let want: Vec<(u32, &str)> = cases
        .iter()
        .map(|(n, _)| (mark(src, n), "hole_type_unknown"))
        .collect();
    expect(&result, &want);
    for (n, hole) in cases {
        let d = diag_at(&result, "hole_type_unknown", mark(src, n));
        assert_eq!(d["hole"], json!(hole), "#{n}");
        assert_eq!(d["symbol"], json!(format!("t.hu.{}", n)), "#{n}");
        assert_eq!(d["category"], json!("hole"), "#{n}");
    }
    assert!(result.holes.is_empty());
}

/// S48 and toolchain 7.2: the report lists every visible binding, innermost
/// scope first and the most recent binding first within a scope, with its
/// kind (`var` marked mutable); one capability entry per `using` parameter
/// with each operation's signature and whether it is in the bound; the
/// function's clauses as canonical text in order; and the enclosing
/// declaration with `facet: "body"`.
#[test]
fn hole_reports_list_bindings_capabilities_and_clauses() {
    let src = r#"module t.hb

pub capability Store {
    fn get(key: Text) -> Int
    fn put(key: Text, value: Int) -> Unit
}

fn f(a: Int, b: Text, using db: Store) -> Int
    requires a  >  0
    requires (a < 100)
    ensures result >= a
    effects { db.get }
{
    let c = db.get(b) + a
    var d = c
    d = d + 1
    match Some(d) {
        Some(e) => {
            let g = e
            g + ?h
        }
        None => 0
    }
}
"#;
    let result = check_source(src);
    expect(&result, &[(line(src, "g + ?h"), "hole")]);
    let p = &result.program;
    assert_eq!(result.holes.len(), 1);
    let h = &result.holes[0];
    assert_eq!(h.name, "h");
    assert_eq!(h.expected, Ty::Int);
    assert_eq!(p.display_path(h.def), "t.hb.f");
    let bindings: Vec<(String, String, BindingKind, bool)> = h
        .bindings
        .iter()
        .map(|b| (b.name.clone(), p.display_ty(&b.ty), b.kind, b.mutable))
        .collect();
    let row = |n: &str, t: &str, k: BindingKind, m: bool| (n.to_string(), t.to_string(), k, m);
    assert_eq!(
        bindings,
        [
            row("g", "Int", BindingKind::Let, false),
            row("e", "Int", BindingKind::Pattern, false),
            row("d", "Int", BindingKind::Var, true),
            row("c", "Int", BindingKind::Let, false),
            row("db", "Store", BindingKind::Using, false),
            row("b", "Text", BindingKind::Parameter, false),
            row("a", "Int", BindingKind::Parameter, false),
        ]
    );
    assert_eq!(h.requires, ["a > 0", "(a < 100)"]);
    assert_eq!(h.ensures, ["result >= a"]);
    assert!(h.invariants.is_empty());
    assert_eq!(h.capabilities.len(), 1);
    let cap = &h.capabilities[0];
    assert_eq!(cap.parameter, "db");
    assert_eq!(p.display_path(cap.capability), "t.hb.Store");
    let mut ops: Vec<(String, String, bool)> = cap
        .operations
        .iter()
        .map(|o| (o.name.clone(), o.signature.clone(), o.in_bound))
        .collect();
    ops.sort();
    assert_eq!(
        ops,
        [
            (
                "get".to_string(),
                "fn get(key: Text) -> Int".to_string(),
                true
            ),
            (
                "put".to_string(),
                "fn put(key: Text, value: Int) -> Unit".to_string(),
                false
            ),
        ]
    );
    let d = &with_code(&result, "hole")[0];
    assert_eq!(d["symbol"], json!("t.hb.f"));
    assert_eq!(d["qualified_name"], json!("t.hb.f"));
    assert_eq!(d["facet"], json!("body"));
    assert_eq!(d["category"], json!("hole"));
    assert_eq!(d["expected_type"], json!("Int"));
    assert_eq!(
        d["bindings"],
        json!([
            { "name": "g", "type": "Int", "kind": "let" },
            { "name": "e", "type": "Int", "kind": "pattern" },
            { "name": "d", "type": "Int", "kind": "var", "mutable": true },
            { "name": "c", "type": "Int", "kind": "let" },
            { "name": "db", "type": "Store", "kind": "using" },
            { "name": "b", "type": "Text", "kind": "parameter" },
            { "name": "a", "type": "Int", "kind": "parameter" },
        ])
    );
    assert_eq!(d["requires"], json!(["a > 0", "(a < 100)"]));
    assert_eq!(d["ensures"], json!(["result >= a"]));
    assert_eq!(d["invariants"], json!([]));
    assert_eq!(d["capabilities"][0]["parameter"], json!("db"));
    assert_eq!(d["capabilities"][0]["capability"], json!("t.hb.Store"));
}

/// S48: property `for` binders (`for`), `forall` binders (`forall`), and
/// wrapper-pattern bindings (`pattern`) appear with their kinds and types;
/// a binding is not visible in its own initializer, nor are the bindings of
/// a sibling arm; tests and properties report empty clause lists.
#[test]
fn hole_report_binding_kinds_and_visibility() {
    let src = r#"module t.hv

type PageId(Text)

property "for binders"
    for n in gen.int
    for s in gen.positive_int
{
    assert n + s == ?in_property
}

fn quantified(xs: List<Int>) -> Bool {
    forall x in xs: x == ?in_forall
}

fn unwrapped(id: PageId) -> Int {
    match id {
        PageId(t) => t.len + ?in_arm
    }
}

fn own_binding(n: Int) -> Int {
    let x: Int = ?in_initializer
    x + n
}

fn sibling(o: Option<Int>) -> Int {
    match o {
        Some(v) => v
        None => ?in_sibling
    }
}

test "in a test" {
    let flag = true
    assert flag == ?in_test
}
"#;
    let result = check_source(src);
    let non_holes: Vec<(u32, String)> = got(&result)
        .into_iter()
        .filter(|(_, c)| c != "hole")
        .collect();
    assert!(non_holes.is_empty(), "\n{}", render(&result));
    let p = &result.program;
    let report = |name: &str| {
        result
            .holes
            .iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("no report for ?{name}"))
    };
    let rows = |name: &str| -> Vec<(String, String, BindingKind)> {
        report(name)
            .bindings
            .iter()
            .map(|b| (b.name.clone(), p.display_ty(&b.ty), b.kind))
            .collect()
    };
    let row = |n: &str, t: &str, k: BindingKind| (n.to_string(), t.to_string(), k);
    assert_eq!(
        rows("in_property"),
        [
            row("s", "Positive", BindingKind::For),
            row("n", "Int", BindingKind::For),
        ]
    );
    assert_eq!(
        rows("in_forall"),
        [
            row("x", "Int", BindingKind::Forall),
            row("xs", "List<Int>", BindingKind::Parameter),
        ]
    );
    assert_eq!(
        rows("in_arm"),
        [
            row("t", "Text", BindingKind::Pattern),
            row("id", "PageId", BindingKind::Parameter),
        ]
    );
    assert_eq!(
        rows("in_initializer"),
        [row("n", "Int", BindingKind::Parameter)]
    );
    assert_eq!(
        rows("in_sibling"),
        [row("o", "Option<Int>", BindingKind::Parameter)]
    );
    assert_eq!(rows("in_test"), [row("flag", "Bool", BindingKind::Let)]);
    for name in [
        "in_property",
        "in_forall",
        "in_arm",
        "in_initializer",
        "in_sibling",
    ] {
        assert_eq!(report(name).expected, Ty::Int, "?{name}");
    }
    assert_eq!(report("in_test").expected, Ty::Bool);
    for name in ["in_property", "in_test"] {
        let r = report(name);
        assert!(
            r.requires.is_empty() && r.ensures.is_empty() && r.capabilities.is_empty(),
            "?{name}"
        );
    }
    let d = with_code(&result, "hole")
        .into_iter()
        .find(|d| d["hole"] == "in_property")
        .expect("the property's hole");
    assert_eq!(d["qualified_name"], json!("t.hv.\"for binders\""));
    assert_eq!(d["facet"], json!("body"));
    assert_eq!(d["requires"], json!([]));
    assert_eq!(d["ensures"], json!([]));
}

/// S64: a hole in a contract expression (`requires`, `ensures`, a `where`
/// predicate, a `for` range) is `hole_in_contract` alone, with `symbol`,
/// `facet: "contract"`, and `clause`: no `hole`, no report.
#[test]
fn holes_in_contracts_are_reported_once() {
    let src = r#"module t.hc

fn f(n: Int) -> Int
    requires ?pre // #pre
    ensures result > ?post // #post
{
    n
}

type R = Int where self > ?bound // #bound

property "range"
    for x in gen.range(0, ?hi) // #hi
{
    assert x >= 0
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("pre"), "hole_in_contract"),
            (m("post"), "hole_in_contract"),
            (m("bound"), "hole_in_contract"),
            (m("hi"), "hole_in_contract"),
        ],
    );
    assert!(result.holes.is_empty());
    let pre = diag_at(&result, "hole_in_contract", m("pre"));
    assert_eq!(pre["symbol"], json!("t.hc.f"));
    assert_eq!(pre["facet"], json!("contract"));
    assert_eq!(pre["clause"], json!("?pre"));
    assert_eq!(
        diag_at(&result, "hole_in_contract", m("post"))["clause"],
        json!("result > ?post")
    );
    let bound = diag_at(&result, "hole_in_contract", m("bound"));
    assert_eq!(bound["symbol"], json!("t.hc.R"));
    assert_eq!(bound["clause"], json!("self > ?bound"));
}

/// 8.1: a declaration containing a hole is otherwise fully checked, and its
/// callers typecheck against its signature.
#[test]
fn declarations_with_holes_are_fully_checked() {
    let src = r#"module t.hf

fn f(n: Int) -> Int {
    let _t: Text = n // #mismatch
    ?rest // #hole
}

fn caller() -> Text {
    f(1) // #caller
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("mismatch"), "type_mismatch"),
            (m("hole"), "hole"),
            (m("caller"), "type_mismatch"),
        ],
    );
    assert_eq!(result.holes.len(), 1);
    assert_eq!(result.holes[0].expected, Ty::Int);
    assert_eq!(
        diag_at(&result, "hole", m("hole"))["severity"],
        json!("info")
    );
}

// ---- 9.4 payloads and S63 spans of the remaining codes ---------------------------

/// 3.7 step 2, S28, 9.4: `arity_mismatch` and `using_arity_mismatch` carry
/// `callee` (the qualified name of the function, variant, or operation)
/// and the `expected`/`actual` counts, for calls, payload variants used
/// bare, nullary variants applied, and operations.
#[test]
fn arity_payloads() {
    let src = r#"module t.ar

type PageId(Text)

enum StoreError {
    Missing(PageId)
    Unavailable
}

pub capability Store {
    fn get(key: Text) -> Int
}

fn two(a: Int, b: Int) -> Int {
    a + b
}

fn a() -> Int {
    two(1) // #a
}

fn b() -> StoreError {
    Missing // #b
}

fn c() -> StoreError {
    Unavailable(1) // #c
}

fn d(key: Text, using db: Store) -> Int
    effects { db.get }
{
    db.get(key, key) // #d
}

fn e(using db: Store) -> Int
    effects { db.get }
{
    two(1, 2, using db) // #e
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "arity_mismatch"),
            (m("b"), "arity_mismatch"),
            (m("c"), "arity_mismatch"),
            (m("d"), "arity_mismatch"),
            (m("e"), "using_arity_mismatch"),
        ],
    );
    let check = |code: &str, n: &str, callee: &str, expected: usize, actual: usize| {
        let d = diag_at(&result, code, m(n));
        assert_eq!(d["callee"], json!(callee), "#{n}");
        assert_eq!(ea(&d), (json!(expected), json!(actual)), "#{n}");
    };
    check("arity_mismatch", "a", "t.ar.two", 2, 1);
    check("arity_mismatch", "b", "t.ar.StoreError.Missing", 1, 0);
    check("arity_mismatch", "c", "t.ar.StoreError.Unavailable", 0, 1);
    check("arity_mismatch", "d", "t.ar.Store.get", 1, 2);
    check("using_arity_mismatch", "e", "t.ar.two", 0, 1);
}

/// 3.8 and S8: a callee that is a local is `not_callable` with `name`, and
/// a local hiding a module item names the hidden item in a note.
#[test]
fn not_callable_names_the_hidden_item() {
    let src = r#"module t.nk

use std.text.{trim}

fn a(t: Text) -> Text {
    let trim = 3
    let _u = trim
    trim(t) // #a
}

fn b(t: Text) -> Text {
    trim(t)
}

fn c() -> Int {
    let f = 1
    f() // #c
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "not_callable"),
            (mark(src, "c"), "not_callable"),
        ],
    );
    let a = diag_at(&result, "not_callable", mark(src, "a"));
    assert_eq!(a["name"], json!("trim"));
    assert!(
        notes(&a).iter().any(|n| n.contains("std.text.trim")),
        "{:?}",
        notes(&a)
    );
    assert_eq!(
        diag_at(&result, "not_callable", mark(src, "c"))["name"],
        json!("f")
    );
}

/// S18 and S21: ordering needs `Int` operands after strip (one
/// `comparison_unsupported` per comparison, with the offending operand's
/// type as `actual`, spanning that operand); `++` on a refined left operand
/// has the type strip(T_e1), and a constant concatenation is proved.
#[test]
fn ordering_and_concatenation() {
    let src = r#"module t.cc

type Positive = Int where self > 0
type Word = Text where self.len > 0

fn a() -> Bool {
    "a" < "b" // #a
}

fn b(xs: List<Int>) -> Bool {
    xs <= xs // #b
}

fn c() -> Bool {
    1 < "z" // #c
}

fn d(p: Positive) -> Bool {
    p < 10
}

fn e(w: Word) -> Text {
    w ++ "x"
}

fn f(w: Word) -> Word {
    w ++ "y" // #f
}

fn g() -> Word {
    "a" ++ "b"
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "comparison_unsupported"),
            (m("b"), "comparison_unsupported"),
            (m("c"), "comparison_unsupported"),
            (m("f"), "cannot_prove_refinement"),
        ],
    );
    let actual = |n: &str| diag_at(&result, "comparison_unsupported", m(n))["actual"].clone();
    assert_eq!(actual("a"), json!("Text"));
    assert_eq!(actual("b"), json!("List<Int>"));
    assert_eq!(actual("c"), json!("Text"));
    assert_eq!(
        start(&diag_at(&result, "comparison_unsupported", m("c"))),
        pos(src, "\"z\" // #c")
    );
    assert_eq!(
        ea(&diag_at(&result, "cannot_prove_refinement", m("f"))),
        (json!("Word"), json!("Text"))
    );
    let p = &result.program;
    let ExprKind::Refine { expr, .. } = &tail(body(p, "t.cc.g")).kind else {
        panic!("a proved concatenation")
    };
    assert!(matches!(expr.kind, ExprKind::Binary { .. }));
}

/// S63 (2): a `type_mismatch` on a clause spans the clause from its keyword
/// (`requires`, `where`).
#[test]
fn clause_mismatches_span_the_clause() {
    let src = r#"module t.cl

fn a(n: Int) -> Int
    requires n + 1
{
    n
}

type Bad = Int where self + 1
"#;
    let result = check_source(src);
    let (lr, lw) = (line(src, "requires n + 1"), line(src, "where self + 1"));
    expect(&result, &[(lr, "type_mismatch"), (lw, "type_mismatch")]);
    let r = diag_at(&result, "type_mismatch", lr);
    let (l, c) = pos(src, "requires n + 1");
    assert_eq!(start(&r), (l, c));
    assert_eq!(end(&r), (l, c + "requires n + 1".len() as u64));
    assert_eq!(ea(&r), (json!("Bool"), json!("Int")));
    let w = diag_at(&result, "type_mismatch", lw);
    let (l, c) = pos(src, "where self + 1");
    assert_eq!(start(&w), (l, c));
    assert_eq!(end(&w), (l, c + "where self + 1".len() as u64));
}

/// The severities and categories that 9.4 fixes for the warnings and infos
/// of Section 3 and 8: `unreachable_arm` and `unreachable_statement` are
/// type warnings, `hole` a hole info, `ambiguous_constructor` a resolve
/// error.
#[test]
fn severities_and_categories() {
    let src = r#"module t.sc

enum A {
    X
}

enum B {
    X
}

fn a(flag: Bool) -> Int {
    match flag {
        _ => 1
        true => 2 // #arm
    }
}

fn b(n: Int) -> Int {
    return n
    let _x = 1 // #stmt
    n
}

fn c() -> Int {
    ?h // #hole
}

fn d() -> Int {
    let _y = X // #ambiguous
    1
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("arm"), "unreachable_arm"),
            (m("stmt"), "unreachable_statement"),
            (m("hole"), "hole"),
            (m("ambiguous"), "ambiguous_constructor"),
        ],
    );
    let sc = |code: &str, n: &str| {
        let d = diag_at(&result, code, m(n));
        (d["severity"].clone(), d["category"].clone())
    };
    assert_eq!(
        sc("unreachable_arm", "arm"),
        (json!("warning"), json!("type"))
    );
    assert_eq!(
        sc("unreachable_statement", "stmt"),
        (json!("warning"), json!("type"))
    );
    assert_eq!(sc("hole", "hole"), (json!("info"), json!("hole")));
    assert_eq!(
        sc("ambiguous_constructor", "ambiguous"),
        (json!("error"), json!("resolve"))
    );
    let triples = project_triples(&result);
    assert!(triples
        .iter()
        .any(|t| t.1 == Severity::Warning && t.2 == "unreachable_arm"));
}

// ---- further adversarial cases ------------------------------------------------------------

/// 3.7 step 1 for a member stub: explicit type arguments instantiate the
/// stub, whose receiver parameter must then accept the receiver, so
/// `xs.first<Text>()` on a `List<Int>` is a `type_mismatch` (never a silent
/// `Option<Text>`); a wrong count is `type_argument_count_mismatch`.
#[test]
fn explicit_type_arguments_on_methods() {
    let src = r#"module t.em

fn a(xs: List<Int>) -> Option<Int> {
    xs.first<Int>()
}

fn b(xs: List<Int>) -> Option<Text> {
    xs.first<Text>() // #b
}

fn c(xs: List<Int>) -> Option<Int> {
    xs.first<Int, Int>() // #c
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "b"), "type_mismatch"),
            (mark(src, "c"), "type_argument_count_mismatch"),
        ],
    );
    assert_eq!(
        ea(&diag_at(
            &result,
            "type_argument_count_mismatch",
            mark(src, "c")
        )),
        (json!(1), json!(2))
    );
}

/// S62: a struct literal that is `missing_field` has a diagnostic, so the
/// type argument its missing field would have fixed is not also
/// `cannot_infer_type` (the fix of one is the fix of the other).
#[test]
fn incomplete_generic_struct_literals_do_not_cascade() {
    let src = r#"module t.ic

struct Pair<A, B> {
    left: A
    right: B
}

fn a() -> Int {
    let _p = Pair { left: 1 } // #a
    1
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "a"), "missing_field")]);
}

/// 3.7 step 1: explicit type arguments are the instantiation, so the
/// argument of `id<Positive>(5)` is checked against `Positive` and proved,
/// while `id(5)` would instantiate `T := Int` (S19).
#[test]
fn explicit_type_arguments_make_check_positions() {
    let src = r#"module t.xa

type Positive = Int where self > 0

fn id<T>(x: T) -> T {
    x
}

fn a() -> Positive {
    id<Positive>(5)
}

fn b() -> Positive {
    id<Positive>(0) // #b
}

fn c() -> Option<Positive> {
    Some<Positive>(0) // #c
}

fn d() -> Option<Int> {
    let o = Some<Positive>(1)
    o // #d
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "b"), "refinement_false"),
            (mark(src, "c"), "refinement_false"),
            (mark(src, "d"), "type_mismatch"),
        ],
    );
    assert_eq!(
        ea(&diag_at(&result, "type_mismatch", mark(src, "d"))),
        (json!("Option<Int>"), json!("Option<Positive>"))
    );
    let p = &result.program;
    let ExprKind::Call {
        generic_args, args, ..
    } = &tail(body(p, "t.xa.a")).kind
    else {
        panic!("a call")
    };
    let shown: Vec<String> = generic_args.iter().map(|t| p.display_ty(t)).collect();
    assert_eq!(shown, ["Positive"]);
    assert!(matches!(args[0].kind, ExprKind::Refine { .. }));
}

/// 3.7 with S12: a generic user enum's constructor creates the variables of
/// its type; one the statement leaves unbound is `cannot_infer_type` at the
/// constructor, typed `Tree<?>`; nested constructors and patterns
/// instantiate recursively.
#[test]
fn generic_enum_constructors() {
    let src = r#"module t.ge

enum Tree<T> {
    Leaf(T)
    Node(List<Tree<T>>)
}

fn a() -> Int {
    let _t = Node([]) // #a
    1
}

fn b() -> Tree<Int> {
    Node([Leaf(1), Node([])])
}

fn c() -> Int {
    let t = Leaf(Leaf(1))
    match t {
        Leaf(Leaf(n)) => n
        Leaf(Node(_)) => 0
        Node(_) => 0
    }
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "a"), "cannot_infer_type")]);
    let a = diag_at(&result, "cannot_infer_type", mark(src, "a"));
    assert_eq!(start(&a), pos(src, "Node([]) // #a"));
    assert_eq!(a["type"], json!("Tree<?>"));
    assert_eq!(local(&result.program, "t.ge.c", "t"), "Tree<Tree<Int>>");
}

/// S56 for holes in constructor payloads (an argument position of a
/// constructor: a wrapper's payload is `Text`, while `Some(?p)` without an
/// expected type leaves `T` unresolved), on the right of an `==` whose left
/// operand is unresolved (inferred, so `hole_type_unknown`, and S62: the
/// empty list is not also `cannot_infer_type`), and in an argument resolved
/// by the expected type that flows out of the call (3.8).
#[test]
fn holes_in_payloads_and_unresolved_comparisons() {
    let src = r#"module t.hp

type Positive = Int where self > 0
type PageId(Text)

fn id<T>(x: T) -> T {
    x
}

fn a() -> PageId {
    let w = PageId(?wrapped) // #a
    w
}

fn b() -> Int {
    let _o = Some(?unbound) // #b
    1
}

fn c() -> Bool {
    [] == ?right // #c
}

fn d() -> Positive {
    let p: Positive = id(?through_call) // #d
    p
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "hole"),
            (m("b"), "hole_type_unknown"),
            (m("c"), "hole_type_unknown"),
            (m("d"), "hole"),
        ],
    );
    assert_eq!(
        diag_at(&result, "hole", m("a"))["expected_type"],
        json!("Text")
    );
    assert_eq!(
        diag_at(&result, "hole", m("d"))["expected_type"],
        json!("Positive")
    );
}

/// S24 through tails and arms: a block whose tail is a diverging `if`/`else`
/// diverges, so a `match` statement whose arms are such blocks diverges and
/// the next statement is unreachable; an arm that is an expression never
/// diverges (only blocks do), and an `if` without `else` never diverges.
#[test]
fn divergence_through_tails_and_arms() {
    let src = r#"module t.dt

fn a(flag: Bool, o: Option<Int>) -> Int {
    match o {
        Some(_) => {
            if flag { return 1 } else { return 2 }
        }
        None => { return 0 }
    }
    let _x = 3 // #a
    4
}

fn b(flag: Bool, o: Option<Int>) -> Int {
    match o {
        Some(_) => if flag { return 1 } else { return 2 }
        None => { return 0 }
    }
    let _y = 3
    4
}

fn c(flag: Bool) -> Int {
    if flag { return 1 }
    let _z = 2
    3
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "a"), "unreachable_statement")]);
}

/// 3.4 rule 2 for expression statements (3.11): a value of a refinement of
/// `Unit` is acceptable where `Unit` is expected, so it is widened, not
/// `unused_value`.
#[test]
fn refinements_of_unit_are_unit_statements() {
    let src = r#"module t.us

type Done = Unit where true

fn finish() -> Done {
    return
}

fn a() -> Int {
    finish()
    1
}
"#;
    let result = check_source(src);
    expect(&result, &[]);
    let p = &result.program;
    let Stmt::Expr(e) = &body(p, "t.us.a").stmts[0] else {
        panic!("an expression statement")
    };
    assert_eq!(widens(p, e).0, ["t.us.Done"]);
}

/// 3.8 `forall`: the range must strip to a `List` (`forall_range_not_list`
/// with `actual`), the binder has the element type unstripped (a
/// `List<Positive>` binds a `Positive`), and the predicate is `⇐ Bool`.
#[test]
fn forall_ranges_and_binders() {
    let src = r#"module t.fa

type Positive = Int where self > 0
type NonEmpty = List<Int> where self.len > 0

fn takes_positive(p: Positive) -> Int {
    p
}

fn a(ps: List<Positive>) -> Bool {
    forall p in ps: takes_positive(p) > 0
}

fn b(ne: NonEmpty) -> Bool {
    forall x in ne: x > 0
}

fn c(t: Text) -> Bool {
    forall x in t: true // #c
}

fn d(n: Int) -> Bool {
    forall x in n: true // #d
}

fn e(xs: List<Int>) -> Bool {
    forall x in xs: x // #e
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("c"), "forall_range_not_list"),
            (m("d"), "forall_range_not_list"),
            (m("e"), "type_mismatch"),
        ],
    );
    assert_eq!(
        diag_at(&result, "forall_range_not_list", m("c"))["actual"],
        json!("Text")
    );
    assert_eq!(
        diag_at(&result, "forall_range_not_list", m("d"))["actual"],
        json!("Int")
    );
    assert_eq!(
        ea(&diag_at(&result, "type_mismatch", m("e"))),
        (json!("Bool"), json!("Int"))
    );
    let p = &result.program;
    assert_eq!(local(p, "t.fa.a", "p"), "Positive");
    assert_eq!(local(p, "t.fa.b", "x"), "Int");
}

/// 3.13: depth-first means every witness under one constructor precedes
/// those under the next, each group in declaration order; a single-arm
/// match on `Unit` or on a wrapper is exhaustive.
#[test]
fn witnesses_are_listed_depth_first() {
    let src = r#"module t.wd

type PageId(Text)

enum Color {
    Red
    Green
    Blue
}

fn a(r: Result<Color, Color>) -> Int {
    match r { // #a
        Ok(Red) => 1
        Err(Green) => 2
    }
}

fn b(u: Unit, id: PageId) -> Int {
    let x = match u {
        Unit => 1
    }
    let y = match id {
        PageId(_) => 2
    }
    x + y
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "a"), "non_exhaustive_match")]);
    assert_eq!(
        diag_at(&result, "non_exhaustive_match", mark(src, "a"))["missing"],
        json!(["Ok(Green)", "Ok(Blue)", "Err(Red)", "Err(Blue)"])
    );
}

/// S10 step 1 in the remaining positions: an arm body with an expected type,
/// the later branch and the later list element whose type the first one set
/// (S20), and a nested pattern payload resolved against the payload type.
#[test]
fn constructors_in_later_parts_and_nested_patterns() {
    let src = r#"module t.lp

enum A {
    Missing(Int)
    Gone
}

enum B {
    Missing(Int)
    Gone
}

fn arm(o: Option<Int>) -> A {
    match o {
        Some(n) => Missing(n)
        None => Gone
    }
}

fn later(flag: Bool) -> Int {
    let x = if flag { A.Gone } else { Missing(1) }
    let xs = [B.Gone, Missing(2)]
    match x {
        Missing(n) => n + xs.len
        Gone => 0
    }
}

fn nested(o: Option<A>) -> Int {
    match o {
        Some(Missing(n)) => n
        Some(Gone) => 0
        None => 0
    }
}
"#;
    let result = check_source(src);
    expect(&result, &[]);
    let p = &result.program;
    assert_eq!(local(p, "t.lp.later", "x"), "A");
    assert_eq!(local(p, "t.lp.later", "xs"), "List<B>");
}

/// 3.9: the fields of a generic struct have their instantiated types (a
/// `Positive` field reads as `Positive`), and calling a field is
/// `not_callable` with `name`.
#[test]
fn fields_of_generic_structs() {
    let src = r#"module t.fg

type Positive = Int where self > 0

struct Pair<A, B> {
    left: A
    right: B
}

fn takes_positive(p: Positive) -> Int {
    p
}

fn a(p: Pair<Positive, Text>) -> Int {
    takes_positive(p.left) + p.right.len
}

fn b(p: Pair<Positive, Text>) -> Int {
    p.left() // #b
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "b"), "not_callable")]);
    assert_eq!(
        diag_at(&result, "not_callable", mark(src, "b"))["name"],
        json!("left")
    );
}

/// S62 for annotations: an annotation that failed to resolve, or has the
/// wrong argument count, makes the binding erroneous, so neither its
/// initializer nor its later uses are reported again.
#[test]
fn erroneous_annotations_do_not_cascade() {
    let src = r#"module t.ea

fn a() -> Int {
    let x: Nope = 1 // #a
    x + 1
}

fn b() -> Int {
    let ys: List = [] // #b
    ys.len
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "unresolved_type"),
            (mark(src, "b"), "type_argument_count_mismatch"),
        ],
    );
}

/// 3.11 `while`: a `decreases` measure must strip to `Int` (a refinement of
/// `Int` qualifies); a mismatch spans the clause from its keyword (S63 (2)).
#[test]
fn loop_measures() {
    let src = r#"module t.lm

type Positive = Int where self > 0

fn a(p: Positive, t: Text) -> Int {
    var i = 0
    while i < 3
        decreases p
    {
        i = i + 1
    }
    while i < 6
        decreases t
    {
        i = i + 1
    }
    i
}
"#;
    let result = check_source(src);
    let l = line(src, "decreases t");
    expect(&result, &[(l, "type_mismatch")]);
    let d = diag_at(&result, "type_mismatch", l);
    assert_eq!(start(&d), pos(src, "decreases t"));
    assert_eq!(ea(&d), (json!("Int"), json!("Text")));
}

/// S21 with S15: the right operand of `++` is checked against strip of the
/// left one, so element refinements must agree exactly; a literal right
/// operand is proved element by element.
#[test]
fn concatenation_is_invariant() {
    let src = r#"module t.cv

type Positive = Int where self > 0

fn a(xs: List<Int>, ps: List<Positive>) -> List<Int> {
    xs ++ ps // #a
}

fn b(xs: List<Int>, ps: List<Positive>) -> List<Positive> {
    ps ++ xs // #b
}

fn c(ps: List<Positive>) -> List<Positive> {
    ps ++ [1]
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[(m("a"), "type_mismatch"), (m("b"), "type_mismatch")],
    );
    let a = diag_at(&result, "type_mismatch", m("a"));
    assert_eq!(ea(&a), (json!("List<Int>"), json!("List<Positive>")));
    assert_eq!(start(&a), pos(src, "ps // #a"));
    assert!(notes(&a).iter().any(|n| n.contains("top level")));
    let b = diag_at(&result, "type_mismatch", m("b"));
    assert_eq!(ea(&b), (json!("List<Positive>"), json!("List<Int>")));
}

/// 3.6: with a resolved left operand the right operand of `==` is checked
/// against the erased left type, element by element, so a wrong element is
/// reported at that element.
#[test]
fn equality_checks_the_right_operand_against_the_erased_left() {
    let src = r#"module t.eq2

type Positive = Int where self > 0

fn a(ps: List<Positive>) -> Bool {
    ps == [1, "a"] // #a
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "a"), "type_mismatch")]);
    let a = diag_at(&result, "type_mismatch", mark(src, "a"));
    assert_eq!(start(&a), pos(src, "\"a\"] // #a"));
    assert_eq!(ea(&a), (json!("Int"), json!("Text")));
}

/// S48: one capability entry per `using` parameter in scope, each listing
/// every operation of its capability and whether `parameter.operation` is
/// in the enclosing function's bound.
#[test]
fn hole_reports_list_every_capability() {
    let src = r#"module t.hk

pub capability Store {
    fn get(key: Text) -> Int
}

pub capability Log {
    fn write(line: Text) -> Unit
    fn flush() -> Unit
}

fn f(k: Text, using db: Store, using log: Log) -> Int
    effects { db.get, log.write }
{
    let _u = log.write(k)
    db.get(k) + ?h
}
"#;
    let result = check_source(src);
    expect(&result, &[(line(src, "?h"), "hole")]);
    let p = &result.program;
    let h = &result.holes[0];
    let caps: BTreeMap<String, (String, Vec<(String, bool)>)> = h
        .capabilities
        .iter()
        .map(|c| {
            let mut ops: Vec<(String, bool)> = c
                .operations
                .iter()
                .map(|o| (o.name.clone(), o.in_bound))
                .collect();
            ops.sort();
            (c.parameter.clone(), (p.display_path(c.capability), ops))
        })
        .collect();
    let want: BTreeMap<String, (String, Vec<(String, bool)>)> = [
        (
            "db".to_string(),
            ("t.hk.Store".to_string(), vec![("get".to_string(), true)]),
        ),
        (
            "log".to_string(),
            (
                "t.hk.Log".to_string(),
                vec![("flush".to_string(), false), ("write".to_string(), true)],
            ),
        ),
    ]
    .into_iter()
    .collect();
    assert_eq!(caps, want);
}

/// S16's constant grammar excludes names and member accesses: `[1, 2].len`
/// and `[1, n]` are not constants, so they are `cannot_prove_refinement`;
/// a constant whose parts the typer already proved (`Some(2)` at a
/// refinement of `Option<Positive>`) is still a constant.
#[test]
fn constant_expressions_exclude_names_and_members() {
    let src = r#"module t.ce

type Positive = Int where self > 0
type NonEmpty = List<Int> where self.len > 0
type SomeSmall = Option<Positive> where self != None

fn a() -> Positive {
    [1, 2].len // #a
}

fn b(n: Int) -> NonEmpty {
    [1, n] // #b
}

fn c() -> SomeSmall {
    Some(2)
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "cannot_prove_refinement"),
            (mark(src, "b"), "cannot_prove_refinement"),
        ],
    );
    assert_eq!(
        ea(&diag_at(&result, "cannot_prove_refinement", mark(src, "b"))),
        (json!("NonEmpty"), json!("List<Int>"))
    );
    let p = &result.program;
    assert_eq!(
        refines(p, body(p, "t.ce.c")),
        pairs(&[
            ("t.ce.SomeSmall", "SomeSmall"),
            ("t.ce.Positive", "Positive")
        ])
    );
}

/// S16 step 2 evaluates predicates in the compile-time evaluable subset
/// (`forall`, `if`, `match` with its bindings, the `let`s of contract
/// sub-blocks, `.is_empty()`, `++`) with the arithmetic of 6.3: arbitrary
/// precision, `/` truncating toward zero, and `%` taking the dividend's
/// sign; a huge value is reported as a JSON number (S40).
#[test]
fn predicates_in_the_evaluable_subset() {
    let src = r#"module t.pv

type AllPos = List<Int> where forall x in self: x > 0
type Clamp = Int where if self > 10 { false } else { true }
type SomePos = Option<Int> where match self {
    Some(v) => v > 0
    None => false
}
type Even = Int where if true {
    let half = self / 2
    half * 2 == self
} else {
    false
}
type Word = Text where not self.is_empty()
type Ab = Text where self ++ "" == "ab"
type NegRem = Int where self % 2 == -1
type HalfNeg = Int where self / 2 == -1
type Huge = Int where self > 9223372036854775807
type Neg = Int where self < 0

fn a() -> AllPos {
    [1, 2]
}

fn b() -> AllPos {
    [1, -1] // #b
}

fn c() -> Clamp {
    5
}

fn d() -> Clamp {
    11 // #d
}

fn e() -> SomePos {
    Some(3)
}

fn f() -> SomePos {
    Some(-1) // #f
}

fn g() -> SomePos {
    None // #g
}

fn h() -> Even {
    4
}

fn i() -> Even {
    5 // #i
}

fn j() -> Word {
    "a"
}

fn k() -> Word {
    "" // #k
}

fn l() -> Ab {
    "a" ++ "b"
}

fn m() -> NegRem {
    -3
}

fn n() -> NegRem {
    3 // #n
}

fn o() -> HalfNeg {
    -3
}

fn p() -> HalfNeg {
    -1 // #p
}

fn q() -> Huge {
    9223372036854775808
}

fn r() -> Neg {
    99999999999999999999999 // #r
}
"#;
    let result = check_source(src);
    let names = ["b", "d", "f", "g", "i", "k", "n", "p", "r"];
    let want: Vec<(u32, &str)> = names
        .iter()
        .map(|n| (mark(src, n), "refinement_false"))
        .collect();
    expect(&result, &want);
    let actual = |n: &str| diag_at(&result, "refinement_false", mark(src, n))["actual"].clone();
    assert_eq!(actual("b"), json!([1, -1]));
    assert_eq!(
        diag_at(&result, "refinement_false", mark(src, "b"))["clause"],
        json!("forall x in self: x > 0")
    );
    assert_eq!(
        actual("f"),
        json!({ "$type": "Option", "$variant": "Some", "$payload": [-1] })
    );
    assert_eq!(actual("k"), json!(""));
    assert_eq!(actual("r").to_string(), "99999999999999999999999");
}

/// S19: calls take their types from their arguments, literal forms from the
/// expected type, so `flatten([[size]])` at `List<Int>` instantiates
/// `T := Positive` and is a `type_mismatch` (S15), while `[size]` at
/// `List<Int>` widens the element and `flatten<Int>([[size]])` is fine.
#[test]
fn calls_infer_from_arguments_literals_from_the_expected_type() {
    let src = r#"module t.af

type Positive = Int where self > 0

fn a(size: Positive) -> List<Int> {
    flatten([[size]]) // #a
}

fn b(size: Positive) -> List<Int> {
    [size]
}

fn c(size: Positive) -> List<Int> {
    flatten<Int>([[size]])
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "a"), "type_mismatch")]);
    assert_eq!(
        ea(&diag_at(&result, "type_mismatch", mark(src, "a"))),
        (json!("List<Int>"), json!("List<Positive>"))
    );
    let p = &result.program;
    let ExprKind::List(items) = &tail(body(p, "t.af.b")).kind else {
        panic!("a list")
    };
    assert_eq!(widens(p, &items[0]).0, ["t.af.Positive"]);
}

/// 3.7 and S56: an empty list as a receiver (infer-only) whose variable the
/// statement never binds is `cannot_infer_type` at the list.
#[test]
fn empty_receivers() {
    let src = r#"module t.er

fn a() -> Int {
    let _f = [].first() // #a
    1
}

fn b() -> Int {
    let _n = [].len // #b
    1
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "cannot_infer_type"),
            (mark(src, "b"), "cannot_infer_type"),
        ],
    );
    for (n, at) in [("a", "[].first() // #a"), ("b", "[].len // #b")] {
        let d = diag_at(&result, "cannot_infer_type", mark(src, n));
        assert_eq!(start(&d), pos(src, at), "#{n}");
        assert_eq!(d["type"], json!("List<?>"), "#{n}");
    }
}

/// S62 for `?`: an operand that already failed is erroneous, so `?` adds no
/// `propagate_not_result`.
#[test]
fn propagating_erroneous_operands() {
    let src = r#"module t.pq

fn a() -> Result<Int, Text> {
    let _v = undefined_thing? // #a
    Ok(1)
}

fn b() -> Result<Int, Text> {
    let _w = (1 + "a")? // #b
    Ok(1)
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "unresolved_name"),
            (mark(src, "b"), "type_mismatch"),
        ],
    );
}

/// S64 under `--release`: a hole without an expected type is
/// `hole_type_unknown` alone (no `hole_in_release` beside it), a typed one
/// in a `fn` body is `hole_in_release` alone (S47).
#[test]
fn release_reports_each_hole_once() {
    let src = r#"module t.rl

fn a() -> Int {
    let _x = ?untyped // #a
    1
}

fn b() -> Int {
    ?typed // #b
}
"#;
    let result = check_source_with(
        src,
        &cpl_sema::CheckConfig {
            release: true,
            ..cpl_sema::CheckConfig::default()
        },
    );
    expect(
        &result,
        &[
            (0, "project_lock_missing"),
            (mark(src, "a"), "hole_type_unknown"),
            (mark(src, "b"), "hole_in_release"),
        ],
    );
    let b = diag_at(&result, "hole_in_release", mark(src, "b"));
    assert_eq!(b["hole"], json!("typed"));
    assert_eq!(b["expected_type"], json!("Int"));
    assert_eq!(b["symbol"], json!("t.rl.b"));
    assert_eq!(result.holes.len(), 1);
}

/// `spec/toolchain.md` 1.6 `HoleReport::dependencies`: every definition the
/// enclosing declaration references (calls, constructors, types), sorted by
/// qualified name; the declaration itself only when it is recursive (as
/// `Index::dependencies`, toolchain 6.5: a recursive definition is its own
/// dependency).
#[test]
fn hole_report_dependencies() {
    let src = r#"module t.hd

type PageId(Text)

struct Page {
    id: PageId
    text: Text
}

fn helper(n: Int) -> Int {
    n
}

fn f(id: PageId) -> Page {
    let _n = helper(1)
    Page { id: id, text: ?text }
}

fn count(n: Int) -> Int
    decreases n
{
    if n <= 0 {
        ?base
    } else {
        helper(count(n - 1)) + 1
    }
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[(line(src, "?text"), "hole"), (line(src, "?base"), "hole")],
    );
    let p = &result.program;
    let hole = |name: &str| {
        result
            .holes
            .iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("no hole {name}"))
    };
    let deps_of = |name: &str| -> Vec<String> {
        hole(name)
            .dependencies
            .iter()
            .map(|d| p.display_path(*d))
            .collect()
    };
    assert_eq!(hole("text").expected, Ty::Text);
    let deps = deps_of("text");
    for want in ["t.hd.helper", "t.hd.Page", "t.hd.PageId"] {
        assert!(deps.contains(&want.to_string()), "{want} in {deps:?}");
    }
    assert!(!deps.contains(&"t.hd.f".to_string()), "{deps:?}");
    // The recursive `count` lists itself, once, beside `helper`.
    let deps = deps_of("base");
    for want in ["t.hd.count", "t.hd.helper"] {
        assert_eq!(
            deps.iter().filter(|d| *d == want).count(),
            1,
            "{want} once in {deps:?}"
        );
    }
    let mut sorted = deps.clone();
    sorted.sort();
    assert_eq!(deps, sorted, "sorted by qualified name");
}

/// 3.12: a bare constructor pattern names a payload-less constructor; one
/// naming a variant or wrapper with a payload is `pattern_arity_mismatch`
/// (`expected` 1, `actual` 0), and the erroneous arm takes no part in the
/// exhaustiveness check (S62).
#[test]
fn bare_patterns_of_payload_constructors() {
    let src = r#"module t.np

type PageId(Text)

enum StoreError {
    Missing(Text)
    Unavailable
}

fn a(e: StoreError) -> Int {
    match e {
        Missing => 1 // #a
        Unavailable => 2
    }
}

fn b(id: PageId) -> Int {
    match id {
        PageId => 1 // #b
    }
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "pattern_arity_mismatch"),
            (mark(src, "b"), "pattern_arity_mismatch"),
        ],
    );
    for n in ["a", "b"] {
        assert_eq!(
            ea(&diag_at(&result, "pattern_arity_mismatch", mark(src, n))),
            (json!(1), json!(0)),
            "#{n}"
        );
    }
}

/// S62 with 3.5: a refinement whose predicate has its own error proves
/// nothing and reports nothing more where it is expected.
#[test]
fn erroneous_predicates_prove_nothing() {
    let src = r#"module t.ep

type Bad = Int where self + 1 // #where

fn a() -> Bad {
    5
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "where"), "type_mismatch")]);
}

/// S68 for an `else if` chain as a statement: `Unit` flows into every
/// branch of the chain, each non-`Unit` tail reported there.
#[test]
fn else_if_chain_statements() {
    let src = r#"module t.ei

fn a(x: Bool, y: Bool) -> Int {
    if x {
        1 // #one
    } else if y {
        2 // #two
    } else {
        3 // #three
    }
    0
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "one"), "type_mismatch"),
            (mark(src, "two"), "type_mismatch"),
            (mark(src, "three"), "type_mismatch"),
        ],
    );
}

/// S19 and S10 step 1: arguments are checked in order against the
/// parameters as instantiated so far, so a constructor after an argument
/// that fixed `T` resolves against it, and one before it is ambiguous.
#[test]
fn constructors_resolve_against_instantiated_parameters() {
    let src = r#"module t.ip

enum A {
    Missing(Int)
    Gone
}

enum B {
    Missing(Int)
    Gone
}

fn pick<T>(a: T, b: T) -> T {
    a
}

fn after() -> A {
    pick(A.Gone, Missing(1))
}

fn before() -> A {
    pick(Missing(1), A.Gone) // #before
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "before"), "ambiguous_constructor")]);
}

/// S62 and S14 for generator properties: `gen.int()` keeps the property's
/// own type `Gen<Int>`, so as a `std.gen` argument it is `property_called`
/// alone, and outside a `for` range it is also `generator_outside_for` (a
/// condition on the position); a generator where an `Int` is expected is
/// `generator_outside_for` alone (no `type_mismatch` from the error type).
#[test]
fn generator_properties_called_or_misplaced() {
    let src = r#"module t.gq

fn a() -> Int {
    let _g = gen.int() // #a
    1
}

fn b() -> Int {
    let n: Int = gen.int // #b
    n
}

property "called inside a combinator"
    for xs in gen.list(gen.int()) // #c
{
    assert xs == xs
}
"#;
    let result = check_source(src);
    let m = |n: &str| mark(src, n);
    expect(
        &result,
        &[
            (m("a"), "property_called"),
            (m("a"), "generator_outside_for"),
            (m("b"), "generator_outside_for"),
            (m("c"), "property_called"),
        ],
    );
    let p = &result.program;
    let DefKind::Property(prop) = &def(p, "t.gq.\"called inside a combinator\"").kind else {
        panic!("a property")
    };
    assert_eq!(p.display_ty(&prop.binders[0].elem_ty), "List<Int>");
}

/// 3.6 and 3.3: the right operand of `==`/`!=` is checked against erase(L),
/// which strips refinements at every position of type arguments (also of a
/// generic struct, whose fields are not entered), so a hole there takes the
/// erased type.
#[test]
fn holes_on_the_right_of_equality_take_the_erased_type() {
    let src = r#"module t.he

type Positive = Int where self > 0
type Big = Positive where self > 100

struct Pair<A, B> {
    left: A
    right: B
}

fn f(p: Pair<Positive, Text>, r: Result<Positive, Big>, o: Option<List<Big>>) -> Bool {
    let a = p != ?pair
    let b = r == ?res
    let c = o == ?opt
    a and b and c
}
"#;
    let result = check_source(src);
    let found: BTreeMap<String, String> = with_code(&result, "hole")
        .iter()
        .map(|d| {
            (
                d["hole"].as_str().unwrap_or_default().to_string(),
                d["expected_type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let want: BTreeMap<String, String> = [
        ("pair", "Pair<Int, Text>"),
        ("res", "Result<Int, Int>"),
        ("opt", "Option<List<Int>>"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(found, want, "\n{}", render(&result));
    assert_eq!(got(&result).len(), 3, "\n{}", render(&result));
}

/// S19: an `else if` chain passes the expected type unchanged into the
/// nested `if`, so a refinement is proved per branch there too; an `if` as
/// an operand checks its branches against the operand's type.
#[test]
fn expected_types_flow_through_else_if_chains() {
    let src = r#"module t.ef

type Positive = Int where self > 0

fn a(x: Bool, y: Bool) -> Positive {
    if x { 1 } else if y { 0 } else { 2 } // #a
}

fn b(size: Positive, flag: Bool) -> Int {
    1 + if flag { size } else { 2 }
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "a"), "refinement_false")]);
    assert_eq!(
        start(&diag_at(&result, "refinement_false", mark(src, "a"))),
        pos(src, "0 } else { 2 } // #a")
    );
    let p = &result.program;
    let ExprKind::Binary { rhs, .. } = &tail(body(p, "t.ef.b")).kind else {
        panic!("+")
    };
    let ExprKind::If { then_block, .. } = &rhs.kind else {
        panic!("an if operand")
    };
    assert_eq!(widens(p, tail(then_block)).0, ["t.ef.Positive"]);
}

/// S63 (1) and (5): the primary span of an expression diagnostic covers
/// exactly that expression (the hole, the call, the generator, the
/// pattern), a `non_exhaustive_match` the `match` up to its closing brace,
/// and an `unreachable_arm` the arm from its pattern to the end of its
/// body (`end_col` exclusive, 9.2).
#[test]
fn primary_spans_cover_the_expression() {
    let src = r#"module t.sp

fn two(a: Int, b: Int) -> Int {
    a + b
}

fn a() -> Int {
    ?tail_hole
}

fn b() -> Int {
    two(1) + 1
}

fn c() -> Int {
    let _g = gen.int
    1
}

fn d(t: Text) -> Int {
    match t {
        0 => 0
        _ => 1
    }
}

fn e(flag: Bool) -> Int {
    match flag {
        _ => 1
        true => 2
    }
}

fn f(o: Option<Int>) -> Int {
    match o {
        None => 0
    }
}
"#;
    let result = check_source(src);
    let span_of = |code: &str, needle: &str| {
        let (l, _) = pos(src, needle);
        let d = diag_at(&result, code, l as u32);
        (start(&d), end(&d))
    };
    let exact = |needle: &str| {
        let (l, c) = pos(src, needle);
        ((l, c), (l, c + needle.chars().count() as u64))
    };
    assert_eq!(span_of("hole", "?tail_hole"), exact("?tail_hole"));
    assert_eq!(span_of("arity_mismatch", "two(1)"), exact("two(1)"));
    assert_eq!(
        span_of("generator_outside_for", "gen.int"),
        exact("gen.int")
    );
    let (pl, pc) = pos(src, "0 => 0");
    assert_eq!(
        span_of("pattern_type_mismatch", "0 => 0"),
        ((pl, pc), (pl, pc + 1))
    );
    assert_eq!(span_of("unreachable_arm", "true => 2"), exact("true => 2"));
    let (ml, mc) = pos(src, "match o {");
    assert_eq!(
        span_of("non_exhaustive_match", "match o {"),
        ((ml, mc), (ml + 2, 6))
    );
}

/// 3.5 step 1: a fault while evaluating a constant that is being proved is
/// `constant_evaluation_fault` with the fault code, also inside a literal
/// form; a constant that no refinement expects is not evaluated at compile
/// time at all (its fault is a runtime matter, 6.7).
#[test]
fn constant_faults_only_where_a_proof_is_needed() {
    let src = r#"module t.cf

type Positive = Int where self > 0

fn a() -> Option<Positive> {
    Some(1 / 0) // #a
}

fn b() -> List<Positive> {
    [1, 7 % 0] // #b
}

fn c() -> Int {
    let x: Int = 1 / 0
    x
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "constant_evaluation_fault"),
            (mark(src, "b"), "constant_evaluation_fault"),
        ],
    );
    for n in ["a", "b"] {
        assert_eq!(
            diag_at(&result, "constant_evaluation_fault", mark(src, n))["fault"],
            json!("division_by_zero"),
            "#{n}"
        );
    }
}

/// S55: a diagnostic or report that names a refinement uses the declaration
/// written at the use site, so of two identical refinements the one the
/// parameter names is `expected` in `refinement_false` (with its own
/// predicate as `clause`) and the hole's `expected_type`.
#[test]
fn identical_refinements_render_as_written() {
    let src = r#"module t.rw

type Positive = Int where self > 0
type Count = Int where self > 0

fn takes_count(c: Count) -> Int {
    c
}

fn a() -> Int {
    takes_count(0) // #a
}

fn b() -> Int {
    takes_count(?h) // #b
}
"#;
    let result = check_source(src);
    expect(
        &result,
        &[
            (mark(src, "a"), "refinement_false"),
            (mark(src, "b"), "hole"),
        ],
    );
    let a = diag_at(&result, "refinement_false", mark(src, "a"));
    assert_eq!(a["expected"], json!("Count"));
    assert_eq!(a["clause"], json!("self > 0"));
    assert_eq!(
        diag_at(&result, "hole", mark(src, "b"))["expected_type"],
        json!("Count")
    );
}

/// S65 and S15: distinct refinements of one base below a type constructor
/// are not a proof question but invariance, so `List<Positive>` where
/// `List<Pos>` is expected is `type_mismatch` with the top-level note.
#[test]
fn nested_distinct_refinements_are_mismatches() {
    let src = r#"module t.nr

type Positive = Int where self > 0
type Pos = Int where self >= 1

fn a(ps: List<Positive>) -> List<Pos> {
    ps // #a
}
"#;
    let result = check_source(src);
    expect(&result, &[(mark(src, "a"), "type_mismatch")]);
    let d = diag_at(&result, "type_mismatch", mark(src, "a"));
    assert_eq!(ea(&d), (json!("List<Pos>"), json!("List<Positive>")));
    assert!(
        notes(&d).iter().any(|n| n.contains("top level")),
        "{:?}",
        notes(&d)
    );
}
