//! The body typer (`spec/semantics.md` 3.4-3.14; `spec/toolchain.md` 3.3,
//! 3.5, 6.5): the typed HIR's shapes (`Widen`/`Refine`, member calls of
//! T19, `Check`, `CapCall`, generators of T18), local types, references
//! with the T17 interface flag, the payloads of the type diagnostics, and
//! the S62 rule that a failed part causes no second diagnostic.

mod common;

use common::*;
use cpl_diag::Severity;
use cpl_hir::{
    Block, Def, DefKind, Expr, ExprKind, FnBody, GenExpr, Program, RefKind, RefinementProof, Stmt,
    Ty,
};
use cpl_sema::CheckResult;
use serde_json::json;

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
    b.tail.as_deref().expect("a tail")
}

fn errors(result: &CheckResult) -> Vec<(u32, String)> {
    triples(result)
        .into_iter()
        .filter(|t| t.1 == Severity::Error)
        .map(|t| (t.0, t.2))
        .collect()
}

fn reader() -> CheckResult {
    cpl_sema::check(&load_dir(&example("reader")))
}

#[test]
fn reader_hir_shapes() {
    let result = reader();
    assert!(!result.has_errors(), "{}", render(&result));
    let p = &result.program;
    let std = p.std_defs;
    let positive = def(p, "reader.pages.Positive").id;

    // paginate_ints: `check<Positive>(size)?` then `Ok(paginate(items, checked_size))`.
    let b = body(p, "reader.pages.paginate_ints");
    let Stmt::Let { value, .. } = &b.stmts[0] else {
        panic!("a let")
    };
    let ExprKind::Propagate(inner) = &value.kind else {
        panic!("{value:?}")
    };
    assert!(matches!(&inner.kind, ExprKind::Check { refined, .. } if *refined == positive));
    assert!(matches!(value.ty, Ty::Refined(..)));
    let ExprKind::Ctor {
        def: d,
        variant: 0,
        args,
    } = &tail(b).kind
    else {
        panic!("Ok(...)")
    };
    assert_eq!(*d, std.result);
    let ExprKind::Call {
        callee,
        generic_args,
        ..
    } = &args[0].kind
    else {
        panic!("a call")
    };
    assert_eq!(*callee, def(p, "reader.pages.paginate").id);
    assert_eq!(generic_args, &[Ty::Int]);

    // The test's literal `2` is proved `Positive` (S16, T39).
    let t = body(p, "tests.pagination.\"pagination preserves a short tail\"");
    let Stmt::Assert { expr, text } = &t.stmts[0] else {
        panic!("an assert")
    };
    assert_eq!(text, "paginate([1, 2, 3], 2) == [[1, 2], [3]]");
    let ExprKind::Binary { lhs, .. } = &expr.kind else {
        panic!("==")
    };
    let ExprKind::Call { args, .. } = &lhs.kind else {
        panic!("paginate(...)")
    };
    assert!(matches!(
        &args[1].kind,
        ExprKind::Refine { refined, proof: RefinementProof::Constant, .. } if *refined == positive
    ));
    assert!(matches!(args[1].ty, Ty::Refined(_, named) if named == positive));

    // The property's binders (T18).
    let DefKind::Property(prop) = &def(p, "tests.pagination.\"pagination preserves input\"").kind
    else {
        panic!("a property")
    };
    assert_eq!(prop.binders.len(), 2);
    assert_eq!(prop.binders[0].gen, GenExpr::List(Box::new(GenExpr::Int)));
    assert_eq!(prop.binders[0].elem_ty, Ty::list(Ty::Int));
    assert_eq!(prop.binders[1].gen, GenExpr::PositiveInt);
    // `gen.positive_int`'s binder is the class every `Int where self > 0`
    // shares (S55), named by `std.gen.Positive` at its use site.
    match &prop.binders[1].elem_ty {
        Ty::Refined(class, named) => {
            assert!(p.refinement_classes[class.0 as usize]
                .members
                .contains(&positive));
            assert_eq!(*named, std.positive);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(prop.locals[1].ty, prop.binders[1].elem_ty);

    // load_text forwards its handle: `load_page(id, using db)`.
    let b = body(p, "reader.pages.load_text");
    let Stmt::Let { value, .. } = &b.stmts[0] else {
        panic!("a let")
    };
    let ExprKind::Propagate(call) = &value.kind else {
        panic!("?")
    };
    let ExprKind::Call { using, .. } = &call.kind else {
        panic!("a call")
    };
    assert!(matches!(using[0].kind, ExprKind::Local(_)));
    assert!(matches!(using[0].ty, Ty::Capability(_)));
    // load_page calls the operation: a CapCall on the handle.
    let b = body(p, "reader.pages.load_page");
    assert!(matches!(&tail(b).kind, ExprKind::CapCall { op: 0, .. }));

    // The refinement's predicate is typed, and so is its class's (S55).
    let rd = def(p, "reader.pages.Positive")
        .kind
        .as_refinement()
        .expect("a refinement");
    assert!(matches!(rd.predicate.expr.kind, ExprKind::Binary { .. }));
    assert!(p
        .refinement_classes
        .iter()
        .all(|c| !matches!(c.predicate.kind, ExprKind::Error)));
}

#[test]
fn widening_members_and_fields() {
    let result = check_source(
        "module m.w\n\npub type Positive = Int where self > 0\npub type Big = Positive where self > 100\n\npub struct Page {\n    text: Text\n}\n\nfn f(b: Big, p: Positive, xs: List<Int>, page: Page) -> Int {\n    let a: Int = b\n    let c = p.to_text().len\n    let d = xs.len + page.text.len\n    a + c + d\n}\n",
    );
    assert!(!result.has_errors(), "{}", render(&result));
    let p = &result.program;
    let big = def(p, "m.w.Big").id;
    let positive = def(p, "m.w.Positive").id;
    let b = body(p, "m.w.f");
    // `let a: Int = b`: Big -> Positive -> Int, one Widen per step (3.4 rule 2).
    let Stmt::Let { value, .. } = &b.stmts[0] else {
        panic!("a let")
    };
    let ExprKind::Widen { expr, from } = &value.kind else {
        panic!("{value:?}")
    };
    assert_eq!(*from, positive);
    assert_eq!(value.ty, Ty::Int);
    assert!(matches!(&expr.kind, ExprKind::Widen { from, .. } if *from == big));
    // `p.to_text().len`: the receiver is widened to its root base (T19).
    let Stmt::Let { value, .. } = &b.stmts[1] else {
        panic!("a let")
    };
    let ExprKind::Call { callee, args, .. } = &value.kind else {
        panic!("len")
    };
    assert_eq!(p.display_path(*callee), "std.prelude.Text.len");
    let ExprKind::Call {
        callee,
        args: inner,
        ..
    } = &args[0].kind
    else {
        panic!("to_text")
    };
    assert_eq!(p.display_path(*callee), "std.prelude.Int.to_text");
    assert!(matches!(inner[0].kind, ExprKind::Widen { .. }));
    // `page.text.len`: a Field, then the Text member.
    let Stmt::Let { value, .. } = &b.stmts[2] else {
        panic!("a let")
    };
    let ExprKind::Binary { rhs, .. } = &value.kind else {
        panic!("+")
    };
    let ExprKind::Call { args, .. } = &rhs.kind else {
        panic!("len")
    };
    assert!(matches!(args[0].kind, ExprKind::Field { index: 0, .. }));
    // Locals carry their inferred types (3.7).
    let f = def(p, "m.w.f").kind.as_fn().expect("a fn");
    let types: Vec<(String, String)> = f
        .locals
        .iter()
        .map(|l| (l.name.clone(), p.display_ty(&l.ty)))
        .collect();
    assert!(
        types.contains(&("c".to_string(), "Int".to_string())),
        "{types:?}"
    );
    assert!(
        types.contains(&("a".to_string(), "Int".to_string())),
        "{types:?}"
    );
}

#[test]
fn references_carry_kind_and_interface() {
    let result = check_source(
        "module m.r\n\npub type Positive = Int where self > 0\n\npub struct Page {\n    text: Text\n}\n\npub capability Store {\n    fn get(key: Text) -> Int\n}\n\nfn helper(n: Int) -> Bool {\n    n > 0\n}\n\nfn f(page: Page, n: Int, using db: Store) -> Result<Positive, ConstraintError>\n    requires helper(n)\n    effects { db.get }\n{\n    let _ = db.get(page.text)\n    let _ = Some(1)\n    check<Positive>(n)\n}\n",
    );
    assert!(!result.has_errors(), "{}", render(&result));
    let p = &result.program;
    let f = def(p, "m.r.f").id;
    let refs: Vec<(String, RefKind, bool)> = p
        .references_from(f)
        .iter()
        .filter(|r| r.kind != RefKind::TypeUse)
        .map(|r| (p.display_path(r.to), r.kind, r.interface))
        .collect();
    assert!(
        refs.contains(&("m.r.helper".to_string(), RefKind::Call, true)),
        "{refs:?}"
    );
    assert!(
        refs.contains(&("m.r.Store".to_string(), RefKind::CapabilityOp, false)),
        "{refs:?}"
    );
    assert!(
        refs.contains(&("m.r.Page".to_string(), RefKind::FieldAccess, false)),
        "{refs:?}"
    );
    assert!(
        refs.contains(&("std.prelude.Option".to_string(), RefKind::Ctor, false)),
        "{refs:?}"
    );
    assert!(
        refs.contains(&("m.r.Positive".to_string(), RefKind::Check, false)),
        "{refs:?}"
    );
}

#[test]
fn diagnostic_payloads() {
    let result = check_source(
        "module m.d\n\npub type Positive = Int where self > 0\npub type Nz = Int where 10 / self > 0\n\nfn a() -> Int {\n    let _x = []\n    let _y: Text = 7\n    let _p: Positive = 0\n    let _q: Nz = 0\n    1\n}\n\nfn b(o: Option<Bool>, n: Int) -> Int {\n    let _p: Positive = n\n    match o {\n        Some(true) => 1\n        None => 2\n    }\n}\n",
    );
    let j = json_values(&result);
    let find = |code: &str| {
        j.iter()
            .find(|d| d["code"] == code)
            .unwrap_or_else(|| panic!("no {code}: {}", render(&result)))
    };
    assert_eq!(find("cannot_infer_type")["type"], "List<?>");
    let m = find("type_mismatch");
    assert_eq!(
        (&m["expected"], &m["actual"]),
        (&json!("Text"), &json!("Int"))
    );
    let f = find("refinement_false");
    assert_eq!(f["expected"], "Positive");
    assert_eq!(f["actual"], json!(0));
    assert_eq!(f["clause"], "self > 0");
    let c = find("constant_evaluation_fault");
    assert_eq!(c["fault"], "division_by_zero");
    assert_eq!(c["clause"], "10 / self > 0");
    let u = find("cannot_prove_refinement");
    assert_eq!(
        (&u["expected"], &u["actual"]),
        (&json!("Positive"), &json!("Int"))
    );
    assert!(u["notes"][0]["message"]
        .as_str()
        .unwrap()
        .contains("check<Positive>"));
    let n = find("non_exhaustive_match");
    assert_eq!(n["missing"], json!(["Some(false)"]));
    assert_eq!(n["type"], "Option<Bool>");
}

/// S65: two refinements of one base that render alike are named by their
/// qualified names, and the note quotes both predicates.
#[test]
fn distinct_refinements_are_qualified() {
    let result = check_source(
        "module m.s\n\npub type Positive = Int where 0 < self\n\nfn take(p: Positive) -> Int {\n    p\n}\n\nproperty \"p\"\n    for size in gen.positive_int\n{\n    assert take(size) > 0\n}\n",
    );
    let j = json_values(&result);
    let d = j
        .iter()
        .find(|d| d["code"] == "cannot_prove_refinement")
        .unwrap_or_else(|| panic!("{}", render(&result)));
    assert_eq!(d["expected"], "m.s.Positive");
    assert_eq!(d["actual"], "std.gen.Positive");
    let note = d["notes"][0]["message"].as_str().unwrap();
    assert!(
        note.contains("self > 0") && note.contains("0 < self"),
        "{note}"
    );
    assert!(note.contains("check<Positive>(size)"), "{note}");
}

/// S62: a failed operand, argument, or initializer causes no second
/// diagnostic.
#[test]
fn errors_do_not_cascade() {
    let result = check_source(
        "module m.e\n\nfn a(xs: List<Int>) -> List<Int> {\n    flatten(xs)\n}\n\nfn b() -> Text {\n    \"a\" + \"b\"\n}\n\nfn c() -> Int {\n    let n: Text = 7\n    let m = undefined_thing\n    let _ = m.len + n.len\n    1\n}\n\nfn d() -> Int {\n    let _x: Option<Int> = Ok(1)\n    1\n}\n",
    );
    assert_eq!(
        errors(&result),
        [
            (4, "type_mismatch".to_string()),
            (8, "type_mismatch".to_string()),
            (12, "type_mismatch".to_string()),
            (13, "unresolved_name".to_string()),
            (19, "type_mismatch".to_string()),
        ],
        "{}",
        render(&result)
    );
}

/// Inference variables are scoped to their statement (3.7): later uses do
/// not resolve them, and the expected type flows into calls after their
/// arguments (S19).
#[test]
fn inference_is_statement_local_and_bidirectional() {
    let result = check_source(
        "module m.i\n\nfn id<T>(x: T) -> T {\n    x\n}\n\nfn a() -> Int {\n    let xs = []\n    let _ = xs.push(1)\n    1\n}\n\nfn b() -> List<Int> {\n    let ys: List<Int> = flatten([])\n    let o: Option<Int> = id(None)\n    ys.push(o.unwrap_or(0))\n}\n",
    );
    assert_eq!(
        errors(&result),
        [(8, "cannot_infer_type".to_string())],
        "{}",
        render(&result)
    );
}

/// The release mode does not change typing (S47: only holes and the
/// verification policy differ).
#[test]
fn release_mode_types_alike() {
    let source = "module m.r\n\nfn f(n: Int) -> Text {\n    n\n}\n";
    let dev = check_source(source);
    let rel = check_source_with(
        source,
        &cpl_sema::CheckConfig {
            release: true,
            ..cpl_sema::CheckConfig::default()
        },
    );
    let type_codes = |r: &CheckResult| -> Vec<String> {
        r.diagnostics
            .iter()
            .filter(|d| d.category.as_str() == "type")
            .map(|d| d.code.0.to_string())
            .collect()
    };
    assert_eq!(type_codes(&dev), ["type_mismatch"]);
    assert_eq!(type_codes(&dev), type_codes(&rel));
}

/// A `using` argument that is not a bare name of a handle is
/// `using_argument_not_capability` (S28); the typer reports it because
/// only the syntax tells a bare name from other expressions.
#[test]
fn using_arguments() {
    let result = check_source(
        "module m.u\n\npub capability Store {\n    fn get(key: Text) -> Int\n}\n\nfn read(key: Text, using s: Store) -> Int\n    effects { s.get }\n{\n    s.get(key)\n}\n\nfn a(key: Text, using db: Store) -> Int\n    effects { db.get }\n{\n    read(key, using key) + read(key, using 5) + read(key, using db)\n}\n",
    );
    let got: Vec<(u32, String)> = triples(&result).into_iter().map(|t| (t.0, t.2)).collect();
    assert_eq!(
        got,
        [
            (16, "using_argument_not_capability".to_string()),
            (16, "using_argument_not_capability".to_string()),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[0]["name"], "key");
    assert_eq!(j[0]["type"], "Text");
    assert_eq!(j[1]["name"], "5");
}

/// Every `Ty::Error` and `ExprKind::Error` in the HIR of `d`, as messages.
fn error_nodes(p: &Program, d: &Def) -> Vec<String> {
    fn has_error(t: &Ty) -> bool {
        match t {
            Ty::Error => true,
            Ty::List(e) | Ty::Gen(e) => has_error(e),
            Ty::Adt(_, args) => args.iter().any(has_error),
            _ => false,
        }
    }
    let mut out = Vec::new();
    let mut check = |e: &Expr| {
        if matches!(e.kind, ExprKind::Error) || has_error(&e.ty) {
            let lc = p.source_map().line_col(e.span.file, e.span.start);
            out.push(format!("{}:{}: {:?}", lc.line, lc.col, e.kind));
        }
    };
    match &d.kind {
        DefKind::Fn(f) => {
            for c in f.requires.iter().chain(&f.ensures).chain(&f.decreases) {
                c.expr.walk(&mut check);
            }
            if let FnBody::Block(b) = &f.body {
                b.walk(&mut check);
            }
        }
        DefKind::Test(t) => t.body.walk(&mut check),
        DefKind::Property(pr) => {
            for b in &pr.binders {
                b.gen.walk(&mut check);
            }
            pr.body.walk(&mut check);
        }
        DefKind::Refinement(r) => r.predicate.expr.walk(&mut check),
        DefKind::Capability(c) => {
            for op in &c.ops {
                for cl in op.requires.iter().chain(&op.ensures) {
                    cl.expr.walk(&mut check);
                }
            }
        }
        _ => {}
    }
    for l in d.kind.locals() {
        if has_error(&l.ty) {
            out.push(format!("local `{}` has an error type", l.name));
        }
    }
    out
}

fn assert_complete(result: &CheckResult, what: &str) {
    let p = &result.program;
    for d in p.defs() {
        if d.status != cpl_hir::DefStatus::Ok {
            continue;
        }
        let bad = error_nodes(p, d);
        assert!(
            bad.is_empty(),
            "{what}: {} has error nodes: {bad:?}",
            p.display_path(d.id)
        );
    }
}

/// A definition that checked without errors has a complete HIR: no
/// `ExprKind::Error` and no `Ty::Error` anywhere (the evaluator executes
/// it), over the examples, the positive corpus, and std.
#[test]
fn checked_definitions_have_complete_hir() {
    for name in ["reader", "holes"] {
        let result = cpl_sema::check(&load_dir(&example(name)));
        assert!(!result.has_errors(), "{}", render(&result));
        assert_complete(&result, name);
    }
    let ok = corpus().join("ok");
    let mut files: Vec<_> = std::fs::read_dir(&ok)
        .expect("the ok corpus")
        .map(|e| e.expect("an entry").path())
        .collect();
    files.sort();
    for path in files {
        let result = if path.is_dir() {
            cpl_sema::check(&load_dir(&path))
        } else if path.extension().is_some_and(|e| e == "cpl") {
            check_source(&std::fs::read_to_string(&path).expect("a corpus file"))
        } else {
            continue;
        };
        let what = path.display().to_string();
        assert!(!result.has_errors(), "{what}: {}", render(&result));
        assert_complete(&result, &what);
    }
}

/// S24, 3.10: a tail is an expression statement too, so a tail after a
/// diverging statement is `unreachable_statement`, spanning the tail
/// (S63 (5)), still once per block (a block that already warned at a
/// statement does not warn at its tail), in nested blocks as well; the
/// tail is typed as usual and the block's type is its type.
#[test]
fn an_unreachable_tail_is_an_unreachable_statement() {
    let src = "module t.ut

fn a() -> Int {
    return 1
    2
}

fn b(c: Bool) -> Int {
    if c {
        return 1
    } else {
        return 2
    }
    3
}

fn c(n: Int) -> Int {
    return n
    let _ = n
    n + 1
}

fn d(o: Option<Int>) -> Int {
    match o {
        Some(v) => {
            return v
            0
        }
        None => 1
    }
}
";
    let result = check_source(src);
    let line = |needle: &str| {
        src.lines()
            .position(|l| l.trim() == needle)
            .unwrap_or_else(|| panic!("no line {needle:?}")) as u32
            + 1
    };
    assert_eq!(
        triples(&result),
        [
            (
                line("2"),
                Severity::Warning,
                "unreachable_statement".to_string()
            ),
            (
                line("3"),
                Severity::Warning,
                "unreachable_statement".to_string()
            ),
            (
                line("let _ = n"),
                Severity::Warning,
                "unreachable_statement".to_string()
            ),
            (
                line("0"),
                Severity::Warning,
                "unreachable_statement".to_string()
            ),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    assert_eq!(j[0]["span"]["col"], json!(5), "the tail `2`");
    assert_eq!(j[0]["span"]["end_col"], json!(6));
    let p = &result.program;
    let a = body(p, "t.ut.a");
    assert!(matches!(a.stmts.as_slice(), [Stmt::Return(Some(_))]));
    assert!(matches!(tail(a).kind, ExprKind::Lit(_)));
    assert_eq!(a.ty, Ty::Int);
}

/// 3.10, 3.11 with 3.5 (S16): the `Unit` that a missing tail or a bare
/// `return` stands for is checked against the expected type like the
/// constant expression `Unit`: at a refinement of `Unit` the proof rule
/// decides, so `where false` is `refinement_false` (at the `return`;
/// spanning the block for a missing tail, as the mismatch does, S63 (1)),
/// a faulting predicate is `constant_evaluation_fault`, a predicate
/// outside the evaluable subset is `cannot_prove_refinement`, and a true
/// chain accepts it (`if` branches included); for any other type rule 4
/// keeps `type_mismatch` with the note and `return_value_missing`. The
/// HIR keeps the operand absent (`Return(None)`, no tail), and a proved
/// missing tail gives the block the refinement type.
#[test]
fn missing_unit_operands_are_proved_against_refinements_of_unit() {
    let src = r#"module t.mu

type Never = Unit where false
type Done = Unit where true
type Deep = Done where self == Unit
type Faulty = Unit where 1 / 0 == 0
type Called = Unit where truth()

fn truth() -> Bool {
    true
}

fn a() -> Never {
    return // #a
}

fn b() -> Never { // #b
}

fn c(n: Int) -> Done {
    if n > 0 {
        return
    }
    let _ = n
}

fn d(flag: Bool) -> Deep {
    if flag {
        let _ = 1
    } else {
        let _ = 2
    }
}

fn e() -> Faulty { // #e
}

fn f() -> Called {
    return // #f
}

fn g() -> Int { // #g
}

fn h() -> Int {
    return // #h
}
"#;
    let result = check_source(src);
    let m = |name: &str| {
        let needle = format!("// #{name}");
        src.lines()
            .position(|l| l.contains(&needle))
            .unwrap_or_else(|| panic!("no mark {name}")) as u32
            + 1
    };
    let err = |line: u32, code: &str| (line, Severity::Error, code.to_string());
    assert_eq!(
        triples(&result),
        [
            err(m("a"), "refinement_false"),
            err(m("b"), "refinement_false"),
            err(m("e"), "constant_evaluation_fault"),
            err(m("f"), "cannot_prove_refinement"),
            err(m("g"), "type_mismatch"),
            err(m("h"), "return_value_missing"),
        ],
        "{}",
        render(&result)
    );
    let j = json_values(&result);
    for d in &j[..2] {
        assert_eq!(d["expected"], json!("Never"));
        assert_eq!(d["actual"], json!({ "$type": "Unit" }));
        assert_eq!(d["clause"], json!("false"));
    }
    // The bare `return` spans the statement; the missing tail the block.
    assert_eq!(
        (j[0]["span"]["col"].clone(), j[0]["span"]["end_col"].clone()),
        (json!(5), json!(11))
    );
    assert_eq!(j[1]["span"]["line"], json!(m("b")));
    assert_eq!(j[1]["span"]["col"], json!(17), "from the body's `{{`");
    assert_eq!(j[1]["span"]["end_line"], json!(m("b") + 1), "to its `}}`");
    assert_eq!(j[2]["fault"], json!("division_by_zero"));
    assert_eq!(j[2]["clause"], json!("1 / 0 == 0"));
    assert_eq!(j[3]["expected"], json!("Called"));
    assert_eq!(j[3]["actual"], json!("Unit"));
    let notes = |d: &serde_json::Value| -> Vec<String> {
        d["notes"]
            .as_array()
            .map(|ns| {
                ns.iter()
                    .map(|n| n["message"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(
        notes(&j[3]).iter().any(|n| n.contains("check<Called>")),
        "{:?}",
        notes(&j[3])
    );
    assert!(notes(&j[4]).iter().any(|n| n == "missing tail expression"));
    assert_eq!(j[5]["expected"], json!("Int"));
    // HIR: the operand stays absent; a proved missing tail has the type.
    let p = &result.program;
    let a = body(p, "t.mu.a");
    assert!(matches!(a.stmts.as_slice(), [Stmt::Return(None)]));
    assert!(a.tail.is_none());
    let c = body(p, "t.mu.c");
    assert!(c.tail.is_none());
    assert_eq!(p.display_ty(&c.ty), "Done");
    assert!(matches!(c.stmts[0], Stmt::Expr(_)));
    let d = body(p, "t.mu.d");
    assert_eq!(p.display_ty(&d.ty), "Deep");
    match &tail(d).kind {
        ExprKind::If {
            then_block,
            else_block: Some(else_block),
            ..
        } => {
            for branch in [then_block, else_block] {
                assert!(branch.tail.is_none());
                assert_eq!(p.display_ty(&branch.ty), "Deep");
            }
        }
        other => panic!("{other:?}"),
    }
    for name in ["t.mu.c", "t.mu.d"] {
        assert_eq!(def(p, name).status, cpl_hir::DefStatus::Ok, "{name}");
    }
}
