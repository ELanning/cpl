//! Adversarial tests of the standard-library builtins (`spec/stdlib.md`
//! 10.6) against their sections of stdlib 1-7, the contracts of the stub
//! files `std/*.cpl` (stdlib 11), and the run-time rules of stdlib 10.4 and
//! `spec/semantics.md` 6.10 (S58).
//!
//! | Section | Rules |
//! | - | - |
//! | A. builtin preconditions | stdlib 10.4, S16, S39; semantics 5.2, 5.6, 6.2, 6.10, S34, S39, S45, S46, S58; toolchain 1.7.1 |
//! | B. builtin postconditions are trusted | stdlib S39; semantics S58, S61 |
//! | C. `List` counts and indices | stdlib 4.1 (note), 4.2, 4.5, S16, L15; design-doc line 98 |
//! | D. list persistence | semantics 6.1; stdlib 4.2 (`push`: "the receiver is unchanged") |
//! | E. `++` and `flatten` laws | stdlib 1.6, 3.3 (T1), 4.3 (L1-L7, L13) |
//! | F. `Text` as scalar values | stdlib 3.3, 6.1, 6.2, S22, S23, S24, S26 and the `std/text.cpl` contracts |
//! | G. `parse_int` | stdlib 2.4, S25, I3, X6 |
//! | H. `Int` members | stdlib 2.1, 2.3, S10 |
//! | I. `Bytes` | stdlib 3.4, S14, B1-B3 |
//! | J. `Option`, `Result` | stdlib 5.1, 5.2, O1-O4; semantics 6.2 |
//! | K. `ConstraintError` payloads | stdlib 1.4, 1.5, S5, S44; semantics 6.10, S40 |
//! | L. no hidden fault sources | stdlib 0.2 |
//!
//! Every expectation is the specification's answer, never the current
//! behaviour of the implementation; where the specification is silent the
//! test follows `design-doc.md` and says so in a comment.

use cpl_diag::json::{BindingKind, TestStatus};
use cpl_diag::{Span, SymbolPath};
use cpl_eval::builtins::{constraint_error, lookup};
use cpl_eval::json::{value_from_json, value_to_json, value_to_json_untyped};
use cpl_eval::stack::on_eval_stack;
use cpl_eval::testing::{
    std_program, FnBuilder, Hir, ProgramBuilder, PropertyBuilder, TestBuilder,
};
use cpl_eval::{run_function, run_test, EvalConfig, Fault, Interpreter, RunArg, TestConfig, Value};
use cpl_hir::{
    Clause, DefId, DefKind, Expr, ExprKind, FnBody, GenExpr, Lit, LocalDef, LocalId, ModuleId,
    Program, Stmt, Ty,
};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use num_bigint::BigInt;
use serde_json::{json, Value as Json};

// ===========================================================================
// Helpers
// ===========================================================================

/// Call the builtin `id` through the registry (stdlib 10.6) while no
/// declaration is active: the top level of `cpl run`.
fn call(p: &Program, id: &str, args: Vec<Value>) -> Result<Value, Fault> {
    let f = lookup(id).unwrap_or_else(|| panic!("the registry has no {id}"));
    let mut it = Interpreter::new(p, EvalConfig::default());
    f(&mut it, &args)
}

/// The value of a builtin call that must succeed.
fn ok(p: &Program, id: &str, args: Vec<Value>) -> Value {
    call(p, id, args).unwrap_or_else(|f| panic!("{id} faulted: {f}"))
}

/// The fault of a builtin call that must fault.
fn fault(p: &Program, id: &str, args: Vec<Value>) -> Fault {
    match call(p, id, args) {
        Ok(v) => panic!("{id} returned {v:?} instead of faulting"),
        Err(f) => f,
    }
}

/// A fault as its diagnostic object (semantics 9.2; toolchain 1.7.3).
fn diag(p: &Program, f: &Fault) -> Json {
    serde_json::to_value(f.to_diagnostic(p).to_json(p.source_map()))
        .expect("a diagnostic serializes")
}

/// JSON text parsed with every digit of every number kept.
fn jparse(s: &str) -> Json {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("bad JSON {s}: {e}"))
}

fn int(n: i64) -> Value {
    Value::int(n)
}

/// An `Int` of any length, from its decimal digits.
fn big(digits: &str) -> Value {
    Value::Int(digits.parse::<BigInt>().expect("decimal digits"))
}

fn ints(ns: &[i64]) -> Value {
    Value::list(ns.iter().map(|&n| Value::int(n)).collect())
}

fn text(s: &str) -> Value {
    Value::text(s)
}

fn texts(ts: &[&str]) -> Value {
    Value::list(ts.iter().map(|t| Value::text(t)).collect())
}

fn list(items: Vec<Value>) -> Value {
    Value::list(items)
}

fn bytes(b: &[u8]) -> Value {
    Value::bytes(b)
}

fn boolean(b: bool) -> Value {
    Value::Bool(b)
}

fn as_int(v: &Value) -> BigInt {
    v.as_int()
        .unwrap_or_else(|| panic!("not an Int: {v:?}"))
        .clone()
}

fn as_bool(v: &Value) -> bool {
    v.as_bool().unwrap_or_else(|| panic!("not a Bool: {v:?}"))
}

fn items(v: &Value) -> Vec<Value> {
    v.as_list()
        .unwrap_or_else(|| panic!("not a List: {v:?}"))
        .to_vec()
}

/// `a ++ b`, the intrinsic of stdlib 10.6 (`Value::concat`).
fn cat(a: &Value, b: &Value) -> Value {
    a.concat(b)
        .unwrap_or_else(|| panic!("{a:?} ++ {b:?} is not a concatenation"))
}

/// The `(type_name, message)` of a value that must be exactly
/// `Err(ConstraintError { .. })`, read from its value JSON (semantics S40):
/// stdlib 1.5 and S5 make `ConstraintError` a struct of exactly these two
/// fields.
fn constraint_parts(p: &Program, v: &Value) -> (String, String) {
    let j = value_to_json_untyped(p, v);
    assert_eq!(j["$type"], "Result", "{j}");
    assert_eq!(j["$variant"], "Err", "{j}");
    let e = &j["$payload"][0];
    assert_eq!(e["$type"], "ConstraintError", "{j}");
    assert_eq!(
        e.as_object().map(serde_json::Map::len),
        Some(3),
        "exactly $type, type_name, and message: {j}"
    );
    let field = |key: &str| {
        e[key]
            .as_str()
            .unwrap_or_else(|| panic!("{key} is not a Text: {j}"))
            .to_string()
    };
    (field("type_name"), field("message"))
}

/// `std.text.parse_int(t)`: `Some(n)` for `Ok(n)`, `None` for an `Err`, which
/// must be a `ConstraintError` with `type_name` "Int" (S44) and a non-empty
/// message on one line (stdlib 1.5: "one line for humans"; its wording is
/// informative only and is not compared).
fn parse(p: &Program, t: &str) -> Option<BigInt> {
    let v = ok(p, "text_parse_int", vec![text(t)]);
    match v.as_result(p) {
        Some(Ok(n)) => Some(as_int(n)),
        Some(Err(_)) => {
            let (type_name, message) = constraint_parts(p, &v);
            assert_eq!(type_name, "Int", "{t:?}");
            assert!(
                !message.is_empty() && !message.contains(['\n', '\r']),
                "{t:?}: {message:?}"
            );
            None
        }
        None => panic!("parse_int({t:?}) is not a Result: {v:?}"),
    }
}

fn li() -> Ty {
    Ty::list(Ty::Int)
}

fn lli() -> Ty {
    Ty::list(li())
}

/// A hand-built program: the real standard library (the five `std/*.cpl`
/// stubs, parsed and lowered by `cpl_eval::testing`) plus the user module
/// `adv.b`.
struct Lab {
    b: ProgramBuilder,
    m: ModuleId,
    h: Hir,
}

impl Lab {
    fn new() -> Lab {
        let mut b = ProgramBuilder::new();
        let m = b.module("adv.b");
        let h = b.hir(m);
        Lab { b, m, h }
    }

    /// A call of the std stub carrying `@builtin(id)`, receiver first, with
    /// the type arguments inference gives it (toolchain T19: a member is a
    /// call of its stub).
    fn call(&self, id: &str, targs: &[Ty], args: Vec<Expr>, ret: Ty) -> Expr {
        self.h
            .call_generic(self.b.builtin(id), targs.to_vec(), args, ret)
    }

    /// `xs.take(n)` on a `List<Int>`.
    fn take_n(&self, xs: Expr, n: Expr) -> Expr {
        self.call("list_take", &[Ty::Int], vec![xs, n], li())
    }

    /// `xs.drop(n)` on a `List<Int>`.
    fn drop_n(&self, xs: Expr, n: Expr) -> Expr {
        self.call("list_drop", &[Ty::Int], vec![xs, n], li())
    }

    /// `xs.push(v)` on a `List<Int>`.
    fn push_v(&self, xs: Expr, v: Expr) -> Expr {
        self.call("list_push", &[Ty::Int], vec![xs, v], li())
    }

    /// `xs.len` on a `List<Int>`.
    fn len_of(&self, xs: Expr) -> Expr {
        self.call("list_len", &[Ty::Int], vec![xs], Ty::Int)
    }

    /// `-n` as the parser builds it: unary minus applied to a literal
    /// (grammar D48).
    fn minus(&self, n: i64) -> Expr {
        self.h.unary(UnaryOp::Neg, self.h.int(n))
    }

    fn add_fn(&mut self, f: FnBuilder) -> DefId {
        self.b.add_fn(self.m, f)
    }

    fn build(self) -> Program {
        self.b.build()
    }
}

/// Call `f` on value-JSON arguments decoded against its parameters, on the
/// evaluation stack: the value JSON of the result, or the fault.
fn eval_with(p: &Program, config: EvalConfig, f: DefId, args: &[Json]) -> Result<Json, Fault> {
    on_eval_stack(move || {
        let fd = p.def(f).kind.as_fn().expect("a function");
        let values = args
            .iter()
            .zip(&fd.params)
            .map(|(j, param)| {
                value_from_json(p, j, &param.ty).unwrap_or_else(|e| panic!("argument {j}: {e}"))
            })
            .collect();
        let mut it = Interpreter::new(p, config);
        it.call(f, values, Vec::new())
            .map(|v| value_to_json(p, &v, &fd.ret))
    })
}

fn eval(p: &Program, f: DefId, args: &[Json]) -> Result<Json, Fault> {
    eval_with(p, EvalConfig::default(), f, args)
}

/// The value JSON of a call that must return.
fn returns(p: &Program, f: DefId, args: &[Json]) -> Json {
    eval(p, f, args).unwrap_or_else(|fault| panic!("faulted: {}", diag(p, &fault)))
}

/// The diagnostic of a call that must fault.
fn faults(p: &Program, f: DefId, args: &[Json]) -> Json {
    match eval(p, f, args) {
        Ok(v) => panic!("returned {v} instead of faulting"),
        Err(fault) => diag(p, &fault),
    }
}

/// The project (non-std) frames of a diagnostic's backtrace, innermost
/// first.
fn user_frames(d: &Json) -> Vec<String> {
    d["backtrace"]
        .as_array()
        .map(|frames| {
            frames
                .iter()
                .filter_map(|f| f["qualified_name"].as_str())
                .filter(|name| !name.starts_with("std."))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The span object of a clause of a std stub file, written on one line
/// indented four spaces: the clause from its keyword (semantics 5.2: "span
/// = the clause"; toolchain 1.7.1; S63 (2)), columns counted in scalars.
fn stub_span(file: &str, line: u32, clause: &str) -> Json {
    let width = clause.chars().count() as u32;
    json!({"file": file, "line": line, "col": 5, "end_line": line, "end_col": 5 + width})
}

/// `fn <name>(v: <base>) -> Result<R, ConstraintError> { check<R>(v) }`.
fn checker(l: &mut Lab, name: &str, refined: DefId, base: Ty) -> DefId {
    let h = l.h;
    let ret = h.result_ty(l.b.refined(refined), h.constraint_error_ty());
    let mut f = FnBuilder::new(name, ret.clone());
    let v = f.param("v", base.clone());
    f.body(h.value(h.check(refined, h.local(v, base), ret)));
    l.add_fn(f)
}

fn ok_json(v: Json) -> Json {
    json!({"$type": "Result", "$variant": "Ok", "$payload": [v]})
}

fn err_json(type_name: &str, message: &str) -> Json {
    json!({"$type": "Result", "$variant": "Err", "$payload": [
        {"$type": "ConstraintError", "type_name": type_name, "message": message}
    ]})
}

// ===========================================================================
// A. Builtin preconditions (stdlib 10.4, S39; semantics 6.10, S58)
// ===========================================================================

#[test]
fn precondition_faults_carry_the_whole_envelope_and_the_stub_clause_span() {
    // stdlib 10.4 / semantics 6.10 and 9.2: code contract_violation
    // (category contract, severity error), symbol and qualified_name the
    // stub's qualified name (S43), facet contract, clause_kind requires,
    // attributed_to caller, clause the clause text, status failed, input the
    // receiver as `self` and the other arguments by parameter name; no
    // caller at the top level (semantics S39 (c)); span the stub's clause
    // (semantics 5.2, toolchain 1.7.1), from its keyword (S63 (2)).
    let p = std_program();
    let from_list = "forall v in values: v >= 0 and v <= 255";
    let cases = [
        (
            "list_take",
            vec![ints(&[4]), int(-2)],
            "std.prelude.List.take",
            "n >= 0",
            json!({"self": [4], "n": -2}),
            stub_span("std/prelude.cpl", 138, "requires n >= 0"),
        ),
        (
            "list_drop",
            vec![ints(&[4]), int(-2)],
            "std.prelude.List.drop",
            "n >= 0",
            json!({"self": [4], "n": -2}),
            stub_span("std/prelude.cpl", 146, "requires n >= 0"),
        ),
        (
            "text_split",
            vec![text("x"), text("")],
            "std.text.split",
            "not sep.is_empty()",
            json!({"text": "x", "sep": ""}),
            stub_span("std/text.cpl", 38, "requires not sep.is_empty()"),
        ),
        (
            "bytes_from_list",
            vec![ints(&[300])],
            "std.bytes.from_list",
            from_list,
            json!({"values": [300]}),
            stub_span("std/bytes.cpl", 7, &format!("requires {from_list}")),
        ),
    ];
    for (id, args, symbol, clause, input, span) in cases {
        let d = diag(&p, &fault(&p, id, args));
        assert_eq!(d["code"], "contract_violation", "{id}: {d}");
        assert_eq!(d["category"], "contract", "{id}");
        assert_eq!(d["severity"], "error", "{id}");
        assert_eq!(d["symbol"], symbol, "{id}");
        assert_eq!(d["qualified_name"], symbol, "{id}");
        assert_eq!(d["facet"], "contract", "{id}");
        assert_eq!(d["clause_kind"], "requires", "{id}");
        assert_eq!(d["attributed_to"], "caller", "{id}");
        assert_eq!(d["clause"], clause, "{id}");
        assert_eq!(d["status"], "failed", "{id}");
        assert_eq!(d["input"], input, "{id}");
        assert!(
            d.get("caller").is_none(),
            "{id}: no caller at the top level: {d}"
        );
        assert!(
            d.get("actual").is_none(),
            "{id}: a requires violation has no actual: {d}"
        );
        assert_eq!(d["span"], span, "{id}");
    }
}

#[test]
fn a_negative_count_faults_even_where_clamping_would_change_nothing() {
    // S16: a negative count is a precondition violation and is never
    // clamped to zero, so `[].take(-1)` faults although `[].take(0) == []`,
    // and `[].drop(-1)` faults although `[].drop(0) == []`.
    let p = std_program();
    for (id, symbol) in [
        ("list_take", "std.prelude.List.take"),
        ("list_drop", "std.prelude.List.drop"),
    ] {
        for receiver in [ints(&[]), ints(&[1, 2, 3])] {
            let d = diag(&p, &fault(&p, id, vec![receiver.clone(), int(-1)]));
            assert_eq!(d["code"], "contract_violation", "{id}");
            assert_eq!(d["symbol"], symbol);
            assert_eq!(d["clause"], "n >= 0", "{id}");
            assert_eq!(
                d["input"],
                json!({"self": value_to_json_untyped(&p, &receiver), "n": -1}),
                "{id}"
            );
        }
        assert_eq!(ok(&p, id, vec![ints(&[]), int(0)]), ints(&[]), "{id}(0)");
    }
}

#[test]
fn a_huge_negative_count_is_reported_with_every_digit() {
    // stdlib 10.4: input rendered as value JSON, in which an Int is a JSON
    // number with every digit (semantics S40, toolchain 7.3).
    let p = std_program();
    for id in ["list_take", "list_drop"] {
        let d = diag(
            &p,
            &fault(
                &p,
                id,
                vec![ints(&[1, 2]), big("-1000000000000000000000000000000")],
            ),
        );
        assert_eq!(d["code"], "contract_violation", "{id}");
        assert_eq!(
            d["input"],
            jparse(r#"{"self": [1, 2], "n": -1000000000000000000000000000000}"#),
            "{id}"
        );
    }
}

#[test]
fn split_faults_on_an_empty_separator_whatever_the_text() {
    // std/text.cpl: `requires not sep.is_empty()` holds for every text, the
    // empty one included (S24: no `[""]`, no splitting into scalars).
    let p = std_program();
    for t in ["", "abc", "\u{1F600}", "q\"\\\n\u{E9}"] {
        let d = diag(&p, &fault(&p, "text_split", vec![text(t), text("")]));
        assert_eq!(d["code"], "contract_violation", "{t:?}");
        assert_eq!(d["symbol"], "std.text.split", "{t:?}");
        assert_eq!(d["clause"], "not sep.is_empty()", "{t:?}");
        // The text is a JSON string of exactly the same scalars.
        assert_eq!(d["input"], json!({"text": t, "sep": ""}), "{t:?}");
    }
}

#[test]
fn from_list_faults_on_values_just_outside_the_byte_range() {
    // std/bytes.cpl: `requires forall v in values: v >= 0 and v <= 255`;
    // stdlib 3.4: a fault (S14), never a Result.
    let p = std_program();
    for v in [
        "-1",
        "256",
        "-256",
        "511",
        "65536",
        "18446744073709551616",
        "-18446744073709551616",
    ] {
        let values = list(vec![int(0), big(v), int(255)]);
        let d = diag(&p, &fault(&p, "bytes_from_list", vec![values]));
        assert_eq!(d["code"], "contract_violation", "{v}");
        assert_eq!(d["symbol"], "std.bytes.from_list", "{v}");
        assert_eq!(
            d["clause"], "forall v in values: v >= 0 and v <= 255",
            "{v}"
        );
        assert_eq!(
            d["input"],
            jparse(&format!(r#"{{"values": [0, {v}, 255]}}"#)),
            "{v}"
        );
    }
}

#[test]
fn from_list_reports_the_whole_list_when_only_the_last_value_is_out_of_range() {
    // stdlib 10.4: input = the arguments by parameter name, i.e. the whole
    // list, not just the offending element.
    let p = std_program();
    let mut values = vec![65i64; 300];
    values.push(-1);
    let d = diag(&p, &fault(&p, "bytes_from_list", vec![ints(&values)]));
    assert_eq!(d["code"], "contract_violation");
    let shown = d["input"]["values"].as_array().expect("an array");
    assert_eq!(shown.len(), 301);
    assert_eq!(shown[0], json!(65));
    assert_eq!(shown[300], json!(-1));
}

#[test]
fn precondition_inputs_render_any_receiver_as_value_json() {
    // stdlib 10.4: the receiver goes under `self`, rendered by the value
    // renderer (semantics S40, toolchain 7.3, T30), whatever its element
    // type: prelude types by bare name, user types by qualified name,
    // payload-less variants without `$payload`.
    let mut b = ProgramBuilder::new();
    let m = b.module("adv.b");
    let page_id = b.add_wrapper(m, "PageId", Ty::Text);
    let point = b.add_struct(m, "Point", &[], &[("x", Ty::Int), ("y", Ty::Int)]);
    let shade = b.add_enum(
        m,
        "Shade",
        &[],
        &[("Dark", vec![]), ("Tint", vec![Ty::Int])],
    );
    let p = b.build();
    let cases = [
        (
            list(vec![Value::some(&p, int(1)), Value::none(&p)]),
            json!([
                {"$type": "Option", "$variant": "Some", "$payload": [1]},
                {"$type": "Option", "$variant": "None"}
            ]),
        ),
        (list(vec![Value::unit(&p)]), json!([{"$type": "Unit"}])),
        (
            list(vec![Value::ok(&p, int(1)), Value::err(&p, text("e"))]),
            json!([
                {"$type": "Result", "$variant": "Ok", "$payload": [1]},
                {"$type": "Result", "$variant": "Err", "$payload": ["e"]}
            ]),
        ),
        (texts(&["q\"\\\n", "\u{E9}"]), json!(["q\"\\\n", "\u{E9}"])),
        (
            list(vec![bytes(&[0, 255]), bytes(&[])]),
            json!([{"$type": "Bytes", "hex": "00ff"}, {"$type": "Bytes", "hex": ""}]),
        ),
        (list(vec![ints(&[1]), ints(&[])]), json!([[1], []])),
        (
            list(vec![Value::wrap(page_id, text("p"))]),
            json!([{"$type": "adv.b.PageId", "$payload": ["p"]}]),
        ),
        (
            list(vec![Value::new_struct(point, vec![int(1), int(-2)])]),
            json!([{"$type": "adv.b.Point", "x": 1, "y": -2}]),
        ),
        (
            list(vec![
                Value::new_enum(shade, 0, vec![]),
                Value::new_enum(shade, 1, vec![int(3)]),
            ]),
            json!([
                {"$type": "adv.b.Shade", "$variant": "Dark"},
                {"$type": "adv.b.Shade", "$variant": "Tint", "$payload": [3]}
            ]),
        ),
        (
            list(vec![constraint_error(&p, "Int", "m")]),
            json!([{"$type": "ConstraintError", "type_name": "Int", "message": "m"}]),
        ),
        (list(vec![boolean(true)]), json!([true])),
        (
            list(vec![big("-18446744073709551617")]),
            jparse("[-18446744073709551617]"),
        ),
    ];
    for (receiver, rendered) in cases {
        for id in ["list_take", "list_drop"] {
            let d = diag(&p, &fault(&p, id, vec![receiver.clone(), int(-1)]));
            assert_eq!(
                d["input"],
                json!({"self": rendered.clone(), "n": -1}),
                "{id}"
            );
        }
    }
}

#[test]
fn the_caller_is_the_innermost_enclosing_declaration() {
    // semantics 6.10 / stdlib 10.4: caller = the enclosing declaration, the
    // function whose body made the builtin call, not one further out; the
    // backtrace lists the active declarations innermost first (6.7).
    let mut l = Lab::new();
    let h = l.h;
    // fn inner(xs: List<Int>, n: Int) -> List<Int> { xs.drop(n) }
    let mut f = FnBuilder::new("inner", li());
    let xs = f.param("xs", li());
    let n = f.param("n", Ty::Int);
    f.body(h.value(l.drop_n(h.local(xs, li()), h.local(n, Ty::Int))));
    let inner = l.add_fn(f);
    // fn outer(xs: List<Int>, n: Int) -> List<Int> { inner(xs, n) }
    let mut f = FnBuilder::new("outer", li());
    let xs = f.param("xs", li());
    let n = f.param("n", Ty::Int);
    f.body(h.value(h.call(inner, vec![h.local(xs, li()), h.local(n, Ty::Int)], li())));
    let outer = l.add_fn(f);
    let p = l.build();
    let d = faults(&p, outer, &[json!([1, 2]), json!(-3)]);
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["symbol"], "std.prelude.List.drop");
    assert_eq!(d["qualified_name"], "std.prelude.List.drop");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["caller"], "adv.b.inner");
    assert_eq!(d["input"], json!({"self": [1, 2], "n": -3}));
    assert_eq!(d["span"]["file"], "std/prelude.cpl");
    assert_eq!(user_frames(&d), ["adv.b.inner", "adv.b.outer"]);
    assert_eq!(returns(&p, outer, &[json!([1, 2]), json!(1)]), json!([2]));
}

#[test]
fn every_builtin_precondition_names_its_caller() {
    // semantics 5.6 and 6.10: `caller` is the calling declaration's
    // qualified name for every builtin that has a requires clause.
    let mut l = Lab::new();
    let h = l.h;
    let lt = Ty::list(Ty::Text);
    // fn split_of(t: Text, sep: Text) -> List<Text> { std.text.split(t, sep) }
    let mut f = FnBuilder::new("split_of", lt.clone());
    let t = f.param("t", Ty::Text);
    let sep = f.param("sep", Ty::Text);
    f.body(h.value(l.call(
        "text_split",
        &[],
        vec![h.local(t, Ty::Text), h.local(sep, Ty::Text)],
        lt,
    )));
    let split_of = l.add_fn(f);
    // fn pack(values: List<Int>) -> Bytes { std.bytes.from_list(values) }
    let mut f = FnBuilder::new("pack", Ty::Bytes);
    let values = f.param("values", li());
    f.body(h.value(l.call(
        "bytes_from_list",
        &[],
        vec![h.local(values, li())],
        Ty::Bytes,
    )));
    let pack = l.add_fn(f);
    // fn head_of(xs: List<Int>, n: Int) -> List<Int> { xs.take(n) }
    let mut f = FnBuilder::new("head_of", li());
    let xs = f.param("xs", li());
    let n = f.param("n", Ty::Int);
    f.body(h.value(l.take_n(h.local(xs, li()), h.local(n, Ty::Int))));
    let head_of = l.add_fn(f);
    let p = l.build();
    let cases = [
        (
            split_of,
            vec![json!("a,b"), json!("")],
            "std.text.split",
            json!({"text": "a,b", "sep": ""}),
            "adv.b.split_of",
        ),
        (
            pack,
            vec![json!([1, 256])],
            "std.bytes.from_list",
            json!({"values": [1, 256]}),
            "adv.b.pack",
        ),
        (
            head_of,
            vec![json!([1]), json!(-1)],
            "std.prelude.List.take",
            json!({"self": [1], "n": -1}),
            "adv.b.head_of",
        ),
    ];
    for (f, args, symbol, input, caller) in cases {
        let d = faults(&p, f, &args);
        assert_eq!(d["code"], "contract_violation", "{symbol}: {d}");
        assert_eq!(d["symbol"], symbol);
        assert_eq!(d["input"], input, "{symbol}");
        assert_eq!(d["caller"], caller, "{symbol}");
        assert_eq!(user_frames(&d), [caller], "{symbol}");
    }
    // The same calls with the precondition met return normally.
    assert_eq!(
        returns(&p, split_of, &[json!("a,b"), json!(",")]),
        json!(["a", "b"])
    );
    assert_eq!(
        returns(&p, pack, &[json!([0, 255])]),
        json!({"$type": "Bytes", "hex": "00ff"})
    );
    assert_eq!(returns(&p, head_of, &[json!([1]), json!(0)]), json!([]));
}

#[test]
fn the_receiver_and_the_arguments_are_evaluated_before_the_precondition() {
    // semantics 6.2: "builtin method or function call: receiver, then
    // arguments, then the builtin's requires clauses, then the operation".
    let mut l = Lab::new();
    let h = l.h;
    let assert_false = || Stmt::Assert {
        expr: h.bool(false),
        text: "false".to_string(),
    };
    // fn boom() -> Int { assert false  0 }
    let mut f = FnBuilder::new("boom", Ty::Int);
    f.body(h.block(vec![assert_false()], Some(h.int(0))));
    let boom = l.add_fn(f);
    // fn boom_text() -> Text { assert false  "" }
    let mut f = FnBuilder::new("boom_text", Ty::Text);
    f.body(h.block(vec![assert_false()], Some(h.text(""))));
    let boom_text = l.add_fn(f);
    // fn receiver_first(z: Int) -> List<Int> { [1 / z].take(-1) }
    let mut f = FnBuilder::new("receiver_first", li());
    let z = f.param("z", Ty::Int);
    let receiver = h.list(
        vec![h.binary(BinaryOp::Div, h.int(1), h.local(z, Ty::Int))],
        Ty::Int,
    );
    f.body(h.value(l.take_n(receiver, l.minus(1))));
    let receiver_first = l.add_fn(f);
    // fn count_first(xs: List<Int>) -> List<Int> { xs.take(boom()) }
    let mut f = FnBuilder::new("count_first", li());
    let xs = f.param("xs", li());
    f.body(h.value(l.take_n(h.local(xs, li()), h.call(boom, Vec::new(), Ty::Int))));
    let count_first = l.add_fn(f);
    // fn text_first(sep: Text) -> List<Text> { std.text.split(boom_text(), sep) }
    let mut f = FnBuilder::new("text_first", Ty::list(Ty::Text));
    let sep = f.param("sep", Ty::Text);
    f.body(h.value(l.call(
        "text_split",
        &[],
        vec![
            h.call(boom_text, Vec::new(), Ty::Text),
            h.local(sep, Ty::Text),
        ],
        Ty::list(Ty::Text),
    )));
    let text_first = l.add_fn(f);
    let p = l.build();
    let d = faults(&p, receiver_first, &[json!(0)]);
    assert_eq!(d["code"], "division_by_zero", "{d}");
    assert_eq!(d["symbol"], "adv.b.receiver_first");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
    assert_eq!(
        faults(&p, receiver_first, &[json!(1)])["code"],
        "contract_violation"
    );
    let d = faults(&p, count_first, &[json!([1])]);
    assert_eq!(d["code"], "assertion_failed", "{d}");
    assert_eq!(d["symbol"], "adv.b.boom");
    let d = faults(&p, text_first, &[json!("")]);
    assert_eq!(d["code"], "assertion_failed", "{d}");
    assert_eq!(d["symbol"], "adv.b.boom_text");
}

#[test]
fn std_functions_run_at_the_top_level_with_no_caller() {
    // semantics S39: `cpl run` evaluates a function of the checked program
    // named by its qualified name, public or private, with no calling
    // declaration, so a precondition fault at the top level omits `caller`
    // (S39 (c)). The std unit is part of the checked program (stdlib 10.1)
    // and S39 excludes none of its functions; member stubs are named
    // `std.prelude.<Receiver>.<name>` (S37).
    let p = std_program();
    let config = EvalConfig::default();
    let run = |path: &str, args: &[&str]| {
        let path = SymbolPath::parse(path).expect("a qualified name");
        let args: Vec<RunArg> = args
            .iter()
            .map(|a| RunArg::ConstExpr((*a).to_string()))
            .collect();
        run_function(&p, &path, &args, &config).unwrap_or_else(|e| panic!("refused: {e}"))
    };
    let out = run("std.text.parse_int", &["\"+0042\""]);
    let v = out.value.expect("parse_int returns");
    assert_eq!(value_to_json(&p, &v, &out.return_ty), ok_json(json!(42)));
    let out = run("std.prelude.Int.abs", &["-5"]);
    let v = out.value.expect("abs returns");
    assert_eq!(value_to_json(&p, &v, &out.return_ty), json!(5));
    let out = run("std.text.split", &["\"a\"", "\"\""]);
    let f = out.value.expect_err("split with an empty separator faults");
    let d = diag(&p, &f);
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["symbol"], "std.text.split");
    assert_eq!(d["input"], json!({"text": "a", "sep": ""}));
    assert!(d.get("caller").is_none(), "no calling declaration: {d}");
}

#[test]
fn a_builtin_fault_inside_a_requires_clause_is_reported_per_s34() {
    // semantics S34: a fault raised while evaluating a contract expression,
    // "a nested contract_violation from a called function's own contract"
    // included, is reported as that fault with facet contract, clause = the
    // text of the clause being evaluated, and symbol = the declaration that
    // owns it. S34 changes nothing else, so clause_kind, attributed_to, and
    // input remain the builtin's, and caller is the enclosing declaration
    // of the builtin call (6.10): the owner of the clause.
    let mut l = Lab::new();
    let h = l.h;
    // fn guarded(xs: List<Int>, n: Int) -> Int requires xs.take(n).len >= 0 { 0 }
    let mut f = FnBuilder::new("guarded", Ty::Int);
    let xs = f.param("xs", li());
    let n = f.param("n", Ty::Int);
    let len = l.len_of(l.take_n(h.local(xs, li()), h.local(n, Ty::Int)));
    f.requires(h.clause(h.binary(BinaryOp::Ge, len, h.int(0)), "xs.take(n).len >= 0"));
    f.body(h.value(h.int(0)));
    let guarded = l.add_fn(f);
    let p = l.build();
    assert_eq!(returns(&p, guarded, &[json!([1]), json!(5)]), json!(0));
    let d = faults(&p, guarded, &[json!([1]), json!(-1)]);
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "adv.b.guarded");
    assert_eq!(d["qualified_name"], "adv.b.guarded");
    assert_eq!(d["clause"], "xs.take(n).len >= 0");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
    assert_eq!(d["caller"], "adv.b.guarded");
}

#[test]
fn a_builtin_fault_inside_an_ensures_clause_is_reported_per_s34() {
    // semantics S34 for an ensures clause: the builtin's fault is not
    // turned into an ensures violation (a faulting clause is not a false
    // one), it is reported as itself with the clause and its owner.
    let mut l = Lab::new();
    let h = l.h;
    // fn measured(xs: List<Int>, n: Int) -> Int ensures result == xs.drop(n).len { 0 }
    let mut f = FnBuilder::new("measured", Ty::Int);
    let xs = f.param("xs", li());
    let n = f.param("n", Ty::Int);
    let result = f.result();
    let len = l.len_of(l.drop_n(h.local(xs, li()), h.local(n, Ty::Int)));
    f.ensures(h.clause(
        h.binary(BinaryOp::Eq, h.local(result, Ty::Int), len),
        "result == xs.drop(n).len",
    ));
    f.body(h.value(h.int(0)));
    let measured = l.add_fn(f);
    let p = l.build();
    assert_eq!(returns(&p, measured, &[json!([1]), json!(5)]), json!(0));
    // A false clause is an ensures violation (semantics 5.2 step 5) ...
    let d = faults(&p, measured, &[json!([1]), json!(0)]);
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["actual"], json!(0));
    // ... a faulting one is the builtin's requires fault, renamed per S34.
    let d = faults(&p, measured, &[json!([1]), json!(-1)]);
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "adv.b.measured");
    assert_eq!(d["clause"], "result == xs.drop(n).len");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
}

#[test]
fn a_builtin_fault_inside_a_refinement_predicate_is_reported_per_s34() {
    // semantics 6.10: "A fault inside a predicate ... is reported per S34
    // with symbol = the refinement declaration, facet: contract, and
    // clause = the predicate text" (stdlib 1.4, evaluation row). It is a
    // fault, not an Err(ConstraintError).
    let mut l = Lab::new();
    let h = l.h;
    // type Countable = Int where [1, 2].take(self).len >= 0
    let pair = h.list(vec![h.int(1), h.int(2)], Ty::Int);
    let pred = h.binary(
        BinaryOp::Ge,
        l.len_of(l.take_n(pair, h.local(LocalId(0), Ty::Int))),
        h.int(0),
    );
    let countable = l.b.add_refinement(
        l.m,
        "Countable",
        Ty::Int,
        pred,
        "[1, 2].take(self).len >= 0",
    );
    let check = checker(&mut l, "count_check", countable, Ty::Int);
    let p = l.build();
    assert_eq!(returns(&p, check, &[json!(5)]), ok_json(json!(5)));
    let d = faults(&p, check, &[json!(-1)]);
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "adv.b.Countable");
    assert_eq!(d["clause"], "[1, 2].take(self).len >= 0");
    assert_eq!(d["input"], json!({"self": [1, 2], "n": -1}));
}

/// `fn <name>(xs: List<Int>, n: Int) -> Int { var i = 0  while i < 1 <clauses> { i = i + 1 }  i }`
/// with the loop clauses built from `xs` and `n`.
fn loop_fn(
    l: &mut Lab,
    name: &str,
    clauses: impl FnOnce(&Lab, Expr, Expr) -> (Vec<Clause>, Option<Clause>),
) -> DefId {
    let h = l.h;
    let mut f = FnBuilder::new(name, Ty::Int);
    let xs = f.param("xs", li());
    let n = f.param("n", Ty::Int);
    let i = f.local("i", Ty::Int, BindingKind::Var, true);
    let (invariants, decreases) = clauses(&*l, h.local(xs, li()), h.local(n, Ty::Int));
    let body = h.block(
        vec![Stmt::Assign {
            local: i,
            value: h.binary(BinaryOp::Add, h.local(i, Ty::Int), h.int(1)),
        }],
        None,
    );
    f.body(h.block(
        vec![
            Stmt::Let {
                local: i,
                value: h.int(0),
            },
            Stmt::While {
                cond: h.binary(BinaryOp::Lt, h.local(i, Ty::Int), h.int(1)),
                invariants,
                decreases,
                body,
            },
        ],
        Some(h.local(i, Ty::Int)),
    ));
    l.add_fn(f)
}

#[test]
fn a_builtin_fault_inside_loop_clauses_is_reported_per_s34() {
    // semantics 5.1: invariant and decreases clauses are contract
    // expressions, so S34 renames a builtin fault raised in them to the
    // clause and its enclosing declaration (semantics 5.3 owner).
    let mut l = Lab::new();
    // ... while i < 1 invariant xs.take(n).len >= 0 { ... }
    let invariant = loop_fn(&mut l, "kept", |l, xs, n| {
        let len = l.len_of(l.take_n(xs, n));
        let clause = l.h.clause(
            l.h.binary(BinaryOp::Ge, len, l.h.int(0)),
            "xs.take(n).len >= 0",
        );
        (vec![clause], None)
    });
    // ... while i < 1 decreases xs.drop(n).len { ... }
    let measure = loop_fn(&mut l, "shrinking", |l, xs, n| {
        let len = l.len_of(l.drop_n(xs, n));
        (Vec::new(), Some(l.h.clause(len, "xs.drop(n).len")))
    });
    let p = l.build();
    for (f, symbol, clause, input) in [
        (
            invariant,
            "adv.b.kept",
            "xs.take(n).len >= 0",
            json!({"self": [1, 2], "n": -1}),
        ),
        (
            measure,
            "adv.b.shrinking",
            "xs.drop(n).len",
            json!({"self": [1, 2], "n": -1}),
        ),
    ] {
        assert_eq!(returns(&p, f, &[json!([1, 2]), json!(0)]), json!(1));
        let d = faults(&p, f, &[json!([1, 2]), json!(-1)]);
        assert_eq!(d["code"], "contract_violation", "{d}");
        assert_eq!(d["clause_kind"], "requires", "{symbol}");
        assert_eq!(d["facet"], "contract", "{symbol}");
        assert_eq!(d["symbol"], symbol);
        assert_eq!(d["clause"], clause, "{symbol}");
        assert_eq!(d["input"], input, "{symbol}");
    }
}

#[test]
fn a_builtin_fault_inside_a_header_measure_is_reported_per_s34() {
    // semantics S35 / 5.2 step 3: the header decreases is evaluated at entry
    // after the requires clauses; it is a contract expression (5.1), so S34
    // applies to a builtin fault raised in it.
    // fn countdown(xs: List<Int>, n: Int) -> Int decreases xs.drop(n).len { 0 }
    let mut l = Lab::new();
    let h = l.h;
    let mut f = FnBuilder::new("countdown", Ty::Int);
    let xs = f.param("xs", li());
    let n = f.param("n", Ty::Int);
    let len = l.len_of(l.drop_n(h.local(xs, li()), h.local(n, Ty::Int)));
    f.decreases(h.clause(len, "xs.drop(n).len"));
    f.body(h.value(h.int(0)));
    let countdown = l.add_fn(f);
    let p = l.build();
    assert_eq!(returns(&p, countdown, &[json!([1]), json!(0)]), json!(0));
    let d = faults(&p, countdown, &[json!([1]), json!(-1)]);
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "adv.b.countdown");
    assert_eq!(d["clause"], "xs.drop(n).len");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
}

#[test]
fn a_builtin_fault_in_the_predicate_of_a_run_argument_is_that_fault() {
    // semantics S39 (a): a refinement-typed `cpl run` argument is checked
    // at run time; a predicate that faults is reported per S34 (6.10), as
    // the builtin's contract_violation renamed to the refinement, not as
    // refinement_violation.
    // type Countable = Int where [1, 2].take(self).len >= 0
    // fn counted(n: Countable) -> Int { n }
    let mut l = Lab::new();
    let h = l.h;
    let pair = h.list(vec![h.int(1), h.int(2)], Ty::Int);
    let pred = h.binary(
        BinaryOp::Ge,
        l.len_of(l.take_n(pair, h.local(LocalId(0), Ty::Int))),
        h.int(0),
    );
    let countable = l.b.add_refinement(
        l.m,
        "Countable",
        Ty::Int,
        pred,
        "[1, 2].take(self).len >= 0",
    );
    let countable_ty = l.b.refined(countable);
    let mut f = FnBuilder::new("counted", Ty::Int);
    let n = f.param("n", countable_ty.clone());
    f.body(h.value(h.widen(h.local(n, countable_ty), countable, Ty::Int)));
    l.add_fn(f);
    let p = l.build();
    let path = SymbolPath::parse("adv.b.counted").expect("a qualified name");
    let config = EvalConfig::default();
    let run = |arg: &str| {
        run_function(&p, &path, &[RunArg::ConstExpr(arg.to_string())], &config)
            .unwrap_or_else(|e| panic!("refused: {e}"))
    };
    let out = run("3");
    let v = out.value.expect("counted(3) returns");
    assert_eq!(value_to_json(&p, &v, &out.return_ty), json!(3));
    let f = run("-1").value.expect_err("the predicate faults");
    let d = diag(&p, &f);
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "adv.b.Countable");
    assert_eq!(d["clause"], "[1, 2].take(self).len >= 0");
    assert_eq!(d["input"], json!({"self": [1, 2], "n": -1}));
}

#[test]
fn a_builtin_precondition_in_a_test_fails_it_without_a_counterexample() {
    // semantics S46: "A requires violation inside a test is not a
    // counterexample against the callee; it is reported only through the
    // test's fault". An assert is not a contract expression (semantics
    // 5.1), so S34 does not rename the fault: it keeps the builtin's
    // symbol and clause, and the caller is the test (its qualified name).
    let mut l = Lab::new();
    let h = l.h;
    let len = l.len_of(l.take_n(h.list(vec![h.int(1)], Ty::Int), l.minus(1)));
    let mut t = TestBuilder::new("negative take");
    t.body(h.block(
        vec![Stmt::Assert {
            expr: h.binary(BinaryOp::Eq, len, h.int(0)),
            text: "[1].take(-1).len == 0".to_string(),
        }],
        None,
    ));
    let test = l.b.add_test(l.m, t);
    let p = l.build();
    let record = run_test(&p, test, &TestConfig::with_seed(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert!(
        record.counterexamples.is_empty(),
        "{:?}",
        record.counterexamples
    );
    let d = serde_json::to_value(record.fault.as_ref().expect("the test's fault"))
        .expect("a diagnostic serializes");
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "std.prelude.List.take");
    assert_eq!(d["clause"], "n >= 0");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
    assert_eq!(d["caller"], "adv.b.\"negative take\"");
}

#[test]
fn a_property_failing_on_a_builtin_precondition_shrinks_toward_minus_one() {
    // property "take of any count" { for n in gen.int: assert [1, 2].take(n).len <= 2 }
    // Every negative n faults with contract_violation (S16). Shrinking keeps
    // only candidates failing with the same code (semantics S45, stdlib 8.4:
    // 0, v / 2, v + 1 for v < 0), so it ends at -1, whose candidates all pass
    // (unless the shrink budget runs out first). The record's fault is the
    // one the shrunk tuple raised; no contract_counterexample (S46).
    let mut l = Lab::new();
    let h = l.h;
    let mut pb = PropertyBuilder::new("take of any count");
    let n = pb.binder("n", GenExpr::Int, Ty::Int);
    let len = l.len_of(l.take_n(
        h.list(vec![h.int(1), h.int(2)], Ty::Int),
        h.local(n, Ty::Int),
    ));
    pb.body(h.block(
        vec![Stmt::Assert {
            expr: h.binary(BinaryOp::Le, len, h.int(2)),
            text: "[1, 2].take(n).len <= 2".to_string(),
        }],
        None,
    ));
    let prop = l.b.add_property(l.m, pb);
    let p = l.build();
    let record = run_test(&p, prop, &TestConfig::with_seed(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert!(record.counterexamples.is_empty());
    let shrunk = record.shrunk.as_ref().expect("a shrunk case");
    let n = shrunk.input["n"].as_i64().expect("a small shrunk n");
    assert!(n < 0, "only negative counts fault: {n}");
    if !shrunk.shrink_exhausted {
        assert_eq!(n, -1);
    }
    let d = serde_json::to_value(record.fault.as_ref().expect("the fault"))
        .expect("a diagnostic serializes");
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["symbol"], "std.prelude.List.take");
    assert_eq!(d["clause"], "n >= 0");
    assert_eq!(d["input"], json!({"self": [1, 2], "n": n}));
    assert_eq!(d["caller"], "adv.b.\"take of any count\"");
}

// ===========================================================================
// B. Builtin postconditions are trusted (stdlib S39; semantics S58, S61)
// ===========================================================================

/// Give the std stub `stub` one more `ensures` clause, as the HIR of
/// cpl-sema carries the clauses of `std/*.cpl` (the test lowering of
/// `cpl_eval::testing` drops them); `make` builds the clause from the
/// `result` local and a span.
fn add_ensures(p: &mut Program, stub: DefId, make: impl FnOnce(LocalId, Span) -> (Expr, String)) {
    let def = &mut p.defs[stub.0 as usize];
    let span = def.span;
    let DefKind::Fn(fd) = &mut def.kind else {
        panic!("DefId({}) is not a function stub", stub.0)
    };
    let result = match fd.result_local {
        Some(r) => r,
        None => {
            let r = LocalId(fd.locals.len() as u32);
            fd.locals.push(LocalDef {
                id: r,
                name: "result".to_string(),
                ty: fd.ret.clone(),
                mutable: false,
                kind: BindingKind::Result,
                span,
            });
            fd.result_local = Some(r);
            r
        }
    };
    let (expr, text) = make(result, span);
    fd.ensures.push(Clause { expr, text, span });
}

fn int_lit(n: i64, span: Span) -> Expr {
    Expr::new(ExprKind::Lit(Lit::Int(BigInt::from(n))), Ty::Int, span)
}

fn binary(op: BinaryOp, lhs: Expr, rhs: Expr, ty: Ty, span: Span) -> Expr {
    Expr::new(
        ExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        ty,
        span,
    )
}

#[test]
fn builtin_ensures_clauses_are_never_evaluated() {
    // stdlib S39 / semantics S58: a builtin's ensures clauses are trusted,
    // "never evaluated at run time, in every contract mode". Here the stubs
    // carry clauses that would fail (`false`) or fault (`1 / 0 == 0`) if
    // they were evaluated.
    let mut l = Lab::new();
    let h = l.h;
    let stubs = [
        l.b.builtin("text_trim"),
        l.b.builtin("list_take"),
        l.b.builtin("flatten"),
    ];
    // fn trimmed(t: Text) -> Text { std.text.trim(t) }
    let mut f = FnBuilder::new("trimmed", Ty::Text);
    let t = f.param("t", Ty::Text);
    f.body(h.value(l.call("text_trim", &[], vec![h.local(t, Ty::Text)], Ty::Text)));
    let trimmed = l.add_fn(f);
    // fn prefix(xs: List<Int>, n: Int) -> List<Int> { xs.take(n) }
    let mut f = FnBuilder::new("prefix", li());
    let xs = f.param("xs", li());
    let n = f.param("n", Ty::Int);
    f.body(h.value(l.take_n(h.local(xs, li()), h.local(n, Ty::Int))));
    let prefix = l.add_fn(f);
    // fn flat(xss: List<List<Int>>) -> List<Int> { flatten(xss) }
    let mut f = FnBuilder::new("flat", li());
    let xss = f.param("xss", lli());
    f.body(h.value(l.call("flatten", &[Ty::Int], vec![h.local(xss, lli())], li())));
    let flat = l.add_fn(f);
    let mut p = l.build();
    for stub in stubs {
        add_ensures(&mut p, stub, |_, span| {
            let no = Expr::new(ExprKind::Lit(Lit::Bool(false)), Ty::Bool, span);
            (no, "false".to_string())
        });
        add_ensures(&mut p, stub, |_, span| {
            let div = binary(
                BinaryOp::Div,
                int_lit(1, span),
                int_lit(0, span),
                Ty::Int,
                span,
            );
            let e = binary(BinaryOp::Eq, div, int_lit(0, span), Ty::Bool, span);
            (e, "1 / 0 == 0".to_string())
        });
    }
    let unchecked = EvalConfig {
        check_contracts: false,
        ..EvalConfig::default()
    };
    for config in [EvalConfig::default(), unchecked] {
        let mode = config.check_contracts;
        assert_eq!(
            eval_with(&p, config.clone(), trimmed, &[json!(" x ")]).unwrap(),
            json!("x"),
            "checked: {mode}"
        );
        assert_eq!(
            eval_with(&p, config.clone(), prefix, &[json!([1, 2, 3]), json!(2)]).unwrap(),
            json!([1, 2]),
            "checked: {mode}"
        );
        assert_eq!(
            eval_with(&p, config, flat, &[json!([[1], [], [2]])]).unwrap(),
            json!([1, 2]),
            "checked: {mode}"
        );
    }
    // The precondition is still checked by the builtin itself (S39): the
    // violation is of `requires n >= 0`, not of an added clause.
    let d = faults(&p, prefix, &[json!([1]), json!(-1)]);
    assert_eq!(d["clause_kind"], "requires", "{d}");
    assert_eq!(d["clause"], "n >= 0");
}

#[test]
fn self_referential_builtin_ensures_do_not_re_enter_the_builtin() {
    // semantics S61: `ensures trim(result) == result` (std/text.cpl) and the
    // matching clauses of to_upper and to_lower name their own declaration;
    // evaluating them would re-enter the builtin without end. They are
    // never evaluated (S58), so each call returns its value.
    let mut l = Lab::new();
    let h = l.h;
    let mut cases = Vec::new();
    for (id, name, input, output) in [
        ("text_trim", "a_trim", " a ", "a"),
        ("text_to_upper", "an_upper", "aB", "AB"),
        ("text_to_lower", "a_lower", "aB", "ab"),
    ] {
        let mut f = FnBuilder::new(name, Ty::Text);
        let t = f.param("t", Ty::Text);
        f.body(h.value(l.call(id, &[], vec![h.local(t, Ty::Text)], Ty::Text)));
        let stub = l.b.builtin(id);
        cases.push((stub, l.add_fn(f), input, output));
    }
    let mut p = l.build();
    for &(stub, _, _, _) in &cases {
        add_ensures(&mut p, stub, |result, span| {
            let r = || Expr::new(ExprKind::Local(result), Ty::Text, span);
            let again = Expr::new(
                ExprKind::Call {
                    callee: stub,
                    generic_args: Vec::new(),
                    args: vec![r()],
                    using: Vec::new(),
                },
                Ty::Text,
                span,
            );
            (
                binary(BinaryOp::Eq, again, r(), Ty::Bool, span),
                "f(result) == result".to_string(),
            )
        });
    }
    for (_, f, input, output) in cases {
        assert_eq!(returns(&p, f, &[json!(input)]), json!(output), "{input:?}");
    }
}

// ===========================================================================
// C. List counts and indices (stdlib 4.1, 4.2, 4.5, S16)
// ===========================================================================

#[test]
fn take_and_drop_clamp_counts_at_every_machine_boundary() {
    // stdlib 4.1 implementation note: compare the count with the length
    // before any conversion, so no count is too large (design-doc line 98:
    // "clamp at the list length").
    let p = std_program();
    let xs = ints(&[1, 2, 3]);
    for count in [
        "2147483647",
        "2147483648",
        "4294967295",
        "4294967296",
        "9223372036854775807",
        "9223372036854775808",
        "18446744073709551615",
        "18446744073709551616",
        "340282366920938463463374607431768211456",
    ] {
        assert_eq!(
            ok(&p, "list_take", vec![xs.clone(), big(count)]),
            xs,
            "take({count})"
        );
        assert_eq!(
            ok(&p, "list_drop", vec![xs.clone(), big(count)]),
            ints(&[]),
            "drop({count})"
        );
    }
}

#[test]
fn take_and_drop_at_every_count_from_zero_past_the_end() {
    // stdlib 4.2 / semantics 6.10: take(n) is the first min(n, k) elements,
    // drop(n) the elements after them.
    let p = std_program();
    for len in 0..=4i64 {
        let elements: Vec<i64> = (1..=len).map(|i| i * 10).collect();
        let xs = ints(&elements);
        for count in 0..=6i64 {
            let k = count.min(len) as usize;
            assert_eq!(
                ok(&p, "list_take", vec![xs.clone(), int(count)]),
                ints(&elements[..k]),
                "{elements:?}.take({count})"
            );
            assert_eq!(
                ok(&p, "list_drop", vec![xs.clone(), int(count)]),
                ints(&elements[k..]),
                "{elements:?}.drop({count})"
            );
        }
    }
}

#[test]
fn composed_counts_follow_l15_beyond_machine_integers() {
    // L15: xs.take(n).take(m) == xs.take(n.min(m)) and
    // xs.drop(n).drop(m) == xs.drop(n + m), where n + m exceeds u64.
    let p = std_program();
    let xs = ints(&[1, 2, 3, 4]);
    let counts = ["0", "1", "3", "9223372036854775808", "18446744073709551615"];
    for n in counts {
        for m in counts {
            let (nv, mv) = (big(n), big(m));
            let once = ok(&p, "list_take", vec![xs.clone(), nv.clone()]);
            let min = ok(&p, "int_min", vec![nv.clone(), mv.clone()]);
            assert_eq!(
                ok(&p, "list_take", vec![once, mv.clone()]),
                ok(&p, "list_take", vec![xs.clone(), min]),
                "take({n}).take({m})"
            );
            let sum = Value::Int(as_int(&nv) + as_int(&mv));
            let once = ok(&p, "list_drop", vec![xs.clone(), nv]);
            assert_eq!(
                ok(&p, "list_drop", vec![once, mv]),
                ok(&p, "list_drop", vec![xs.clone(), sum]),
                "drop({n}).drop({m})"
            );
        }
    }
}

#[test]
fn get_is_none_at_every_index_outside_the_list() {
    // stdlib 4.5: Some(element i) for 0 <= i < len, else None, negative i
    // included (no indexing from the end), at any magnitude.
    let p = std_program();
    let xs = ints(&[7, 8, 9]);
    let none = Value::none(&p);
    for i in [
        "-1",
        "-2",
        "-3",
        "-4",
        "3",
        "4",
        "-9223372036854775808",
        "-18446744073709551616",
        "9223372036854775808",
        "18446744073709551615",
        "18446744073709551616",
    ] {
        assert_eq!(
            ok(&p, "list_get", vec![xs.clone(), big(i)]),
            none,
            "get({i})"
        );
    }
    for (i, v) in [(0, 7), (1, 8), (2, 9)] {
        assert_eq!(
            ok(&p, "list_get", vec![xs.clone(), int(i)]),
            Value::some(&p, int(v)),
            "get({i})"
        );
    }
}

#[test]
fn members_on_lists_of_empty_lists_see_the_empty_elements() {
    // A list of one empty list is not empty: its first element is Some([]),
    // not None; drop_last, push, and reverse work on the outer list only.
    let p = std_program();
    let e = ints(&[]);
    let one = list(vec![e.clone()]);
    assert_eq!(ok(&p, "list_len", vec![one.clone()]), int(1));
    assert_eq!(ok(&p, "list_is_empty", vec![one.clone()]), boolean(false));
    assert_eq!(
        ok(&p, "list_first", vec![one.clone()]),
        Value::some(&p, e.clone())
    );
    assert_eq!(
        ok(&p, "list_last", vec![list(vec![ints(&[1]), e.clone()])]),
        Value::some(&p, e.clone())
    );
    assert_eq!(ok(&p, "list_drop_last", vec![one.clone()]), list(vec![]));
    assert_eq!(
        ok(&p, "list_push", vec![one.clone(), e.clone()]),
        list(vec![e.clone(), e.clone()])
    );
    assert_eq!(
        ok(
            &p,
            "list_reverse",
            vec![list(vec![ints(&[1, 2]), ints(&[3])])]
        ),
        list(vec![ints(&[3]), ints(&[1, 2])])
    );
    assert_eq!(ok(&p, "list_contains", vec![one, e.clone()]), boolean(true));
    assert_eq!(
        ok(&p, "list_contains", vec![list(vec![]), e]),
        boolean(false)
    );
}

#[test]
fn contains_compares_structurally_through_options_and_wrappers() {
    // stdlib 4.5 and S7: contains uses structural equality; Text inside a
    // wrapper is compared as scalars, with no normalization (3.3).
    let mut b = ProgramBuilder::new();
    let m = b.module("adv.b");
    let page_id = b.add_wrapper(m, "PageId", Ty::Text);
    let p = b.build();
    let xs = list(vec![Value::some(&p, int(1)), Value::none(&p)]);
    assert_eq!(
        ok(&p, "list_contains", vec![xs.clone(), Value::none(&p)]),
        boolean(true)
    );
    assert_eq!(
        ok(
            &p,
            "list_contains",
            vec![xs.clone(), Value::some(&p, int(1))]
        ),
        boolean(true)
    );
    assert_eq!(
        ok(&p, "list_contains", vec![xs, Value::some(&p, int(2))]),
        boolean(false)
    );
    let ids = list(vec![Value::wrap(page_id, text("e\u{301}"))]);
    assert_eq!(
        ok(
            &p,
            "list_contains",
            vec![ids.clone(), Value::wrap(page_id, text("e\u{301}"))]
        ),
        boolean(true)
    );
    assert_eq!(
        ok(
            &p,
            "list_contains",
            vec![ids, Value::wrap(page_id, text("\u{E9}"))]
        ),
        boolean(false)
    );
}

#[test]
fn list_stub_contracts_hold_on_adversarial_lists() {
    // Every ensures clause of the List members in std/prelude.cpl, checked
    // on the builtins' results (S39: at run time they are trusted, so this
    // self-test is the only place they are ever evaluated, stdlib 10.7).
    let p = std_program();
    let zero = BigInt::from(0);
    let samples = [
        ints(&[]),
        ints(&[0]),
        ints(&[5, 5, 5]),
        ints(&[3, -1, 3, -1]),
        ints(&[9, 8, 7, 6, 5, 4, 3]),
    ];
    let counts = ["0", "1", "2", "6", "7", "8", "18446744073709551616"];
    let len = |v: &Value| as_int(&ok(&p, "list_len", vec![v.clone()]));
    for xs in &samples {
        let k = len(xs);
        // len: result >= 0; is_empty: result == (list.len == 0)
        assert!(k >= zero);
        assert_eq!(
            ok(&p, "list_is_empty", vec![xs.clone()]),
            boolean(k == zero)
        );
        for c in counts {
            let n = as_int(&big(c));
            let taken = ok(&p, "list_take", vec![xs.clone(), big(c)]);
            let dropped = ok(&p, "list_drop", vec![xs.clone(), big(c)]);
            // take: result.len == n.min(list.len); result ++ list.drop(n) == list
            assert_eq!(len(&taken), n.clone().min(k.clone()), "{xs:?}.take({c})");
            assert_eq!(cat(&taken, &dropped), *xs, "{xs:?} at {c}");
            // drop: result.len == (list.len - n).max(0); list.take(n) ++ result == list
            assert_eq!(
                len(&dropped),
                (k.clone() - n).max(zero.clone()),
                "{xs:?}.drop({c})"
            );
        }
        // drop_last: result.len == (list.len - 1).max(0);
        // result == list.take((list.len - 1).max(0))
        let front = ok(&p, "list_drop_last", vec![xs.clone()]);
        let m = (k.clone() - 1u32).max(zero.clone());
        assert_eq!(len(&front), m);
        assert_eq!(front, ok(&p, "list_take", vec![xs.clone(), Value::Int(m)]));
        // push: result.len == list.len + 1; result == list ++ [value];
        // result.drop_last() == list
        for v in [int(0), int(-5), int(5)] {
            let pushed = ok(&p, "list_push", vec![xs.clone(), v.clone()]);
            assert_eq!(len(&pushed), k.clone() + 1u32);
            assert_eq!(pushed, cat(xs, &list(vec![v])));
            assert_eq!(ok(&p, "list_drop_last", vec![pushed]), *xs);
        }
        // get: result.is_some() == (index >= 0 and index < list.len)
        for i in -2..=8i64 {
            let got = ok(&p, "list_get", vec![xs.clone(), int(i)]);
            let in_range = i >= 0 && BigInt::from(i) < k;
            assert_eq!(
                got.as_option(&p).expect("an Option").is_some(),
                in_range,
                "{xs:?}.get({i})"
            );
        }
        // first: result == list.get(0); result.is_none() == list.is_empty()
        let empty = k == zero;
        let first = ok(&p, "list_first", vec![xs.clone()]);
        assert_eq!(first, ok(&p, "list_get", vec![xs.clone(), int(0)]));
        assert_eq!(first.as_option(&p).expect("an Option").is_none(), empty);
        // last: result == list.get(list.len - 1); result.is_none() == list.is_empty()
        let last = ok(&p, "list_last", vec![xs.clone()]);
        assert_eq!(
            last,
            ok(
                &p,
                "list_get",
                vec![xs.clone(), Value::Int(k.clone() - 1u32)]
            )
        );
        assert_eq!(last.as_option(&p).expect("an Option").is_none(), empty);
        // reverse: same len; every x of each is contained in the other
        let rev = ok(&p, "list_reverse", vec![xs.clone()]);
        assert_eq!(len(&rev), k);
        for x in items(xs) {
            assert_eq!(ok(&p, "list_contains", vec![rev.clone(), x]), boolean(true));
        }
        for x in items(&rev) {
            assert_eq!(ok(&p, "list_contains", vec![xs.clone(), x]), boolean(true));
        }
        // contains: result == (not (forall x in list: x != value))
        for v in [int(0), int(3), int(-1), int(42)] {
            let expected = items(xs).contains(&v);
            assert_eq!(
                ok(&p, "list_contains", vec![xs.clone(), v]),
                boolean(expected)
            );
        }
    }
}

// ===========================================================================
// D. List persistence (semantics 6.1; stdlib 4.2)
// ===========================================================================

/// One `let` of a [`snapshots`] function: its initializer, built from the
/// locals bound so far (`xs` first).
type Step<'a> = &'a dyn Fn(&Lab, &[Expr]) -> Expr;

/// `fn <name>(xs: List<Int>) -> List<List<Int>> { let v1 = ..  let vn = ..  [xs, v1, .., vn] }`.
fn snapshots(l: &mut Lab, name: &str, steps: &[Step<'_>]) -> DefId {
    let h = l.h;
    let mut f = FnBuilder::new(name, lli());
    let mut bound = vec![f.param("xs", li())];
    let mut stmts = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let seen: Vec<Expr> = bound.iter().map(|&b| h.local(b, li())).collect();
        let value = step(&*l, &seen);
        let local = f.local(&format!("v{}", i + 1), li(), BindingKind::Let, false);
        stmts.push(Stmt::Let { local, value });
        bound.push(local);
    }
    let shown = bound.iter().map(|&b| h.local(b, li())).collect();
    f.body(h.block(stmts, Some(h.list(shown, li()))));
    l.add_fn(f)
}

#[test]
fn pushing_onto_a_taken_prefix_leaves_the_original_list_unchanged() {
    // semantics 6.1: lists are persistent; `push` returns a new list and the
    // receiver is unchanged (stdlib 4.2). A view-sharing implementation that
    // appended in place would overwrite the original's third element.
    let mut l = Lab::new();
    let f = snapshots(
        &mut l,
        "prefix_push",
        &[&|l, v| l.take_n(v[0].clone(), l.h.int(2)), &|l, v| {
            l.push_v(v[1].clone(), l.h.int(9))
        }],
    );
    let p = l.build();
    assert_eq!(
        returns(&p, f, &[json!([1, 2, 3])]),
        json!([[1, 2, 3], [1, 2], [1, 2, 9]])
    );
}

#[test]
fn two_pushes_onto_one_list_do_not_alias() {
    // semantics 6.1: "any other binding that referred to the old list still
    // sees the old list"; two pushes onto one base give two distinct lists
    // (the append-aliasing bug of Go slices gives [1, 2, 8] twice).
    let mut l = Lab::new();
    let f = snapshots(
        &mut l,
        "fork",
        &[
            &|l, v| l.call("list_drop_last", &[Ty::Int], vec![v[0].clone()], li()),
            &|l, v| l.push_v(v[1].clone(), l.h.int(7)),
            &|l, v| l.push_v(v[1].clone(), l.h.int(8)),
            &|l, v| l.push_v(v[0].clone(), l.h.int(6)),
            &|l, v| l.push_v(v[0].clone(), l.h.int(5)),
        ],
    );
    let p = l.build();
    assert_eq!(
        returns(&p, f, &[json!([1, 2, 3])]),
        json!([
            [1, 2, 3],
            [1, 2],
            [1, 2, 7],
            [1, 2, 8],
            [1, 2, 3, 6],
            [1, 2, 3, 5]
        ])
    );
}

#[test]
fn pushing_onto_a_suffix_a_clamped_take_or_an_empty_concatenation_copies() {
    // semantics 6.1 / stdlib 4.2: every list operation returns a new list;
    // results that equal the receiver (take past the end, ++ []) or a part
    // of it (drop) never share mutable state with it.
    let mut l = Lab::new();
    let f = snapshots(
        &mut l,
        "shared",
        &[
            &|l, v| l.drop_n(v[0].clone(), l.h.int(1)),
            &|l, v| l.push_v(v[1].clone(), l.h.int(4)),
            &|l, v| l.take_n(v[0].clone(), l.h.int(10)),
            &|l, v| l.push_v(v[3].clone(), l.h.int(9)),
            &|l, v| {
                l.h.binary(
                    BinaryOp::Concat,
                    v[0].clone(),
                    l.h.list(Vec::new(), Ty::Int),
                )
            },
            &|l, v| l.push_v(v[5].clone(), l.h.int(8)),
            &|l, v| {
                l.h.binary(
                    BinaryOp::Concat,
                    l.h.list(Vec::new(), Ty::Int),
                    v[0].clone(),
                )
            },
            &|l, v| l.push_v(v[7].clone(), l.h.int(7)),
        ],
    );
    let p = l.build();
    assert_eq!(
        returns(&p, f, &[json!([1, 2, 3])]),
        json!([
            [1, 2, 3],
            [2, 3],
            [2, 3, 4],
            [1, 2, 3],
            [1, 2, 3, 9],
            [1, 2, 3],
            [1, 2, 3, 8],
            [1, 2, 3],
            [1, 2, 3, 7]
        ])
    );
}

#[test]
fn pushing_onto_a_flattened_or_reversed_list_copies() {
    // flatten([xs]) and xs.reverse().reverse() equal xs (L7, L17) but are
    // new lists; pushing onto them leaves xs unchanged.
    let mut l = Lab::new();
    let f = snapshots(
        &mut l,
        "rebuilt",
        &[
            &|l, v| {
                let wrapped = l.h.list(vec![v[0].clone()], li());
                l.call("flatten", &[Ty::Int], vec![wrapped], li())
            },
            &|l, v| l.push_v(v[1].clone(), l.h.int(6)),
            &|l, v| {
                let once = l.call("list_reverse", &[Ty::Int], vec![v[0].clone()], li());
                l.call("list_reverse", &[Ty::Int], vec![once], li())
            },
            &|l, v| l.push_v(v[3].clone(), l.h.int(5)),
        ],
    );
    let p = l.build();
    assert_eq!(
        returns(&p, f, &[json!([1, 2])]),
        json!([[1, 2], [1, 2], [1, 2, 6], [1, 2], [1, 2, 5]])
    );
}

#[test]
fn a_loop_that_pushes_keeps_every_earlier_snapshot() {
    // The design's paginate idiom (`pages = pages.push(...)` in a loop):
    // each snapshot pushed into `out` keeps its length afterwards.
    // fn grow(n: Int) -> List<List<Int>> {
    //     var acc: List<Int> = []  var out: List<List<Int>> = []  var i = 0
    //     while i < n { acc = acc.push(i)  out = out.push(acc)  i = i + 1 }
    //     out
    // }
    let mut l = Lab::new();
    let h = l.h;
    let mut f = FnBuilder::new("grow", lli());
    let n = f.param("n", Ty::Int);
    let acc = f.local("acc", li(), BindingKind::Var, true);
    let out = f.local("out", lli(), BindingKind::Var, true);
    let i = f.local("i", Ty::Int, BindingKind::Var, true);
    let body = h.block(
        vec![
            Stmt::Assign {
                local: acc,
                value: l.push_v(h.local(acc, li()), h.local(i, Ty::Int)),
            },
            Stmt::Assign {
                local: out,
                value: l.call(
                    "list_push",
                    &[li()],
                    vec![h.local(out, lli()), h.local(acc, li())],
                    lli(),
                ),
            },
            Stmt::Assign {
                local: i,
                value: h.binary(BinaryOp::Add, h.local(i, Ty::Int), h.int(1)),
            },
        ],
        None,
    );
    f.body(h.block(
        vec![
            Stmt::Let {
                local: acc,
                value: h.list(Vec::new(), Ty::Int),
            },
            Stmt::Let {
                local: out,
                value: h.list(Vec::new(), li()),
            },
            Stmt::Let {
                local: i,
                value: h.int(0),
            },
            Stmt::While {
                cond: h.binary(BinaryOp::Lt, h.local(i, Ty::Int), h.local(n, Ty::Int)),
                invariants: Vec::new(),
                decreases: None,
                body,
            },
        ],
        Some(h.local(out, lli())),
    ));
    let grow = l.add_fn(f);
    let p = l.build();
    assert_eq!(
        returns(&p, grow, &[json!(4)]),
        json!([[0], [0, 1], [0, 1, 2], [0, 1, 2, 3]])
    );
    assert_eq!(returns(&p, grow, &[json!(0)]), json!([]));
}

#[test]
fn rebinding_a_var_leaves_every_other_binding_on_the_old_list() {
    // semantics 6.1: a var is a rebindable name, not a mutable cell.
    // fn alias(xs: List<Int>) -> List<List<Int>> { var a = xs  let b = a  a = a.push(1)  a = a.drop(1)  [a, b, xs] }
    let mut l = Lab::new();
    let h = l.h;
    let mut f = FnBuilder::new("alias", lli());
    let xs = f.param("xs", li());
    let a = f.local("a", li(), BindingKind::Var, true);
    let b = f.local("b", li(), BindingKind::Let, false);
    f.body(h.block(
        vec![
            Stmt::Let {
                local: a,
                value: h.local(xs, li()),
            },
            Stmt::Let {
                local: b,
                value: h.local(a, li()),
            },
            Stmt::Assign {
                local: a,
                value: l.push_v(h.local(a, li()), h.int(1)),
            },
            Stmt::Assign {
                local: a,
                value: l.drop_n(h.local(a, li()), h.int(1)),
            },
        ],
        Some(h.list(
            vec![h.local(a, li()), h.local(b, li()), h.local(xs, li())],
            li(),
        )),
    ));
    let alias = l.add_fn(f);
    let p = l.build();
    assert_eq!(
        returns(&p, alias, &[json!([7, 8])]),
        json!([[8, 1], [7, 8], [7, 8]])
    );
}

// ===========================================================================
// E. `++` and `flatten` laws (stdlib 1.6, 3.3, 4.3)
// ===========================================================================

#[test]
fn concatenation_laws_hold_for_lists_of_empty_lists() {
    // L2, L3, L4 on List<List<Int>> values whose elements are themselves
    // empty; L13 distinguishes [[]] from [].
    let p = std_program();
    let e = ints(&[]);
    let samples = [
        list(vec![]),
        list(vec![e.clone()]),
        list(vec![e.clone(), e.clone()]),
        list(vec![ints(&[1]), e.clone()]),
        list(vec![ints(&[2, 3])]),
    ];
    let count = |v: &Value| v.as_list().expect("a list").len();
    for xs in &samples {
        // L3 identities
        assert_eq!(cat(&list(vec![]), xs), *xs);
        assert_eq!(cat(xs, &list(vec![])), *xs);
        for ys in &samples {
            let xy = cat(xs, ys);
            // L4
            assert_eq!(count(&xy), count(xs) + count(ys));
            // L2
            let k = int(count(xs) as i64);
            assert_eq!(ok(&p, "list_take", vec![xy.clone(), k.clone()]), *xs);
            assert_eq!(ok(&p, "list_drop", vec![xy.clone(), k]), *ys);
            // L3 associativity
            for zs in &samples {
                assert_eq!(cat(&xy, zs), cat(xs, &cat(ys, zs)));
            }
        }
    }
    // L13: [[]].len == 1, so [[]] != [].
    assert_ne!(list(vec![e.clone()]), list(vec![]));
    assert_eq!(ok(&p, "list_is_empty", vec![list(vec![e])]), boolean(false));
}

#[test]
fn flatten_keeps_order_and_duplicates_and_flattens_one_level() {
    // stdlib 1.6: the concatenation of the elements of `lists` in order.
    let p = std_program();
    let e = ints(&[]);
    let flat = |xss: Value| ok(&p, "flatten", vec![xss]);
    assert_eq!(flat(list(vec![e.clone(), e.clone(), e.clone()])), ints(&[]));
    assert_eq!(
        flat(list(vec![ints(&[1, 1]), e.clone(), ints(&[1])])),
        ints(&[1, 1, 1])
    );
    assert_eq!(
        flat(list(vec![ints(&[3, 2]), ints(&[1])])),
        ints(&[3, 2, 1])
    );
    assert_eq!(
        flat(list(vec![texts(&["a"]), texts(&[]), texts(&["b", "c"])])),
        texts(&["a", "b", "c"])
    );
    // One level only: a List<List<List<Int>>> becomes a List<List<Int>>,
    // inner empty lists kept.
    let deep = list(vec![
        list(vec![e.clone()]),
        list(vec![]),
        list(vec![e.clone(), ints(&[4])]),
    ]);
    assert_eq!(flat(deep), list(vec![e.clone(), e, ints(&[4])]));
}

#[test]
fn flatten_laws_l7_hold_with_nested_and_empty_parts() {
    // L7: flatten([]) == [], flatten([xs]) == xs,
    // flatten(xss.push(xs)) == flatten(xss) ++ xs,
    // flatten(xss ++ yss) == flatten(xss) ++ flatten(yss); and the flatten
    // stub's ensures (forall part in lists: part.len <= result.len).
    let p = std_program();
    let e = ints(&[]);
    let samples = [
        list(vec![]),
        list(vec![e.clone()]),
        list(vec![e.clone(), e.clone()]),
        list(vec![ints(&[1]), e.clone(), ints(&[2, 3])]),
        list(vec![ints(&[4, 4])]),
    ];
    let parts = [e.clone(), ints(&[0]), ints(&[5, 6])];
    let flat = |xss: &Value| ok(&p, "flatten", vec![xss.clone()]);
    assert_eq!(flat(&list(vec![])), e);
    for xs in &parts {
        assert_eq!(flat(&list(vec![xs.clone()])), *xs);
    }
    for xss in &samples {
        for xs in &parts {
            let pushed = ok(&p, "list_push", vec![xss.clone(), xs.clone()]);
            assert_eq!(flat(&pushed), cat(&flat(xss), xs));
        }
        for yss in &samples {
            assert_eq!(flat(&cat(xss, yss)), cat(&flat(xss), &flat(yss)));
        }
        let result = items(&flat(xss)).len();
        for part in items(xss) {
            assert!(items(&part).len() <= result);
        }
    }
}

#[test]
fn flatten_and_push_laws_hold_in_the_evaluator() {
    // L5, L6, and L7 written as CPL expressions and evaluated by the
    // interpreter (`++` and `==` are intrinsics, stdlib 10.6):
    // fn laws(xss: List<List<Int>>, xs: List<Int>) -> List<Bool> {
    //     [flatten(xss.push(xs)) == flatten(xss) ++ xs,
    //      flatten(xss ++ [xs]) == flatten(xss) ++ flatten([xs]),
    //      flatten([xs]) == xs,
    //      xs.push(0) == xs ++ [0],
    //      xs.push(0).drop_last() == xs]
    // }
    let mut l = Lab::new();
    let h = l.h;
    let mut f = FnBuilder::new("laws", Ty::list(Ty::Bool));
    let xss = f.param("xss", lli());
    let xs = f.param("xs", li());
    let xss_e = || h.local(xss, lli());
    let xs_e = || h.local(xs, li());
    let flatten = |e: Expr| l.call("flatten", &[Ty::Int], vec![e], li());
    let eq = |a: Expr, b: Expr| h.binary(BinaryOp::Eq, a, b);
    let concat = |a: Expr, b: Expr| h.binary(BinaryOp::Concat, a, b);
    let push_outer = l.call("list_push", &[li()], vec![xss_e(), xs_e()], lli());
    let l7_push = eq(flatten(push_outer), concat(flatten(xss_e()), xs_e()));
    let l7_concat = eq(
        flatten(concat(xss_e(), h.list(vec![xs_e()], li()))),
        concat(flatten(xss_e()), flatten(h.list(vec![xs_e()], li()))),
    );
    let l7_single = eq(flatten(h.list(vec![xs_e()], li())), xs_e());
    let l5 = eq(
        l.push_v(xs_e(), h.int(0)),
        concat(xs_e(), h.list(vec![h.int(0)], Ty::Int)),
    );
    let l6 = eq(
        l.call(
            "list_drop_last",
            &[Ty::Int],
            vec![l.push_v(xs_e(), h.int(0))],
            li(),
        ),
        xs_e(),
    );
    f.body(h.value(h.list(vec![l7_push, l7_concat, l7_single, l5, l6], Ty::Bool)));
    let laws = l.add_fn(f);
    let p = l.build();
    for (xss, xs) in [
        (json!([]), json!([])),
        (json!([[]]), json!([])),
        (json!([[], [1]]), json!([2, 3])),
        (json!([[1, 1], []]), json!([1])),
        (json!([[0], [0], [0]]), json!([0])),
    ] {
        assert_eq!(
            returns(&p, laws, &[xss.clone(), xs.clone()]),
            json!([true, true, true, true, true]),
            "{xss} {xs}"
        );
    }
}

#[test]
fn text_concatenation_never_normalizes() {
    // stdlib 3.3: normalization "none, ever"; T1: (a ++ b).len == a.len + b.len.
    let p = std_program();
    let joined = cat(&text("e"), &text("\u{301}"));
    assert_eq!(joined, text("e\u{301}"));
    assert_ne!(joined, text("\u{E9}"));
    assert_eq!(ok(&p, "text_len", vec![joined]), int(2));
    let flag = cat(&text("\u{1F1FA}"), &text("\u{1F1F8}"));
    assert_eq!(ok(&p, "text_len", vec![flag]), int(2));
    let crlf = cat(&text("\r"), &text("\n"));
    assert_eq!(ok(&p, "text_len", vec![crlf]), int(2));
    // X7 across a combining mark: chars(a ++ b) == chars(a) ++ chars(b).
    assert_eq!(
        ok(&p, "text_chars", vec![cat(&text("e"), &text("\u{301}"))]),
        texts(&["e", "\u{301}"])
    );
}

#[test]
fn bytes_concatenate_and_compare_bytewise() {
    // stdlib 3.4: `a ++ b` on Bytes (an extension, S14) concatenates bytes;
    // `==` is bytewise, so two encodings of "é" as text differ as Bytes.
    let p = std_program();
    let lead = ok(&p, "bytes_from_list", vec![ints(&[0xC3])]);
    let trail = ok(&p, "bytes_from_list", vec![ints(&[0xA9])]);
    let both = cat(&lead, &trail);
    assert_eq!(both, ok(&p, "bytes_from_text", vec![text("\u{E9}")]));
    assert_eq!(
        ok(&p, "bytes_to_text", vec![both]),
        Value::ok(&p, text("\u{E9}"))
    );
    // Half a UTF-8 sequence is not text on its own (an Err, not a fault).
    assert_eq!(
        constraint_parts(&p, &ok(&p, "bytes_to_text", vec![lead.clone()])).0,
        "Text"
    );
    let empty = bytes(&[]);
    assert_eq!(cat(&empty, &lead), lead);
    assert_eq!(cat(&lead, &empty), lead);
    assert_ne!(
        ok(&p, "bytes_from_text", vec![text("\u{E9}")]),
        ok(&p, "bytes_from_text", vec![text("e\u{301}")])
    );
}

// ===========================================================================
// F. Text as Unicode scalar values (stdlib 3.3, 6.2, S22-S24, S26)
// ===========================================================================

#[test]
fn len_counts_scalar_values_not_graphemes_bytes_or_utf16_units() {
    // stdlib 3.3: the element is the Unicode scalar value, "never a byte, a
    // UTF-16 code unit, or a grapheme cluster".
    let p = std_program();
    let cases: [(&str, i64); 12] = [
        ("\r\n", 2),
        ("\u{1F1FA}\u{1F1F8}", 2),
        ("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", 5),
        ("\u{0}", 1),
        ("a\u{0}b", 3),
        ("\u{10FFFF}", 1),
        ("\u{FEFF}", 1),
        ("\u{200B}", 1),
        ("\u{1C5}", 1),
        ("e\u{301}\u{301}\u{301}", 4),
        ("\u{FFFD}", 1),
        ("\u{D7FF}\u{E000}", 2),
    ];
    for (t, n) in cases {
        assert_eq!(ok(&p, "text_len", vec![text(t)]), int(n), "{t:?}");
    }
    let many = "\u{1F600}".repeat(1000);
    assert_eq!(ok(&p, "text_len", vec![text(&many)]), int(1000));
}

#[test]
fn is_empty_is_true_only_for_the_text_without_scalars() {
    // std/prelude.cpl: is_empty ensures result == (text.len == 0); an
    // invisible scalar still counts.
    let p = std_program();
    assert_eq!(ok(&p, "text_is_empty", vec![text("")]), boolean(true));
    for t in ["\u{0}", "\u{200B}", "\u{FEFF}", " ", "\u{301}"] {
        assert_eq!(
            ok(&p, "text_is_empty", vec![text(t)]),
            boolean(false),
            "{t:?}"
        );
    }
}

#[test]
fn chars_yields_one_text_per_scalar_value() {
    // stdlib 6.2 (S26): one Text per scalar value, in order; a grapheme
    // cluster of several scalars is taken apart.
    let p = std_program();
    let cases: [(&str, &[&str]); 5] = [
        ("\u{1F1FA}\u{1F1F8}", &["\u{1F1FA}", "\u{1F1F8}"]),
        ("\r\n", &["\r", "\n"]),
        (
            "\u{1F468}\u{200D}\u{1F467}",
            &["\u{1F468}", "\u{200D}", "\u{1F467}"],
        ),
        ("a\u{0}", &["a", "\u{0}"]),
        ("e\u{301}\u{301}", &["e", "\u{301}", "\u{301}"]),
    ];
    for (t, parts) in cases {
        assert_eq!(ok(&p, "text_chars", vec![text(t)]), texts(parts), "{t:?}");
    }
}

#[test]
fn trim_excludes_lookalike_separators_and_format_characters() {
    // S22: whitespace is exactly the 25 White_Space scalars. Python's
    // str.strip also removes U+001C-U+001F, which are not White_Space;
    // format characters and blank-looking letters are not either.
    let p = std_program();
    for c in [
        '\u{1D}',
        '\u{1E}',
        '\u{1F}',
        '\u{AD}',
        '\u{200C}',
        '\u{200D}',
        '\u{2060}',
        '\u{2800}',
        '\u{3164}',
        '\u{FFA0}',
        '\u{E0020}',
        '\u{FFFC}',
    ] {
        let t = format!("{c}x{c}");
        for id in ["text_trim", "text_trim_start", "text_trim_end"] {
            assert_eq!(
                ok(&p, id, vec![text(&t)]),
                text(&t),
                "{id} U+{:04X}",
                c as u32
            );
        }
    }
}

#[test]
fn trim_keeps_interior_white_space() {
    let p = std_program();
    assert_eq!(
        ok(&p, "text_trim", vec![text(" a \u{3000} b\t")]),
        text("a \u{3000} b")
    );
    assert_eq!(
        ok(&p, "text_trim", vec![text("\u{A0}a\u{A0}\u{A0}b\u{A0}")]),
        text("a\u{A0}\u{A0}b")
    );
    assert_eq!(
        ok(&p, "text_trim_start", vec![text("\n\na\n\nb\n")]),
        text("a\n\nb\n")
    );
    assert_eq!(
        ok(&p, "text_trim_end", vec![text("\n\na\n\nb\n")]),
        text("\n\na\n\nb")
    );
}

#[test]
fn trimming_a_text_of_white_space_alone_leaves_the_empty_text() {
    let p = std_program();
    for t in [
        "\u{85}",
        "\u{2028}\u{2029}",
        "\u{1680}\u{2000}\u{200A}",
        "\u{202F}\u{205F}\u{3000}",
        "\u{B}\u{C}\r",
        " ",
    ] {
        for id in ["text_trim", "text_trim_start", "text_trim_end"] {
            assert_eq!(ok(&p, id, vec![text(t)]), text(""), "{id} {t:?}");
        }
    }
}

#[test]
fn trim_works_on_scalars_not_grapheme_clusters() {
    // stdlib 3.3: trim operates "on the scalar sequence as given". A space
    // followed by a combining mark is one grapheme cluster, but its first
    // scalar is White_Space and goes; the mark is not and stays.
    let p = std_program();
    assert_eq!(
        ok(&p, "text_trim", vec![text(" \u{301}x")]),
        text("\u{301}x")
    );
    assert_eq!(
        ok(&p, "text_trim_start", vec![text("\u{3000}\u{301}")]),
        text("\u{301}")
    );
    assert_eq!(
        ok(&p, "text_trim_end", vec![text("x\u{301} ")]),
        text("x\u{301}")
    );
    assert_eq!(
        ok(&p, "text_trim_end", vec![text("x \u{301}")]),
        text("x \u{301}")
    );
}

#[test]
fn trim_start_and_trim_end_are_one_sided() {
    let p = std_program();
    let t = "\u{2028} x \u{2029}";
    assert_eq!(ok(&p, "text_trim_start", vec![text(t)]), text("x \u{2029}"));
    assert_eq!(ok(&p, "text_trim_end", vec![text(t)]), text("\u{2028} x"));
    assert_eq!(ok(&p, "text_trim_start", vec![text("x\t")]), text("x\t"));
    assert_eq!(ok(&p, "text_trim_end", vec![text("\tx")]), text("\tx"));
}

#[test]
fn prefix_suffix_and_containment_compare_scalar_sequences() {
    // stdlib 3.3 and 6.2: contiguous scalar subsequences; no normalization,
    // no grapheme boundaries, no case folding.
    let p = std_program();
    let t = |id: &str, a: &str, b: &str| ok(&p, id, vec![text(a), text(b)]);
    let yes = boolean(true);
    let no = boolean(false);
    // A combining mark is its own scalar.
    assert_eq!(t("text_starts_with", "e\u{301}", "e"), yes);
    assert_eq!(t("text_ends_with", "e\u{301}", "\u{301}"), yes);
    assert_eq!(t("text_contains", "xe\u{301}y", "e"), yes);
    // A precomposed letter is one scalar and contains neither part.
    assert_eq!(t("text_starts_with", "\u{E9}", "e"), no);
    assert_eq!(t("text_ends_with", "\u{E9}", "\u{301}"), no);
    assert_eq!(t("text_contains", "\u{E9}", "e\u{301}"), no);
    // An occurrence may straddle two grapheme clusters (two flags).
    assert_eq!(
        t(
            "text_contains",
            "\u{1F1FA}\u{1F1F8}\u{1F1FA}\u{1F1F8}",
            "\u{1F1F8}\u{1F1FA}"
        ),
        yes
    );
    // Case matters.
    assert_eq!(t("text_starts_with", "Abc", "a"), no);
    assert_eq!(t("text_contains", "ABC", "b"), no);
    assert_eq!(t("text_ends_with", "abc", "C"), no);
}

#[test]
fn empty_needles_always_match_and_longer_needles_never_do() {
    // std/text.cpl ensures: `not prefix.is_empty() or result` and
    // `not result or prefix.len <= text.len` (likewise suffix and needle).
    let p = std_program();
    for id in ["text_starts_with", "text_ends_with", "text_contains"] {
        let t = |a: &str, b: &str| ok(&p, id, vec![text(a), text(b)]);
        assert_eq!(t("", ""), boolean(true), "{id}");
        assert_eq!(t("\u{1F600}", ""), boolean(true), "{id}");
        assert_eq!(t("", "a"), boolean(false), "{id}");
        assert_eq!(t("\u{E9}", "\u{E9}\u{E9}"), boolean(false), "{id}");
        assert_eq!(t("ab", "aba"), boolean(false), "{id}");
        assert_eq!(
            t("\u{E9}\u{1F600}", "\u{E9}\u{1F600}"),
            boolean(true),
            "{id}"
        );
    }
}

#[test]
fn split_finds_leftmost_non_overlapping_occurrences() {
    // S24: occurrences found leftmost first, not overlapping; adjacent
    // occurrences and an occurrence at either end give "" pieces.
    let p = std_program();
    let s = |t: &str, sep: &str| ok(&p, "text_split", vec![text(t), text(sep)]);
    assert_eq!(s("aaaa", "aa"), texts(&["", "", ""]));
    assert_eq!(s("aaaaa", "aa"), texts(&["", "", "a"]));
    assert_eq!(s("abab", "aba"), texts(&["", "b"]));
    assert_eq!(s("ababa", "aba"), texts(&["", "ba"]));
    assert_eq!(s("baaab", "aa"), texts(&["b", "ab"]));
    assert_eq!(s("a,,", ","), texts(&["a", "", ""]));
    assert_eq!(s(",,", ","), texts(&["", "", ""]));
}

#[test]
fn split_on_the_whole_text_or_on_a_longer_separator() {
    let p = std_program();
    let s = |t: &str, sep: &str| ok(&p, "text_split", vec![text(t), text(sep)]);
    assert_eq!(s("abc", "abc"), texts(&["", ""]));
    assert_eq!(s("abc", "abcd"), texts(&["abc"]));
    assert_eq!(s("", "\u{1F600}"), texts(&[""]));
    assert_eq!(s("\u{1F600}", "\u{1F600}"), texts(&["", ""]));
}

#[test]
fn split_works_on_scalars_not_grapheme_clusters() {
    let p = std_program();
    let s = |t: &str, sep: &str| ok(&p, "text_split", vec![text(t), text(sep)]);
    assert_eq!(s("e\u{301}e", "e"), texts(&["", "\u{301}", ""]));
    assert_eq!(
        s("\u{1F1FA}\u{1F1F8}\u{1F1FA}\u{1F1F8}", "\u{1F1F8}\u{1F1FA}"),
        texts(&["\u{1F1FA}", "\u{1F1F8}"])
    );
    assert_eq!(s("\u{E9}", "e"), texts(&["\u{E9}"]));
    assert_eq!(s("a\r\nb", "\n"), texts(&["a\r", "b"]));
    assert_eq!(s("a\u{0}b", "\u{0}"), texts(&["a", "b"]));
}

#[test]
fn split_contract_holds_for_overlapping_separators() {
    // std/text.cpl split ensures: result.len >= 1, join(result, sep) ==
    // text, forall part in result: not contains(part, sep) (X3, X4).
    let p = std_program();
    for (t, sep) in [
        ("aaaa", "aa"),
        ("aaaaa", "aa"),
        ("ababa", "aba"),
        ("", "x"),
        ("xx", "x"),
        ("e\u{301}e\u{301}", "\u{301}e"),
        ("\u{1F600}\u{1F600}\u{1F600}", "\u{1F600}\u{1F600}"),
        ("abcabc", "cab"),
    ] {
        let parts = ok(&p, "text_split", vec![text(t), text(sep)]);
        let pieces = items(&parts);
        assert!(!pieces.is_empty(), "{t:?} {sep:?}");
        assert_eq!(
            ok(&p, "text_join", vec![parts.clone(), text(sep)]),
            text(t),
            "{t:?} {sep:?}"
        );
        for piece in pieces {
            assert_eq!(
                ok(&p, "text_contains", vec![piece, text(sep)]),
                boolean(false),
                "{t:?} {sep:?}"
            );
        }
    }
}

#[test]
fn join_places_the_separator_only_between_parts() {
    // stdlib 6.2: "with sep between consecutive elements"; join ensures
    // `parts.len != 1 or result == parts.get(0).unwrap_or("")`.
    let p = std_program();
    let j = |parts: &[&str], sep: &str| ok(&p, "text_join", vec![texts(parts), text(sep)]);
    assert_eq!(j(&["a"], "--"), text("a"));
    assert_eq!(j(&[""], "--"), text(""));
    assert_eq!(j(&["", "", ""], ","), text(",,"));
    assert_eq!(j(&[], ""), text(""));
    assert_eq!(j(&["e", "e"], "\u{301}"), text("e\u{301}e"));
    assert_eq!(
        j(&["\u{1F1FA}", "\u{1F1F8}"], ""),
        text("\u{1F1FA}\u{1F1F8}")
    );
}

#[test]
fn case_mapping_changes_exactly_the_ascii_letters() {
    // S23: a-z and A-Z only; their ASCII neighbours ` { @ [ stay.
    let p = std_program();
    assert_eq!(
        ok(&p, "text_to_upper", vec![text("`az{@AZ[")]),
        text("`AZ{@AZ[")
    );
    assert_eq!(
        ok(&p, "text_to_lower", vec![text("`az{@AZ[")]),
        text("`az{@az[")
    );
    let lower: String = ('a'..='z').collect();
    let upper: String = ('A'..='Z').collect();
    assert_eq!(ok(&p, "text_to_upper", vec![text(&lower)]), text(&upper));
    assert_eq!(ok(&p, "text_to_lower", vec![text(&upper)]), text(&lower));
    assert_eq!(
        ok(&p, "text_to_upper", vec![text("0123456789 _-")]),
        text("0123456789 _-")
    );
}

#[test]
fn case_mapping_ignores_unicode_mappings_into_ascii() {
    // S23: Unicode case mapping would turn U+017F (long s) into "S", U+0131
    // (dotless i) into "I", and U+212A (Kelvin sign) into "k"; the ASCII-only
    // mapping leaves every non-ASCII scalar unchanged.
    let p = std_program();
    assert_eq!(
        ok(&p, "text_to_upper", vec![text("a\u{17F}b\u{131}")]),
        text("A\u{17F}B\u{131}")
    );
    assert_eq!(
        ok(&p, "text_to_lower", vec![text("K\u{212A}\u{212B}")]),
        text("k\u{212A}\u{212B}")
    );
    assert_eq!(
        ok(&p, "text_to_upper", vec![text("\u{FF41}")]),
        text("\u{FF41}")
    );
    assert_eq!(
        ok(&p, "text_to_lower", vec![text("\u{FF21}")]),
        text("\u{FF21}")
    );
}

#[test]
fn case_mapping_keeps_the_length_where_unicode_mapping_would_change_it() {
    // S23 and the stubs' `ensures result.len == text.len`: Unicode maps
    // U+00DF to "SS", U+FB00 to "FF", U+0149 to two scalars, U+0390 to
    // three, and lowercases U+0130 to two; none of that happens here.
    let p = std_program();
    for t in [
        "\u{DF}",
        "\u{FB00}",
        "\u{149}",
        "\u{390}",
        "\u{130}",
        "\u{1C5}",
        "\u{3A3}\u{3C3}",
        "\u{E9}",
    ] {
        assert_eq!(ok(&p, "text_to_upper", vec![text(t)]), text(t), "{t:?}");
        assert_eq!(ok(&p, "text_to_lower", vec![text(t)]), text(t), "{t:?}");
    }
}

#[test]
fn text_stub_contracts_hold_on_adversarial_texts() {
    // Every ensures clause of std/text.cpl, checked on the builtins'
    // results (trusted at run time, S39; stdlib 10.7 self-test), and the
    // laws X1, X3, X4, X5.
    let p = std_program();
    let samples = [
        "",
        " ",
        "\u{85}",
        "\u{A0}x\u{2029}",
        " \u{301}x\u{301} ",
        "e\u{301}",
        "\u{E9}",
        "\u{1F1FA}\u{1F1F8}",
        "a\"b\\c\n",
        "\u{0}\u{0}",
        "\u{FEFF}9\u{FEFF}",
        "\u{17F}\u{131}K\u{212A}",
        "MiXeD \u{DF}",
        "007",
        "-0",
        "+",
    ];
    let one = |id: &str, a: &Value| ok(&p, id, vec![a.clone()]);
    let two = |id: &str, a: &Value, b: &Value| ok(&p, id, vec![a.clone(), b.clone()]);
    let len = |v: &Value| as_int(&one("text_len", v));
    let yes = boolean(true);
    for s in samples {
        let t = text(s);
        // trim: result.len <= text.len; trim(result) == result
        let trimmed = one("text_trim", &t);
        assert!(len(&trimmed) <= len(&t), "{s:?}");
        assert_eq!(one("text_trim", &trimmed), trimmed, "{s:?}");
        // trim_start: result.len <= text.len; ends_with(text, result)
        let start = one("text_trim_start", &t);
        assert!(len(&start) <= len(&t), "{s:?}");
        assert_eq!(two("text_ends_with", &t, &start), yes, "{s:?}");
        // trim_end: result.len <= text.len; starts_with(text, result)
        let end = one("text_trim_end", &t);
        assert!(len(&end) <= len(&t), "{s:?}");
        assert_eq!(two("text_starts_with", &t, &end), yes, "{s:?}");
        // X1: trim(t) == trim_start(trim_end(t)), and the other order
        assert_eq!(one("text_trim_start", &end), trimmed, "{s:?}");
        assert_eq!(one("text_trim_end", &start), trimmed, "{s:?}");
        // to_upper/to_lower: result.len == text.len; idempotent; X5
        let up = one("text_to_upper", &t);
        let low = one("text_to_lower", &t);
        assert_eq!(len(&up), len(&t), "{s:?}");
        assert_eq!(len(&low), len(&t), "{s:?}");
        assert_eq!(one("text_to_upper", &up), up, "{s:?}");
        assert_eq!(one("text_to_lower", &low), low, "{s:?}");
        assert_eq!(one("text_to_lower", &up), low, "{s:?}");
        // chars: result.len == text.len; each c.len == 1; join(result, "") == text
        let cs = one("text_chars", &t);
        assert_eq!(BigInt::from(items(&cs).len()), len(&t), "{s:?}");
        for c in items(&cs) {
            assert_eq!(len(&c), BigInt::from(1), "{s:?}");
        }
        assert_eq!(two("text_join", &cs, &text("")), t, "{s:?}");
        // parse_int: not text.is_empty() or result.is_err()
        if s.is_empty() {
            assert_eq!(parse(&p, s), None);
        }
        for n in samples {
            let needle = text(n);
            // starts_with / ends_with / contains:
            // not result or needle.len <= text.len; not needle.is_empty() or result
            for id in ["text_starts_with", "text_ends_with", "text_contains"] {
                let r = as_bool(&two(id, &t, &needle));
                assert!(!r || len(&needle) <= len(&t), "{id}({s:?}, {n:?})");
                assert!(!n.is_empty() || r, "{id}({s:?}, {n:?})");
            }
            if !n.is_empty() {
                // split ensures and X3, X4
                let parts = two("text_split", &t, &needle);
                let pieces = items(&parts);
                assert!(!pieces.is_empty(), "split({s:?}, {n:?})");
                assert_eq!(two("text_join", &parts, &needle), t, "split({s:?}, {n:?})");
                for piece in pieces {
                    assert_eq!(
                        two("text_contains", &piece, &needle),
                        boolean(false),
                        "split({s:?}, {n:?})"
                    );
                }
            }
        }
        // join ensures: parts.len != 1 or result == parts.get(0).unwrap_or("")
        assert_eq!(two("text_join", &list(vec![t.clone()]), &text(",")), t);
    }
    // join ensures: not parts.is_empty() or result == ""
    assert_eq!(two("text_join", &texts(&[]), &text("\u{1F600}")), text(""));
}

// ===========================================================================
// G. parse_int (stdlib S25, 2.4)
// ===========================================================================

fn n(v: i64) -> Option<BigInt> {
    Some(BigInt::from(v))
}

#[test]
fn parse_int_accepts_every_spelling_of_zero() {
    // S25: an optional sign, then one or more ASCII digits; leading zeros
    // accepted; "-0" is 0.
    let p = std_program();
    for t in [
        "0",
        "+0",
        "-0",
        "00",
        "-00",
        "+000",
        "0000000000000000000000000",
    ] {
        assert_eq!(parse(&p, t), n(0), "{t:?}");
    }
}

#[test]
fn parse_int_accepts_a_sign_before_leading_zeros() {
    let p = std_program();
    assert_eq!(parse(&p, "-007"), n(-7));
    assert_eq!(parse(&p, "+0042"), n(42));
    assert_eq!(parse(&p, "0000000000000000000000000000001"), n(1));
    assert_eq!(parse(&p, "-0000000000000000000000000000001"), n(-1));
}

#[test]
fn parse_int_has_no_length_limit() {
    // S25: "no length limit"; Int is arbitrary precision (stdlib 2.1).
    let p = std_program();
    let power = format!("1{}", "0".repeat(5000));
    assert_eq!(
        parse(&p, &power),
        Some(num_traits::pow(BigInt::from(10), 5000))
    );
    let negative = format!("-{power}");
    assert_eq!(
        parse(&p, &negative),
        Some(-num_traits::pow(BigInt::from(10), 5000))
    );
    let nines = "9".repeat(3000);
    assert_eq!(
        parse(&p, &nines),
        Some(num_traits::pow(BigInt::from(10), 3000) - 1u32)
    );
    for t in [
        "9223372036854775807",
        "9223372036854775808",
        "-9223372036854775809",
        "18446744073709551616",
        "-170141183460469231731687303715884105729",
        "340282366920938463463374607431768211456",
    ] {
        assert_eq!(parse(&p, t), Some(as_int(&big(t))), "{t}");
    }
}

#[test]
fn parse_int_rejects_white_space_anywhere() {
    // S25: "no surrounding whitespace" (callers compose trim, as the corpus's
    // parse_size does); inside the digits there is no place for any.
    let p = std_program();
    for t in [
        "\t1",
        "1\t",
        "\n1",
        "1\n",
        "\r\n1",
        "1\r\n",
        " ",
        "\u{3000}1",
        "1\u{2009}",
        "\u{85}1",
        "1\u{2028}",
        "1 2",
        "- 1",
        "+ 1",
        "\u{A0}",
    ] {
        assert_eq!(parse(&p, t), None, "{t:?}");
    }
}

#[test]
fn parse_int_rejects_digit_group_separators() {
    // S25: no `_` separators (and no other grouping).
    let p = std_program();
    for t in [
        "1_000",
        "1,000",
        "1.000",
        "1'000",
        "1\u{2009}000",
        "_1",
        "1_",
        "1__0",
        "-_1",
    ] {
        assert_eq!(parse(&p, t), None, "{t:?}");
    }
}

#[test]
fn parse_int_rejects_misplaced_or_repeated_signs() {
    let p = std_program();
    for t in [
        "++1", "--1", "+-1", "-+1", "1-", "1+", "+", "-", "+-", "-+", "1-1", "1+1", "0-", "-0-",
    ] {
        assert_eq!(parse(&p, t), None, "{t:?}");
    }
}

#[test]
fn parse_int_rejects_signs_that_are_not_ascii() {
    // S25: the sign is "+" (U+002B) or "-" (U+002D); minus signs, dashes,
    // and full-width or superscript signs are not.
    let p = std_program();
    for t in [
        "\u{2212}1",
        "\u{FF0B}1",
        "\u{FF0D}1",
        "\u{FE63}1",
        "\u{2010}1",
        "\u{2013}1",
        "\u{207A}1",
        "\u{207B}1",
    ] {
        assert_eq!(parse(&p, t), None, "{t:?}");
    }
}

#[test]
fn parse_int_rejects_digits_of_other_scripts() {
    // S25: "no other digit scripts", decimal digits (Nd) and other numeric
    // forms (No, Nl) alike, alone or mixed with ASCII digits.
    let p = std_program();
    for t in [
        "\u{663}",
        "\u{6F3}",
        "\u{969}",
        "\u{E53}",
        "\u{FF13}",
        "\u{1D7D1}",
        "\u{B3}",
        "\u{2083}",
        "\u{2462}",
        "\u{2162}",
        "1\u{663}",
        "\u{663}1",
        "-\u{FF13}",
        "+\u{969}",
    ] {
        assert_eq!(parse(&p, t), None, "{t:?}");
    }
}

#[test]
fn parse_int_rejects_radix_float_and_suffix_forms() {
    let p = std_program();
    for t in [
        "0x1F", "0X1F", "0o17", "0b101", "1E3", "1.", ".1", "1.5", "-1.0", "Infinity", "inf",
        "NaN", "1n", "1L", "1i64", "#1", "(1)", "1e", "e1",
    ] {
        assert_eq!(parse(&p, t), None, "{t:?}");
    }
}

#[test]
fn parse_int_rejects_control_and_format_characters() {
    let p = std_program();
    for t in [
        "1\u{0}",
        "\u{0}1",
        "1\u{7F}",
        "\u{FEFF}1",
        "1\u{200B}",
        "\u{200E}1",
        "1\u{301}",
    ] {
        assert_eq!(parse(&p, t), None, "{t:?}");
    }
}

#[test]
fn parse_int_round_trips_through_the_canonical_rendering() {
    // S10: a non-canonical spelling parses to a value whose rendering is
    // canonical; I3 and X6 at the machine boundaries; from_int equals
    // to_text (std/text.cpl ensures).
    let p = std_program();
    for (t, canonical) in [
        ("-007", "-7"),
        ("+0042", "42"),
        ("-0", "0"),
        ("+0", "0"),
        ("00100", "100"),
    ] {
        let v = Value::Int(parse(&p, t).unwrap_or_else(|| panic!("{t:?} parses")));
        assert_eq!(
            ok(&p, "int_to_text", vec![v.clone()]),
            text(canonical),
            "{t:?}"
        );
        assert_eq!(ok(&p, "text_from_int", vec![v]), text(canonical), "{t:?}");
    }
    for digits in [
        "9223372036854775807",
        "-9223372036854775808",
        "-9223372036854775809",
        "18446744073709551615",
        "18446744073709551616",
        "-340282366920938463463374607431768211456",
    ] {
        let v = big(digits);
        let rendered = ok(&p, "int_to_text", vec![v.clone()]);
        assert_eq!(rendered, text(digits));
        assert_eq!(ok(&p, "text_from_int", vec![v.clone()]), rendered);
        assert_eq!(ok(&p, "text_parse_int", vec![rendered]), Value::ok(&p, v));
    }
}

// ===========================================================================
// H. Int members (stdlib 2.1, 2.3, S10)
// ===========================================================================

#[test]
fn to_text_is_the_canonical_decimal_rendering() {
    // stdlib 2.3 and S10: an optional leading "-", digits without leading
    // zeros ("0" for zero), no "+", no separators, ASCII only.
    let p = std_program();
    let power = format!("1{}", "0".repeat(30));
    for (v, rendered) in [
        (int(0), "0".to_string()),
        (int(-1), "-1".to_string()),
        (int(10), "10".to_string()),
        (big(&power), power.clone()),
        (big(&format!("-{power}")), format!("-{power}")),
        (
            big("-9223372036854775808"),
            "-9223372036854775808".to_string(),
        ),
    ] {
        assert_eq!(ok(&p, "int_to_text", vec![v]), text(&rendered));
    }
    // -0 does not exist: 0 * -1 is 0 and renders as "0".
    let negated_zero = Value::Int(-BigInt::from(0));
    assert_eq!(ok(&p, "int_to_text", vec![negated_zero]), text("0"));
}

#[test]
fn abs_min_and_max_at_machine_boundaries() {
    // stdlib 2.1: no overflow, so i64::MIN.abs() and i128::MIN.abs() exist.
    let p = std_program();
    assert_eq!(
        ok(&p, "int_abs", vec![big("-9223372036854775808")]),
        big("9223372036854775808")
    );
    assert_eq!(
        ok(
            &p,
            "int_abs",
            vec![big("-170141183460469231731687303715884105728")]
        ),
        big("170141183460469231731687303715884105728")
    );
    assert_eq!(
        ok(
            &p,
            "int_min",
            vec![big("-9223372036854775809"), big("9223372036854775807")]
        ),
        big("-9223372036854775809")
    );
    assert_eq!(
        ok(
            &p,
            "int_max",
            vec![big("18446744073709551616"), big("18446744073709551615")]
        ),
        big("18446744073709551616")
    );
    assert_eq!(
        ok(
            &p,
            "int_max",
            vec![big("-18446744073709551616"), big("-18446744073709551617")]
        ),
        big("-18446744073709551616")
    );
}

#[test]
fn int_stub_contracts_hold_at_machine_boundaries() {
    // std/prelude.cpl: to_text ensures result.len >= 1; abs ensures result
    // >= 0 and result == value or result == -value; min and max return an
    // operand that bounds both.
    let p = std_program();
    let zero = BigInt::from(0);
    let one = BigInt::from(1);
    let values = [
        "0",
        "1",
        "-1",
        "127",
        "-128",
        "255",
        "256",
        "2147483647",
        "-2147483648",
        "9223372036854775807",
        "-9223372036854775808",
        "18446744073709551615",
        "18446744073709551616",
        "-18446744073709551616",
    ];
    for a in values {
        let ai = as_int(&big(a));
        let rendered = ok(&p, "int_to_text", vec![big(a)]);
        assert!(as_int(&ok(&p, "text_len", vec![rendered])) >= one, "{a}");
        let abs = as_int(&ok(&p, "int_abs", vec![big(a)]));
        assert!(abs >= zero, "{a}");
        assert!(abs == ai || abs == -&ai, "{a}");
        for b in values {
            let bi = as_int(&big(b));
            let min = as_int(&ok(&p, "int_min", vec![big(a), big(b)]));
            let max = as_int(&ok(&p, "int_max", vec![big(a), big(b)]));
            assert!(min == ai || min == bi, "{a}.min({b})");
            assert!(min <= ai && min <= bi, "{a}.min({b})");
            assert!(max == ai || max == bi, "{a}.max({b})");
            assert!(max >= ai && max >= bi, "{a}.max({b})");
        }
    }
}

// ===========================================================================
// I. Bytes (stdlib 3.4, S14)
// ===========================================================================

#[test]
fn from_list_accepts_every_value_from_0_to_255() {
    let p = std_program();
    let all: Vec<i64> = (0..=255).collect();
    let raw: Vec<u8> = (0..=255).collect();
    let b = ok(&p, "bytes_from_list", vec![ints(&all)]);
    assert_eq!(b, bytes(&raw));
    assert_eq!(ok(&p, "bytes_len", vec![b.clone()]), int(256));
    assert_eq!(ok(&p, "bytes_to_list", vec![b]), ints(&all));
}

#[test]
fn bytes_len_counts_bytes_not_scalars() {
    // stdlib 3.4: `b.len` is the number of bytes; from_text is the UTF-8
    // encoding (1-4 bytes per scalar).
    let p = std_program();
    for (t, count) in [
        ("", 0),
        ("a", 1),
        ("\u{0}", 1),
        ("\u{E9}", 2),
        ("\u{20AC}", 3),
        ("\u{1F600}", 4),
        ("e\u{301}", 3),
        ("\u{10FFFF}", 4),
    ] {
        let b = ok(&p, "bytes_from_text", vec![text(t)]);
        assert_eq!(ok(&p, "bytes_len", vec![b.clone()]), int(count), "{t:?}");
        assert_eq!(
            ok(&p, "bytes_is_empty", vec![b]),
            boolean(count == 0),
            "{t:?}"
        );
    }
    assert_eq!(
        ok(
            &p,
            "bytes_to_list",
            vec![ok(&p, "bytes_from_text", vec![text("\u{1F600}")])]
        ),
        ints(&[240, 159, 152, 128])
    );
}

#[test]
fn to_text_accepts_well_formed_edge_scalars_unchanged() {
    // stdlib 3.4: strict but complete UTF-8. A byte order mark is decoded
    // and kept (no stripping), NUL and noncharacters are scalar values, and
    // the boundary scalars of each sequence length decode.
    let p = std_program();
    let cases: [(&[u8], &str); 11] = [
        (&[0xEF, 0xBB, 0xBF, 0x61], "\u{FEFF}a"),
        (&[0x00], "\u{0}"),
        (&[0x7F], "\u{7F}"),
        (&[0xC2, 0x80], "\u{80}"),
        (&[0xDF, 0xBF], "\u{7FF}"),
        (&[0xE0, 0xA0, 0x80], "\u{800}"),
        (&[0xED, 0x9F, 0xBF], "\u{D7FF}"),
        (&[0xEE, 0x80, 0x80], "\u{E000}"),
        (&[0xEF, 0xBF, 0xBE, 0xEF, 0xBF, 0xBF], "\u{FFFE}\u{FFFF}"),
        (&[0xF0, 0x90, 0x80, 0x80], "\u{10000}"),
        (&[0xEF, 0xBF, 0xBD], "\u{FFFD}"),
    ];
    for (raw, t) in cases {
        assert_eq!(
            ok(&p, "bytes_to_text", vec![bytes(raw)]),
            Value::ok(&p, text(t)),
            "{raw:02x?}"
        );
    }
}

#[test]
fn to_text_rejects_ill_formed_utf8_as_a_constraint_error_named_text() {
    // stdlib 3.4: no overlong forms, no encoded surrogates, nothing above
    // U+10FFFF, no truncated sequence; the failure is an Err (S14) whose
    // ConstraintError names Text (S44), never a fault.
    let p = std_program();
    let cases: [&[u8]; 22] = [
        &[0xC0, 0xAF],
        &[0xC1, 0xBF],
        &[0xE0, 0x80, 0xAF],
        &[0xE0, 0x9F, 0xBF],
        &[0xF0, 0x80, 0x80, 0x80],
        &[0xF0, 0x8F, 0xBF, 0xBF],
        &[0xED, 0xA0, 0x80],
        &[0xED, 0xBF, 0xBF],
        &[0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80],
        &[0xF4, 0x90, 0x80, 0x80],
        &[0xF5, 0x80, 0x80, 0x80],
        &[0xF8, 0x88, 0x80, 0x80, 0x80],
        &[0xFC, 0x84, 0x80, 0x80, 0x80, 0x80],
        &[0xFE],
        &[0xFF, 0xFE],
        &[0xBF],
        &[0xC2],
        &[0xE2, 0x82],
        &[0xF0, 0x9F, 0x98],
        &[0x61, 0xC3, 0x28],
        &[0xE2, 0x28, 0xA1],
        &[0x61, 0x62, 0x63, 0xFF],
    ];
    for raw in cases {
        let v = call(&p, "bytes_to_text", vec![bytes(raw)])
            .unwrap_or_else(|f| panic!("{raw:02x?}: a fault instead of an Err: {f}"));
        let (type_name, message) = constraint_parts(&p, &v);
        assert_eq!(type_name, "Text", "{raw:02x?}");
        assert!(
            !message.is_empty() && !message.contains(['\n', '\r']),
            "{raw:02x?}: {message:?}"
        );
    }
}

#[test]
fn bytes_stub_contracts_hold_on_adversarial_texts() {
    // std/bytes.cpl: from_text ensures text.len <= result.len <= 4 *
    // text.len and to_text(result) == Ok(text); to_list ensures len,
    // range, and from_list(result) == bytes (B1-B3).
    let p = std_program();
    let low = BigInt::from(0);
    let high = BigInt::from(255);
    for t in [
        "",
        "\u{0}",
        "\u{FEFF}",
        "e\u{301}",
        "\u{10FFFF}",
        "\u{1F468}\u{200D}\u{1F467}",
        "a\"\\\n",
    ] {
        let b = ok(&p, "bytes_from_text", vec![text(t)]);
        let count = as_int(&ok(&p, "bytes_len", vec![b.clone()]));
        let scalars = BigInt::from(t.chars().count());
        assert!(count >= scalars && count <= &scalars * 4, "{t:?}");
        assert_eq!(
            ok(&p, "bytes_to_text", vec![b.clone()]),
            Value::ok(&p, text(t)),
            "{t:?}"
        );
        let values = ok(&p, "bytes_to_list", vec![b.clone()]);
        assert_eq!(BigInt::from(items(&values).len()), count, "{t:?}");
        for v in items(&values) {
            let v = as_int(&v);
            assert!(v >= low && v <= high, "{t:?}");
        }
        assert_eq!(ok(&p, "bytes_from_list", vec![values]), b, "{t:?}");
    }
}

// ===========================================================================
// J. Option and Result (stdlib 5.1, 5.2)
// ===========================================================================

#[test]
fn option_and_result_members_unwrap_exactly_one_level() {
    // stdlib 5.2 (ok, err) and 5.1 (ok_or) on nested payloads: one level of
    // Option or Result is removed or added, never more.
    let p = std_program();
    let some = |v| Value::some(&p, v);
    let okv = |v| Value::ok(&p, v);
    let errv = |v| Value::err(&p, v);
    assert_eq!(
        ok(&p, "result_ok", vec![okv(errv(text("x")))]),
        some(errv(text("x")))
    );
    assert_eq!(
        ok(&p, "result_err", vec![errv(okv(int(1)))]),
        some(okv(int(1)))
    );
    assert_eq!(
        ok(&p, "result_ok", vec![okv(Value::none(&p))]),
        some(Value::none(&p))
    );
    assert_eq!(
        ok(&p, "result_is_ok", vec![okv(errv(int(0)))]),
        boolean(true)
    );
    assert_eq!(
        ok(&p, "option_is_some", vec![some(Value::none(&p))]),
        boolean(true)
    );
    assert_eq!(
        ok(&p, "option_ok_or", vec![some(Value::none(&p)), text("e")]),
        okv(Value::none(&p))
    );
    assert_eq!(
        ok(
            &p,
            "result_unwrap_or",
            vec![okv(errv(int(1))), errv(int(2))]
        ),
        errv(int(1))
    );
    assert_eq!(
        ok(&p, "result_unwrap_or", vec![errv(okv(int(1))), okv(int(2))]),
        okv(int(2))
    );
}

#[test]
fn option_and_result_stub_contracts_hold_on_nested_payloads() {
    // Every ensures clause of the Option and Result members of
    // std/prelude.cpl, and the laws O1-O4, with payloads that are
    // themselves options, results, lists, or Unit.
    let p = std_program();
    let pairs = [
        (int(0), int(7)),
        (Value::none(&p), Value::some(&p, int(5))),
        (Value::some(&p, Value::none(&p)), Value::none(&p)),
        (Value::err(&p, text("e")), Value::ok(&p, int(1))),
        (ints(&[]), ints(&[1])),
        (Value::unit(&p), Value::unit(&p)),
    ];
    let bool_of = |id: &str, v: &Value| as_bool(&ok(&p, id, vec![v.clone()]));
    for (payload, default) in &pairs {
        let none = Value::none(&p);
        for o in [Value::some(&p, payload.clone()), none.clone()] {
            let is_some = bool_of("option_is_some", &o);
            let is_none = bool_of("option_is_none", &o);
            // is_some: result == (not option.is_none()); is_none: result == (option == None)
            assert_eq!(is_some, !is_none);
            assert_eq!(is_none, o == none);
            // unwrap_or: option.is_none() or option == Some(result);
            // option.is_some() or result == default
            let r = ok(&p, "option_unwrap_or", vec![o.clone(), default.clone()]);
            assert!(is_none || o == Value::some(&p, r.clone()));
            assert!(is_some || r == *default);
            // ok_or: result.is_ok() == option.is_some(); option.is_some() or result == Err(error)
            let e = text("error");
            let res = ok(&p, "option_ok_or", vec![o.clone(), e.clone()]);
            assert_eq!(bool_of("result_is_ok", &res), is_some);
            assert!(is_some || res == Value::err(&p, e));
        }
        for r in [
            Value::ok(&p, payload.clone()),
            Value::err(&p, payload.clone()),
        ] {
            let is_ok = bool_of("result_is_ok", &r);
            let is_err = bool_of("result_is_err", &r);
            // is_err: result == (not res.is_ok())
            assert_eq!(is_err, !is_ok);
            // unwrap_or: res.is_err() or res == Ok(result); res.is_ok() or result == default
            let u = ok(&p, "result_unwrap_or", vec![r.clone(), default.clone()]);
            assert!(is_err || r == Value::ok(&p, u.clone()));
            assert!(is_ok || u == *default);
            // ok: result.is_some() == res.is_ok(); err: result.is_some() == res.is_err() (O4)
            let okd = ok(&p, "result_ok", vec![r.clone()]);
            let errd = ok(&p, "result_err", vec![r.clone()]);
            assert_eq!(bool_of("option_is_some", &okd), is_ok);
            assert_eq!(bool_of("option_is_some", &errd), is_err);
        }
    }
}

#[test]
fn unwrap_or_and_ok_or_evaluate_their_argument_even_when_it_is_unused() {
    // stdlib 5.1: "d is always evaluated (it is an argument)"; semantics
    // 6.2: a method call evaluates the receiver, then the arguments, then
    // the builtin, so `Some(1).unwrap_or(a / b)` with b == 0 faults.
    let mut l = Lab::new();
    let h = l.h;
    let opt = h.option_ty(Ty::Int);
    let res = h.result_ty(Ty::Int, Ty::Text);
    let div =
        |a: LocalId, b: LocalId| h.binary(BinaryOp::Div, h.local(a, Ty::Int), h.local(b, Ty::Int));
    // fn opt_default(o: Option<Int>, a: Int, b: Int) -> Int { o.unwrap_or(a / b) }
    let mut f = FnBuilder::new("opt_default", Ty::Int);
    let o = f.param("o", opt.clone());
    let a = f.param("a", Ty::Int);
    let b = f.param("b", Ty::Int);
    f.body(h.value(l.call(
        "option_unwrap_or",
        &[Ty::Int],
        vec![h.local(o, opt.clone()), div(a, b)],
        Ty::Int,
    )));
    let opt_default = l.add_fn(f);
    // fn res_default(r: Result<Int, Text>, a: Int, b: Int) -> Int { r.unwrap_or(a / b) }
    let mut f = FnBuilder::new("res_default", Ty::Int);
    let r = f.param("r", res.clone());
    let a = f.param("a", Ty::Int);
    let b = f.param("b", Ty::Int);
    f.body(h.value(l.call(
        "result_unwrap_or",
        &[Ty::Int, Ty::Text],
        vec![h.local(r, res.clone()), div(a, b)],
        Ty::Int,
    )));
    let res_default = l.add_fn(f);
    // fn opt_error(o: Option<Int>, a: Int, b: Int) -> Result<Int, Int> { o.ok_or(a / b) }
    let mut f = FnBuilder::new("opt_error", h.result_ty(Ty::Int, Ty::Int));
    let o = f.param("o", opt.clone());
    let a = f.param("a", Ty::Int);
    let b = f.param("b", Ty::Int);
    f.body(h.value(l.call(
        "option_ok_or",
        &[Ty::Int, Ty::Int],
        vec![h.local(o, opt), div(a, b)],
        h.result_ty(Ty::Int, Ty::Int),
    )));
    let opt_error = l.add_fn(f);
    let p = l.build();
    let some1 = json!({"$type": "Option", "$variant": "Some", "$payload": [1]});
    let ok1 = ok_json(json!(1));
    assert_eq!(
        returns(&p, opt_default, &[some1.clone(), json!(6), json!(3)]),
        json!(1)
    );
    assert_eq!(
        returns(&p, res_default, &[ok1.clone(), json!(6), json!(3)]),
        json!(1)
    );
    assert_eq!(
        returns(&p, opt_error, &[some1.clone(), json!(6), json!(3)]),
        ok_json(json!(1))
    );
    for (f, receiver) in [
        (opt_default, &some1),
        (res_default, &ok1),
        (opt_error, &some1),
    ] {
        let d = faults(&p, f, &[receiver.clone(), json!(6), json!(0)]);
        assert_eq!(d["code"], "division_by_zero", "{d}");
        assert_eq!(d["input"], json!({"a": 6, "b": 0}));
    }
}

// ===========================================================================
// K. ConstraintError payloads (stdlib 1.4, 1.5, S5, S44; semantics 6.10)
// ===========================================================================

#[test]
fn check_messages_render_text_with_the_value_text_escapes() {
    // semantics 6.10: message = "<value text of v> does not satisfy
    // <predicate text>", fixed (stdlib 1.5: tests compare it). Value text
    // (semantics S40) escapes `"` and `\`, writes U+000A, U+000D, U+0009 as
    // \n, \r, \t, every other scalar of U+0000-U+001F and U+007F as \u{H}
    // with the minimal number of lowercase hex digits, and every other
    // scalar literally (U+0085 and U+00E9 here).
    let mut l = Lab::new();
    let h = l.h;
    // type Short = Text where self.len < 3
    let pred = h.binary(
        BinaryOp::Lt,
        l.call(
            "text_len",
            &[],
            vec![h.local(LocalId(0), Ty::Text)],
            Ty::Int,
        ),
        h.int(3),
    );
    let short =
        l.b.add_refinement(l.m, "Short", Ty::Text, pred, "self.len < 3");
    let check = checker(&mut l, "short_check", short, Ty::Text);
    let p = l.build();
    let input = "q\"\\\n\r\t\u{0}\u{1B}\u{7F}\u{85}\u{E9}";
    let mut expected = String::from(r#""q\"\\\n\r\t\u{0}\u{1b}\u{7f}"#);
    expected.push_str("\u{85}\u{E9}");
    expected.push_str(r#"" does not satisfy self.len < 3"#);
    assert_eq!(
        returns(&p, check, &[json!(input)]),
        err_json("adv.b.Short", &expected)
    );
    assert_eq!(returns(&p, check, &[json!("ab")]), ok_json(json!("ab")));
    // U+2028 and U+2029 are outside U+0000-U+001F and U+007F: literal.
    let separators = "\u{2028}\u{2029}\u{2028}";
    assert_eq!(
        returns(&p, check, &[json!(separators)]),
        err_json(
            "adv.b.Short",
            &format!("\"{separators}\" does not satisfy self.len < 3")
        )
    );
}

#[test]
fn check_messages_render_structs_and_variants_as_value_text() {
    // semantics S40 value text: structs as `Point { x: 1, y: -2 }` (bare
    // name), variants as `Tint(3)` / `Dark`; any type may be a refinement
    // base (semantics 3.3: `type T = B where p`).
    let mut l = Lab::new();
    let h = l.h;
    let point =
        l.b.add_struct(l.m, "Point", &[], &[("x", Ty::Int), ("y", Ty::Int)]);
    let shade = l.b.add_enum(
        l.m,
        "Shade",
        &[],
        &[("Dark", vec![]), ("Tint", vec![Ty::Int])],
    );
    let point_ty = l.b.adt(point);
    let shade_ty = l.b.adt(shade);
    // type Origin = Point where self.x == 0
    let pred = h.binary(
        BinaryOp::Eq,
        h.field(h.local(LocalId(0), point_ty.clone()), point, 0, Ty::Int),
        h.int(0),
    );
    let origin =
        l.b.add_refinement(l.m, "Origin", point_ty.clone(), pred, "self.x == 0");
    // type Pale = Shade where self == Dark
    let pred = h.binary(
        BinaryOp::Eq,
        h.local(LocalId(0), shade_ty.clone()),
        h.ctor(shade, 0, Vec::new(), shade_ty.clone()),
    );
    let pale =
        l.b.add_refinement(l.m, "Pale", shade_ty.clone(), pred, "self == Dark");
    let c_origin = checker(&mut l, "origin_check", origin, point_ty);
    let c_pale = checker(&mut l, "pale_check", pale, shade_ty);
    let p = l.build();
    assert_eq!(
        returns(
            &p,
            c_origin,
            &[json!({"$type": "adv.b.Point", "x": 1, "y": -2})]
        ),
        err_json(
            "adv.b.Origin",
            "Point { x: 1, y: -2 } does not satisfy self.x == 0"
        )
    );
    assert_eq!(
        returns(
            &p,
            c_pale,
            &[json!({"$type": "adv.b.Shade", "$variant": "Tint", "$payload": [3]})]
        ),
        err_json("adv.b.Pale", "Tint(3) does not satisfy self == Dark")
    );
    assert_eq!(
        returns(
            &p,
            c_pale,
            &[json!({"$type": "adv.b.Shade", "$variant": "Dark"})]
        ),
        ok_json(json!({"$type": "adv.b.Shade", "$variant": "Dark"}))
    );
}

#[test]
fn check_messages_render_lists_options_and_bytes_as_value_text() {
    // semantics S40 value text: lists as `[a, b]`, variants by name with
    // their payload, Bytes as `Bytes("<lowercase hex>")`; type_name is the
    // refinement's qualified name (S44).
    let mut l = Lab::new();
    let h = l.h;
    let oi = h.option_ty(Ty::Int);
    let loi = Ty::list(oi.clone());
    // type NonEmpty = List<Int> where not self.is_empty()
    let pred = h.unary(
        UnaryOp::Not,
        l.call(
            "list_is_empty",
            &[Ty::Int],
            vec![h.local(LocalId(0), li())],
            Ty::Bool,
        ),
    );
    let non_empty =
        l.b.add_refinement(l.m, "NonEmpty", li(), pred, "not self.is_empty()");
    // type Few = List<Option<Int>> where self.len < 2
    let pred = h.binary(
        BinaryOp::Lt,
        l.call(
            "list_len",
            &[oi],
            vec![h.local(LocalId(0), loi.clone())],
            Ty::Int,
        ),
        h.int(2),
    );
    let few =
        l.b.add_refinement(l.m, "Few", loi.clone(), pred, "self.len < 2");
    // type Tiny = Bytes where self.len < 2
    let pred = h.binary(
        BinaryOp::Lt,
        l.call(
            "bytes_len",
            &[],
            vec![h.local(LocalId(0), Ty::Bytes)],
            Ty::Int,
        ),
        h.int(2),
    );
    let tiny =
        l.b.add_refinement(l.m, "Tiny", Ty::Bytes, pred, "self.len < 2");
    let c_non_empty = checker(&mut l, "non_empty_check", non_empty, li());
    let c_few = checker(&mut l, "few_check", few, loi);
    let c_tiny = checker(&mut l, "tiny_check", tiny, Ty::Bytes);
    let p = l.build();
    assert_eq!(
        returns(&p, c_non_empty, &[json!([])]),
        err_json("adv.b.NonEmpty", "[] does not satisfy not self.is_empty()")
    );
    assert_eq!(returns(&p, c_non_empty, &[json!([3])]), ok_json(json!([3])));
    assert_eq!(
        returns(
            &p,
            c_few,
            &[json!([
                {"$type": "Option", "$variant": "Some", "$payload": [1]},
                {"$type": "Option", "$variant": "None"}
            ])]
        ),
        err_json("adv.b.Few", "[Some(1), None] does not satisfy self.len < 2")
    );
    assert_eq!(
        returns(&p, c_tiny, &[json!({"$type": "Bytes", "hex": "0102ff"})]),
        err_json(
            "adv.b.Tiny",
            "Bytes(\"0102ff\") does not satisfy self.len < 2"
        )
    );
}

#[test]
fn a_check_error_equals_the_struct_written_by_hand() {
    // S5 and S7: ConstraintError is an ordinary struct with structural
    // equality, and check's message is fixed (semantics 6.10), so the Err
    // value can be written out in full and compared with `==`.
    // fn same(t: Text) -> Bool {
    //     check<Short>(t) == Err(ConstraintError { type_name: "adv.b.Short",
    //                                              message: "\"abcd\" does not satisfy self.len < 3" })
    // }
    let mut l = Lab::new();
    let h = l.h;
    let pred = h.binary(
        BinaryOp::Lt,
        l.call(
            "text_len",
            &[],
            vec![h.local(LocalId(0), Ty::Text)],
            Ty::Int,
        ),
        h.int(3),
    );
    let short =
        l.b.add_refinement(l.m, "Short", Ty::Text, pred, "self.len < 3");
    let ce = l.b.std_defs().constraint_error;
    let index = |name: &str| {
        l.b.def(ce)
            .kind
            .as_struct()
            .expect("ConstraintError is a struct")
            .fields
            .iter()
            .position(|f| f.name == name)
            .unwrap_or_else(|| panic!("ConstraintError has no field {name}")) as u32
    };
    let lit = h.struct_lit(
        ce,
        None,
        vec![
            (index("type_name"), h.text("adv.b.Short")),
            (
                index("message"),
                h.text("\"abcd\" does not satisfy self.len < 3"),
            ),
        ],
        h.constraint_error_ty(),
    );
    let short_ty = l.b.refined(short);
    let result_ty = h.result_ty(short_ty.clone(), h.constraint_error_ty());
    let mut f = FnBuilder::new("same", Ty::Bool);
    let t = f.param("t", Ty::Text);
    let checked = h.check(short, h.local(t, Ty::Text), result_ty);
    f.body(h.value(h.binary(BinaryOp::Eq, checked, h.err(lit, short_ty))));
    let same = l.add_fn(f);
    let p = l.build();
    assert_eq!(returns(&p, same, &[json!("abcd")]), json!(true));
    assert_eq!(returns(&p, same, &[json!("abcde")]), json!(false));
    assert_eq!(returns(&p, same, &[json!("ab")]), json!(false));
}

#[test]
fn parse_and_decode_errors_are_values_not_faults() {
    // stdlib 0.2 and S14: parse_int and to_text fail with Err(...), an
    // ordinary value; ConstraintError names the prelude type bare (S44).
    let p = std_program();
    for t in ["", "x", "1.0", "\u{FF11}"] {
        let v = call(&p, "text_parse_int", vec![text(t)])
            .unwrap_or_else(|f| panic!("{t:?}: a fault instead of an Err: {f}"));
        assert_eq!(constraint_parts(&p, &v).0, "Int", "{t:?}");
    }
    let v = call(&p, "bytes_to_text", vec![bytes(&[0x80])])
        .unwrap_or_else(|f| panic!("a fault instead of an Err: {f}"));
    assert_eq!(constraint_parts(&p, &v).0, "Text");
}

// ===========================================================================
// L. No hidden fault sources (stdlib 0.2)
// ===========================================================================

#[test]
fn builtins_without_a_requires_clause_never_fault() {
    // stdlib 0.2: "Every potential fault of a standard operation is visible
    // as a requires clause in its stub ...; there are no hidden fault
    // sources". Only take, drop, split, from_list (and gen.range) have one.
    let p = std_program();
    let huge = big("-340282366920938463463374607431768211457");
    let calls = [
        ("list_get", vec![ints(&[]), huge.clone()]),
        (
            "list_get",
            vec![ints(&[1]), big("340282366920938463463374607431768211456")],
        ),
        ("list_first", vec![ints(&[])]),
        ("list_last", vec![ints(&[])]),
        ("list_drop_last", vec![ints(&[])]),
        ("list_reverse", vec![ints(&[])]),
        ("list_contains", vec![ints(&[]), int(0)]),
        ("list_push", vec![ints(&[]), huge.clone()]),
        ("flatten", vec![list(vec![])]),
        ("int_to_text", vec![huge.clone()]),
        ("int_abs", vec![huge.clone()]),
        ("int_min", vec![huge.clone(), int(0)]),
        ("int_max", vec![huge.clone(), int(0)]),
        ("text_parse_int", vec![text("\u{FF11}")]),
        ("text_parse_int", vec![text("")]),
        ("text_join", vec![texts(&[]), text("")]),
        ("text_chars", vec![text("")]),
        ("text_starts_with", vec![text(""), text("\u{1F600}")]),
        ("text_ends_with", vec![text(""), text("\u{1F600}")]),
        ("text_contains", vec![text(""), text("\u{1F600}")]),
        ("text_trim", vec![text("")]),
        ("text_to_upper", vec![text("\u{0}")]),
        ("text_from_int", vec![huge.clone()]),
        ("bytes_to_text", vec![bytes(&[0xFF, 0xFE, 0xFD])]),
        ("bytes_from_text", vec![text("\u{10FFFF}")]),
        ("bytes_to_list", vec![bytes(&[])]),
        ("option_unwrap_or", vec![Value::none(&p), int(0)]),
        ("result_unwrap_or", vec![Value::err(&p, text("x")), int(0)]),
        ("option_ok_or", vec![Value::none(&p), huge]),
    ];
    for (id, args) in calls {
        assert!(call(&p, id, args).is_ok(), "{id}");
    }
}

/// A well-typed value of `ty` that satisfies every builtin precondition
/// (counts and bytes in range, a non-empty separator); a type parameter is
/// instantiated as `Int`.
fn sample(p: &Program, ty: &Ty) -> Value {
    match ty {
        Ty::Int | Ty::Param(_) | Ty::Refined(..) => int(1),
        Ty::Bool => boolean(true),
        Ty::Text => text("a,b"),
        Ty::Bytes => bytes(b"hi"),
        Ty::List(elem) => list(vec![sample(p, elem)]),
        Ty::Adt(def, args) if *def == p.std_defs.option => Value::some(p, sample(p, &args[0])),
        Ty::Adt(def, args) if *def == p.std_defs.result => Value::ok(p, sample(p, &args[0])),
        other => panic!("no sample for {}", p.display_ty(other)),
    }
}

/// Whether `v` is a value of `ty` (a type parameter admits anything).
fn has_type(p: &Program, v: &Value, ty: &Ty) -> bool {
    match (ty, v) {
        (Ty::Param(_), _) => true,
        (Ty::Int | Ty::Refined(..), Value::Int(_))
        | (Ty::Bool, Value::Bool(_))
        | (Ty::Text, Value::Text(_))
        | (Ty::Bytes, Value::Bytes(_)) => true,
        (Ty::List(elem), Value::List(xs)) => xs.iter().all(|x| has_type(p, x, elem)),
        (
            Ty::Adt(def, args),
            Value::Enum {
                def: d, payload, ..
            },
        ) if def == d => {
            if *def == p.std_defs.option {
                payload.iter().all(|x| has_type(p, x, &args[0]))
            } else if *def == p.std_defs.result {
                match v.as_result(p) {
                    Some(Ok(x)) => has_type(p, x, &args[0]),
                    Some(Err(e)) => has_type(p, e, &args[1]),
                    None => false,
                }
            } else {
                true
            }
        }
        (Ty::Adt(def, _), Value::Struct { def: d, .. }) => def == d,
        _ => false,
    }
}

#[test]
fn every_builtin_accepts_its_stub_signature_and_returns_its_declared_type() {
    // stdlib 10.6: a builtin is called with the evaluated arguments in
    // declaration order, receiver first, so every callable builtin accepts
    // one well-typed value per parameter of its stub (no arity or type
    // confusion) and returns a value of the stub's return type (0.1: the
    // stub text is the signature). Generators are never called (S28, 10.6).
    let p = std_program();
    let mut checked = 0;
    for d in p.defs() {
        let DefKind::Fn(fd) = &d.kind else { continue };
        let FnBody::Builtin(id) = &fd.body else {
            continue;
        };
        if id.starts_with("gen_") {
            continue;
        }
        let args: Vec<Value> = fd
            .params
            .iter()
            .map(|param| sample(&p, &param.ty))
            .collect();
        let v = call(&p, id, args).unwrap_or_else(|f| panic!("{id}: {f}"));
        assert!(
            has_type(&p, &v, &fd.ret),
            "{id} returned {v:?}, not a {}",
            p.display_ty(&fd.ret)
        );
        checked += 1;
    }
    // 59 registry ids less the 6 builtin types and the 7 generators.
    assert_eq!(checked, 46);
}

/// `property "<name>" { for x in gen.range(lo, hi): assert x == lo }`.
fn range_property(l: &mut Lab, name: &str, lo: &str, hi: &str) -> DefId {
    let h = l.h;
    let lo = as_int(&big(lo));
    let hi = as_int(&big(hi));
    let mut pb = PropertyBuilder::new(name);
    let x = pb.binder(
        "x",
        GenExpr::Range(Box::new(h.int(lo.clone())), Box::new(h.int(hi))),
        Ty::Int,
    );
    pb.body(h.block(
        vec![Stmt::Assert {
            expr: h.binary(BinaryOp::Eq, h.local(x, Ty::Int), h.int(lo)),
            text: "x == lo".to_string(),
        }],
        None,
    ));
    l.b.add_property(l.m, pb)
}

#[test]
fn gen_range_precondition_reports_huge_bounds_with_every_digit() {
    // std/gen.cpl: `requires lo <= hi`, checked once before case 0 (toolchain
    // 1.7.2); the fault fails the property with zero cases and has the
    // builtin shape of stdlib 10.4 (input by parameter name, value JSON).
    let mut l = Lab::new();
    let inverted = range_property(
        &mut l,
        "inverted",
        "100000000000000000000",
        "99999999999999999999",
    );
    let p = l.build();
    let record = run_test(&p, inverted, &TestConfig::with_seed(3));
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.cases, Some(0));
    let d = serde_json::to_value(record.fault.as_ref().expect("the setup fault"))
        .expect("a diagnostic serializes");
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "std.gen.range");
    assert_eq!(d["clause"], "lo <= hi");
    assert_eq!(
        d["input"],
        jparse(r#"{"lo": 100000000000000000000, "hi": 99999999999999999999}"#)
    );
}

#[test]
fn a_one_value_range_draws_its_only_value_at_any_magnitude() {
    // stdlib 8.3: gen.range(lo, hi) draws lo + below(hi - lo + 1), and
    // below(1) is 0, so lo == hi always yields lo (the precondition holds
    // with equality), however large or negative.
    let mut l = Lab::new();
    let huge = range_property(
        &mut l,
        "huge point",
        "-340282366920938463463374607431768211457",
        "-340282366920938463463374607431768211457",
    );
    let zero = range_property(&mut l, "zero point", "0", "0");
    let p = l.build();
    for prop in [huge, zero] {
        let record = run_test(&p, prop, &TestConfig::with_seed(3));
        assert_eq!(record.status, TestStatus::Tested, "{:?}", record.fault);
        assert_eq!(record.cases, Some(100));
    }
}

/// `property "<name>" { for x in gen.range(<lo>, 5): assert true }`.
fn bound_property(l: &mut Lab, name: &str, lo: Expr) -> DefId {
    let h = l.h;
    let mut pb = PropertyBuilder::new(name);
    pb.binder(
        "x",
        GenExpr::Range(Box::new(lo), Box::new(h.int(5))),
        Ty::Int,
    );
    pb.body(h.block(
        vec![Stmt::Assert {
            expr: h.bool(true),
            text: "true".to_string(),
        }],
        None,
    ));
    l.b.add_property(l.m, pb)
}

#[test]
fn a_builtin_fault_inside_a_for_range_is_reported_per_s34() {
    // semantics 7.4: each `for` range "is a contract expression (5.1) ...
    // evaluated once when the property starts"; S34: a fault raised while
    // evaluating a contract expression is reported as that fault with facet
    // contract, clause = the clause text, and symbol = the declaration whose
    // clause was being evaluated, here the property. (stdlib 12's row for
    // `gen.range(5, 1)` concerns the generator's own precondition, not a
    // fault raised inside its bounds; toolchain 1.7.2: the property fails
    // with that fault and zero cases.)
    // property "bad bound" { for x in gen.range([1].take(-1).len, 5): assert true }
    let mut l = Lab::new();
    let h = l.h;
    let lo = l.len_of(l.take_n(h.list(vec![h.int(1)], Ty::Int), l.minus(1)));
    let prop = bound_property(&mut l, "bad bound", lo);
    let p = l.build();
    let record = run_test(&p, prop, &TestConfig::with_seed(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.cases, Some(0));
    let d = serde_json::to_value(record.fault.as_ref().expect("the setup fault"))
        .expect("a diagnostic serializes");
    assert_eq!(d["code"], "contract_violation", "{d}");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "adv.b.\"bad bound\"", "S34: {d}");
    assert!(
        d["clause"].is_string() && d["clause"] != "n >= 0",
        "S34: the clause is the for range's, not the builtin's: {d}"
    );
}

#[test]
fn a_division_fault_inside_a_for_range_is_reported_per_s34() {
    // The intrinsic `/` of stdlib 10.6 (S8) inside a `for` range bound:
    // division_by_zero, reported per S34 (semantics 7.4: the range is a
    // contract expression) with facet contract and the clause, blamed on
    // the property.
    // fn zero() -> Int { 0 }
    // property "zero bound" { for x in gen.range(1 / zero(), 5): assert true }
    let mut l = Lab::new();
    let h = l.h;
    let mut f = FnBuilder::new("zero", Ty::Int);
    f.body(h.value(h.int(0)));
    let zero = l.add_fn(f);
    let lo = h.binary(BinaryOp::Div, h.int(1), h.call(zero, Vec::new(), Ty::Int));
    let prop = bound_property(&mut l, "zero bound", lo);
    let p = l.build();
    let record = run_test(&p, prop, &TestConfig::with_seed(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.cases, Some(0));
    let d = serde_json::to_value(record.fault.as_ref().expect("the setup fault"))
        .expect("a diagnostic serializes");
    assert_eq!(d["code"], "division_by_zero", "{d}");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
    assert_eq!(d["symbol"], "adv.b.\"zero bound\"");
    assert_eq!(d["facet"], "contract", "S34: {d}");
    assert!(d["clause"].is_string(), "S34: the clause is reported: {d}");
}
