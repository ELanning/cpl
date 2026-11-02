//! Adversarial tests of cpl-eval's dynamic semantics. Every expectation is
//! the specification's answer, not the implementation's current behaviour:
//!
//! | Area | Specification |
//! | - | - |
//! | `Int` arithmetic: every sign combination, operands far beyond 64 bits, division by zero | semantics 6.3, S37; stdlib 2.2, S8, 12; toolchain T9 |
//! | structural equality; values built by different operations | semantics 6.1, 6.4, 6.5 |
//! | strict left-to-right evaluation of every row of the 6.2 table | semantics 6.2, 5.2 step 1 |
//! | every row of the contract-checking table: `requires` order and short-circuit, `ensures` on every exit, invariants at entry and after each iteration, loop and header `decreases` with both failure modes, faults inside clauses | toolchain 1.7.1; semantics 5.2-5.4, 5.6, S33-S35, S57, 6.10 |
//! | fault payload keys, categories, `status`, no `null` | toolchain 1.7.3; semantics 6.7, 9.2, 9.4, 9.5 |
//! | value JSON and value text of every type form: huge `Int`, Unicode, nested `Option`, round trips | toolchain 7.3, 7.2; semantics S40 |
//! | the call-depth and time budgets | semantics S38; toolchain 1.7 |
//!
//! The programs are built by hand with `cpl_eval::testing`, in the forms the
//! checker gives them (a negative literal is a negation, a refined operand
//! of `<` is widened); clause and assertion texts are the canonical texts
//! the checker records. A test whose expectation depends on what only the
//! checker decides (a clause's span from its keyword) checks CPL source with
//! cpl-sema (a dev-dependency, toolchain 1.2). Where a specification is
//! silent, the test follows `design-doc.md` and says so in a comment.

mod common;

use cpl_diag::json::{BindingKind, TestStatus};
use cpl_diag::Span;
use cpl_eval::json::{value_from_json, value_text, value_to_json};
use cpl_eval::stack::on_eval_stack;
use cpl_eval::testing::{FnBuilder, Hir, ProgramBuilder, PropertyBuilder};
use cpl_eval::{
    run_function, run_test, CapabilityImpl, EvalConfig, Fault, HostContext, Interpreter, RunArg,
    RunOutcome, TestConfig, Value,
};
use cpl_hir::{
    Block, Clause, DefId, DefKind, Expr, GenExpr, LocalId, ModuleId, PatternKind, Program, Stmt, Ty,
};
use cpl_syntax::ast::BinaryOp::{self, *};
use cpl_syntax::ast::UnaryOp;
use num_bigint::{BigInt, Sign};
use num_traits::Zero;
use serde_json::{json, Value as Json};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

// ===========================================================================
// Support
// ===========================================================================

/// The file of module `m`: 80 lines of 60 characters, so that spans set
/// with [`at`] resolve to distinct lines and columns.
const FILE: &str = "src/m.cpl";
const WIDTH: u32 = 60;

fn module(b: &mut ProgramBuilder, path: &str) -> (ModuleId, Hir) {
    let text = format!("{}\n", "x".repeat(WIDTH as usize)).repeat(80);
    let m = b.module_with_text(path, &text);
    (m, b.hir(m))
}

/// Line `line`, columns `col..col + len` (1-based) of a [`module`] file.
fn at(h: &Hir, line: u32, col: u32, len: u32) -> Span {
    let start = (line - 1) * (WIDTH + 1) + (col - 1);
    h.at(start, start + len)
}

/// The S49 `span` object of [`at`] in `src/m.cpl`.
fn loc(line: u32, col: u32, len: u32) -> Json {
    json!({"file": FILE, "line": line, "col": col, "end_line": line, "end_col": col + len})
}

fn spanned(mut e: Expr, span: Span) -> Expr {
    e.span = span;
    e
}

fn clause_at(h: &Hir, e: Expr, text: &str, span: Span) -> Clause {
    Clause {
        span,
        ..h.clause(e, text)
    }
}

fn iv(h: &Hir, l: LocalId) -> Expr {
    h.local(l, Ty::Int)
}

fn bin(h: &Hir, op: BinaryOp, a: Expr, b: Expr) -> Expr {
    h.binary(op, a, b)
}

fn neg(h: &Hir, e: Expr) -> Expr {
    h.unary(UnaryOp::Neg, e)
}

fn not(h: &Hir, e: Expr) -> Expr {
    h.unary(UnaryOp::Not, e)
}

fn let_(local: LocalId, value: Expr) -> Stmt {
    Stmt::Let { local, value }
}

fn assign(local: LocalId, value: Expr) -> Stmt {
    Stmt::Assign { local, value }
}

fn ret(e: Expr) -> Stmt {
    Stmt::Return(Some(e))
}

fn stmt(e: Expr) -> Stmt {
    Stmt::Expr(e)
}

fn assert_(e: Expr, text: &str) -> Stmt {
    Stmt::Assert {
        expr: e,
        text: text.to_string(),
    }
}

fn while_(cond: Expr, invariants: Vec<Clause>, decreases: Option<Clause>, body: Block) -> Stmt {
    Stmt::While {
        cond,
        invariants,
        decreases,
        body,
    }
}

fn var_int(f: &mut FnBuilder, name: &str) -> LocalId {
    f.local(name, Ty::Int, BindingKind::Var, true)
}

fn let_int(f: &mut FnBuilder, name: &str) -> LocalId {
    f.local(name, Ty::Int, BindingKind::Let, false)
}

/// A function `name` with positional parameters `params`; `body` gets the
/// builder (for clauses and further locals) and the parameters' locals.
fn define(
    b: &mut ProgramBuilder,
    m: ModuleId,
    name: &str,
    params: &[(&str, Ty)],
    ret: Ty,
    body: impl FnOnce(&mut FnBuilder, &[LocalId]) -> Block,
) -> DefId {
    let mut f = FnBuilder::new(name, ret);
    let locals: Vec<LocalId> = params.iter().map(|(n, t)| f.param(n, t.clone())).collect();
    let block = body(&mut f, &locals);
    f.body(block);
    b.add_fn(m, f)
}

/// `fn <name>(a: Int, b: Int) -> <ret> { <body> }`.
fn fn2(
    b: &mut ProgramBuilder,
    m: ModuleId,
    h: &Hir,
    name: &str,
    ret: Ty,
    body: impl FnOnce(&Hir, LocalId, LocalId) -> Expr,
) -> DefId {
    define(
        b,
        m,
        name,
        &[("a", Ty::Int), ("b", Ty::Int)],
        ret,
        |_, ps| h.value(body(h, ps[0], ps[1])),
    )
}

/// `fn <name>() -> <ty> { assert false  <tail> }`: calling it raises
/// `assertion_failed` whose `clause` is the upper-cased name, so a test can
/// tell which of several operands was evaluated first.
fn bomb(b: &mut ProgramBuilder, m: ModuleId, h: &Hir, name: &str, ty: Ty, tail: Expr) -> DefId {
    let tag = name.to_uppercase();
    define(b, m, name, &[], ty, |_, _| {
        h.block(vec![assert_(h.bool(false), &tag)], Some(tail))
    })
}

// ---- JSON --------------------------------------------------------------------

/// A JSON number with every digit (`serde_json` is built with
/// `arbitrary_precision`, toolchain 1.3).
fn num(s: &str) -> Json {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("not a JSON number {s}: {e}"))
}

fn big(s: &str) -> BigInt {
    s.parse().unwrap_or_else(|_| panic!("not an integer: {s}"))
}

fn int_json(n: &BigInt) -> Json {
    num(&n.to_string())
}

fn some(v: Json) -> Json {
    json!({"$type": "Option", "$variant": "Some", "$payload": [v]})
}

fn none() -> Json {
    json!({"$type": "Option", "$variant": "None"})
}

fn ok_v(v: Json) -> Json {
    json!({"$type": "Result", "$variant": "Ok", "$payload": [v]})
}

fn err_v(v: Json) -> Json {
    json!({"$type": "Result", "$variant": "Err", "$payload": [v]})
}

fn unit_v() -> Json {
    json!({"$type": "Unit"})
}

fn bytes_v(hex: &str) -> Json {
    json!({"$type": "Bytes", "hex": hex})
}

/// S49: optional fields are omitted when absent, never `null`.
fn assert_no_null(j: &Json, path: &str) {
    match j {
        Json::Null => panic!("null at {path}"),
        Json::Array(items) => {
            for (i, x) in items.iter().enumerate() {
                assert_no_null(x, &format!("{path}/{i}"));
            }
        }
        Json::Object(m) => {
            for (k, x) in m {
                assert_no_null(x, &format!("{path}/{k}"));
            }
        }
        _ => {}
    }
}

// ---- evaluation ------------------------------------------------------------------

/// A fault as its S49 diagnostic object.
fn diag(p: &Program, fault: &Fault) -> Json {
    serde_json::to_value(fault.to_diagnostic(p).to_json(p.source_map()))
        .expect("a serializable diagnostic")
}

/// The qualified names of a diagnostic's backtrace, innermost first.
fn backtrace(d: &Json) -> Vec<String> {
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

/// Decode `args` against `f`'s parameters and call `f` on this thread.
fn call_here(p: &Program, config: &EvalConfig, f: DefId, args: &[Json]) -> Result<Json, Fault> {
    let fd = p.def(f).kind.as_fn().expect("a function");
    assert_eq!(args.len(), fd.params.len(), "{}", p.display_path(f));
    let values = args
        .iter()
        .zip(&fd.params)
        .map(|(j, param)| {
            value_from_json(p, j, &param.ty)
                .unwrap_or_else(|e| panic!("argument {j} of {}: {e}", p.display_path(f)))
        })
        .collect();
    let mut it = Interpreter::new(p, config.clone());
    it.call(f, values, Vec::new())
        .map(|v| value_to_json(p, &v, &fd.ret))
}

/// [`call_here`] on the large evaluation stack.
fn eval_with(p: &Program, config: &EvalConfig, f: DefId, args: &[Json]) -> Result<Json, Fault> {
    on_eval_stack(|| call_here(p, config, f, args))
}

/// The value JSON of a call that must return.
fn value(p: &Program, f: DefId, args: &[Json]) -> Json {
    eval_with(p, &EvalConfig::default(), f, args)
        .unwrap_or_else(|e| panic!("{} faulted: {}", p.display_path(f), diag(p, &e)))
}

/// The diagnostic of a call that must fault.
fn fault_with(p: &Program, config: &EvalConfig, f: DefId, args: &[Json]) -> Json {
    match eval_with(p, config, f, args) {
        Ok(v) => panic!("{} returned {v} instead of faulting", p.display_path(f)),
        Err(e) => diag(p, &e),
    }
}

fn fault(p: &Program, f: DefId, args: &[Json]) -> Json {
    fault_with(p, &EvalConfig::default(), f, args)
}

/// Many calls of `f` on one evaluation thread: the value JSON or the
/// diagnostic of each.
fn batch(p: &Program, f: DefId, rows: &[Vec<Json>]) -> Vec<Result<Json, Json>> {
    on_eval_stack(|| {
        rows.iter()
            .map(|args| call_here(p, &EvalConfig::default(), f, args).map_err(|e| diag(p, &e)))
            .collect()
    })
}

/// What calling the nullary `f` did: `{"value": v}`, `{"bomb": "A"}` for a
/// [`bomb`]'s assertion, or `{"fault": code}`.
fn outcome_of(p: &Program, f: DefId) -> Json {
    match eval_with(p, &EvalConfig::default(), f, &[]) {
        Ok(v) => json!({ "value": v }),
        Err(e) => {
            let d = diag(p, &e);
            if d["code"] == "assertion_failed" {
                json!({"bomb": d["clause"]})
            } else {
                json!({"fault": d["code"]})
            }
        }
    }
}

fn bombed(tag: &str) -> Json {
    json!({ "bomb": tag })
}

fn returned(v: Json) -> Json {
    json!({ "value": v })
}

fn faulted(code: &str) -> Json {
    json!({ "fault": code })
}

fn depth(limit: u32) -> EvalConfig {
    EvalConfig {
        max_call_depth: limit,
        ..EvalConfig::default()
    }
}

fn budget(ms: u64) -> EvalConfig {
    EvalConfig {
        time_budget: Some(Duration::from_millis(ms)),
        ..EvalConfig::default()
    }
}

/// `run_function` on `f` that must not be refused.
fn run(p: &Program, f: DefId, args: Vec<RunArg>, config: &EvalConfig) -> RunOutcome {
    run_function(p, &p.symbol_path(f), &args, config)
        .unwrap_or_else(|e| panic!("{} was refused: {e}", p.display_path(f)))
}

fn c(s: &str) -> RunArg {
    RunArg::ConstExpr(s.to_string())
}

// ===========================================================================
// 1. Int arithmetic (semantics 6.3, S37; stdlib 2.2, S8, 12)
// ===========================================================================

/// Operands of every sign and magnitude: zero, small values, the `i64`
/// bounds, `2^64`, `2^100 + 1`, and `2^200 + 7`.
const SAMPLES: [&str; 17] = [
    "0",
    "1",
    "-1",
    "2",
    "-2",
    "3",
    "-3",
    "7",
    "-7",
    "9223372036854775807",
    "-9223372036854775808",
    "18446744073709551616",
    "-18446744073709551616",
    "1267650600228229401496703205377",
    "-1267650600228229401496703205377",
    "1606938044258990275541962092341162602522202993782792835301383",
    "-1606938044258990275541962092341162602522202993782792835301383",
];

/// S37 from its definition: `|a / b|` is `|a|` divided by `|b|` rounded
/// down (no rounding question arises for magnitudes), negative exactly when
/// the signs differ, and `a % b == a - (a / b) * b`.
fn s37(a: &BigInt, b: &BigInt) -> (BigInt, BigInt) {
    let magnitude = BigInt::from(a.magnitude() / b.magnitude());
    let q = if (a.sign() == Sign::Minus) != (b.sign() == Sign::Minus) {
        -magnitude
    } else {
        magnitude
    };
    let r = a - &q * b;
    (q, r)
}

#[test]
fn arith_quotient_and_remainder_for_every_sign_combination_s37() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn qr(a: Int, b: Int) -> List<Int> { [a / b, a % b] }
    let qr = fn2(&mut b, m, &h, "qr", Ty::list(Ty::Int), |h, a, d| {
        h.list(
            vec![
                bin(h, Div, iv(h, a), iv(h, d)),
                bin(h, Rem, iv(h, a), iv(h, d)),
            ],
            Ty::Int,
        )
    });
    let p = b.build();
    // (a, b, a / b, a % b) written out: the worked values of S37 and stdlib
    // 2.2, and rows that a 64-bit host division gets wrong or cannot compute.
    let written: [(&str, &str, &str, &str); 36] = [
        ("7", "2", "3", "1"),
        ("-7", "2", "-3", "-1"),
        ("7", "-2", "-3", "1"),
        ("-7", "-2", "3", "-1"),
        ("6", "3", "2", "0"),
        ("-6", "3", "-2", "0"),
        ("6", "-3", "-2", "0"),
        ("-6", "-3", "2", "0"),
        ("0", "5", "0", "0"),
        ("0", "-5", "0", "0"),
        ("1", "2", "0", "1"),
        ("-1", "2", "0", "-1"),
        ("1", "-2", "0", "1"),
        ("-1", "-2", "0", "-1"),
        ("2", "7", "0", "2"),
        ("-2", "7", "0", "-2"),
        ("5", "1", "5", "0"),
        ("5", "-1", "-5", "0"),
        ("-5", "-1", "5", "0"),
        ("-9223372036854775808", "-1", "9223372036854775808", "0"),
        ("-9223372036854775808", "9223372036854775807", "-1", "-1"),
        (
            "9223372036854775807",
            "-9223372036854775808",
            "0",
            "9223372036854775807",
        ),
        (
            "-170141183460469231731687303715884105728",
            "-1",
            "170141183460469231731687303715884105728",
            "0",
        ),
        ("18446744073709551616", "3", "6148914691236517205", "1"),
        ("-18446744073709551616", "3", "-6148914691236517205", "-1"),
        (
            "1267650600228229401496703205377",
            "2",
            "633825300114114700748351602688",
            "1",
        ),
        (
            "-1267650600228229401496703205377",
            "2",
            "-633825300114114700748351602688",
            "-1",
        ),
        (
            "1267650600228229401496703205377",
            "-2",
            "-633825300114114700748351602688",
            "1",
        ),
        (
            "-1267650600228229401496703205377",
            "-2",
            "633825300114114700748351602688",
            "-1",
        ),
        ("5", "1267650600228229401496703205376", "0", "5"),
        ("-5", "1267650600228229401496703205376", "0", "-5"),
        ("5", "-1267650600228229401496703205376", "0", "5"),
        ("-5", "-1267650600228229401496703205376", "0", "-5"),
        (
            "1606938044258990275541962092341162602522202993782792835301383",
            "1267650600228229401496703205376",
            "1267650600228229401496703205376",
            "7",
        ),
        (
            "-1606938044258990275541962092341162602522202993782792835301383",
            "1267650600228229401496703205376",
            "-1267650600228229401496703205376",
            "-7",
        ),
        (
            "1606938044258990275541962092341162602522202993782792835301383",
            "-1267650600228229401496703205376",
            "-1267650600228229401496703205376",
            "7",
        ),
    ];
    let mut cases: Vec<(String, String, String, String)> = written
        .iter()
        .map(|&(a, d, q, r)| (a.into(), d.into(), q.into(), r.into()))
        .collect();
    // Every ordered pair of SAMPLES with a nonzero divisor, from the definition.
    for a in SAMPLES {
        for d in SAMPLES {
            let (an, dn) = (big(a), big(d));
            if dn.is_zero() {
                continue;
            }
            let (q, r) = s37(&an, &dn);
            cases.push((a.into(), d.into(), q.to_string(), r.to_string()));
        }
    }
    let args: Vec<Vec<Json>> = cases
        .iter()
        .map(|(a, d, _, _)| vec![num(a), num(d)])
        .collect();
    for ((a, d, q, r), got) in cases.iter().zip(batch(&p, qr, &args)) {
        assert_eq!(got, Ok(json!([num(q), num(r)])), "{a} / {d} and {a} % {d}");
    }
}

#[test]
fn arith_sum_difference_product_and_negation_are_exact_beyond_64_bits() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn ops(a: Int, b: Int) -> List<Int> { [a + b, a - b, a * b, -a, -(-b)] }
    let ops = fn2(&mut b, m, &h, "ops", Ty::list(Ty::Int), |h, a, d| {
        h.list(
            vec![
                bin(h, Add, iv(h, a), iv(h, d)),
                bin(h, Sub, iv(h, a), iv(h, d)),
                bin(h, Mul, iv(h, a), iv(h, d)),
                neg(h, iv(h, a)),
                neg(h, neg(h, iv(h, d))),
            ],
            Ty::Int,
        )
    });
    let p = b.build();
    // Written out at the i64 bounds, where a 64-bit host would overflow.
    assert_eq!(
        value(&p, ops, &[num("9223372036854775807"), json!(1)]),
        json!([
            num("9223372036854775808"),
            num("9223372036854775806"),
            num("9223372036854775807"),
            num("-9223372036854775807"),
            1
        ])
    );
    assert_eq!(
        value(&p, ops, &[num("-9223372036854775808"), json!(-1)]),
        json!([
            num("-9223372036854775809"),
            num("-9223372036854775807"),
            num("9223372036854775808"),
            num("9223372036854775808"),
            -1
        ])
    );
    // stdlib 12: 12345678901234567890 * 10 == 123456789012345678900.
    assert_eq!(
        value(&p, ops, &[num("12345678901234567890"), json!(10)])[2],
        num("123456789012345678900")
    );
    let mut rows = Vec::new();
    let mut expected = Vec::new();
    for a in SAMPLES {
        for d in SAMPLES {
            let (x, y) = (big(a), big(d));
            rows.push(vec![num(a), num(d)]);
            expected.push(json!([
                int_json(&(&x + &y)),
                int_json(&(&x - &y)),
                int_json(&(&x * &y)),
                int_json(&-&x),
                int_json(&y)
            ]));
        }
    }
    for ((args, want), got) in rows.iter().zip(expected).zip(batch(&p, ops, &rows)) {
        assert_eq!(got, Ok(want), "ops({}, {})", args[0], args[1]);
    }
}

#[test]
fn arith_comparisons_are_the_mathematical_order_at_every_magnitude() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn cmp(a: Int, b: Int) -> List<Bool> { [a < b, a <= b, a > b, a >= b, a == b, a != b] }
    let cmp = fn2(&mut b, m, &h, "cmp", Ty::list(Ty::Bool), |h, a, d| {
        let ops = [Lt, Le, Gt, Ge, Eq, Ne];
        h.list(
            ops.iter()
                .map(|&op| bin(h, op, iv(h, a), iv(h, d)))
                .collect(),
            Ty::Bool,
        )
    });
    let p = b.build();
    let mut rows = Vec::new();
    let mut expected = Vec::new();
    for a in SAMPLES {
        for d in SAMPLES {
            let (x, y) = (big(a), big(d));
            rows.push(vec![num(a), num(d)]);
            expected.push(json!([x < y, x <= y, x > y, x >= y, x == y, x != y]));
        }
    }
    for ((args, want), got) in rows.iter().zip(expected).zip(batch(&p, cmp, &rows)) {
        assert_eq!(got, Ok(want), "cmp({}, {})", args[0], args[1]);
    }
}

#[test]
fn arith_division_by_zero_is_a_runtime_fault_carrying_both_operands() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn div(a, b) -> Int { a / b }  (the division at 3:5)  fn rem(a, b) -> Int { a % b }  (at 4:5)
    let div = fn2(&mut b, m, &h, "div", Ty::Int, |h, a, d| {
        spanned(bin(h, Div, iv(h, a), iv(h, d)), at(h, 3, 5, 5))
    });
    let rem = fn2(&mut b, m, &h, "rem", Ty::Int, |h, a, d| {
        spanned(bin(h, Rem, iv(h, a), iv(h, d)), at(h, 4, 5, 5))
    });
    // fn div_diff(a, b) -> Int { a / (b - b) }: a divisor that is computed to be 0.
    let div_diff = fn2(&mut b, m, &h, "div_diff", Ty::Int, |h, a, d| {
        bin(h, Div, iv(h, a), bin(h, Sub, iv(h, d), iv(h, d)))
    });
    let p = b.build();
    let two100 = "1267650600228229401496703205376";
    let minus_two100 = "-1267650600228229401496703205376";
    for (f, a, line) in [
        (div, two100, 3),
        (div, minus_two100, 3),
        (div, "0", 3),
        (rem, two100, 4),
        (rem, "-7", 4),
        (rem, "0", 4),
    ] {
        let d = fault(&p, f, &[num(a), json!(0)]);
        let name = p.display_path(f);
        // stdlib S8: a runtime fault, never a contract violation.
        assert_eq!(d["code"], "division_by_zero", "{name}({a}, 0)");
        assert_eq!(d["category"], "runtime");
        assert_eq!(d["severity"], "error");
        assert_eq!(d["symbol"], name.as_str());
        assert_eq!(d["qualified_name"], name.as_str());
        assert_eq!(d["facet"], "body");
        assert_eq!(d["input"], json!({"a": num(a), "b": 0}));
        assert_eq!(d["span"], loc(line, 5, 5));
        assert_eq!(backtrace(&d), [name.as_str()]);
        // Not a contract or assertion diagnostic (9.2), and `status` appears
        // only on the codes 9.5 lists.
        for key in ["status", "clause", "clause_kind", "attributed_to"] {
            assert!(d.get(key).is_none(), "{key} on {d}");
        }
    }
    let d = fault(&p, div_diff, &[json!(-3), json!(5)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["input"], json!({"a": -3, "b": 0}));
}

// ===========================================================================
// 2. Structural equality (semantics 6.1, 6.4, 6.5)
// ===========================================================================

/// `fn <name>(x: T, y: T) -> List<Bool> { [x == y, x != y] }`.
fn eq_fn(b: &mut ProgramBuilder, m: ModuleId, h: &Hir, name: &str, ty: Ty) -> DefId {
    define(
        b,
        m,
        name,
        &[("x", ty.clone()), ("y", ty.clone())],
        Ty::list(Ty::Bool),
        |_, ps| {
            let (x, y) = (h.local(ps[0], ty.clone()), h.local(ps[1], ty.clone()));
            h.value(h.list(
                vec![bin(h, Eq, x.clone(), y.clone()), bin(h, Ne, x, y)],
                Ty::Bool,
            ))
        },
    )
}

#[test]
fn eq_is_structural_on_every_type_form_6_5() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let std = b.std_defs();
    let point = b.add_struct(m, "Point", &[], &[("x", Ty::Int), ("y", Ty::Int)]);
    let e = b.add_enum(
        m,
        "E",
        &[],
        &[
            ("A", vec![Ty::Int]),
            ("B", vec![Ty::Int]),
            ("C", vec![Ty::Int, Ty::Int]),
            ("D", vec![]),
        ],
    );
    let meters = b.add_wrapper(m, "Meters", Ty::Int);
    let positive = b.add_int_refinement(m, "Positive", Gt, 0);
    let o2 = Ty::option(&std, Ty::option(&std, Ty::Int));
    let types = [
        ("eq_int", Ty::Int),
        ("eq_text", Ty::Text),
        ("eq_bytes", Ty::Bytes),
        ("eq_lists", Ty::list(Ty::list(Ty::Int))),
        ("eq_point", b.adt(point)),
        ("eq_e", b.adt(e)),
        ("eq_opts", o2),
        ("eq_result", Ty::result(&std, Ty::Int, Ty::Int)),
        ("eq_meters", Ty::Wrapper(meters)),
        ("eq_unit", Ty::unit(&std)),
        ("eq_bool", Ty::Bool),
        ("eq_positive", b.refined(positive)),
    ];
    let mut fns = BTreeMap::new();
    for (name, ty) in types {
        fns.insert(name, eq_fn(&mut b, m, &h, name, ty));
    }
    let p = b.build();
    let pt = |x: i64, y: i64| json!({"$type": "m.Point", "x": x, "y": y});
    let ev = |v: &str, payload: &[i64]| {
        if payload.is_empty() {
            json!({"$type": "m.E", "$variant": v})
        } else {
            json!({"$type": "m.E", "$variant": v, "$payload": payload})
        }
    };
    let meters_v = |n: i64| json!({"$type": "m.Meters", "$payload": [n]});
    let two100 = "1267650600228229401496703205376";
    let cases = [
        ("eq_int", num(two100), num(two100), true),
        (
            "eq_int",
            num(two100),
            num("1267650600228229401496703205377"),
            false,
        ),
        ("eq_int", num("-0"), json!(0), true),
        (
            "eq_int",
            num("-9223372036854775808"),
            num("9223372036854775808"),
            false,
        ),
        // 6.4: no Unicode normalization.
        ("eq_text", json!("\u{e9}"), json!("e\u{301}"), false),
        ("eq_text", json!("\u{1F600}"), json!("\u{1F600}"), true),
        ("eq_text", json!(""), json!(""), true),
        ("eq_text", json!("a\u{0}"), json!("a"), false),
        ("eq_bytes", bytes_v("00ff"), bytes_v("00ff"), true),
        ("eq_bytes", bytes_v("00ff"), bytes_v("00fe"), false),
        ("eq_bytes", bytes_v("00ff"), bytes_v("00ff00"), false),
        ("eq_bytes", bytes_v(""), bytes_v(""), true),
        ("eq_lists", json!([[1], [2]]), json!([[1], [2]]), true),
        ("eq_lists", json!([[1], [2]]), json!([[1, 2]]), false),
        ("eq_lists", json!([[1, 2]]), json!([[1, 2, 3]]), false),
        ("eq_lists", json!([[]]), json!([]), false),
        ("eq_lists", json!([]), json!([]), true),
        ("eq_point", pt(1, 2), pt(1, 2), true),
        ("eq_point", pt(1, 2), pt(2, 1), false),
        // Same payload, different variant.
        ("eq_e", ev("A", &[1]), ev("B", &[1]), false),
        ("eq_e", ev("C", &[1, 2]), ev("C", &[1, 2]), true),
        ("eq_e", ev("C", &[1, 2]), ev("C", &[1, 3]), false),
        ("eq_e", ev("D", &[]), ev("D", &[]), true),
        ("eq_e", ev("A", &[1]), ev("D", &[]), false),
        // Nested options are distinct values.
        ("eq_opts", some(none()), none(), false),
        ("eq_opts", some(none()), some(none()), true),
        ("eq_opts", some(some(json!(1))), some(some(json!(1))), true),
        ("eq_opts", some(some(json!(1))), some(none()), false),
        ("eq_opts", none(), none(), true),
        ("eq_result", ok_v(json!(1)), err_v(json!(1)), false),
        ("eq_result", err_v(json!(1)), err_v(json!(1)), true),
        ("eq_meters", meters_v(1), meters_v(1), true),
        ("eq_meters", meters_v(1), meters_v(2), false),
        ("eq_unit", unit_v(), unit_v(), true),
        ("eq_bool", json!(true), json!(false), false),
        // A refinement is erased: equality is the base value's.
        ("eq_positive", json!(3), json!(3), true),
        ("eq_positive", json!(3), json!(4), false),
    ];
    for (name, x, y, equal) in cases {
        assert_eq!(
            value(&p, fns[name], &[x.clone(), y.clone()]),
            json!([equal, !equal]),
            "{name}({x}, {y})"
        );
    }
}

#[test]
fn eq_values_built_by_different_operations_are_equal_6_1() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let point = b.add_struct(m, "Point", &[], &[("x", Ty::Int), ("y", Ty::Int)]);
    let pt = b.adt(point);
    let push = b.builtin("list_push");
    let take = b.builtin("list_take");
    let drop_ = b.builtin("list_drop");
    let len = b.builtin("list_len");
    let flatten = b.builtin("flatten");
    let lt = Ty::list(Ty::Int);
    // fn built(p: Point, xs: List<Int>) -> List<Bool> { [...] }
    let built = define(
        &mut b,
        m,
        "built",
        &[("p", pt.clone()), ("xs", lt.clone())],
        Ty::list(Ty::Bool),
        |_, ps| {
            let (pp, xs) = (h.local(ps[0], pt.clone()), h.local(ps[1], lt.clone()));
            let ints = |ns: &[i64]| h.list(ns.iter().map(|&n| h.int(n)).collect(), Ty::Int);
            let call =
                |f: DefId, args: Vec<Expr>, ret: Ty| h.call_generic(f, vec![Ty::Int], args, ret);
            h.value(h.list(
                vec![
                    // [1].push(2) == [1, 2]
                    bin(
                        &h,
                        Eq,
                        call(push, vec![ints(&[1]), h.int(2)], lt.clone()),
                        ints(&[1, 2]),
                    ),
                    // [1, 2, 3].take(2) == [1, 2]
                    bin(
                        &h,
                        Eq,
                        call(take, vec![ints(&[1, 2, 3]), h.int(2)], lt.clone()),
                        ints(&[1, 2]),
                    ),
                    // Point { ..p, x: p.x } == p
                    bin(
                        &h,
                        Eq,
                        h.struct_lit(
                            point,
                            Some(pp.clone()),
                            vec![(0, h.field(pp.clone(), point, 0, Ty::Int))],
                            pt.clone(),
                        ),
                        pp,
                    ),
                    // flatten([[1], [], [2]]) == [1, 2]
                    bin(
                        &h,
                        Eq,
                        call(
                            flatten,
                            vec![h.list(vec![ints(&[1]), ints(&[]), ints(&[2])], lt.clone())],
                            lt.clone(),
                        ),
                        ints(&[1, 2]),
                    ),
                    // ("a" ++ "b") == "ab"
                    bin(
                        &h,
                        Eq,
                        bin(&h, Concat, h.text("a"), h.text("b")),
                        h.text("ab"),
                    ),
                    // (xs ++ []) == xs
                    bin(&h, Eq, bin(&h, Concat, xs.clone(), ints(&[])), xs.clone()),
                    // Some(xs) != Some(xs.push(0))
                    bin(
                        &h,
                        Ne,
                        h.some(xs.clone()),
                        h.some(call(push, vec![xs.clone(), h.int(0)], lt.clone())),
                    ),
                    // [xs.take(xs.len), xs.drop(0)] == [xs, xs]
                    bin(
                        &h,
                        Eq,
                        h.list(
                            vec![
                                call(
                                    take,
                                    vec![xs.clone(), call(len, vec![xs.clone()], Ty::Int)],
                                    lt.clone(),
                                ),
                                call(drop_, vec![xs.clone(), h.int(0)], lt.clone()),
                            ],
                            lt.clone(),
                        ),
                        h.list(vec![xs.clone(), xs], lt.clone()),
                    ),
                ],
                Ty::Bool,
            ))
        },
    );
    let p = b.build();
    assert_eq!(
        value(
            &p,
            built,
            &[json!({"$type": "m.Point", "x": 1, "y": 2}), json!([5, 6])]
        ),
        json!([true, true, true, true, true, true, true, true])
    );
}

#[test]
fn eq_text_and_bytes_concatenation_and_length_6_4() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let text_len = b.builtin("text_len");
    let bytes_len = b.builtin("bytes_len");
    // fn joined(a: Text, b: Text) -> List<Bool> { [(a ++ b) == "e\u{301}", (a ++ b).len == 2, (a ++ b) != "\u{e9}"] }
    let joined = define(
        &mut b,
        m,
        "joined",
        &[("a", Ty::Text), ("b", Ty::Text)],
        Ty::list(Ty::Bool),
        |_, ps| {
            let ab = || {
                bin(
                    &h,
                    Concat,
                    h.local(ps[0], Ty::Text),
                    h.local(ps[1], Ty::Text),
                )
            };
            h.value(h.list(
                vec![
                    bin(&h, Eq, ab(), h.text("e\u{301}")),
                    bin(&h, Eq, h.call(text_len, vec![ab()], Ty::Int), h.int(2)),
                    bin(&h, Ne, ab(), h.text("\u{e9}")),
                ],
                Ty::Bool,
            ))
        },
    );
    // fn bytes_joined(x: Bytes, y: Bytes, z: Bytes) -> List<Bool> { [x ++ y == z, (x ++ y).len == 3] }
    let bytes_joined = define(
        &mut b,
        m,
        "bytes_joined",
        &[("x", Ty::Bytes), ("y", Ty::Bytes), ("z", Ty::Bytes)],
        Ty::list(Ty::Bool),
        |_, ps| {
            let xy = || {
                bin(
                    &h,
                    Concat,
                    h.local(ps[0], Ty::Bytes),
                    h.local(ps[1], Ty::Bytes),
                )
            };
            h.value(h.list(
                vec![
                    bin(&h, Eq, xy(), h.local(ps[2], Ty::Bytes)),
                    bin(&h, Eq, h.call(bytes_len, vec![xy()], Ty::Int), h.int(3)),
                ],
                Ty::Bool,
            ))
        },
    );
    let p = b.build();
    assert_eq!(
        value(&p, joined, &[json!("e"), json!("\u{301}")]),
        json!([true, true, true])
    );
    assert_eq!(
        value(
            &p,
            bytes_joined,
            &[bytes_v("00ff"), bytes_v("10"), bytes_v("00ff10")]
        ),
        json!([true, true])
    );
    assert_eq!(
        value(
            &p,
            bytes_joined,
            &[bytes_v("00ff"), bytes_v("10"), bytes_v("1000ff")]
        ),
        json!([false, true])
    );
}

// ===========================================================================
// 3. Strict left-to-right evaluation (semantics 6.2)
// ===========================================================================

/// `a`, `b` (Int), `la`, `lb` (List<Int>), `ta`, `tb` (Bool): [`bomb`]s.
struct Bombs {
    a: DefId,
    b: DefId,
    la: DefId,
    lb: DefId,
    ta: DefId,
    tb: DefId,
}

fn bombs(b: &mut ProgramBuilder, m: ModuleId, h: &Hir) -> Bombs {
    Bombs {
        a: bomb(b, m, h, "a", Ty::Int, h.int(0)),
        b: bomb(b, m, h, "b", Ty::Int, h.int(0)),
        la: bomb(b, m, h, "la", Ty::list(Ty::Int), h.list(vec![], Ty::Int)),
        lb: bomb(b, m, h, "lb", Ty::list(Ty::Int), h.list(vec![], Ty::Int)),
        ta: bomb(b, m, h, "ta", Ty::Bool, h.bool(true)),
        tb: bomb(b, m, h, "tb", Ty::Bool, h.bool(true)),
    }
}

/// Define one nullary function per case (its body the expression) and
/// check what calling each does.
fn check_cases(mut b: ProgramBuilder, m: ModuleId, h: &Hir, cases: Vec<(&str, Expr, Json)>) {
    let defs: Vec<(DefId, Json)> = cases
        .into_iter()
        .map(|(name, e, want)| {
            let ty = e.ty.clone();
            (define(&mut b, m, name, &[], ty, |_, _| h.value(e)), want)
        })
        .collect();
    let p = b.build();
    for (f, want) in defs {
        assert_eq!(outcome_of(&p, f), want, "{}", p.display_path(f));
    }
}

#[test]
fn order_binary_operands_evaluate_left_then_right_then_the_operation_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let a = || h.call(bm.a, vec![], Ty::Int);
    let bb = || h.call(bm.b, vec![], Ty::Int);
    let la = || h.call(bm.la, vec![], Ty::list(Ty::Int));
    let lb = || h.call(bm.lb, vec![], Ty::list(Ty::Int));
    let cases = vec![
        ("add", bin(&h, Add, a(), bb()), bombed("A")),
        ("lt", bin(&h, Lt, a(), bb()), bombed("A")),
        ("eq", bin(&h, Eq, a(), bb()), bombed("A")),
        ("concat", bin(&h, Concat, la(), lb()), bombed("LA")),
        // The divisor is evaluated before the division happens.
        ("divisor", bin(&h, Div, h.int(0), bb()), bombed("B")),
        (
            "operands_before_division",
            bin(&h, Div, a(), h.int(0)),
            bombed("A"),
        ),
        (
            "left_fault",
            bin(&h, Rem, bin(&h, Div, h.int(1), h.int(0)), bb()),
            faulted("division_by_zero"),
        ),
        (
            "nested",
            bin(&h, Mul, bin(&h, Sub, h.int(1), bb()), a()),
            bombed("B"),
        ),
    ];
    check_cases(b, m, &h, cases);
}

#[test]
fn order_and_or_evaluate_the_right_operand_only_when_needed_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let ta = || h.call(bm.ta, vec![], Ty::Bool);
    let tb = || h.call(bm.tb, vec![], Ty::Bool);
    let cases = vec![
        (
            "false_and",
            bin(&h, And, h.bool(false), tb()),
            returned(json!(false)),
        ),
        (
            "true_or",
            bin(&h, Or, h.bool(true), tb()),
            returned(json!(true)),
        ),
        ("true_and", bin(&h, And, h.bool(true), tb()), bombed("TB")),
        ("false_or", bin(&h, Or, h.bool(false), tb()), bombed("TB")),
        // The left operand runs even when the right one would decide.
        ("left_and", bin(&h, And, ta(), h.bool(false)), bombed("TA")),
        ("left_or", bin(&h, Or, ta(), h.bool(true)), bombed("TA")),
        (
            "not_or",
            not(&h, bin(&h, Or, h.bool(true), tb())),
            returned(json!(false)),
        ),
        (
            "chained",
            bin(
                &h,
                Or,
                bin(&h, And, h.bool(false), tb()),
                bin(&h, Or, h.bool(true), tb()),
            ),
            returned(json!(true)),
        ),
    ];
    check_cases(b, m, &h, cases);
}

#[test]
fn order_call_arguments_left_to_right_before_the_callee_requires_5_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    // fn g(x: Int, y: Int) -> Int requires x > 100 { x }
    let g = define(
        &mut b,
        m,
        "g",
        &[("x", Ty::Int), ("y", Ty::Int)],
        Ty::Int,
        |f, ps| {
            f.requires(h.clause(bin(&h, Gt, iv(&h, ps[0]), h.int(100)), "x > 100"));
            h.value(iv(&h, ps[0]))
        },
    );
    let a = || h.call(bm.a, vec![], Ty::Int);
    let bb = || h.call(bm.b, vec![], Ty::Int);
    let g_of = |x: Expr, y: Expr| h.call(g, vec![x, y], Ty::Int);
    let cases = vec![
        ("both", g_of(a(), bb()), bombed("A")),
        // Arguments are evaluated before the precondition is checked.
        ("second", g_of(h.int(1), bb()), bombed("B")),
        (
            "requires",
            g_of(h.int(1), h.int(2)),
            faulted("contract_violation"),
        ),
        ("holds", g_of(h.int(101), bb()), bombed("B")),
        ("ok", g_of(h.int(101), h.int(2)), returned(json!(101))),
    ];
    check_cases(b, m, &h, cases);
}

#[test]
fn order_method_call_receiver_arguments_then_builtin_requires_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let take = b.builtin("list_take");
    let lt = Ty::list(Ty::Int);
    let take_of = |xs: Expr, n: Expr| h.call_generic(take, vec![Ty::Int], vec![xs, n], lt.clone());
    let one = || h.list(vec![h.int(1)], Ty::Int);
    let la = || h.call(bm.la, vec![], lt.clone());
    let bb = || h.call(bm.b, vec![], Ty::Int);
    let cases = vec![
        ("receiver_first", take_of(la(), bb()), bombed("LA")),
        (
            "argument_before_requires",
            take_of(one(), bb()),
            bombed("B"),
        ),
        (
            "receiver_before_requires",
            take_of(la(), neg(&h, h.int(1))),
            bombed("LA"),
        ),
        (
            "requires",
            take_of(one(), neg(&h, h.int(1))),
            faulted("contract_violation"),
        ),
        (
            "faulting_argument",
            take_of(one(), bin(&h, Div, h.int(1), h.int(0))),
            faulted("division_by_zero"),
        ),
        (
            "ok",
            take_of(h.list(vec![h.int(1), h.int(2)], Ty::Int), h.int(1)),
            returned(json!([1])),
        ),
    ];
    check_cases(b, m, &h, cases);
}

#[test]
fn order_struct_literal_spread_first_then_fields_in_source_order_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let point = b.add_struct(m, "Point", &[], &[("x", Ty::Int), ("y", Ty::Int)]);
    let pt = b.adt(point);
    let lit =
        |x: i64, y: i64| h.struct_lit(point, None, vec![(0, h.int(x)), (1, h.int(y))], pt.clone());
    let pa = bomb(&mut b, m, &h, "pa", pt.clone(), lit(0, 0));
    let a = || h.call(bm.a, vec![], Ty::Int);
    let bb = || h.call(bm.b, vec![], Ty::Int);
    let cases = vec![
        // Point { y: a(), x: b() }: source order, not declaration order.
        (
            "y_first",
            h.struct_lit(point, None, vec![(1, a()), (0, bb())], pt.clone()),
            bombed("A"),
        ),
        (
            "x_first",
            h.struct_lit(point, None, vec![(0, bb()), (1, a())], pt.clone()),
            bombed("B"),
        ),
        // Point { ..pa(), x: b() }: the spread base before every field.
        (
            "spread_first",
            h.struct_lit(
                point,
                Some(h.call(pa, vec![], pt.clone())),
                vec![(0, bb())],
                pt.clone(),
            ),
            bombed("PA"),
        ),
        // Point { ..pa(), y: a(), x: b() }: the base runs even when every field is given.
        (
            "spread_all_fields",
            h.struct_lit(
                point,
                Some(h.call(pa, vec![], pt.clone())),
                vec![(1, a()), (0, bb())],
                pt.clone(),
            ),
            bombed("PA"),
        ),
        // Point { ..Point { x: 1, y: 2 }, y: 7 }
        (
            "spread_value",
            h.struct_lit(point, Some(lit(1, 2)), vec![(1, h.int(7))], pt.clone()),
            returned(json!({"$type": "m.Point", "x": 1, "y": 7})),
        ),
    ];
    check_cases(b, m, &h, cases);
}

#[test]
fn order_list_elements_and_constructor_payloads_left_to_right_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let e = b.add_enum(m, "E", &[], &[("C", vec![Ty::Int, Ty::Int])]);
    let et = b.adt(e);
    let a = || h.call(bm.a, vec![], Ty::Int);
    let bb = || h.call(bm.b, vec![], Ty::Int);
    let cases = vec![
        ("list", h.list(vec![a(), bb()], Ty::Int), bombed("A")),
        (
            "list_middle",
            h.list(vec![h.int(1), bb(), a()], Ty::Int),
            bombed("B"),
        ),
        (
            "payloads",
            h.ctor(e, 0, vec![a(), bb()], et.clone()),
            bombed("A"),
        ),
        (
            "second_payload",
            h.ctor(e, 0, vec![h.int(1), bb()], et.clone()),
            bombed("B"),
        ),
        ("some", h.some(a()), bombed("A")),
    ];
    check_cases(b, m, &h, cases);
}

#[test]
fn order_if_evaluates_the_condition_then_exactly_one_branch_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let ta = || h.call(bm.ta, vec![], Ty::Bool);
    let bb = || h.call(bm.b, vec![], Ty::Int);
    let cases = vec![
        (
            "condition",
            h.if_(ta(), h.value(bb()), Some(h.value(bb()))),
            bombed("TA"),
        ),
        (
            "then",
            h.if_(h.bool(true), h.value(h.int(1)), Some(h.value(bb()))),
            returned(json!(1)),
        ),
        (
            "else",
            h.if_(h.bool(false), h.value(bb()), Some(h.value(h.int(2)))),
            returned(json!(2)),
        ),
    ];
    check_cases(b, m, &h, cases);
}

#[test]
fn order_match_runs_only_the_first_matching_arm_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let std = b.std_defs();
    let a = || h.call(bm.a, vec![], Ty::Int);
    let bb = || h.call(bm.b, vec![], Ty::Int);
    let int_pat = |n: i64| h.pattern(PatternKind::Int(n.into()), Ty::Int);
    let wild = || h.pattern(PatternKind::Wildcard, Ty::Int);
    let cases = vec![
        // match a() { _ => 0 }
        (
            "scrutinee",
            h.match_(a(), vec![h.arm(wild(), h.value(h.int(0)))], Ty::Int),
            bombed("A"),
        ),
        // match 1 { 1 => 10  1 => b()  _ => b() }
        (
            "first_arm",
            h.match_(
                h.int(1),
                vec![
                    h.arm(int_pat(1), h.value(h.int(10))),
                    h.arm(int_pat(1), h.value(bb())),
                    h.arm(wild(), h.value(bb())),
                ],
                Ty::Int,
            ),
            returned(json!(10)),
        ),
        // match 2 { 1 => b()  _ => 20 }
        (
            "skips_non_matching",
            h.match_(
                h.int(2),
                vec![
                    h.arm(int_pat(1), h.value(bb())),
                    h.arm(wild(), h.value(h.int(20))),
                ],
                Ty::Int,
            ),
            returned(json!(20)),
        ),
    ];
    let mut b2 = b;
    // match Some(1) { Some(2) => b()  Some(x) => x  None => b() }
    let ot = h.option_ty(Ty::Int);
    let nested = define(&mut b2, m, "nested", &[], Ty::Int, |f, _| {
        let x = f.local("x", Ty::Int, BindingKind::Pattern, false);
        let some_of = |p: cpl_hir::Pattern| {
            h.pattern(
                PatternKind::Ctor {
                    def: std.option,
                    variant: 0,
                    args: vec![p],
                },
                ot.clone(),
            )
        };
        let none_pat = h.pattern(
            PatternKind::Ctor {
                def: std.option,
                variant: 1,
                args: vec![],
            },
            ot.clone(),
        );
        h.value(h.match_(
            h.some(h.int(1)),
            vec![
                h.arm(some_of(int_pat(2)), h.value(bb())),
                h.arm(
                    some_of(h.pattern(PatternKind::Bind(x), Ty::Int)),
                    h.value(iv(&h, x)),
                ),
                h.arm(none_pat, h.value(bb())),
            ],
            Ty::Int,
        ))
    });
    let defs: Vec<(DefId, Json)> = cases
        .into_iter()
        .map(|(name, e, want)| {
            let ty = e.ty.clone();
            (define(&mut b2, m, name, &[], ty, |_, _| h.value(e)), want)
        })
        .collect();
    let p = b2.build();
    for (f, want) in defs {
        assert_eq!(outcome_of(&p, f), want, "{}", p.display_path(f));
    }
    assert_eq!(outcome_of(&p, nested), returned(json!(1)));
}

#[test]
fn order_forall_evaluates_the_collection_then_each_element_in_order_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let tb = || h.call(bm.tb, vec![], Ty::Bool);
    let la = || h.call(bm.la, vec![], Ty::list(Ty::Int));
    let ints = |ns: &[i64]| h.list(ns.iter().map(|&n| h.int(n)).collect(), Ty::Int);
    // The predicates, over the binder x.
    let ten_over = |x: LocalId| bin(&h, Gt, bin(&h, Div, h.int(10), iv(&h, x)), h.int(0));
    let positive_then_bomb = |x: LocalId| bin(&h, And, bin(&h, Gt, iv(&h, x), h.int(0)), tb());
    let forall_fn =
        |b: &mut ProgramBuilder, name: &str, coll: Expr, pred: &dyn Fn(LocalId) -> Expr| {
            define(b, m, name, &[], Ty::Bool, |f, _| {
                let x = f.local("x", Ty::Int, BindingKind::Forall, false);
                h.value(h.forall(x, coll, pred(x)))
            })
        };
    let defs = [
        (
            forall_fn(&mut b, "collection_first", la(), &|_| tb()),
            bombed("LA"),
        ),
        // 10 / 0 is reached before 2 is looked at.
        (
            forall_fn(&mut b, "in_order", ints(&[1, 0, 2]), &ten_over),
            faulted("division_by_zero"),
        ),
        // Stops at the first false (10 / -1 > 0 is false) and never divides by 0.
        (
            forall_fn(&mut b, "stops_at_false", ints(&[-1, 0]), &ten_over),
            returned(json!(false)),
        ),
        (
            forall_fn(&mut b, "predicate", ints(&[1, 2]), &positive_then_bomb),
            bombed("TB"),
        ),
        // An empty list is true without evaluating the predicate.
        (
            forall_fn(&mut b, "empty", ints(&[]), &|_| tb()),
            returned(json!(true)),
        ),
    ];
    let p = b.build();
    for (f, want) in defs {
        assert_eq!(outcome_of(&p, f), want, "{}", p.display_path(f));
    }
}

#[test]
fn order_check_evaluates_its_argument_before_the_predicates_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    // type Nz = Int where 10 / self > 0
    let nz_pred = bin(
        &h,
        Gt,
        bin(&h, Div, h.int(10), h.local(LocalId(0), Ty::Int)),
        h.int(0),
    );
    let nz = b.add_refinement(m, "Nz", Ty::Int, nz_pred, "10 / self > 0");
    let rt = h.result_ty(b.refined(nz), h.constraint_error_ty());
    let a = || h.call(bm.a, vec![], Ty::Int);
    let cases = vec![
        ("argument_first", h.check(nz, a(), rt.clone()), bombed("A")),
        (
            "predicate_faults",
            h.check(nz, h.int(0), rt.clone()),
            faulted("division_by_zero"),
        ),
        (
            "holds",
            h.check(nz, h.int(5), rt.clone()),
            returned(ok_v(json!(5))),
        ),
    ];
    check_cases(b, m, &h, cases);
}

#[test]
fn order_block_statements_run_in_order_before_the_tail_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let bm = bombs(&mut b, m, &h);
    let a = || h.call(bm.a, vec![], Ty::Int);
    let bb = || h.call(bm.b, vec![], Ty::Int);
    // fn block() -> Int { let x = a()  b() }
    let block = define(&mut b, m, "block", &[], Ty::Int, |f, _| {
        let x = let_int(f, "x");
        h.block(vec![let_(x, a())], Some(bb()))
    });
    // fn later() -> Int { assert true  let _ = b()  a() }
    let later = define(&mut b, m, "later", &[], Ty::Int, |_, _| {
        h.block(
            vec![assert_(h.bool(true), "true"), Stmt::Discard(bb())],
            Some(a()),
        )
    });
    let p = b.build();
    assert_eq!(outcome_of(&p, block), bombed("A"));
    assert_eq!(outcome_of(&p, later), bombed("B"));
}

// ===========================================================================
// 4. `requires` (toolchain 1.7.1 row 1; semantics 5.2 step 3, S39 (c), S58)
// ===========================================================================

#[test]
fn requires_clauses_run_in_source_order_and_stop_at_the_first_false_5_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(x: Int) -> Int requires x > 0  requires x < 100  requires x % 2 == 0  requires 100 / (x - 7) > -1000 { x }
    let f = define(&mut b, m, "f", &[("x", Ty::Int)], Ty::Int, |f, ps| {
        let x = || iv(&h, ps[0]);
        f.requires(h.clause(bin(&h, Gt, x(), h.int(0)), "x > 0"));
        f.requires(h.clause(bin(&h, Lt, x(), h.int(100)), "x < 100"));
        f.requires(h.clause(
            bin(&h, Eq, bin(&h, Rem, x(), h.int(2)), h.int(0)),
            "x % 2 == 0",
        ));
        f.requires(h.clause(
            bin(
                &h,
                Gt,
                bin(&h, Div, h.int(100), bin(&h, Sub, x(), h.int(7))),
                neg(&h, h.int(1000)),
            ),
            "100 / (x - 7) > -1000",
        ));
        h.value(x())
    });
    let p = b.build();
    for (x, clause) in [
        (json!(-4), "x > 0"),
        (json!(150), "x < 100"),
        // The fourth clause would divide by zero; it is never evaluated.
        (json!(7), "x % 2 == 0"),
        (num("1180591620717411303424"), "x < 100"),
    ] {
        let d = fault(&p, f, std::slice::from_ref(&x));
        assert_eq!(d["code"], "contract_violation", "f({x}): {d}");
        assert_eq!(d["clause_kind"], "requires");
        assert_eq!(d["clause"], clause);
        assert_eq!(d["attributed_to"], "caller");
        assert_eq!(d["input"], json!({ "x": x }));
        assert_eq!(d["status"], "failed");
    }
    assert_eq!(value(&p, f, &[json!(8)]), json!(8));
}

#[test]
fn requires_names_the_calling_declaration_a_function_or_a_test_5_2() {
    // Checked from source: the clause's span runs from its keyword and the
    // callee's frame carries the span of the call (toolchain 3.3, 1.7.3).
    const SOURCE: &str = r#"module m

fn f(a: Int, s: Text, xs: List<Int>, o: Option<Int>) -> Int
    requires a > 0
{
    a
}

fn g() -> Int {
    f(0, "\u{e9}\n", [1], None)
}

test "calls f" {
    let _ = f(0, "\u{e9}\n", [1], None)
}
"#;
    let p = common::check_files(&[(FILE, SOURCE)]);
    let f = common::def(&p, "m.f");
    let g = common::def(&p, "m.g");
    let test = common::def(&p, "m.\"calls f\"");
    let input = json!({"a": 0, "s": "\u{e9}\n", "xs": [1], "o": none()});

    let d = fault(&p, g, &[]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["category"], "contract");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["caller"], "m.g");
    assert_eq!(d["input"], input);
    assert_eq!(
        d["span"],
        common::location_of(SOURCE, FILE, "requires a > 0")
    );
    assert_eq!(backtrace(&d), ["m.f", "m.g"]);
    assert_eq!(
        d["backtrace"][0]["span"],
        common::location_after(SOURCE, FILE, "fn g()", "f(0, \"\\u{e9}\\n\", [1], None)")
    );

    // A test body is the calling declaration of a call it makes.
    let record = run_test(&p, test, &TestConfig::with_seed(0));
    assert_eq!(record.status, TestStatus::Failed);
    let d = serde_json::to_value(record.fault.as_ref().expect("the test's fault")).unwrap();
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["caller"], "m.\"calls f\"");
    assert_eq!(d["input"], input);
    assert_eq!(backtrace(&d), ["m.f", "m.\"calls f\""]);

    // The command line is no declaration (S39 (c)).
    let d = fault(&p, f, &[json!(0), json!("\u{e9}\n"), json!([1]), none()]);
    assert_eq!(d["clause"], "a > 0");
    assert!(d.get("caller").is_none(), "{d}");
}

#[test]
fn requires_symbol_is_the_callee_id_and_caller_its_qualified_name_s2_5_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // @id("fn_7c2d") fn f(n: Int) -> Int requires n > 0 { n }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(bin(&h, Gt, iv(&h, ps[0]), h.int(0)), "n > 0"));
        h.value(iv(&h, ps[0]))
    });
    b.set_stable_id(f, "fn_7c2d");
    // @id("fn_9e1b") fn g() -> Int { f(0) }
    let g = define(&mut b, m, "g", &[], Ty::Int, |_, _| {
        h.value(h.call(f, vec![h.int(0)], Ty::Int))
    });
    b.set_stable_id(g, "fn_9e1b");
    let p = b.build();
    let d = fault(&p, g, &[]);
    assert_eq!(d["code"], "contract_violation");
    // symbol: the stable identity; qualified_name beside it (9.2, S2).
    assert_eq!(d["symbol"], "fn_7c2d");
    assert_eq!(d["qualified_name"], "m.f");
    // caller: "the calling declaration's qualified name" (5.2 step 3).
    assert_eq!(d["caller"], "m.g");
    assert_eq!(backtrace(&d), ["m.f", "m.g"]);
}

#[test]
fn requires_of_a_builtin_run_from_the_command_line_has_no_caller_s39_s58() {
    let b = ProgramBuilder::new();
    let split = b.std_def("std.text.split");
    let trim = b.std_def("std.text.trim");
    let p = b.build();
    // cpl run std.text.split '"a"' '""': S39 runs any function of the
    // program, and the builtin checks its own requires (S58).
    let outcome = run(
        &p,
        split,
        vec![c("\"a\""), c("\"\"")],
        &EvalConfig::default(),
    );
    let d = diag(
        &p,
        outcome.value.as_ref().expect_err("a precondition fault"),
    );
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "std.text.split");
    assert_eq!(d["qualified_name"], "std.text.split");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["clause"], "not sep.is_empty()");
    assert_eq!(d["input"], json!({"text": "a", "sep": ""}));
    assert_eq!(d["status"], "failed");
    assert!(d.get("caller").is_none(), "{d}");
    let outcome = run(&p, trim, vec![c("\"  x \"")], &EvalConfig::default());
    let v = outcome.value.as_ref().expect("a value");
    assert_eq!(value_to_json(&p, v, &outcome.return_ty), json!("x"));
}

#[test]
fn contract_clauses_short_circuit_and_and_or_inside_one_clause_6_2() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(x: Int) -> Int requires x != 0 and 100 / x > 1 { x }
    let f = define(&mut b, m, "f", &[("x", Ty::Int)], Ty::Int, |f, ps| {
        let x = || iv(&h, ps[0]);
        f.requires(h.clause(
            bin(
                &h,
                And,
                bin(&h, Ne, x(), h.int(0)),
                bin(&h, Gt, bin(&h, Div, h.int(100), x()), h.int(1)),
            ),
            "x != 0 and 100 / x > 1",
        ));
        h.value(x())
    });
    // fn g(x: Int) -> Int ensures result == 0 or 10 / result > 0 { x }
    let g = define(&mut b, m, "g", &[("x", Ty::Int)], Ty::Int, |f, ps| {
        let r = f.result();
        f.ensures(h.clause(
            bin(
                &h,
                Or,
                bin(&h, Eq, iv(&h, r), h.int(0)),
                bin(&h, Gt, bin(&h, Div, h.int(10), iv(&h, r)), h.int(0)),
            ),
            "result == 0 or 10 / result > 0",
        ));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    // x == 0: the right operand (100 / 0) is never evaluated.
    for x in [0, 200] {
        let d = fault(&p, f, &[json!(x)]);
        assert_eq!(d["code"], "contract_violation", "f({x}): {d}");
        assert_eq!(d["clause"], "x != 0 and 100 / x > 1");
    }
    assert_eq!(value(&p, f, &[json!(5)]), json!(5));
    // result == 0: the right operand (10 / 0) is never evaluated.
    assert_eq!(value(&p, g, &[json!(0)]), json!(0));
    assert_eq!(value(&p, g, &[json!(3)]), json!(3));
    let d = fault(&p, g, &[json!(-3)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["actual"], json!(-3));
}

// ===========================================================================
// 5. `ensures` on every exit (toolchain 1.7.1 row 2; semantics 5.2 step 5)
// ===========================================================================

#[test]
fn ensures_runs_on_a_return_inside_a_loop() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn count_to(n: Int) -> Int ensures result < 3 { var i = 0  while true { if i == n { return i }  i = i + 1 }  0 }
    let count_to = define(
        &mut b,
        m,
        "count_to",
        &[("n", Ty::Int)],
        Ty::Int,
        |f, ps| {
            let n = ps[0];
            let result = f.result();
            f.ensures(h.clause(bin(&h, Lt, iv(&h, result), h.int(3)), "result < 3"));
            let i = var_int(f, "i");
            h.block(
                vec![
                    let_(i, h.int(0)),
                    while_(
                        h.bool(true),
                        vec![],
                        None,
                        h.block(
                            vec![
                                stmt(h.if_(
                                    bin(&h, Eq, iv(&h, i), iv(&h, n)),
                                    h.block(vec![ret(iv(&h, i))], None),
                                    None,
                                )),
                                assign(i, bin(&h, Add, iv(&h, i), h.int(1))),
                            ],
                            None,
                        ),
                    ),
                ],
                Some(h.int(0)),
            )
        },
    );
    let p = b.build();
    assert_eq!(value(&p, count_to, &[json!(2)]), json!(2));
    let d = fault(&p, count_to, &[json!(5)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "result < 3");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["actual"], json!(5));
    assert_eq!(d["input"], json!({"n": 5}));
}

#[test]
fn ensures_runs_on_a_bare_return_with_the_value_unit() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn flag(b: Bool) -> Unit ensures b { if not b { return } }
    let flag = define(
        &mut b,
        m,
        "flag",
        &[("b", Ty::Bool)],
        h.unit_ty(),
        |f, ps| {
            f.ensures(h.clause(h.local(ps[0], Ty::Bool), "b"));
            h.block(
                vec![stmt(h.if_(
                    not(&h, h.local(ps[0], Ty::Bool)),
                    h.block(vec![Stmt::Return(None)], None),
                    None,
                ))],
                None,
            )
        },
    );
    let p = b.build();
    assert_eq!(value(&p, flag, &[json!(true)]), unit_v());
    let d = fault(&p, flag, &[json!(false)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "b");
    assert_eq!(d["actual"], unit_v());
    assert_eq!(d["input"], json!({"b": false}));
}

#[test]
fn ensures_runs_on_a_return_inside_a_match_arm() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let std = b.std_defs();
    let opt = h.option_ty(Ty::Int);
    // fn pick(o: Option<Int>) -> Int ensures result >= 0 { match o { Some(v) => { return v }  None => 0 } }
    let pick = define(
        &mut b,
        m,
        "pick",
        &[("o", opt.clone())],
        Ty::Int,
        |f, ps| {
            let result = f.result();
            f.ensures(h.clause(bin(&h, Ge, iv(&h, result), h.int(0)), "result >= 0"));
            let v = f.local("v", Ty::Int, BindingKind::Pattern, false);
            let some_pat = h.pattern(
                PatternKind::Ctor {
                    def: std.option,
                    variant: 0,
                    args: vec![h.pattern(PatternKind::Bind(v), Ty::Int)],
                },
                opt.clone(),
            );
            let none_pat = h.pattern(
                PatternKind::Ctor {
                    def: std.option,
                    variant: 1,
                    args: vec![],
                },
                opt.clone(),
            );
            h.value(h.match_(
                h.local(ps[0], opt.clone()),
                vec![
                    h.arm(some_pat, h.block(vec![ret(iv(&h, v))], None)),
                    h.arm(none_pat, h.value(h.int(0))),
                ],
                Ty::Int,
            ))
        },
    );
    let p = b.build();
    assert_eq!(value(&p, pick, &[some(json!(4))]), json!(4));
    assert_eq!(value(&p, pick, &[none()]), json!(0));
    let d = fault(&p, pick, &[some(json!(-3))]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["actual"], json!(-3));
    assert_eq!(d["input"], json!({"o": some(json!(-3))}));
}

#[test]
fn ensures_runs_on_a_question_mark_inside_a_loop() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let is_ok = b.builtin("result_is_ok");
    let rt = h.result_ty(Ty::Int, Ty::Text);
    // fn sum_ok(r: Result<Int, Text>, n: Int) -> Result<Int, Text> ensures result.is_ok() {
    //     var i = 0  while i < n { let v = r?  i = i + v }  Ok(i) }
    let sum_ok = define(
        &mut b,
        m,
        "sum_ok",
        &[("r", rt.clone()), ("n", Ty::Int)],
        rt.clone(),
        |f, ps| {
            let (r, n) = (ps[0], ps[1]);
            let result = f.result();
            f.ensures(h.clause(
                h.call_generic(
                    is_ok,
                    vec![Ty::Int, Ty::Text],
                    vec![h.local(result, rt.clone())],
                    Ty::Bool,
                ),
                "result.is_ok()",
            ));
            let i = var_int(f, "i");
            let v = let_int(f, "v");
            h.block(
                vec![
                    let_(i, h.int(0)),
                    while_(
                        bin(&h, Lt, iv(&h, i), iv(&h, n)),
                        vec![],
                        None,
                        h.block(
                            vec![
                                let_(v, h.propagate(h.local(r, rt.clone()), Ty::Int)),
                                assign(i, bin(&h, Add, iv(&h, i), iv(&h, v))),
                            ],
                            None,
                        ),
                    ),
                ],
                Some(h.ok(iv(&h, i), Ty::Text)),
            )
        },
    );
    let p = b.build();
    assert_eq!(
        value(&p, sum_ok, &[ok_v(json!(1)), json!(3)]),
        ok_v(json!(3))
    );
    assert_eq!(
        value(&p, sum_ok, &[err_v(json!("bad")), json!(0)]),
        ok_v(json!(0))
    );
    // The `?` exits with the propagated Err; ensures sees that whole Result.
    let d = fault(&p, sum_ok, &[err_v(json!("bad")), json!(1)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["clause"], "result.is_ok()");
    assert_eq!(d["actual"], err_v(json!("bad")));
    assert_eq!(d["input"], json!({"r": err_v(json!("bad")), "n": 1}));
}

#[test]
fn ensures_clauses_run_in_order_and_stop_at_the_first_false() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn g(n: Int) -> Int ensures result > 0  ensures 10 / result > 0  ensures result != 7 { n }
    let g = define(&mut b, m, "g", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let r = f.result();
        f.ensures(h.clause(bin(&h, Gt, iv(&h, r), h.int(0)), "result > 0"));
        f.ensures(h.clause(
            bin(&h, Gt, bin(&h, Div, h.int(10), iv(&h, r)), h.int(0)),
            "10 / result > 0",
        ));
        f.ensures(h.clause(bin(&h, Ne, iv(&h, r), h.int(7)), "result != 7"));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    // 10 / 0 in the second clause is never evaluated.
    for (n, clause) in [
        (0, "result > 0"),
        (20, "10 / result > 0"),
        (7, "result != 7"),
    ] {
        let d = fault(&p, g, &[json!(n)]);
        assert_eq!(d["code"], "contract_violation", "g({n}): {d}");
        assert_eq!(d["clause_kind"], "ensures");
        assert_eq!(d["clause"], clause);
        assert_eq!(d["actual"], json!(n));
    }
    assert_eq!(value(&p, g, &[json!(5)]), json!(5));
}

#[test]
fn ensures_is_not_evaluated_when_the_body_faults() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn body_faults(n: Int) -> Int ensures 1 / 0 == 0 { 10 / n }   (the clause at 8:13, the division at 9:5)
    let body_faults = define(
        &mut b,
        m,
        "body_faults",
        &[("n", Ty::Int)],
        Ty::Int,
        |f, ps| {
            f.ensures(clause_at(
                &h,
                bin(&h, Eq, bin(&h, Div, h.int(1), h.int(0)), h.int(0)),
                "1 / 0 == 0",
                at(&h, 8, 13, 10),
            ));
            h.value(spanned(
                bin(&h, Div, h.int(10), iv(&h, ps[0])),
                at(&h, 9, 5, 6),
            ))
        },
    );
    let p = b.build();
    // 5.2: a fault inside the body propagates without evaluating ensures.
    let d = fault(&p, body_faults, &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "body");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
    assert_eq!(d["span"], loc(9, 5, 6));
    assert!(d.get("clause").is_none(), "{d}");
    // When the body returns, the clause runs and its own fault is S34's.
    let d = fault(&p, body_faults, &[json!(1)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "1 / 0 == 0");
    assert_eq!(d["symbol"], "m.body_faults");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
}

#[test]
fn ensures_of_the_innermost_recursive_activation_reports_its_own_input() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn rec(n: Int) -> Int ensures result != 1 { if n == 0 { 1 } else { rec(n - 1) + 1 } }
    let me = b.next_def();
    let rec = define(&mut b, m, "rec", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let r = f.result();
        f.ensures(h.clause(bin(&h, Ne, iv(&h, r), h.int(1)), "result != 1"));
        let n = ps[0];
        h.value(h.if_(
            bin(&h, Eq, iv(&h, n), h.int(0)),
            h.value(h.int(1)),
            Some(h.value(bin(
                &h,
                Add,
                h.call(me, vec![bin(&h, Sub, iv(&h, n), h.int(1))], Ty::Int),
                h.int(1),
            ))),
        ))
    });
    assert_eq!(rec, me);
    let p = b.build();
    let d = fault(&p, rec, &[json!(3)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["input"], json!({"n": 0}));
    assert_eq!(d["actual"], json!(1));
    assert_eq!(backtrace(&d), ["m.rec", "m.rec", "m.rec", "m.rec"]);
}

#[test]
fn ensures_actual_is_the_value_json_of_the_exit_value() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let point = b.add_struct(m, "Point", &[], &[("x", Ty::Int), ("y", Ty::Int)]);
    let pt = b.adt(point);
    let is_err = b.builtin("result_is_err");
    let opt_pt = h.option_ty(pt.clone());
    let rt = h.result_ty(opt_pt.clone(), Ty::Text);
    // fn mk(n: Int) -> Result<Option<Point>, Text> ensures result.is_err() { Ok(Some(Point { x: n, y: -n })) }
    let mk = define(&mut b, m, "mk", &[("n", Ty::Int)], rt.clone(), |f, ps| {
        let r = f.result();
        f.ensures(h.clause(
            h.call_generic(
                is_err,
                vec![opt_pt.clone(), Ty::Text],
                vec![h.local(r, rt.clone())],
                Ty::Bool,
            ),
            "result.is_err()",
        ));
        let n = ps[0];
        h.value(h.ok(
            h.some(h.struct_lit(
                point,
                None,
                vec![(0, iv(&h, n)), (1, neg(&h, iv(&h, n)))],
                pt.clone(),
            )),
            Ty::Text,
        ))
    });
    let p = b.build();
    let d = fault(&p, mk, &[json!(3)]);
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(
        d["actual"],
        ok_v(some(json!({"$type": "m.Point", "x": 3, "y": -3})))
    );
    assert_eq!(d["input"], json!({"n": 3}));
}

// ===========================================================================
// 6. Loop invariants (toolchain 1.7.1 row 3; semantics 5.3)
// ===========================================================================

#[test]
fn invariant_is_checked_before_the_first_test_of_the_condition() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn entry(d: Int) -> Int { var i = 0  while 10 / d > i invariant i > 0 { i = i + 1 }  i }
    let entry = define(&mut b, m, "entry", &[("d", Ty::Int)], Ty::Int, |f, ps| {
        let i = var_int(f, "i");
        h.block(
            vec![
                let_(i, h.int(0)),
                while_(
                    bin(&h, Gt, bin(&h, Div, h.int(10), iv(&h, ps[0])), iv(&h, i)),
                    vec![h.clause(bin(&h, Gt, iv(&h, i), h.int(0)), "i > 0")],
                    None,
                    h.block(vec![assign(i, bin(&h, Add, iv(&h, i), h.int(1)))], None),
                ),
            ],
            Some(iv(&h, i)),
        )
    });
    let p = b.build();
    // The invariant is false at entry; the condition (10 / 0) never runs.
    let d = fault(&p, entry, &[json!(0)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["clause"], "i > 0");
    assert_eq!(d["iteration"], json!(0));
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.entry");
    assert_eq!(d["input"], json!({"d": 0}));
    assert_eq!(d["actual"], json!({"bindings": {"d": 0, "i": 0}}));
}

#[test]
fn invariants_run_in_order_after_every_iteration_and_stop_at_the_first_false() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(n: Int) -> Int { var i = 0  while i < n invariant i < 10  invariant i != 2  invariant 10 / (i - 2) > -100 { i = i + 1 }  i }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let i = var_int(f, "i");
        h.block(
            vec![
                let_(i, h.int(0)),
                while_(
                    bin(&h, Lt, iv(&h, i), iv(&h, ps[0])),
                    vec![
                        h.clause(bin(&h, Lt, iv(&h, i), h.int(10)), "i < 10"),
                        h.clause(bin(&h, Ne, iv(&h, i), h.int(2)), "i != 2"),
                        h.clause(
                            bin(
                                &h,
                                Gt,
                                bin(&h, Div, h.int(10), bin(&h, Sub, iv(&h, i), h.int(2))),
                                neg(&h, h.int(100)),
                            ),
                            "10 / (i - 2) > -100",
                        ),
                    ],
                    None,
                    h.block(vec![assign(i, bin(&h, Add, iv(&h, i), h.int(1)))], None),
                ),
            ],
            Some(iv(&h, i)),
        )
    });
    let p = b.build();
    assert_eq!(value(&p, f, &[json!(1)]), json!(1));
    // After the second body i == 2: the second invariant fails and the third,
    // which would divide by zero, never runs. The check after the last body
    // happens even though the condition is false next (n == 2).
    for n in [2, 5] {
        let d = fault(&p, f, &[json!(n)]);
        assert_eq!(d["code"], "contract_violation", "f({n}): {d}");
        assert_eq!(d["clause"], "i != 2");
        assert_eq!(d["iteration"], json!(2));
        assert_eq!(d["actual"], json!({"bindings": {"n": n, "i": 2}}));
    }
}

#[test]
fn invariant_bindings_are_exactly_those_visible_at_the_loop() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn visible(n: Int) -> Int { let k = 7  var i = 0  while i < n invariant i < 2 { let tmp = i * 2  i = i + 1 }  i + k }
    let visible = define(&mut b, m, "visible", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let k = let_int(f, "k");
        let i = var_int(f, "i");
        let tmp = let_int(f, "tmp");
        h.block(
            vec![
                let_(k, h.int(7)),
                let_(i, h.int(0)),
                while_(
                    bin(&h, Lt, iv(&h, i), iv(&h, ps[0])),
                    vec![h.clause(bin(&h, Lt, iv(&h, i), h.int(2)), "i < 2")],
                    None,
                    h.block(
                        vec![
                            let_(tmp, bin(&h, Mul, iv(&h, i), h.int(2))),
                            assign(i, bin(&h, Add, iv(&h, i), h.int(1))),
                        ],
                        None,
                    ),
                ),
            ],
            Some(bin(&h, Add, iv(&h, i), iv(&h, k))),
        )
    });
    let p = b.build();
    assert_eq!(value(&p, visible, &[json!(1)]), json!(8));
    let d = fault(&p, visible, &[json!(3)]);
    assert_eq!(d["iteration"], json!(2));
    // The body's `tmp` went out of scope with the body block.
    assert_eq!(d["actual"], json!({"bindings": {"n": 3, "k": 7, "i": 2}}));
    assert_eq!(d["input"], json!({"n": 3}));
}

#[test]
fn invariant_iteration_counts_the_current_execution_of_a_nested_loop() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn nested() -> Int { var i = 0  while i < 3 { var j = 0  while j < 3 invariant i + j < 4 { j = j + 1 }  i = i + 1 }  i }
    let nested = define(&mut b, m, "nested", &[], Ty::Int, |f, _| {
        let i = var_int(f, "i");
        let j = var_int(f, "j");
        let inner = while_(
            bin(&h, Lt, iv(&h, j), h.int(3)),
            vec![h.clause(
                bin(&h, Lt, bin(&h, Add, iv(&h, i), iv(&h, j)), h.int(4)),
                "i + j < 4",
            )],
            None,
            h.block(vec![assign(j, bin(&h, Add, iv(&h, j), h.int(1)))], None),
        );
        h.block(
            vec![
                let_(i, h.int(0)),
                while_(
                    bin(&h, Lt, iv(&h, i), h.int(3)),
                    vec![],
                    None,
                    h.block(
                        vec![
                            let_(j, h.int(0)),
                            inner,
                            assign(i, bin(&h, Add, iv(&h, i), h.int(1))),
                        ],
                        None,
                    ),
                ),
            ],
            Some(iv(&h, i)),
        )
    });
    let p = b.build();
    // The second run of the inner loop fails after its third body: 1 + 3 < 4.
    let d = fault(&p, nested, &[]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["iteration"], json!(3));
    assert_eq!(d["actual"], json!({"bindings": {"i": 1, "j": 3}}));
    assert_eq!(d["input"], json!({}));
}

#[test]
fn invariant_that_faults_is_reported_as_that_fault_s34() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(n: Int) -> Int { var i = 0  while i < n invariant 10 / (3 - i) > 0 { i = i + 1 }  i }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let i = var_int(f, "i");
        h.block(
            vec![
                let_(i, h.int(0)),
                while_(
                    bin(&h, Lt, iv(&h, i), iv(&h, ps[0])),
                    vec![h.clause(
                        bin(
                            &h,
                            Gt,
                            bin(&h, Div, h.int(10), bin(&h, Sub, h.int(3), iv(&h, i))),
                            h.int(0),
                        ),
                        "10 / (3 - i) > 0",
                    )],
                    None,
                    h.block(vec![assign(i, bin(&h, Add, iv(&h, i), h.int(1)))], None),
                ),
            ],
            Some(iv(&h, i)),
        )
    });
    let p = b.build();
    assert_eq!(value(&p, f, &[json!(2)]), json!(2));
    let d = fault(&p, f, &[json!(5)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["category"], "runtime");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "10 / (3 - i) > 0");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
    assert!(d.get("clause_kind").is_none(), "{d}");
}

#[test]
fn invariant_is_not_checked_after_a_body_that_returns() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f() -> Int { var i = 0  while true invariant i < 1 { i = i + 1  return i }  0 }
    let f = define(&mut b, m, "f", &[], Ty::Int, |f, _| {
        let i = var_int(f, "i");
        h.block(
            vec![
                let_(i, h.int(0)),
                while_(
                    h.bool(true),
                    vec![h.clause(bin(&h, Lt, iv(&h, i), h.int(1)), "i < 1")],
                    None,
                    h.block(
                        vec![assign(i, bin(&h, Add, iv(&h, i), h.int(1))), ret(iv(&h, i))],
                        None,
                    ),
                ),
            ],
            Some(h.int(0)),
        )
    });
    let p = b.build();
    // The body did not complete an iteration: the function exits with 1 and
    // the invariant (now false) is not evaluated (5.3 pseudo-code).
    assert_eq!(value(&p, f, &[]), json!(1));
}

// ===========================================================================
// 7. Loop `decreases` (toolchain 1.7.1 row 4; semantics 5.3)
// ===========================================================================

/// A loop clause of [`counting_loop`]: its canonical text and its
/// expression over the locals `n` and `i`.
type LoopClause<'a> = (&'a str, fn(&Hir, LocalId, LocalId) -> Expr);

/// `fn <name>(n: Int) -> Int { var i = 0  while i < n [invariant inv] decreases <measure> { i = i + 1 }  i }`.
fn counting_loop(
    b: &mut ProgramBuilder,
    m: ModuleId,
    h: &Hir,
    name: &str,
    invariant: Option<LoopClause<'_>>,
    measure: LoopClause<'_>,
) -> DefId {
    define(b, m, name, &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let n = ps[0];
        let i = var_int(f, "i");
        let invariants = invariant
            .map(|(text, e)| vec![h.clause(e(h, n, i), text)])
            .unwrap_or_default();
        h.block(
            vec![
                let_(i, h.int(0)),
                while_(
                    bin(h, Lt, iv(h, i), iv(h, n)),
                    invariants,
                    Some(h.clause((measure.1)(h, n, i), measure.0)),
                    h.block(vec![assign(i, bin(h, Add, iv(h, i), h.int(1)))], None),
                ),
            ],
            Some(iv(h, i)),
        )
    })
}

#[test]
fn loop_measure_that_does_not_decrease_reports_previous_and_measure() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let up = counting_loop(&mut b, m, &h, "up", None, ("i", |h, _, i| iv(h, i)));
    let p = b.build();
    assert_eq!(value(&p, up, &[json!(1)]), json!(1));
    let d = fault(&p, up, &[json!(3)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "i");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.up");
    assert_eq!(d["iteration"], json!(1));
    assert_eq!(d["input"], json!({"n": 3}));
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 3, "i": 1}, "previous": 0, "measure": 1})
    );
}

#[test]
fn loop_measure_that_turns_negative_reports_the_measure_only() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let down = counting_loop(
        &mut b,
        m,
        &h,
        "down",
        None,
        ("2 - i", |h, _, i| bin(h, Sub, h.int(2), iv(h, i))),
    );
    let p = b.build();
    // Measures 2, 1, 0: zero is a nonnegative measure.
    assert_eq!(value(&p, down, &[json!(3)]), json!(3));
    // The fourth measure is -1: negative, so no `previous` (5.3 pseudo-code).
    let d = fault(&p, down, &[json!(5)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "2 - i");
    assert_eq!(d["iteration"], json!(3));
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": 5, "i": 3}, "measure": -1})
    );
}

#[test]
fn loop_measure_is_never_evaluated_after_the_loop_exits() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // decreases n - i + 0 * (10 / (n - i)): dividing by zero exactly when the loop has exited.
    let exact = counting_loop(
        &mut b,
        m,
        &h,
        "exact",
        None,
        ("n - i + 0 * (10 / (n - i))", |h, n, i| {
            let gap = || bin(h, Sub, iv(h, n), iv(h, i));
            bin(
                h,
                Add,
                gap(),
                bin(h, Mul, h.int(0), bin(h, Div, h.int(10), gap())),
            )
        }),
    );
    let p = b.build();
    assert_eq!(value(&p, exact, &[json!(3)]), json!(3));
    assert_eq!(value(&p, exact, &[json!(0)]), json!(0));
}

#[test]
fn loop_condition_runs_before_the_measure_and_is_not_a_contract_expression() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(d: Int) -> Int { var i = 0  while 10 / d > i decreases 0 - 1 { i = i + 1 }  i }
    let f = define(&mut b, m, "f", &[("d", Ty::Int)], Ty::Int, |f, ps| {
        let i = var_int(f, "i");
        h.block(
            vec![
                let_(i, h.int(0)),
                while_(
                    bin(&h, Gt, bin(&h, Div, h.int(10), iv(&h, ps[0])), iv(&h, i)),
                    vec![],
                    Some(h.clause(bin(&h, Sub, h.int(0), h.int(1)), "0 - 1")),
                    h.block(vec![assign(i, bin(&h, Add, iv(&h, i), h.int(1)))], None),
                ),
            ],
            Some(iv(&h, i)),
        )
    });
    let p = b.build();
    let d = fault(&p, f, &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "body");
    assert!(d.get("clause").is_none(), "{d}");
    // With a nonzero divisor the condition holds and the negative measure fails.
    let d = fault(&p, f, &[json!(1)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["iteration"], json!(0));
    assert_eq!(
        d["actual"],
        json!({"bindings": {"d": 1, "i": 0}, "measure": -1})
    );
}

#[test]
fn loop_invariants_after_a_body_run_before_the_next_measure() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // invariant i < 1, decreases i: after the first body the invariant is
    // false and the (non-decreasing) second measure is never reached.
    let f = counting_loop(
        &mut b,
        m,
        &h,
        "f",
        Some(("i < 1", |h, _, i| bin(h, Lt, iv(h, i), h.int(1)))),
        ("i", |h, _, i| iv(h, i)),
    );
    let p = b.build();
    let d = fault(&p, f, &[json!(5)]);
    assert_eq!(d["clause_kind"], "invariant");
    assert_eq!(d["clause"], "i < 1");
    assert_eq!(d["iteration"], json!(1));
    assert_eq!(d["actual"], json!({"bindings": {"n": 5, "i": 1}}));
}

#[test]
fn loop_measure_that_faults_is_reported_as_that_fault_s34() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // decreases 3 - i + 0 * (10 / (2 - i)): measures 3, 2, then 10 / 0.
    let f = counting_loop(
        &mut b,
        m,
        &h,
        "f",
        None,
        ("3 - i + 0 * (10 / (2 - i))", |h, _, i| {
            bin(
                h,
                Add,
                bin(h, Sub, h.int(3), iv(h, i)),
                bin(
                    h,
                    Mul,
                    h.int(0),
                    bin(h, Div, h.int(10), bin(h, Sub, h.int(2), iv(h, i))),
                ),
            )
        }),
    );
    let p = b.build();
    let d = fault(&p, f, &[json!(3)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "3 - i + 0 * (10 / (2 - i))");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
}

#[test]
fn loop_measures_beyond_64_bits_compare_exactly() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let halving = |name: &str, step: BinaryOp, b: &mut ProgramBuilder| {
        // fn <name>(n: Int) -> Int { var k = n  while k > 0 decreases k { k = k <step> <2 or 1> }  k }
        define(b, m, name, &[("n", Ty::Int)], Ty::Int, |f, ps| {
            let k = var_int(f, "k");
            let by = if step == Div { 2 } else { 1 };
            h.block(
                vec![
                    let_(k, iv(&h, ps[0])),
                    while_(
                        bin(&h, Gt, iv(&h, k), h.int(0)),
                        vec![],
                        Some(h.clause(iv(&h, k), "k")),
                        h.block(vec![assign(k, bin(&h, step, iv(&h, k), h.int(by)))], None),
                    ),
                ],
                Some(iv(&h, k)),
            )
        })
    };
    let halve = halving("halve", Div, &mut b);
    let stuck = halving("stuck", Mul, &mut b);
    let p = b.build();
    let two100 = num("1267650600228229401496703205376");
    assert_eq!(value(&p, halve, std::slice::from_ref(&two100)), json!(0));
    let d = fault(&p, stuck, std::slice::from_ref(&two100));
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["iteration"], json!(1));
    assert_eq!(
        d["actual"],
        json!({"bindings": {"n": two100.clone(), "k": two100.clone()}, "previous": two100.clone(), "measure": two100})
    );
}

#[test]
fn loop_measure_history_restarts_with_each_execution_of_the_loop() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f() -> Int { var i = 0  var total = 0  while i < 3 { var j = 0
    //     while j < 2 decreases 2 - j { j = j + 1  total = total + 1 }  i = i + 1 }  total }
    let f = define(&mut b, m, "f", &[], Ty::Int, |f, _| {
        let i = var_int(f, "i");
        let total = var_int(f, "total");
        let j = var_int(f, "j");
        let inner = while_(
            bin(&h, Lt, iv(&h, j), h.int(2)),
            vec![],
            Some(h.clause(bin(&h, Sub, h.int(2), iv(&h, j)), "2 - j")),
            h.block(
                vec![
                    assign(j, bin(&h, Add, iv(&h, j), h.int(1))),
                    assign(total, bin(&h, Add, iv(&h, total), h.int(1))),
                ],
                None,
            ),
        );
        h.block(
            vec![
                let_(i, h.int(0)),
                let_(total, h.int(0)),
                while_(
                    bin(&h, Lt, iv(&h, i), h.int(3)),
                    vec![],
                    None,
                    h.block(
                        vec![
                            let_(j, h.int(0)),
                            inner,
                            assign(i, bin(&h, Add, iv(&h, i), h.int(1))),
                        ],
                        None,
                    ),
                ),
            ],
            Some(iv(&h, total)),
        )
    });
    let p = b.build();
    // Each run of the inner loop starts a new history (measures 2, 1); the
    // 2 of a later run is not compared with the 1 of the run before.
    assert_eq!(value(&p, f, &[]), json!(6));
}

// ===========================================================================
// 8. Header `decreases` (toolchain 1.7.1 row 5; semantics 5.4, S35)
// ===========================================================================

#[test]
fn header_measure_compares_with_the_nearest_activation_not_the_outermost_s35() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(n: Int, mode: Int) -> Int decreases n {
    //     if mode == 0 { 0 } else { if mode == 1 { f(n - 2, 2) } else { f(n + 1, 0) } } }
    let me = b.next_def();
    let f = define(
        &mut b,
        m,
        "f",
        &[("n", Ty::Int), ("mode", Ty::Int)],
        Ty::Int,
        |f, ps| {
            let (n, mode) = (ps[0], ps[1]);
            f.decreases(h.clause(iv(&h, n), "n"));
            let call = |arg: Expr, md: i64| h.call(me, vec![arg, h.int(md)], Ty::Int);
            h.value(h.if_(
                bin(&h, Eq, iv(&h, mode), h.int(0)),
                h.value(h.int(0)),
                Some(h.value(h.if_(
                    bin(&h, Eq, iv(&h, mode), h.int(1)),
                    h.value(call(bin(&h, Sub, iv(&h, n), h.int(2)), 2)),
                    Some(h.value(call(bin(&h, Add, iv(&h, n), h.int(1)), 0))),
                ))),
            ))
        },
    );
    assert_eq!(f, me);
    let p = b.build();
    assert_eq!(value(&p, f, &[json!(5), json!(0)]), json!(0));
    // f(5, 1) -> f(3, 2) -> f(4, 0): 4 is below the outermost 5 but not
    // below the nearest activation's 3.
    let d = fault(&p, f, &[json!(5), json!(1)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["clause"], "n");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["actual"], json!({"previous": 3, "measure": 4}));
    assert_eq!(backtrace(&d), ["m.f", "m.f", "m.f"]);
    // A header measure has no iteration (loops only, 5.6).
    assert!(d.get("iteration").is_none(), "{d}");
    let d = fault(&p, f, &[json!(7), json!(1)]);
    assert_eq!(d["actual"], json!({"previous": 5, "measure": 6}));
}

#[test]
fn header_measure_is_compared_through_intermediate_frames_of_other_functions_s35() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(n: Int) -> Int decreases n { if n <= 0 { 0 } else { g(n) } }   fn g(k: Int) -> Int { f(k) }
    let f_id = b.next_def();
    let g_id = DefId(f_id.0 + 1);
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.decreases(h.clause(iv(&h, ps[0]), "n"));
        h.value(h.if_(
            bin(&h, Le, iv(&h, ps[0]), h.int(0)),
            h.value(h.int(0)),
            Some(h.value(h.call(g_id, vec![iv(&h, ps[0])], Ty::Int))),
        ))
    });
    let g = define(&mut b, m, "g", &[("k", Ty::Int)], Ty::Int, |_, ps| {
        h.value(h.call(f_id, vec![iv(&h, ps[0])], Ty::Int))
    });
    assert_eq!((f, g), (f_id, g_id));
    // fn f2(n: Int) -> Int decreases n { if n <= 0 { 0 } else { g2(n - 1) + 1 } }   fn g2(k: Int) -> Int { f2(k) }
    let f2_id = b.next_def();
    let g2_id = DefId(f2_id.0 + 1);
    let f2 = define(&mut b, m, "f2", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.decreases(h.clause(iv(&h, ps[0]), "n"));
        h.value(h.if_(
            bin(&h, Le, iv(&h, ps[0]), h.int(0)),
            h.value(h.int(0)),
            Some(h.value(bin(
                &h,
                Add,
                h.call(g2_id, vec![bin(&h, Sub, iv(&h, ps[0]), h.int(1))], Ty::Int),
                h.int(1),
            ))),
        ))
    });
    let g2 = define(&mut b, m, "g2", &[("k", Ty::Int)], Ty::Int, |_, ps| {
        h.value(h.call(f2_id, vec![iv(&h, ps[0])], Ty::Int))
    });
    assert_eq!((f2, g2), (f2_id, g2_id));
    let p = b.build();
    let d = fault(&p, f, &[json!(1)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["actual"], json!({"previous": 1, "measure": 1}));
    assert_eq!(d["input"], json!({"n": 1}));
    assert_eq!(backtrace(&d), ["m.f", "m.g", "m.f"]);
    assert_eq!(value(&p, f, &[json!(0)]), json!(0));
    assert_eq!(value(&p, f2, &[json!(5)]), json!(5));
}

#[test]
fn header_measures_of_different_functions_are_never_compared_s35() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(n: Int) -> Int decreases n { if n == 0 { 0 } else { g(n - 1) } }   fn g(k: Int) -> Int decreases k { f(k) }
    // f(2) follows g(2) on the stack: compared with g's 2 it would fail; S35
    // compares it with the nearest activation of f itself (3).
    let f_id = b.next_def();
    let g_id = DefId(f_id.0 + 1);
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.decreases(h.clause(iv(&h, ps[0]), "n"));
        h.value(h.if_(
            bin(&h, Eq, iv(&h, ps[0]), h.int(0)),
            h.value(h.int(0)),
            Some(h.value(h.call(g_id, vec![bin(&h, Sub, iv(&h, ps[0]), h.int(1))], Ty::Int))),
        ))
    });
    let g = define(&mut b, m, "g", &[("k", Ty::Int)], Ty::Int, |f, ps| {
        f.decreases(h.clause(iv(&h, ps[0]), "k"));
        h.value(h.call(f_id, vec![iv(&h, ps[0])], Ty::Int))
    });
    assert_eq!((f, g), (f_id, g_id));
    let p = b.build();
    assert_eq!(value(&p, f, &[json!(3)]), json!(0));
}

#[test]
fn header_measure_that_is_negative_reports_the_measure_only_s35() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(n: Int) -> Int decreases n { if n < -5 { 0 } else { f(n - 1) } }
    let me = b.next_def();
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.decreases(h.clause(iv(&h, ps[0]), "n"));
        h.value(h.if_(
            bin(&h, Lt, iv(&h, ps[0]), neg(&h, h.int(5))),
            h.value(h.int(0)),
            Some(h.value(h.call(me, vec![bin(&h, Sub, iv(&h, ps[0]), h.int(1))], Ty::Int))),
        ))
    });
    assert_eq!(f, me);
    let p = b.build();
    // On a recursive entry: f(0) -> f(-1).
    let d = fault(&p, f, &[json!(0)]);
    assert_eq!(d["clause_kind"], "decreases");
    assert_eq!(d["actual"], json!({"measure": -1}));
    assert_eq!(d["input"], json!({"n": -1}));
    assert_eq!(backtrace(&d), ["m.f", "m.f"]);
    // On the first entry, from the top level.
    let d = fault(&p, f, &[json!(-3)]);
    assert_eq!(d["actual"], json!({"measure": -3}));
    assert_eq!(backtrace(&d), ["m.f"]);
}

#[test]
fn header_measure_that_increases_reports_previous_and_measure_s35() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(n: Int) -> Int decreases n { if n > 3 { 0 } else { f(n + 1) } }
    let me = b.next_def();
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.decreases(h.clause(iv(&h, ps[0]), "n"));
        h.value(h.if_(
            bin(&h, Gt, iv(&h, ps[0]), h.int(3)),
            h.value(h.int(0)),
            Some(h.value(h.call(me, vec![bin(&h, Add, iv(&h, ps[0]), h.int(1))], Ty::Int))),
        ))
    });
    assert_eq!(f, me);
    let p = b.build();
    let d = fault(&p, f, &[json!(1)]);
    // Exactly {measure, previous} (toolchain 1.7.1): no bindings for a header measure.
    assert_eq!(d["actual"], json!({"previous": 1, "measure": 2}));
    assert_eq!(d["input"], json!({"n": 2}));
    assert_eq!(value(&p, f, &[json!(4)]), json!(0));
}

#[test]
fn header_measure_tree_recursion_compares_with_live_activations_only_s35() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn fib(n: Int) -> Int decreases n { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }
    let me = b.next_def();
    let fib = define(&mut b, m, "fib", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let n = ps[0];
        f.decreases(h.clause(iv(&h, n), "n"));
        let call = |k: i64| h.call(me, vec![bin(&h, Sub, iv(&h, n), h.int(k))], Ty::Int);
        h.value(h.if_(
            bin(&h, Lt, iv(&h, n), h.int(2)),
            h.value(iv(&h, n)),
            Some(h.value(bin(&h, Add, call(1), call(2)))),
        ))
    });
    assert_eq!(fib, me);
    let p = b.build();
    // The second call's activation is compared with fib(n), not with the
    // finished activations of the first call's subtree.
    assert_eq!(value(&p, fib, &[json!(12)]), json!(144));
}

#[test]
fn header_measure_that_faults_is_reported_as_that_fault_s34() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(a: Int, b: Int) -> Int decreases a / b { 0 }
    let f = define(
        &mut b,
        m,
        "f",
        &[("a", Ty::Int), ("b", Ty::Int)],
        Ty::Int,
        |f, ps| {
            f.decreases(h.clause(bin(&h, Div, iv(&h, ps[0]), iv(&h, ps[1])), "a / b"));
            h.value(h.int(0))
        },
    );
    let p = b.build();
    let d = fault(&p, f, &[json!(1), json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "a / b");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
    assert_eq!(value(&p, f, &[json!(4), json!(2)]), json!(0));
}

// ===========================================================================
// 9. Faults inside contract expressions (semantics S33, S34, 6.10)
// ===========================================================================

#[test]
fn s34_a_fault_in_a_function_a_clause_calls_names_the_clause_owner() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn helper(n: Int) -> Int { 10 / n }   (the division at 12:5)
    let helper = define(&mut b, m, "helper", &[("n", Ty::Int)], Ty::Int, |_, ps| {
        h.value(spanned(
            bin(&h, Div, h.int(10), iv(&h, ps[0])),
            at(&h, 12, 5, 6),
        ))
    });
    // fn f(n: Int) -> Int requires helper(n) > 0 { n }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(
            bin(
                &h,
                Gt,
                h.call(helper, vec![iv(&h, ps[0])], Ty::Int),
                h.int(0),
            ),
            "helper(n) > 0",
        ));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    let d = fault(&p, f, &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "helper(n) > 0");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["qualified_name"], "m.f");
    assert_eq!(d["input"], json!({"a": 10, "b": 0}));
    // The fault keeps the span of the expression that raised it (6.7).
    assert_eq!(d["span"], loc(12, 5, 6));
    assert_eq!(backtrace(&d), ["m.helper", "m.f"]);
    assert!(d.get("clause_kind").is_none(), "{d}");
}

#[test]
fn s34_resource_exhaustion_inside_a_clause() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn deep(n: Int) -> Int { if n == 0 { 0 } else { deep(n - 1) } }
    let me = b.next_def();
    let deep = define(&mut b, m, "deep", &[("n", Ty::Int)], Ty::Int, |_, ps| {
        h.value(h.if_(
            bin(&h, Eq, iv(&h, ps[0]), h.int(0)),
            h.value(h.int(0)),
            Some(h.value(h.call(me, vec![bin(&h, Sub, iv(&h, ps[0]), h.int(1))], Ty::Int))),
        ))
    });
    assert_eq!(deep, me);
    // fn f(n: Int) -> Int requires deep(n) == 0 { n }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(
            bin(&h, Eq, h.call(deep, vec![iv(&h, ps[0])], Ty::Int), h.int(0)),
            "deep(n) == 0",
        ));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    let d = fault_with(&p, &depth(20), f, &[json!(100)]);
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["category"], "internal");
    assert_eq!(d["resource"], "stack");
    assert_eq!(d["limit"], json!(20));
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "deep(n) == 0");
    assert_eq!(d["symbol"], "m.f");
}

#[test]
fn s33_indirect_recursion_through_a_contract_ends_in_resource_exhaustion() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn f(n: Int) -> Int requires g(n) { n }   fn g(n: Int) -> Bool { f(n) > 0 }
    let f_id = b.next_def();
    let g_id = DefId(f_id.0 + 1);
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(h.call(g_id, vec![iv(&h, ps[0])], Ty::Bool), "g(n)"));
        h.value(iv(&h, ps[0]))
    });
    let g = define(&mut b, m, "g", &[("n", Ty::Int)], Ty::Bool, |_, ps| {
        h.value(bin(
            &h,
            Gt,
            h.call(f_id, vec![iv(&h, ps[0])], Ty::Int),
            h.int(0),
        ))
    });
    assert_eq!((f, g), (f_id, g_id));
    let p = b.build();
    let d = fault_with(&p, &depth(50), f, &[json!(1)]);
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["resource"], "stack");
    assert_eq!(d["limit"], json!(50));
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "g(n)");
    assert_eq!(d["symbol"], "m.f");
}

#[test]
fn s34_an_assertion_in_a_function_a_clause_calls() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn positive(n: Int) -> Bool { assert n > 0  true }
    let positive = define(
        &mut b,
        m,
        "positive",
        &[("n", Ty::Int)],
        Ty::Bool,
        |_, ps| {
            h.block(
                vec![assert_(bin(&h, Gt, iv(&h, ps[0]), h.int(0)), "n > 0")],
                Some(h.bool(true)),
            )
        },
    );
    // fn f(n: Int) -> Int requires positive(n) { n }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(
            h.call(positive, vec![iv(&h, ps[0])], Ty::Bool),
            "positive(n)",
        ));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    let d = fault(&p, f, &[json!(0)]);
    // S34 literally: the assertion's own fault, with clause, symbol, and
    // facet of the clause being evaluated (see the spec issue on `clause`).
    assert_eq!(d["code"], "assertion_failed");
    assert_eq!(d["category"], "test");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["clause"], "positive(n)");
    assert_eq!(
        d["bindings"],
        json!([{"name": "n", "type": "Int", "kind": "parameter", "value": 0}])
    );
    assert_eq!(backtrace(&d), ["m.positive", "m.f"]);
    assert_eq!(value(&p, f, &[json!(2)]), json!(2));
}

#[test]
fn s34_a_hole_reached_from_a_clause() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn todo(n: Int) -> Bool { ?later }   fn f(n: Int) -> Int requires todo(n) { n }
    let todo = define(&mut b, m, "todo", &[("n", Ty::Int)], Ty::Bool, |_, _| {
        h.value(h.hole(0, "later", Ty::Bool))
    });
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(h.call(todo, vec![iv(&h, ps[0])], Ty::Bool), "todo(n)"));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    let d = fault(&p, f, &[json!(1)]);
    assert_eq!(d["code"], "unresolved_hole");
    assert_eq!(d["category"], "hole");
    assert_eq!(d["hole"], "later");
    assert_eq!(d["expected_type"], "Bool");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["clause"], "todo(n)");
}

#[test]
fn s34_a_callee_ensures_violated_inside_a_requires_clause() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn half(n: Int) -> Int ensures result * 2 == n { n / 2 }
    let half = define(&mut b, m, "half", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let r = f.result();
        f.ensures(h.clause(
            bin(&h, Eq, bin(&h, Mul, iv(&h, r), h.int(2)), iv(&h, ps[0])),
            "result * 2 == n",
        ));
        h.value(bin(&h, Div, iv(&h, ps[0]), h.int(2)))
    });
    // fn f(n: Int) -> Int requires half(n) >= 0 { n }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(
            bin(&h, Ge, h.call(half, vec![iv(&h, ps[0])], Ty::Int), h.int(0)),
            "half(n) >= 0",
        ));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    // S34 lists "a nested contract_violation from a called function's own
    // contract": reported as that violation, with the clause and symbol of
    // the clause being evaluated.
    let d = fault(&p, f, &[json!(5)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["clause"], "half(n) >= 0");
    assert_eq!(d["input"], json!({"n": 5}));
    assert_eq!(d["actual"], json!(2));
    assert_eq!(backtrace(&d), ["m.half", "m.f"]);
    assert_eq!(value(&p, f, &[json!(4)]), json!(4));
}

#[test]
fn s34_a_builtin_precondition_violated_inside_a_clause() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let take = b.builtin("list_take");
    let lt = Ty::list(Ty::Int);
    // fn f(xs: List<Int>, n: Int) -> Int requires xs.take(n) == xs { n }
    let f = define(
        &mut b,
        m,
        "f",
        &[("xs", lt.clone()), ("n", Ty::Int)],
        Ty::Int,
        |f, ps| {
            let xs = || h.local(ps[0], lt.clone());
            f.requires(h.clause(
                bin(
                    &h,
                    Eq,
                    h.call_generic(take, vec![Ty::Int], vec![xs(), iv(&h, ps[1])], lt.clone()),
                    xs(),
                ),
                "xs.take(n) == xs",
            ));
            h.value(iv(&h, ps[1]))
        },
    );
    let p = b.build();
    let d = fault(&p, f, &[json!([1]), json!(-1)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.f");
    assert_eq!(d["clause"], "xs.take(n) == xs");
    // The builtin's caller is the declaration whose clause called it (6.10).
    assert_eq!(d["caller"], "m.f");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
    assert_eq!(backtrace(&d), ["std.prelude.List.take", "m.f"]);
    assert_eq!(value(&p, f, &[json!([1]), json!(5)]), json!(5));
}

#[test]
fn s34_a_predicate_fault_inside_a_clause_names_the_refinement_6_10() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let is_ok = b.builtin("result_is_ok");
    // type Nz = Int where 10 / self > 0
    let nz_pred = bin(
        &h,
        Gt,
        bin(&h, Div, h.int(10), h.local(LocalId(0), Ty::Int)),
        h.int(0),
    );
    let nz = b.add_refinement(m, "Nz", Ty::Int, nz_pred, "10 / self > 0");
    let nz_ty = b.refined(nz);
    let ce = h.constraint_error_ty();
    // fn f(n: Int) -> Int requires check<Nz>(n).is_ok() { n }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let checked = h.check(nz, iv(&h, ps[0]), h.result_ty(nz_ty.clone(), ce.clone()));
        f.requires(h.clause(
            h.call_generic(
                is_ok,
                vec![nz_ty.clone(), ce.clone()],
                vec![checked],
                Ty::Bool,
            ),
            "check<Nz>(n).is_ok()",
        ));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    // 6.10: a fault inside a predicate is reported with symbol = the
    // refinement and clause = the predicate text.
    let d = fault(&p, f, &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.Nz");
    assert_eq!(d["clause"], "10 / self > 0");
    assert_eq!(backtrace(&d), ["m.Nz", "m.f"]);
    // A false predicate is an Err value, so the requires itself is false.
    let d = fault(&p, f, &[json!(20)]);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause"], "check<Nz>(n).is_ok()");
    assert_eq!(value(&p, f, &[json!(5)]), json!(5));
}

#[test]
fn s34_a_fault_in_a_property_for_range_is_a_contract_fault_5_1() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // property "ranged" for x in gen.range(1 / 0, 5) { assert x >= 0 }
    let mut pb = PropertyBuilder::new("ranged");
    let lo = bin(&h, Div, h.int(1), h.int(0));
    let x = pb.binder(
        "x",
        GenExpr::Range(Box::new(lo), Box::new(h.int(5))),
        Ty::Int,
    );
    pb.body(h.block(
        vec![assert_(bin(&h, Ge, iv(&h, x), h.int(0)), "x >= 0")],
        None,
    ));
    let prop = b.add_property(m, pb);
    let p = b.build();
    let record = run_test(&p, prop, &TestConfig::with_seed(0));
    assert_eq!(record.status, TestStatus::Failed);
    // toolchain 1.7.2: a fault in the range arguments fails the property with zero cases.
    assert_eq!(record.cases, Some(0));
    let d = serde_json::to_value(record.fault.as_ref().expect("the property's fault")).unwrap();
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
    // 5.1: a property `for` range is a contract expression, so S34 reports
    // the fault with facet contract, the clause, and the declaration whose
    // clause was being evaluated (the property).
    assert_eq!(d["symbol"], "m.\"ranged\"");
    assert_eq!(d["facet"], "contract", "{d}");
    assert!(d["clause"].is_string(), "S34 names the clause: {d}");
}

#[test]
fn satisfies_refinement_runs_the_whole_chain_with_the_full_evaluator_1_7() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let positive = b.add_int_refinement(m, "Positive", Gt, 0);
    let pos_ty = b.refined(positive);
    // type Small = Positive where self < 10 (`self` widened to Int for `<`,
    // as the checker types it)
    let self_int = h.widen(h.local(LocalId(0), pos_ty.clone()), positive, Ty::Int);
    let small_pred = bin(&h, Lt, self_int, h.int(10));
    let small = b.add_refinement(m, "Small", pos_ty, small_pred, "self < 10");
    // type Nz = Int where 10 / self > 0
    let nz_pred = bin(
        &h,
        Gt,
        bin(&h, Div, h.int(10), h.local(LocalId(0), Ty::Int)),
        h.int(0),
    );
    let nz = b.add_refinement(m, "Nz", Ty::Int, nz_pred, "10 / self > 0");
    // fn is_even(n: Int) -> Bool { n % 2 == 0 }   type Even = Int where is_even(self)
    let is_even = define(
        &mut b,
        m,
        "is_even",
        &[("n", Ty::Int)],
        Ty::Bool,
        |_, ps| h.value(bin(&h, Eq, bin(&h, Rem, iv(&h, ps[0]), h.int(2)), h.int(0))),
    );
    let even_pred = h.call(is_even, vec![h.local(LocalId(0), Ty::Int)], Ty::Bool);
    let even = b.add_refinement(m, "Even", Ty::Int, even_pred, "is_even(self)");
    let p = b.build();
    let results = on_eval_stack(|| {
        let mut it = Interpreter::new(&p, EvalConfig::default());
        let mut check = |r: DefId, n: &str| it.satisfies_refinement(r, &Value::Int(big(n)));
        let bools: Vec<bool> = [
            (positive, "1"),
            (positive, "0"),
            (positive, "1267650600228229401496703205376"),
            (positive, "-1267650600228229401496703205376"),
            (small, "9"),
            (small, "10"),
            (small, "0"),
            (nz, "5"),
            (nz, "11"),
            (even, "4"),
            (even, "-3"),
            (even, "1267650600228229401496703205376"),
        ]
        .into_iter()
        .map(|(r, n)| check(r, n).unwrap_or_else(|e| panic!("{n}: {e}")))
        .collect();
        let fault = check(nz, "0").expect_err("10 / 0 faults");
        (bools, diag(&p, &fault))
    });
    let (bools, d) = results;
    assert_eq!(
        bools,
        [true, false, true, false, true, false, false, true, false, true, false, true]
    );
    // A predicate that faults is that fault, per S34 (6.10).
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "m.Nz");
    assert_eq!(d["clause"], "10 / self > 0");
}

// ===========================================================================
// 10. Capability operations (toolchain 1.7.1 rows 6-7; semantics S31, S34)
// ===========================================================================

/// A binding that answers its argument and counts its calls.
struct Echo {
    calls: Rc<Cell<u32>>,
}

impl CapabilityImpl for Echo {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        _: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        assert_eq!(op, "get");
        self.calls.set(self.calls.get() + 1);
        Ok(args.into_iter().next().expect("one argument"))
    }
}

#[test]
fn capability_arguments_then_operation_requires_then_the_host_s31_s34() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let fa = bomb(&mut b, m, &h, "a", Ty::Int, h.int(0));
    let store = b.add_capability(m, "Store", &[("get", vec![("id", Ty::Int)], Ty::Int)]);
    let cap = Ty::Capability(store);
    // fn load(id: Int, using db: Store) -> Int { db.get(id) }
    let mut f = FnBuilder::new("load", Ty::Int);
    let id = f.param("id", Ty::Int);
    let db = f.using("db", store);
    f.body(h.value(h.cap_call(
        h.local(db, cap.clone()),
        store,
        0,
        vec![iv(&h, id)],
        Ty::Int,
    )));
    let load = b.add_fn(m, f);
    // fn load_a(using db: Store) -> Int { db.get(a()) }
    let mut f = FnBuilder::new("load_a", Ty::Int);
    let db = f.using("db", store);
    f.body(h.value(h.cap_call(
        h.local(db, cap.clone()),
        store,
        0,
        vec![h.call(fa, vec![], Ty::Int)],
        Ty::Int,
    )));
    let load_a = b.add_fn(m, f);
    let mut p = b.build();
    // The operation's contract: requires 100 / id > 0 (the builder adds operations without one).
    let DefKind::Capability(capability) = &mut p.defs[store.0 as usize].kind else {
        unreachable!("Store is a capability")
    };
    capability.ops[0].requires.push(h.clause(
        bin(
            &h,
            Gt,
            bin(&h, Div, h.int(100), h.local(LocalId(0), Ty::Int)),
            h.int(0),
        ),
        "100 / id > 0",
    ));
    let calls = Rc::new(Cell::new(0));
    let mut it = Interpreter::new(&p, EvalConfig::default());
    let handle = it.bind_capability(
        store,
        Box::new(Echo {
            calls: Rc::clone(&calls),
        }),
        "fixture:store",
    );
    // The arguments first: their fault ends the call before the requires and the host.
    let d = diag(
        &p,
        &it.call(load_a, vec![], vec![handle.clone()]).unwrap_err(),
    );
    assert_eq!(d["code"], "assertion_failed");
    assert_eq!(d["clause"], "A");
    assert_eq!(calls.get(), 0);
    // A faulting operation requires: S34 names the operation; the host is not invoked.
    let d = diag(
        &p,
        &it.call(load, vec![Value::int(0)], vec![handle.clone()])
            .unwrap_err(),
    );
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "100 / id > 0");
    assert_eq!(d["symbol"], "m.Store.get");
    assert_eq!(d["qualified_name"], "m.Store.get");
    assert_eq!(calls.get(), 0);
    // A false operation requires.
    let d = diag(
        &p,
        &it.call(load, vec![Value::int(200)], vec![handle.clone()])
            .unwrap_err(),
    );
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["symbol"], "m.Store.get");
    assert_eq!(d["caller"], "m.load");
    assert_eq!(d["input"], json!({"id": 200}));
    assert_eq!(calls.get(), 0);
    assert_eq!(
        it.call(load, vec![Value::int(5)], vec![handle]).unwrap(),
        Value::int(5)
    );
    assert_eq!(calls.get(), 1);
}

// ===========================================================================
// 11. Fault payloads and categories (toolchain 1.7.3; semantics 6.7, 9.2-9.5)
// ===========================================================================

#[test]
fn payload_every_fault_kind_has_its_code_category_and_keys_1_7_3() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let positive = b.add_int_refinement(m, "Positive", Gt, 0);
    let pos_ty = b.refined(positive);
    let req = define(&mut b, m, "req", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(bin(&h, Gt, iv(&h, ps[0]), h.int(0)), "n > 0"));
        h.value(iv(&h, ps[0]))
    });
    let asserting = define(
        &mut b,
        m,
        "asserting",
        &[("n", Ty::Int)],
        Ty::Int,
        |_, ps| {
            h.block(
                vec![assert_(bin(&h, Gt, iv(&h, ps[0]), h.int(0)), "n > 0")],
                Some(iv(&h, ps[0])),
            )
        },
    );
    let holey = define(&mut b, m, "holey", &[("n", Ty::Int)], Ty::Int, |_, _| {
        h.value(h.hole(0, "todo", Ty::Int))
    });
    let div = fn2(&mut b, m, &h, "div", Ty::Int, |h, a, d| {
        bin(h, Div, iv(h, a), iv(h, d))
    });
    let pos = define(
        &mut b,
        m,
        "pos",
        &[("p", pos_ty.clone())],
        Ty::Int,
        |_, ps| h.value(h.widen(h.local(ps[0], pos_ty.clone()), positive, Ty::Int)),
    );
    let me = b.next_def();
    let forever = define(&mut b, m, "forever", &[("n", Ty::Int)], Ty::Int, |_, ps| {
        h.value(h.call(me, vec![bin(&h, Add, iv(&h, ps[0]), h.int(1))], Ty::Int))
    });
    assert_eq!(forever, me);
    let spin = define(&mut b, m, "spin", &[], Ty::Int, |_, _| {
        h.block(
            vec![while_(h.bool(true), vec![], None, h.block(vec![], None))],
            Some(h.int(0)),
        )
    });
    let no_arm = define(&mut b, m, "no_arm", &[("n", Ty::Int)], Ty::Int, |_, ps| {
        h.value(h.match_(
            iv(&h, ps[0]),
            vec![h.arm(
                h.pattern(PatternKind::Int(1.into()), Ty::Int),
                h.value(h.int(1)),
            )],
            Ty::Int,
        ))
    });
    let p = b.build();
    let refinement = diag(
        &p,
        run(
            &p,
            pos,
            vec![RunArg::Json(json!(0))],
            &EvalConfig::default(),
        )
        .value
        .as_ref()
        .expect_err("a refinement violation"),
    );
    let cases = [
        (
            fault(&p, req, &[json!(0)]),
            "contract_violation",
            "contract",
            Some("m.req"),
        ),
        (
            fault(&p, asserting, &[json!(0)]),
            "assertion_failed",
            "test",
            Some("m.asserting"),
        ),
        (
            fault(&p, holey, &[json!(0)]),
            "unresolved_hole",
            "hole",
            Some("m.holey"),
        ),
        (
            fault(&p, div, &[json!(1), json!(0)]),
            "division_by_zero",
            "runtime",
            Some("m.div"),
        ),
        (refinement, "refinement_violation", "runtime", Some("m.pos")),
        (
            fault_with(&p, &depth(10), forever, &[json!(0)]),
            "resource_exhaustion",
            "internal",
            Some("m.forever"),
        ),
        (
            fault_with(&p, &budget(20), spin, &[]),
            "resource_exhaustion",
            "internal",
            Some("m.spin"),
        ),
        (
            fault(&p, no_arm, &[json!(2)]),
            "internal_error",
            "internal",
            None,
        ),
    ];
    for (d, code, category, symbol) in &cases {
        assert_eq!(d["code"], *code, "{d}");
        assert_eq!(d["category"], *category, "{d}");
        assert_eq!(d["severity"], "error", "{d}");
        assert!(d["message"].as_str().is_some_and(|s| !s.is_empty()), "{d}");
        assert_no_null(d, "");
        assert!(d.get("span").is_some(), "a fault carries a span (6.7): {d}");
        match symbol {
            Some(s) => {
                assert_eq!(d["symbol"], *s, "{d}");
                assert_eq!(d["qualified_name"], *s, "{d}");
            }
            None => {
                // internal_error blames no declaration (1.7.3).
                assert!(
                    d.get("symbol").is_none() && d.get("qualified_name").is_none(),
                    "{d}"
                );
            }
        }
        if *code == "contract_violation" {
            assert_eq!(d["facet"], "contract");
            assert_eq!(d["status"], "failed");
        } else {
            if symbol.is_some() {
                assert_eq!(d["facet"], "body", "{d}");
            }
            // 9.5: `status` appears only on the codes listed there; 9.2:
            // `clause_kind` and `attributed_to` belong to contract_violation.
            for key in ["status", "clause_kind", "attributed_to"] {
                assert!(d.get(key).is_none(), "{key} on {d}");
            }
        }
        for frame in d["backtrace"].as_array().into_iter().flatten() {
            assert!(frame["qualified_name"].is_string(), "{d}");
        }
    }
    let [cv, af, uh, dz, rv, stack, time, ie] = &cases;
    assert_eq!(cv.0["clause_kind"], "requires");
    assert_eq!(cv.0["attributed_to"], "caller");
    assert_eq!(cv.0["clause"], "n > 0");
    assert_eq!(cv.0["input"], json!({"n": 0}));
    assert_eq!(af.0["clause"], "n > 0");
    assert_eq!(
        af.0["bindings"],
        json!([{"name": "n", "type": "Int", "kind": "parameter", "value": 0}])
    );
    assert_eq!(uh.0["hole"], "todo");
    assert_eq!(uh.0["expected_type"], "Int");
    assert_eq!(
        uh.0["bindings"],
        json!([{"name": "n", "type": "Int", "kind": "parameter", "value": 0}])
    );
    assert_eq!(dz.0["input"], json!({"a": 1, "b": 0}));
    assert_eq!(rv.0["expected_type"], "Positive");
    assert_eq!(rv.0["actual"], json!(0));
    assert_eq!(rv.0["clause"], "self > 0");
    assert_eq!(stack.0["resource"], "stack");
    assert_eq!(stack.0["limit"], json!(10));
    assert_eq!(backtrace(&stack.0).len(), 10);
    assert_eq!(time.0["resource"], "time");
    assert_eq!(time.0["limit"], json!(20));
    assert!(
        ie.0["kind"].as_str().is_some_and(|k| !k.is_empty()),
        "{}",
        ie.0
    );
    assert!(ie.0["detail"].is_string(), "{}", ie.0);
    for (d, ..) in [cv, af, uh, dz, stack, time, ie] {
        assert!(
            !backtrace(d).is_empty(),
            "a fault inside a declaration has a backtrace: {d}"
        );
    }
}

// ===========================================================================
// 12. Value JSON and value text (toolchain 7.3, 7.2; semantics S40)
// ===========================================================================

#[test]
fn value_ints_beyond_64_bits_keep_every_digit_7_3() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn pow2(n: Int) -> Int { var r = 1  var i = 0  while i < n { r = r * 2  i = i + 1 }  r }
    let pow2 = define(&mut b, m, "pow2", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        let r = var_int(f, "r");
        let i = var_int(f, "i");
        h.block(
            vec![
                let_(r, h.int(1)),
                let_(i, h.int(0)),
                while_(
                    bin(&h, Lt, iv(&h, i), iv(&h, ps[0])),
                    vec![],
                    None,
                    h.block(
                        vec![
                            assign(r, bin(&h, Mul, iv(&h, r), h.int(2))),
                            assign(i, bin(&h, Add, iv(&h, i), h.int(1))),
                        ],
                        None,
                    ),
                ),
            ],
            Some(iv(&h, r)),
        )
    });
    // fn negate(n: Int) -> Int { -n }
    let negate = define(&mut b, m, "negate", &[("n", Ty::Int)], Ty::Int, |_, ps| {
        h.value(neg(&h, iv(&h, ps[0])))
    });
    let p = b.build();
    let two200 = "1606938044258990275541962092341162602522202993782792835301376";
    let rendered = |outcome: &RunOutcome| {
        let v = outcome
            .value
            .as_ref()
            .unwrap_or_else(|e| panic!("{}", diag(&p, e)));
        let j = value_to_json(&p, v, &outcome.return_ty);
        assert_eq!(&value_from_json(&p, &j, &outcome.return_ty).unwrap(), v);
        (
            serde_json::to_string(&j).unwrap(),
            value_text(&p, v, &outcome.return_ty),
        )
    };
    let outcome = run(&p, pow2, vec![c("200")], &EvalConfig::default());
    assert_eq!(rendered(&outcome), (two200.to_string(), two200.to_string()));
    let minus = format!("-{two200}");
    let outcome = run(
        &p,
        negate,
        vec![RunArg::Json(num(two200))],
        &EvalConfig::default(),
    );
    assert_eq!(rendered(&outcome), (minus.clone(), minus.clone()));
    let outcome = run(&p, negate, vec![c(&minus)], &EvalConfig::default());
    assert_eq!(rendered(&outcome), (two200.to_string(), two200.to_string()));
    // Either side of the u64 boundary of 64-bit JSON readers.
    for n in [
        "18446744073709551615",
        "18446744073709551616",
        "9223372036854775808",
    ] {
        let outcome = run(
            &p,
            negate,
            vec![c(&format!("-{n}"))],
            &EvalConfig::default(),
        );
        assert_eq!(rendered(&outcome), (n.to_string(), n.to_string()));
    }
}

/// The program of the value-form tests: module `m` with a wrapper over a
/// wrapper, an enum with a payload-less and a two-payload variant, a struct
/// whose fields are not in alphabetical order, and one identity function
/// `id_<name>(x: T) -> T { x }` per type form.
struct Forms {
    p: Program,
    ids: BTreeMap<&'static str, DefId>,
}

fn forms() -> Forms {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let std = b.std_defs();
    let meters = b.add_wrapper(m, "Meters", Ty::Int);
    let outer = b.add_wrapper(m, "Outer", Ty::Wrapper(meters));
    let shape = b.add_enum(
        m,
        "Shape",
        &[],
        &[("Dot", vec![]), ("Line", vec![Ty::Int, Ty::Int])],
    );
    let rec = b.add_struct(m, "Rec", &[], &[("z", Ty::Int), ("a", Ty::Text)]);
    let o1 = Ty::option(&std, Ty::Int);
    let o3 = Ty::option(&std, Ty::option(&std, o1.clone()));
    let types: [(&'static str, Ty); 12] = [
        ("int", Ty::Int),
        ("text", Ty::Text),
        ("bool", Ty::Bool),
        ("unit", Ty::unit(&std)),
        ("bytes", Ty::Bytes),
        ("lists", Ty::list(Ty::list(Ty::Int))),
        ("rec", b.adt(rec)),
        ("shapes", Ty::list(b.adt(shape))),
        ("opts", Ty::list(o3)),
        ("res", Ty::result(&std, o1, Ty::option(&std, Ty::Text))),
        ("outer", Ty::Wrapper(outer)),
        ("ce", Ty::Adt(std.constraint_error, vec![])),
    ];
    let mut ids = BTreeMap::new();
    for (name, ty) in types {
        let f = define(
            &mut b,
            m,
            &format!("id_{name}"),
            &[("x", ty.clone())],
            ty.clone(),
            |_, ps| h.value(h.local(ps[0], ty.clone())),
        );
        ids.insert(name, f);
    }
    Forms { p: b.build(), ids }
}

/// A text with every class of S40's escape rule: the escaped `"` and `\`,
/// the three named controls, other C0 controls and DEL, then scalars that
/// are written literally (C1 controls, NBSP, a combining mark, astral and
/// noncharacter-adjacent code points, BOM, LINE SEPARATOR).
const TRICKY: &str = "q\"b\\s\n\r\t\u{0}\u{1}\u{1b}\u{1f}\u{7f}\u{80}\u{85}\u{9f}\u{a0}\u{e9} e\u{301}\u{1F600}\u{10FFFF}\u{FEFF}\u{2028}\u{FFFD}";
const TRICKY_TEXT: &str = "\"q\\\"b\\\\s\\n\\r\\t\\u{0}\\u{1}\\u{1b}\\u{1f}\\u{7f}\u{80}\u{85}\u{9f}\u{a0}\u{e9} e\u{301}\u{1F600}\u{10FFFF}\u{FEFF}\u{2028}\u{FFFD}\"";

/// (identity function, value JSON, value text) for every form.
fn form_cases() -> Vec<(&'static str, Json, String)> {
    vec![
        (
            "int",
            num("-1606938044258990275541962092341162602522202993782792835301376"),
            "-1606938044258990275541962092341162602522202993782792835301376".to_string(),
        ),
        ("text", json!(TRICKY), TRICKY_TEXT.to_string()),
        ("text", json!(""), "\"\"".to_string()),
        ("bool", json!(false), "false".to_string()),
        ("unit", unit_v(), "Unit".to_string()),
        ("lists", json!([[], [1, -2]]), "[[], [1, -2]]".to_string()),
        (
            "rec",
            json!({"$type": "m.Rec", "z": -1, "a": "\""}),
            // Fields in declaration order (z before a), not alphabetical.
            r#"Rec { z: -1, a: "\"" }"#.to_string(),
        ),
        (
            "shapes",
            json!([{"$type": "m.Shape", "$variant": "Dot"}, {"$type": "m.Shape", "$variant": "Line", "$payload": [1, -2]}]),
            "[Dot, Line(1, -2)]".to_string(),
        ),
        (
            "opts",
            json!([none(), some(none()), some(some(none())), some(some(some(json!(-1))))]),
            "[None, Some(None), Some(Some(None)), Some(Some(Some(-1)))]".to_string(),
        ),
        ("res", err_v(some(json!("x"))), r#"Err(Some("x"))"#.to_string()),
        ("res", ok_v(none()), "Ok(None)".to_string()),
        (
            "outer",
            json!({"$type": "m.Outer", "$payload": [{"$type": "m.Meters", "$payload": [-3]}]}),
            "Outer(Meters(-3))".to_string(),
        ),
        (
            "ce",
            json!({"$type": "ConstraintError", "type_name": "m.Positive", "message": "0 does not satisfy self > 0"}),
            r#"ConstraintError { type_name: "m.Positive", message: "0 does not satisfy self > 0" }"#.to_string(),
        ),
    ]
}

#[test]
fn value_every_form_has_the_s40_json_and_text_and_json_round_trips() {
    let fx = forms();
    let p = &fx.p;
    for (name, j, text) in form_cases() {
        let ty = p.def(fx.ids[name]).kind.as_fn().unwrap().ret.clone();
        let v = value_from_json(p, &j, &ty).unwrap_or_else(|e| panic!("{j} at {name}: {e}"));
        assert_eq!(value_text(p, &v, &ty), text, "value text of {j}");
        assert_eq!(value_to_json(p, &v, &ty), j, "value JSON of {text}");
        // Through the evaluator: an identity call returns the same value.
        assert_eq!(
            value(p, fx.ids[name], std::slice::from_ref(&j)),
            j,
            "id_{name}"
        );
    }
    // Bytes: value JSON only (value text is a display form, D8).
    let ty = Ty::Bytes;
    let j = bytes_v("00ff7f80");
    let v = value_from_json(p, &j, &ty).unwrap();
    assert_eq!(value_text(p, &v, &ty), r#"Bytes("00ff7f80")"#);
    assert_eq!(value_to_json(p, &v, &ty), j);
    assert_eq!(value(p, fx.ids["bytes"], std::slice::from_ref(&j)), j);
}

#[test]
fn value_text_re_parses_as_a_constant_expression_of_its_type_s40_7_2() {
    // S40: value text "is CPL literal syntax"; every form below lies in the
    // constant-expression grammar of toolchain 7.2 (literals, constructors,
    // `[...]`, struct literals, unary minus) and every name in it is
    // unambiguous in this program, so `cpl run id_T '<value text>'` must
    // return the value itself. (Bytes is excluded: no literal form, D8.)
    let fx = forms();
    let p = &fx.p;
    for (name, j, text) in form_cases() {
        let outcome = run(p, fx.ids[name], vec![c(&text)], &EvalConfig::default());
        let back = outcome
            .value
            .as_ref()
            .unwrap_or_else(|e| panic!("{text}: {}", diag(p, e)));
        assert_eq!(
            value_to_json(p, back, &outcome.return_ty),
            j,
            "{text} re-parsed"
        );
        assert_eq!(value_text(p, back, &outcome.return_ty), text);
    }
}

#[test]
fn value_nested_options_stay_distinguishable_from_the_evaluator_s40() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let o1 = h.option_ty(Ty::Int);
    let o2 = h.option_ty(o1.clone());
    let o3 = h.option_ty(o2.clone());
    // fn opts(n: Int) -> List<Option<Option<Option<Int>>>> { [None, Some(None), Some(Some(None)), Some(Some(Some(n)))] }
    let opts = define(
        &mut b,
        m,
        "opts",
        &[("n", Ty::Int)],
        Ty::list(o3.clone()),
        |_, ps| {
            h.value(h.list(
                vec![
                    h.none(o2.clone()),
                    h.some(h.none(o1.clone())),
                    h.some(h.some(h.none(Ty::Int))),
                    h.some(h.some(h.some(iv(&h, ps[0])))),
                ],
                o3.clone(),
            ))
        },
    );
    let p = b.build();
    let outcome = run(&p, opts, vec![c("-1")], &EvalConfig::default());
    let v = outcome.value.as_ref().expect("a value");
    let expected = json!([
        none(),
        some(none()),
        some(some(none())),
        some(some(some(json!(-1))))
    ]);
    assert_eq!(value_to_json(&p, v, &outcome.return_ty), expected);
    assert_eq!(
        value_text(&p, v, &outcome.return_ty),
        "[None, Some(None), Some(Some(None)), Some(Some(Some(-1)))]"
    );
    // Each element decodes to itself and is equal to no other.
    let items = v.as_list().expect("a list");
    for (i, j) in expected.as_array().unwrap().iter().enumerate() {
        let x = value_from_json(&p, j, &o3).unwrap();
        for (k, y) in items.iter().enumerate() {
            assert_eq!(x.structurally_equal(y), i == k, "element {i} against {k}");
        }
    }
}

#[test]
fn value_every_type_form_built_by_the_evaluator_s40() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let positive = b.add_int_refinement(m, "Positive", Gt, 0);
    let pos_ty = b.refined(positive);
    let meters = b.add_wrapper(m, "Meters", Ty::Int);
    let outer = b.add_wrapper(m, "Outer", Ty::Wrapper(meters));
    let shape = b.add_enum(
        m,
        "Shape",
        &[],
        &[("Dot", vec![]), ("Line", vec![Ty::Int, Ty::Int])],
    );
    let rec = b.add_struct(m, "Rec", &[], &[("z", Ty::Int), ("a", Ty::Text)]);
    let checked = h.result_ty(pos_ty.clone(), h.constraint_error_ty());
    let shape_ty = b.adt(shape);
    let rec_ty = b.adt(rec);
    let all = b.add_struct(
        m,
        "All",
        &[],
        &[
            ("u", h.unit_ty()),
            ("b", Ty::Bytes),
            ("p", pos_ty.clone()),
            ("w", Ty::Wrapper(outer)),
            ("s", Ty::list(shape_ty.clone())),
            ("r", rec_ty.clone()),
            ("c", checked.clone()),
            ("o", h.option_ty(h.unit_ty())),
        ],
    );
    let all_ty = b.adt(all);
    // fn every(bs: Bytes, n: Int) -> All { All { u: Unit, b: bs, p: 5, w: Outer(Meters(n)),
    //     s: [Dot, Line(n, -n)], r: Rec { z: n, a: "t\"" }, c: check<Positive>(0 - n), o: Some(Unit) } }
    let every = define(
        &mut b,
        m,
        "every",
        &[("bs", Ty::Bytes), ("n", Ty::Int)],
        all_ty.clone(),
        |_, ps| {
            let (bs, n) = (ps[0], ps[1]);
            h.value(h.struct_lit(
                all,
                None,
                vec![
                    (0, h.unit()),
                    (1, h.local(bs, Ty::Bytes)),
                    (2, h.refine(h.int(5), positive, pos_ty.clone())),
                    (3, h.wrap(outer, h.wrap(meters, iv(&h, n)))),
                    (
                        4,
                        h.list(
                            vec![
                                h.ctor(shape, 0, vec![], shape_ty.clone()),
                                h.ctor(
                                    shape,
                                    1,
                                    vec![iv(&h, n), neg(&h, iv(&h, n))],
                                    shape_ty.clone(),
                                ),
                            ],
                            shape_ty.clone(),
                        ),
                    ),
                    (
                        5,
                        h.struct_lit(
                            rec,
                            None,
                            vec![(0, iv(&h, n)), (1, h.text("t\""))],
                            rec_ty.clone(),
                        ),
                    ),
                    (
                        6,
                        h.check(positive, bin(&h, Sub, h.int(0), iv(&h, n)), checked.clone()),
                    ),
                    (7, h.some(h.unit())),
                ],
                all_ty.clone(),
            ))
        },
    );
    let p = b.build();
    let outcome = run(
        &p,
        every,
        vec![RunArg::Json(bytes_v("00ff10")), c("3")],
        &EvalConfig::default(),
    );
    let v = outcome
        .value
        .as_ref()
        .unwrap_or_else(|e| panic!("{}", diag(&p, e)));
    let expected = json!({
        "$type": "m.All",
        "u": {"$type": "Unit"},
        "b": {"$type": "Bytes", "hex": "00ff10"},
        "p": 5,
        "w": {"$type": "m.Outer", "$payload": [{"$type": "m.Meters", "$payload": [3]}]},
        "s": [{"$type": "m.Shape", "$variant": "Dot"}, {"$type": "m.Shape", "$variant": "Line", "$payload": [3, -3]}],
        "r": {"$type": "m.Rec", "z": 3, "a": "t\""},
        "c": {"$type": "Result", "$variant": "Err", "$payload": [
            {"$type": "ConstraintError", "type_name": "m.Positive", "message": "-3 does not satisfy self > 0"}]},
        "o": {"$type": "Option", "$variant": "Some", "$payload": [{"$type": "Unit"}]}
    });
    assert_eq!(value_to_json(&p, v, &outcome.return_ty), expected);
    assert_eq!(
        value_text(&p, v, &outcome.return_ty),
        r#"All { u: Unit, b: Bytes("00ff10"), p: 5, w: Outer(Meters(3)), s: [Dot, Line(3, -3)], r: Rec { z: 3, a: "t\"" }, c: Err(ConstraintError { type_name: "m.Positive", message: "-3 does not satisfy self > 0" }), o: Some(Unit) }"#
    );
    assert_eq!(
        &value_from_json(&p, &expected, &outcome.return_ty).unwrap(),
        v
    );
}

#[test]
fn value_text_inside_constraint_error_messages_uses_the_s40_escapes_6_10() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let text_len = b.builtin("text_len");
    // type Short = Text where self.len < 2
    let pred = bin(
        &h,
        Lt,
        h.call(text_len, vec![h.local(LocalId(0), Ty::Text)], Ty::Int),
        h.int(2),
    );
    let short = b.add_refinement(m, "Short", Ty::Text, pred, "self.len < 2");
    let rt = h.result_ty(b.refined(short), h.constraint_error_ty());
    // fn to_short(t: Text) -> Result<Short, ConstraintError> { check<Short>(t) }
    let to_short = define(
        &mut b,
        m,
        "to_short",
        &[("t", Ty::Text)],
        rt.clone(),
        |_, ps| h.value(h.check(short, h.local(ps[0], Ty::Text), rt.clone())),
    );
    let p = b.build();
    let ce = |message: &str| {
        err_v(json!({"$type": "ConstraintError", "type_name": "m.Short", "message": message}))
    };
    assert_eq!(
        value(&p, to_short, &[json!("a\"b\n\u{7f}")]),
        ce(r#""a\"b\n\u{7f}" does not satisfy self.len < 2"#)
    );
    assert_eq!(
        value(&p, to_short, &[json!("e\u{301}")]),
        ce("\"e\u{301}\" does not satisfy self.len < 2")
    );
    assert_eq!(
        value(&p, to_short, &[json!("\u{e9}")]),
        ok_v(json!("\u{e9}"))
    );
}

#[test]
fn value_json_decoding_locates_errors_inside_nested_forms_7_3() {
    let p = ProgramBuilder::new().build();
    let std = p.std_defs;
    let o1 = Ty::option(&std, Ty::Int);
    let o2 = Ty::option(&std, o1.clone());
    let pointer = |j: Json, ty: &Ty| {
        value_from_json(&p, &j, ty)
            .expect_err("must not decode")
            .pointer
    };
    // The inner Some needs its payload; the inner None must omit it.
    assert_eq!(
        pointer(some(json!({"$type": "Option", "$variant": "Some"})), &o2),
        "/$payload/0"
    );
    assert_eq!(
        pointer(
            some(json!({"$type": "Option", "$variant": "None", "$payload": []})),
            &o2
        ),
        "/$payload/0/$payload"
    );
    // Int: never a string, no fraction, no exponent.
    assert_eq!(pointer(json!([1, num("2.5")]), &Ty::list(Ty::Int)), "/1");
    assert_eq!(pointer(json!([1, num("1e3")]), &Ty::list(Ty::Int)), "/1");
    assert_eq!(pointer(json!(["5"]), &Ty::list(Ty::Int)), "/0");
    // An Int beyond 64 bits nested in an Option decodes exactly.
    let j = some(num("123456789012345678901234567890"));
    let v = value_from_json(&p, &j, &o1).unwrap();
    assert_eq!(value_to_json(&p, &v, &o1), j);
    assert_eq!(
        value_text(&p, &v, &o1),
        "Some(123456789012345678901234567890)"
    );
}

/// Set in the child process of [`value_a_deeply_nested_result_is_returned_without_crashing`].
const DEEP_CHILD: &str = "CPL_ADVERSARIAL_SEMANTICS_DEEP_CHILD";

#[test]
fn value_a_deeply_nested_result_is_returned_without_crashing() {
    // A loop builds a linked list of 10 000 nodes (a value nested 20 000
    // levels deep) without approaching any call-depth limit; a recursive
    // builder within the default limit of 10 000 activations (S38) reaches
    // the same depth. `cpl run` must return it (toolchain 7.2, 7.4: exit 0
    // with the value), and S38's rationale is that the runtime never ends in
    // a process crash. The specifications bound no value depth (see the spec
    // issue). A host stack overflow aborts the whole process, so the check
    // runs in a child process of this test binary, on the default test
    // thread like any caller of `run_function`.
    if std::env::var_os(DEEP_CHILD).is_some() {
        deep_value_child();
        return;
    }
    let exe = std::env::current_exe().expect("the path of this test binary");
    let output = std::process::Command::new(exe)
        .args([
            "--exact",
            "value_a_deeply_nested_result_is_returned_without_crashing",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(DEEP_CHILD, "1")
        .output()
        .expect("this test binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
    assert!(
        output.status.success(),
        "run_function of chain(10000) did not return its value: the process ended with {} ({})",
        output.status,
        tail.join(" | ")
    );
}

fn deep_value_child() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let std = b.std_defs();
    // struct Node { next: Option<Node> }: legal, the cycle passes through Option (S7).
    let node = b.next_def();
    let node_ty = Ty::Adt(node, vec![]);
    let declared = b.add_struct(
        m,
        "Node",
        &[],
        &[("next", Ty::option(&std, node_ty.clone()))],
    );
    assert_eq!(declared, node);
    // fn chain(n: Int) -> Node { var acc = Node { next: None }  var i = 0
    //     while i < n { acc = Node { next: Some(acc) }  i = i + 1 }  acc }
    let chain = define(
        &mut b,
        m,
        "chain",
        &[("n", Ty::Int)],
        node_ty.clone(),
        |f, ps| {
            let acc = f.local("acc", node_ty.clone(), BindingKind::Var, true);
            let i = var_int(f, "i");
            let lit = |next: Expr| h.struct_lit(node, None, vec![(0, next)], node_ty.clone());
            h.block(
                vec![
                    let_(acc, lit(h.none(node_ty.clone()))),
                    let_(i, h.int(0)),
                    while_(
                        bin(&h, Lt, iv(&h, i), iv(&h, ps[0])),
                        vec![],
                        None,
                        h.block(
                            vec![
                                assign(acc, lit(h.some(h.local(acc, node_ty.clone())))),
                                assign(i, bin(&h, Add, iv(&h, i), h.int(1))),
                            ],
                            None,
                        ),
                    ),
                ],
                Some(h.local(acc, node_ty.clone())),
            )
        },
    );
    let p = b.build();
    eprintln!("child: calling run_function");
    let outcome = run(&p, chain, vec![c("10000")], &EvalConfig::default());
    eprintln!("child: run_function returned");
    // Measure the depth without recursion.
    let mut levels = 0u32;
    let mut current = outcome.value.as_ref().expect("a value, not a fault");
    while let Value::Struct { fields, .. } = current {
        match &fields[0] {
            Value::Enum { payload, .. } if payload.len() == 1 => {
                levels += 1;
                current = &payload[0];
            }
            _ => break,
        }
    }
    assert_eq!(levels, 10_000);
    drop(outcome);
    eprintln!("child: the outcome was dropped");
}

// ===========================================================================
// 13. Call-depth and time budgets (semantics S38; toolchain 1.7)
// ===========================================================================

#[test]
fn budget_defaults_are_ten_thousand_activations_and_no_time_budget() {
    let config = EvalConfig::default();
    assert_eq!(config.max_call_depth, 10_000);
    assert_eq!(config.time_budget, None);
    assert!(config.check_contracts);
    assert!(!config.trace_effects);
}

#[test]
fn budget_depth_counts_mutual_recursion_and_blames_the_caller_s38() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn even(n: Int) -> Bool { if n == 0 { true } else { odd(n - 1) } }    (the call at 5:9)
    // fn odd(n: Int) -> Bool { if n == 0 { false } else { even(n - 1) } }   (the call at 6:9)
    let even_id = b.next_def();
    let odd_id = DefId(even_id.0 + 1);
    let parity = |b: &mut ProgramBuilder, name: &str, base: bool, other: DefId, line: u32| {
        define(b, m, name, &[("n", Ty::Int)], Ty::Bool, |_, ps| {
            let call = h.call(other, vec![bin(&h, Sub, iv(&h, ps[0]), h.int(1))], Ty::Bool);
            h.value(h.if_(
                bin(&h, Eq, iv(&h, ps[0]), h.int(0)),
                h.value(h.bool(base)),
                Some(h.value(spanned(call, at(&h, line, 9, 10)))),
            ))
        })
    };
    let even = parity(&mut b, "even", true, odd_id, 5);
    let odd = parity(&mut b, "odd", false, even_id, 6);
    assert_eq!((even, odd), (even_id, odd_id));
    let p = b.build();
    // even(9) is exactly ten activations.
    assert_eq!(
        eval_with(&p, &depth(10), even, &[json!(9)]).unwrap(),
        json!(false)
    );
    // even(10) needs eleven: odd(1), the tenth, cannot call even(0).
    let d = fault_with(&p, &depth(10), even, &[json!(10)]);
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["resource"], "stack");
    assert_eq!(d["limit"], json!(10));
    assert_eq!(d["symbol"], "m.odd");
    assert_eq!(d["span"], loc(6, 9, 10));
    let frames = backtrace(&d);
    assert_eq!(frames.len(), 10);
    assert_eq!(
        (frames[0].as_str(), frames[9].as_str()),
        ("m.odd", "m.even")
    );
}

#[test]
fn budget_depth_counts_the_calls_a_contract_clause_makes_s38() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn deep(n: Int) -> Int { if n == 0 { 0 } else { deep(n - 1) } }   fn f(n: Int) -> Int requires deep(n) == 0 { n }
    let me = b.next_def();
    let deep = define(&mut b, m, "deep", &[("n", Ty::Int)], Ty::Int, |_, ps| {
        h.value(h.if_(
            bin(&h, Eq, iv(&h, ps[0]), h.int(0)),
            h.value(h.int(0)),
            Some(h.value(h.call(me, vec![bin(&h, Sub, iv(&h, ps[0]), h.int(1))], Ty::Int))),
        ))
    });
    assert_eq!(deep, me);
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(
            bin(&h, Eq, h.call(deep, vec![iv(&h, ps[0])], Ty::Int), h.int(0)),
            "deep(n) == 0",
        ));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    // f plus deep(8) .. deep(0): ten activations.
    assert_eq!(eval_with(&p, &depth(10), f, &[json!(8)]).unwrap(), json!(8));
    let d = fault_with(&p, &depth(10), f, &[json!(9)]);
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["limit"], json!(10));
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "deep(n) == 0");
}

#[test]
fn budget_run_function_honours_the_configured_call_depth() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    // fn count(n: Int) -> Int { if n == 0 { 0 } else { count(n - 1) + 1 } }
    let me = b.next_def();
    let count = define(&mut b, m, "count", &[("n", Ty::Int)], Ty::Int, |_, ps| {
        h.value(h.if_(
            bin(&h, Eq, iv(&h, ps[0]), h.int(0)),
            h.value(h.int(0)),
            Some(h.value(bin(
                &h,
                Add,
                h.call(me, vec![bin(&h, Sub, iv(&h, ps[0]), h.int(1))], Ty::Int),
                h.int(1),
            ))),
        ))
    });
    assert_eq!(count, me);
    let p = b.build();
    let outcome = run(&p, count, vec![c("4")], &depth(5));
    let v = outcome.value.as_ref().expect("five activations fit");
    assert_eq!(value_to_json(&p, v, &outcome.return_ty), json!(4));
    let outcome = run(&p, count, vec![c("5")], &depth(5));
    let d = diag(
        &p,
        outcome.value.as_ref().expect_err("six activations do not"),
    );
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["resource"], "stack");
    assert_eq!(d["limit"], json!(5));
    // The default admits 10 000 activations: count(9999) is exactly that many.
    let outcome = run(&p, count, vec![c("9999")], &EvalConfig::default());
    let v = outcome.value.as_ref().expect("10 000 activations fit");
    assert_eq!(value_to_json(&p, v, &outcome.return_ty), json!(9999));
    let outcome = run(&p, count, vec![c("10000")], &EvalConfig::default());
    let d = diag(
        &p,
        outcome
            .value
            .as_ref()
            .expect_err("10 001 activations do not"),
    );
    assert_eq!(d["limit"], json!(10_000));
}

/// `fn tree(n: Int) -> Int { if n == 0 { 1 } else { tree(n - 1) + tree(n - 1) } }`: 2^n calls.
fn tree(b: &mut ProgramBuilder, m: ModuleId, h: &Hir) -> DefId {
    let me = b.next_def();
    let tree = define(b, m, "tree", &[("n", Ty::Int)], Ty::Int, |_, ps| {
        let call = || h.call(me, vec![bin(h, Sub, iv(h, ps[0]), h.int(1))], Ty::Int);
        h.value(h.if_(
            bin(h, Eq, iv(h, ps[0]), h.int(0)),
            h.value(h.int(1)),
            Some(h.value(bin(h, Add, call(), call()))),
        ))
    });
    assert_eq!(tree, me);
    tree
}

#[test]
fn budget_the_time_budget_ends_runaway_recursion_without_a_loop_s38() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let tree = tree(&mut b, m, &h);
    let p = b.build();
    assert_eq!(value(&p, tree, &[json!(10)]), json!(1024));
    let start = Instant::now();
    let d = fault_with(&p, &budget(30), tree, &[json!(60)]);
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["category"], "internal");
    assert_eq!(d["resource"], "time");
    assert_eq!(d["limit"], json!(30));
    assert_eq!(d["symbol"], "m.tree");
}

#[test]
fn budget_the_time_budget_inside_a_clause_is_reported_per_s34() {
    let mut b = ProgramBuilder::new();
    let (m, h) = module(&mut b, "m");
    let tree = tree(&mut b, m, &h);
    // fn f(n: Int) -> Int requires tree(n) > 0 { n }
    let f = define(&mut b, m, "f", &[("n", Ty::Int)], Ty::Int, |f, ps| {
        f.requires(h.clause(
            bin(&h, Gt, h.call(tree, vec![iv(&h, ps[0])], Ty::Int), h.int(0)),
            "tree(n) > 0",
        ));
        h.value(iv(&h, ps[0]))
    });
    let p = b.build();
    let d = fault_with(&p, &budget(30), f, &[json!(60)]);
    assert_eq!(d["code"], "resource_exhaustion");
    assert_eq!(d["resource"], "time");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "tree(n) > 0");
    assert_eq!(d["symbol"], "m.f");
}
