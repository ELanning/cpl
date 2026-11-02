//! Shared fixtures: the `reader.pages` program of the design (wrappers,
//! refinements, structs, enums, a capability) built by hand, and helpers
//! that call builtins through the registry.

use cpl_diag::json::BindingKind;
use cpl_eval::builtins;
use cpl_eval::testing::{FnBuilder, ProgramBuilder};
use cpl_eval::{EvalConfig, Fault, Interpreter, Value};
use cpl_hir::{DefId, LocalId, ModuleId, Program, Ty};
use cpl_syntax::ast::BinaryOp;
use num_bigint::BigInt;

/// `reader.pages` plus a module `m`, as the stdlib examples use them.
pub struct Reader {
    pub p: Program,
    pub pages: ModuleId,
    /// `pub type PageId(Text)`
    pub page_id: DefId,
    /// `pub type Positive = Int where self > 0`
    pub positive: DefId,
    /// `pub struct Page { id: PageId  text: Text }`
    pub page: DefId,
    /// `pub enum StoreError { Missing(PageId)  Unavailable }`
    pub store_error: DefId,
    /// `pub struct Dimensions { width: Positive  height: Positive }`
    pub dimensions: DefId,
    /// `pub struct Pair<T> { first: T  second: T }`
    pub pair: DefId,
    /// `pub type Size(Positive)`
    pub size: DefId,
    /// `pub struct Empty {}`
    pub empty: DefId,
    /// `pub capability PageRead { fn get(id: PageId) -> Text }`
    pub page_read: DefId,
    /// `m.Small = Positive where self < 10` (base named as reader.pages.Positive)
    pub small: DefId,
    /// `fn take_n(xs: List<Int>, n: Int) -> List<Int> { xs.take(n) }`
    pub take_n: DefId,
    /// `@id("pgn_7c2d") pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>`
    pub paginate: DefId,
}

pub fn reader() -> Reader {
    let mut b = ProgramBuilder::new();
    let pages = b.module("reader.pages");
    let page_id = b.add_wrapper(pages, "PageId", Ty::Text);
    let positive = b.add_int_refinement(pages, "Positive", BinaryOp::Gt, 0);
    let page = b.add_struct(
        pages,
        "Page",
        &[],
        &[("id", Ty::Wrapper(page_id)), ("text", Ty::Text)],
    );
    let store_error = b.add_enum(
        pages,
        "StoreError",
        &[],
        &[
            ("Missing", vec![Ty::Wrapper(page_id)]),
            ("Unavailable", vec![]),
        ],
    );
    let positive_ty = b.refined(positive);
    let dimensions = b.add_struct(
        pages,
        "Dimensions",
        &[],
        &[
            ("width", positive_ty.clone()),
            ("height", positive_ty.clone()),
        ],
    );
    let pair = b.add_struct(
        pages,
        "Pair",
        &["T"],
        &[("first", Ty::Param(0)), ("second", Ty::Param(0))],
    );
    let size = b.add_wrapper(pages, "Size", positive_ty.clone());
    let empty = b.add_struct(pages, "Empty", &[], &[]);
    let page_read = b.add_capability(
        pages,
        "PageRead",
        &[("get", vec![("id", Ty::Wrapper(page_id))], Ty::Text)],
    );

    let m = b.module("m");
    let hm = b.hir(m);
    // `self` is widened to Int for `<`, as the checker types it.
    let small_pred = hm.binary(
        BinaryOp::Lt,
        hm.widen(hm.local(LocalId(0), positive_ty.clone()), positive, Ty::Int),
        hm.int(10),
    );
    let small = b.add_refinement(m, "Small", positive_ty.clone(), small_pred, "self < 10");

    let h = b.hir(pages);
    let take = b.builtin("list_take");
    let mut f = FnBuilder::new("take_n", Ty::list(Ty::Int));
    let xs = f.param("xs", Ty::list(Ty::Int));
    let n = f.param("n", Ty::Int);
    f.body(h.value(h.call_generic(
        take,
        vec![Ty::Int],
        vec![h.local(xs, Ty::list(Ty::Int)), h.local(n, Ty::Int)],
        Ty::list(Ty::Int),
    )));
    let take_n = b.add_fn(pages, f);

    let mut f = FnBuilder::new("paginate", Ty::Error).public().prove();
    let t = f.generic("T");
    let pages_ty = Ty::list(Ty::list(t.clone()));
    f.returns(pages_ty.clone());
    f.param("items", Ty::list(t));
    f.param("size", positive_ty);
    f.local("pages", pages_ty.clone(), BindingKind::Var, true);
    f.body(h.value(h.hole(0, "build_pages", pages_ty)));
    let paginate = b.add_fn(pages, f);
    b.set_stable_id(paginate, "pgn_7c2d");

    Reader {
        p: b.build(),
        pages,
        page_id,
        positive,
        page,
        store_error,
        dimensions,
        pair,
        size,
        empty,
        page_read,
        small,
        take_n,
        paginate,
    }
}

/// `Ty::Refined(class, def)` for a refinement declaration of `p`.
pub fn refined(p: &Program, def: DefId) -> Ty {
    let class = p
        .refinement_classes
        .iter()
        .position(|c| c.members.contains(&def))
        .unwrap_or_else(|| panic!("DefId({}) is in no refinement class", def.0));
    Ty::Refined(cpl_hir::RefinementClass(class as u32), def)
}

// ---- values -------------------------------------------------------------------

pub fn int(n: i64) -> Value {
    Value::int(n)
}

/// An `Int` from decimal digits of any length.
pub fn big(digits: &str) -> Value {
    Value::Int(digits.parse::<BigInt>().expect("decimal digits"))
}

pub fn ints(ns: &[i64]) -> Value {
    Value::list(ns.iter().map(|&n| Value::int(n)).collect())
}

pub fn text(s: &str) -> Value {
    Value::text(s)
}

pub fn texts(ts: &[&str]) -> Value {
    Value::list(ts.iter().map(|t| Value::text(t)).collect())
}

pub fn list(items: Vec<Value>) -> Value {
    Value::list(items)
}

pub fn bytes(b: &[u8]) -> Value {
    Value::bytes(b)
}

/// Parse a JSON document (numbers keep every digit).
pub fn json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("bad JSON {s}: {e}"))
}

// ---- calling builtins -----------------------------------------------------------

/// Call the builtin `id` through the registry, with no enclosing
/// declaration (the top level of `cpl run`).
pub fn call(p: &Program, id: &str, args: Vec<Value>) -> Result<Value, Fault> {
    let f = builtins::lookup(id).unwrap_or_else(|| panic!("no builtin {id}"));
    let mut it = Interpreter::new(p, EvalConfig::default());
    f(&mut it, &args)
}

/// Call the builtin `id` from inside an activation of `caller`.
pub fn call_from(p: &Program, caller: DefId, id: &str, args: Vec<Value>) -> Result<Value, Fault> {
    let f = builtins::lookup(id).unwrap_or_else(|| panic!("no builtin {id}"));
    let mut it = Interpreter::new(p, EvalConfig::default());
    it.push_frame(caller, None);
    f(&mut it, &args)
}

/// The value of a builtin call that must succeed.
pub fn ok(p: &Program, id: &str, args: Vec<Value>) -> Value {
    call(p, id, args).unwrap_or_else(|f| panic!("{id} faulted: {f}"))
}

/// The fault of a builtin call that must fail.
pub fn fault(p: &Program, id: &str, args: Vec<Value>) -> Fault {
    match call(p, id, args) {
        Ok(v) => panic!("{id} returned {v:?} instead of faulting"),
        Err(f) => f,
    }
}
