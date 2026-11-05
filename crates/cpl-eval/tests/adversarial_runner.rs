//! Adversarial tests of the test and property runner, written from the
//! specifications alone: semantics 7 (7.2-7.5: S42-S46), stdlib 8.2-8.4
//! (S30-S32), toolchain 1.7.2, and the `cpl test` contract of toolchain 7.2.
//!
//! The core of the file is an independent oracle (`mod oracle`). From the
//! text of stdlib 8.2-8.4 alone it re-derives every property's random
//! stream (`ChaCha20Rng::from_seed(blake3(seed_le(8) ++ identity))`), the
//! size of every case (`min(i, 100)`), every generator's draw, every shrink
//! candidate list, and the greedy coordinate-wise shrinker of S45. A test
//! builds a property whose failure condition is also written in Rust (the
//! *model*), asks the oracle for the first failing case, the shrunk tuple
//! and the exact number of shrink steps, and compares them with the record
//! `run_test` returns. Where the specifications leave a reading open, the
//! comment at the test says which reading it encodes.
//!
//! Programs are built by hand with `cpl_eval::testing`, in the forms the
//! checker gives them (a negative literal is a negation); the draw-replay
//! properties (`replay_check`) are CPL source checked by cpl-sema (a
//! dev-dependency, toolchain 1.2), so their comparisons are typed exactly as
//! the checker types them (semantics 3.6).

mod common;

use cpl_diag::json::{BindingKind, TestCaseRecord, TestKind, TestStatus};
use cpl_diag::{Category, Severity, StableId};
use cpl_eval::test::{property_seed, test_result_diagnostic, TestConfig, TestSelector};
use cpl_eval::testing::{FnBuilder, Hir, ProgramBuilder, PropertyBuilder, TestBuilder};
use cpl_eval::{run_test, run_tests, select_tests};
use cpl_hir::{Block, DefId, Expr, GenExpr, LocalId, ModuleId, Program, Stmt, Ty};
use cpl_syntax::ast::BinaryOp as B;
use cpl_syntax::ast::UnaryOp;
use num_bigint::BigInt;
use oracle::{list, opt, Model, G, V};
use serde_json::{json, Value as Json};
use std::time::Duration;

// ===========================================================================
// The oracle: stdlib 8.2-8.4 and semantics S44-S45, re-derived
// ===========================================================================

mod oracle {
    use num_bigint::{BigInt, BigUint};
    use num_traits::{One, Signed, ToPrimitive, Zero};
    use rand_chacha::rand_core::{Rng, SeedableRng};
    use rand_chacha::ChaCha20Rng;
    use serde_json::{json, Map, Value as Json};

    /// stdlib 8.3: the fixed list of `gen.text`'s fourth bucket, in order.
    pub const EXTRAS: [char; 10] = [
        '\u{e9}',
        '\u{df}',
        '\u{130}',
        '\u{3a9}',
        '\u{4e2d}',
        '\u{1f600}',
        '\u{200b}',
        '\u{301}',
        '\u{fffd}',
        '\u{a0}',
    ];

    /// A test or property name as its qualified name spells it (semantics
    /// 1.4, S40; toolchain 4.3): quoted, with `\`, `"`, LF, CR, and TAB
    /// escaped, every other control character and DEL as `\u{h}`.
    pub fn quote(name: &str) -> String {
        let mut out = String::from("\"");
        for c in name.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                    out.push_str(&format!("\\u{{{:x}}}", c as u32));
                }
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// `<module path>."<name>"`: the provisional identity of a test or
    /// property (semantics S2, S42).
    pub fn qualified(module: &str, name: &str) -> String {
        format!("{module}.{}", quote(name))
    }

    /// S44 / stdlib 8.2: `blake3(seed_le(8) ++ identity_utf8)`.
    pub fn seed_bytes(run_seed: u64, identity: &str) -> [u8; 32] {
        let mut bytes = run_seed.to_le_bytes().to_vec();
        bytes.extend_from_slice(identity.as_bytes());
        *blake3::hash(&bytes).as_bytes()
    }

    /// The `property_seed` field: 64 lowercase hex digits (T45).
    pub fn seed_hex(run_seed: u64, identity: &str) -> String {
        seed_bytes(run_seed, identity)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// One property's stream and the primitive draws of stdlib 8.3.
    pub struct Stream(ChaCha20Rng);

    impl Stream {
        pub fn new(run_seed: u64, identity: &str) -> Stream {
            Stream(ChaCha20Rng::from_seed(seed_bytes(run_seed, identity)))
        }

        /// `u()`: the next `u32` word.
        fn u(&mut self) -> u32 {
            self.0.next_u32()
        }

        /// `bits(b)`: `ceil(b / 32)` words, little-endian, low `b` bits.
        fn bits(&mut self, b: u64) -> BigUint {
            let mut x = BigUint::zero();
            for i in 0..b.div_ceil(32) {
                x += BigUint::from(self.u()) << (32 * i as usize);
            }
            x % (BigUint::one() << (b as usize))
        }

        /// `below(k)`: `bits(bit length of k - 1)` until it is below `k`.
        fn below(&mut self, k: &BigUint) -> BigUint {
            let b = (k - BigUint::one()).bits();
            loop {
                let x = self.bits(b);
                if &x < k {
                    return x;
                }
            }
        }

        fn below_u64(&mut self, k: u64) -> u64 {
            self.below(&BigUint::from(k))
                .to_u64()
                .expect("a draw below a u64")
        }

        /// `sign()`: odd is negative.
        fn negative(&mut self) -> bool {
            self.u() % 2 == 1
        }
    }

    /// A generator of stdlib 8.1 (a `gen.range` with its bounds evaluated).
    #[derive(Clone, Debug)]
    pub enum G {
        Int,
        Pos,
        Bool,
        Text,
        List(Box<G>),
        Opt(Box<G>),
        Range(i128, i128),
    }

    pub fn list(g: G) -> G {
        G::List(Box::new(g))
    }

    pub fn opt(g: G) -> G {
        G::Opt(Box::new(g))
    }

    /// A drawn value.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum V {
        Int(BigInt),
        Bool(bool),
        Text(String),
        List(Vec<V>),
        None,
        Some(Box<V>),
    }

    impl V {
        pub fn int(&self) -> &BigInt {
            match self {
                V::Int(n) => n,
                other => panic!("{other:?} is not an Int"),
            }
        }

        pub fn items(&self) -> &[V] {
            match self {
                V::List(items) => items,
                other => panic!("{other:?} is not a list"),
            }
        }

        pub fn text(&self) -> &str {
            match self {
                V::Text(t) => t,
                other => panic!("{other:?} is not a text"),
            }
        }
    }

    /// One draw of `g` at size `s` (stdlib 8.3, draws in the order written).
    pub fn draw(st: &mut Stream, g: &G, s: u64) -> V {
        match g {
            G::Int => V::Int(draw_int(st, s)),
            G::Pos => V::Int(BigInt::one() + draw_int(st, s).abs()),
            G::Bool => V::Bool(st.u() % 2 == 1),
            G::Text => {
                let len = st.below_u64(s + 1);
                let mut text = String::new();
                for _ in 0..len {
                    let scalar = match st.u() % 4 {
                        0 | 1 => char::from(b'a' + st.below_u64(26) as u8),
                        2 => char::from(0x20 + st.below_u64(95) as u8),
                        _ => EXTRAS[st.below_u64(10) as usize],
                    };
                    text.push(scalar);
                }
                V::Text(text)
            }
            G::List(e) => {
                let len = st.below_u64(s + 1);
                V::List((0..len).map(|_| draw(st, e, s)).collect())
            }
            G::Opt(e) => {
                if st.u().is_multiple_of(4) {
                    V::None
                } else {
                    V::Some(Box::new(draw(st, e, s)))
                }
            }
            G::Range(lo, hi) => {
                let width: BigInt = BigInt::from(*hi) - BigInt::from(*lo) + 1;
                let k = width.to_biguint().expect("lo <= hi");
                V::Int(BigInt::from(*lo) + BigInt::from(st.below(&k)))
            }
        }
    }

    fn draw_int(st: &mut Stream, s: u64) -> BigInt {
        match st.u() % 8 {
            0 => BigInt::zero(),
            1 => {
                if st.negative() {
                    -BigInt::one()
                } else {
                    BigInt::one()
                }
            }
            2 if s >= 8 => {
                let length = 64 + st.below_u64(8 * s + 1);
                let top = BigUint::one() << ((length - 1) as usize);
                let magnitude = BigInt::from(top + st.bits(length - 1));
                if st.negative() {
                    -magnitude
                } else {
                    magnitude
                }
            }
            _ => BigInt::from(st.below_u64(2 * s + 1)) - BigInt::from(s),
        }
    }

    /// The tuples of the first `n` cases: case `i` at size `min(i, 100)`,
    /// the clauses drawing in clause order from the one stream (S44).
    pub fn cases(run_seed: u64, identity: &str, gens: &[G], n: u32) -> Vec<Vec<V>> {
        let mut st = Stream::new(run_seed, identity);
        (0..n)
            .map(|i| {
                let s = u64::from(i.min(100));
                gens.iter().map(|g| draw(&mut st, g, s)).collect()
            })
            .collect()
    }

    /// The shrink candidates of `v` (stdlib 8.4), in the table's order, each
    /// different from `v`. Duplicates are kept: the table lists rules, and
    /// nothing in 8.4 merges equal candidates.
    pub fn candidates(g: &G, v: &V) -> Vec<V> {
        let raw: Vec<V> = match (g, v) {
            (G::Int, V::Int(n)) => {
                let mut c = vec![BigInt::zero(), n / 2];
                if n.is_positive() {
                    c.push(n - 1);
                } else if n.is_negative() {
                    c.push(n + 1);
                }
                c.into_iter().map(V::Int).collect()
            }
            (G::Pos, V::Int(n)) => {
                let one = BigInt::one();
                [one.clone(), &one + (n - &one) / 2, n - &one]
                    .into_iter()
                    .filter(|c| *c >= one)
                    .map(V::Int)
                    .collect()
            }
            (G::Bool, V::Bool(b)) => {
                if *b {
                    vec![V::Bool(false)]
                } else {
                    Vec::new()
                }
            }
            (G::Text, V::Text(t)) => {
                let cs: Vec<char> = t.chars().collect();
                let n = cs.len();
                let text = |c: &[char]| V::Text(c.iter().collect());
                let mut c = vec![
                    V::Text(String::new()),
                    text(&cs[..n / 2]),
                    text(&cs[n / 2..]),
                ];
                if n > 0 {
                    c.push(text(&cs[..n - 1]));
                }
                if let Some(i) = cs.iter().position(|&ch| ch != 'a') {
                    let mut replaced = cs.clone();
                    replaced[i] = 'a';
                    c.push(text(&replaced));
                }
                c
            }
            (G::List(e), V::List(xs)) => {
                let n = xs.len();
                let mut c = vec![
                    V::List(Vec::new()),
                    V::List(xs[..n / 2].to_vec()),
                    V::List(xs[n / 2..].to_vec()),
                ];
                for i in 0..n {
                    let mut without = xs.clone();
                    without.remove(i);
                    c.push(V::List(without));
                }
                for (i, item) in xs.iter().enumerate() {
                    for x in candidates(e, item) {
                        let mut replaced = xs.clone();
                        replaced[i] = x;
                        c.push(V::List(replaced));
                    }
                }
                c
            }
            (G::Opt(e), V::Some(x)) => {
                let mut c = vec![V::None];
                c.extend(candidates(e, x).into_iter().map(|x| V::Some(Box::new(x))));
                c
            }
            (G::Opt(_), V::None) => Vec::new(),
            (G::Range(lo, hi), V::Int(n)) => {
                let (lo, hi) = (BigInt::from(*lo), BigInt::from(*hi));
                let o = BigInt::zero().max(lo.clone()).min(hi.clone());
                let mut c = vec![o.clone(), &o + (n - &o) / 2];
                if n > &o {
                    c.push(n - 1);
                } else if n < &o {
                    c.push(n + 1);
                }
                c.into_iter()
                    .filter(|x| *x >= lo && *x <= hi)
                    .map(V::Int)
                    .collect()
            }
            _ => panic!("{v:?} is not a value of {g:?}"),
        };
        raw.into_iter().filter(|c| c != v).collect()
    }

    /// The fault code a body raises for a tuple, or `None` when it passes.
    pub type Model<'a> = &'a dyn Fn(&[V]) -> Option<&'static str>;

    /// The outcome of shrinking one failure.
    #[derive(Debug)]
    pub struct Shrunk {
        pub values: Vec<V>,
        pub steps: u32,
        pub exhausted: bool,
    }

    /// S45 / stdlib 8.4: greedy and coordinate-wise; the first candidate
    /// failing with the original code is accepted and the walk continues
    /// with the same binder; after the last binder it restarts at the first
    /// until a full pass accepts nothing; every rerun is a step, and a rerun
    /// wanted after `max_shrinks` steps exhausts the budget.
    pub fn shrink(gens: &[G], start: &[V], body: Model<'_>, max_shrinks: u32) -> Shrunk {
        let code = body(start).expect("the original tuple fails");
        let mut cur = start.to_vec();
        let mut steps = 0;
        loop {
            let mut accepted_in_pass = false;
            for (j, g) in gens.iter().enumerate() {
                'same_binder: loop {
                    for candidate in candidates(g, &cur[j]) {
                        if steps == max_shrinks {
                            return Shrunk {
                                values: cur,
                                steps,
                                exhausted: true,
                            };
                        }
                        steps += 1;
                        let mut trial = cur.clone();
                        trial[j] = candidate;
                        if body(&trial) == Some(code) {
                            cur = trial;
                            accepted_in_pass = true;
                            continue 'same_binder;
                        }
                    }
                    break;
                }
            }
            if !accepted_in_pass {
                return Shrunk {
                    values: cur,
                    steps,
                    exhausted: false,
                };
            }
        }
    }

    /// What a failing property's record must say.
    #[derive(Debug)]
    pub struct Expected {
        pub index: u32,
        pub input: Vec<V>,
        pub shrunk: Shrunk,
    }

    /// The first failing case of `cases` and its shrinking, or `None` when
    /// every case passes.
    pub fn expect(
        run_seed: u64,
        identity: &str,
        gens: &[G],
        cases: u32,
        max_shrinks: u32,
        body: Model<'_>,
    ) -> Option<Expected> {
        let mut st = Stream::new(run_seed, identity);
        for i in 0..cases {
            let s = u64::from(i.min(100));
            let tuple: Vec<V> = gens.iter().map(|g| draw(&mut st, g, s)).collect();
            if body(&tuple).is_some() {
                let shrunk = shrink(gens, &tuple, body, max_shrinks);
                return Some(Expected {
                    index: i,
                    input: tuple,
                    shrunk,
                });
            }
        }
        None
    }

    /// Value JSON (semantics S40).
    pub fn to_json(v: &V) -> Json {
        match v {
            V::Int(n) => serde_json::from_str(&n.to_string()).expect("an integer is a JSON number"),
            V::Bool(b) => Json::Bool(*b),
            V::Text(t) => Json::String(t.clone()),
            V::List(items) => Json::Array(items.iter().map(to_json).collect()),
            V::None => json!({"$type": "Option", "$variant": "None"}),
            V::Some(x) => json!({"$type": "Option", "$variant": "Some", "$payload": [to_json(x)]}),
        }
    }

    /// A binder tuple as the `input` of `failing_case` and `shrunk`.
    pub fn tuple(names: &[&str], values: &[V]) -> Json {
        let mut m = Map::new();
        for (name, v) in names.iter().zip(values) {
            m.insert(name.to_string(), to_json(v));
        }
        Json::Object(m)
    }
}

// ===========================================================================
// Building programs
// ===========================================================================

/// Properties as CPL source, for the tests that run on the checker's
/// program (`replay_check`).
mod source {
    use super::{G, V};

    /// A drawn value as a CPL literal: its value text (semantics S40),
    /// which is literal syntax.
    pub fn literal(v: &V) -> String {
        match v {
            V::Int(n) => n.to_string(),
            V::Bool(b) => b.to_string(),
            V::Text(t) => cpl_diag::quote_text(t),
            V::List(items) => {
                let items: Vec<String> = items.iter().map(literal).collect();
                format!("[{}]", items.join(", "))
            }
            V::None => "None".to_string(),
            V::Some(x) => format!("Some({})", literal(x)),
        }
    }

    /// A generator as its `for` range (stdlib 8.1).
    pub fn generator(g: &G) -> String {
        match g {
            G::Int => "gen.int".to_string(),
            G::Pos => "gen.positive_int".to_string(),
            G::Bool => "gen.bool".to_string(),
            G::Text => "gen.text".to_string(),
            G::List(e) => format!("gen.list({})", generator(e)),
            G::Opt(e) => format!("gen.option({})", generator(e)),
            G::Range(lo, hi) => format!("gen.range({lo}, {hi})"),
        }
    }

    /// `module <module>` holding `property "<name>" for ... { assert not
    /// (<condition>) }`.
    pub fn property(module: &str, name: &str, binders: &[(&str, G)], condition: &str) -> String {
        let fors: String = binders
            .iter()
            .map(|(n, g)| format!("    for {n} in {}\n", generator(g)))
            .collect();
        format!(
            "module {module}\n\nproperty {}\n{fors}{{\n    assert not ({condition})\n}}\n",
            cpl_diag::quote_text(name)
        )
    }
}

/// The node factory of one module plus the std declarations the tests use.
#[derive(Clone)]
struct Cx {
    h: Hir,
    positive: DefId,
    pos: Ty,
    list_len: DefId,
    list_is_empty: DefId,
    list_take: DefId,
    text_len: DefId,
    text_is_empty: DefId,
    text_contains: DefId,
    unwrap_or: DefId,
    flatten: DefId,
    list_first: DefId,
    list_last: DefId,
}

fn module_cx(b: &mut ProgramBuilder, path: &str) -> (ModuleId, Cx) {
    let m = b.module(path);
    let positive = b.std_def("std.gen.Positive");
    let cx = Cx {
        h: b.hir(m),
        positive,
        pos: b.refined(positive),
        list_len: b.builtin("list_len"),
        list_is_empty: b.builtin("list_is_empty"),
        list_take: b.builtin("list_take"),
        text_len: b.builtin("text_len"),
        text_is_empty: b.builtin("text_is_empty"),
        text_contains: b.builtin("text_contains"),
        unwrap_or: b.builtin("option_unwrap_or"),
        flatten: b.builtin("flatten"),
        list_first: b.builtin("list_first"),
        list_last: b.builtin("list_last"),
    };
    (m, cx)
}

impl Cx {
    /// The binder type of a generator (stdlib 8.1, toolchain 3.5).
    fn ty(&self, g: &G) -> Ty {
        match g {
            G::Int | G::Range(..) => Ty::Int,
            G::Pos => self.pos.clone(),
            G::Bool => Ty::Bool,
            G::Text => Ty::Text,
            G::List(e) => Ty::list(self.ty(e)),
            G::Opt(e) => self.h.option_ty(self.ty(e)),
        }
    }

    fn gen(&self, g: &G) -> GenExpr {
        match g {
            G::Int => GenExpr::Int,
            G::Pos => GenExpr::PositiveInt,
            G::Bool => GenExpr::Bool,
            G::Text => GenExpr::Text,
            G::List(e) => GenExpr::List(Box::new(self.gen(e))),
            G::Opt(e) => GenExpr::Option(Box::new(self.gen(e))),
            G::Range(lo, hi) => GenExpr::Range(
                Box::new(self.int_literal(*lo)),
                Box::new(self.int_literal(*hi)),
            ),
        }
    }

    /// The literal `n` as the checker types it: a negative literal is the
    /// negation of the literal of its magnitude (grammar 3.12: an `INT` has
    /// no sign).
    fn int_literal(&self, n: i128) -> Expr {
        if n < 0 {
            self.h.unary(
                UnaryOp::Neg,
                self.h.int(BigInt::from(n).magnitude().clone()),
            )
        } else {
            self.h.int(n)
        }
    }

    fn int(&self, local: LocalId) -> Expr {
        self.h.local(local, Ty::Int)
    }

    /// A `Positive` binder used at `Int`.
    fn widened(&self, local: LocalId) -> Expr {
        self.h.widen(
            self.h.local(local, self.pos.clone()),
            self.positive,
            Ty::Int,
        )
    }
}

fn bin(h: &Hir, op: B, a: Expr, b: Expr) -> Expr {
    h.binary(op, a, b)
}

fn not(h: &Hir, e: Expr) -> Expr {
    h.unary(UnaryOp::Not, e)
}

fn assert_(e: Expr, text: &str) -> Stmt {
    Stmt::Assert {
        expr: e,
        text: text.to_string(),
    }
}

/// A block holding one `assert`.
fn asserting(h: &Hir, e: Expr, text: &str) -> Block {
    h.block(vec![assert_(e, text)], None)
}

fn add_prop(
    b: &mut ProgramBuilder,
    m: ModuleId,
    cx: &Cx,
    name: &str,
    binders: &[(&str, G)],
    body: impl FnOnce(&mut PropertyBuilder, &[LocalId]) -> Block,
) -> DefId {
    let mut pb = PropertyBuilder::new(name);
    let locals: Vec<LocalId> = binders
        .iter()
        .map(|(n, g)| pb.binder(n, cx.gen(g), cx.ty(g)))
        .collect();
    let block = body(&mut pb, &locals);
    pb.body(block);
    b.add_property(m, pb)
}

fn add_test(b: &mut ProgramBuilder, m: ModuleId, name: &str, body: Block) -> DefId {
    let mut t = TestBuilder::new(name);
    t.body(body);
    b.add_test(m, t)
}

/// A program holding one property (with an `@id` when `id` is given).
fn single_property(
    module: &str,
    name: &str,
    id: Option<&str>,
    binders: &[(&str, G)],
    body: impl FnOnce(&Cx, &mut PropertyBuilder, &[LocalId]) -> Block,
) -> (Program, DefId) {
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, module);
    let prop = add_prop(&mut b, m, &cx, name, binders, |pb, l| body(&cx, pb, l));
    if let Some(id) = id {
        b.set_stable_id(prop, id);
    }
    (b.build(), prop)
}

fn cfg(seed: u64) -> TestConfig {
    TestConfig::with_seed(seed)
}

/// The record as JSON with `duration_ms` cleared (wall time differs).
fn stable(record: &TestCaseRecord) -> Json {
    let mut r = record.clone();
    r.duration_ms = 0;
    serde_json::to_value(r).expect("a record serializes")
}

fn shrunk_input(record: &TestCaseRecord) -> Json {
    Json::Object(
        record
            .shrunk
            .as_ref()
            .expect("a shrunk tuple")
            .input
            .clone(),
    )
}

fn failing_input(record: &TestCaseRecord) -> Json {
    Json::Object(
        record
            .failing_case
            .as_ref()
            .expect("a failing case")
            .input
            .clone(),
    )
}

fn keys(j: &Json) -> Vec<String> {
    j.as_object().expect("an object").keys().cloned().collect()
}

fn names(p: &Program, defs: &[DefId]) -> Vec<String> {
    defs.iter().map(|&d| p.display_path(d)).collect()
}

/// One property of a built program and what the oracle needs to know about
/// it: its identity (computed independently) and its binders.
struct Run<'a> {
    p: &'a Program,
    prop: DefId,
    identity: &'a str,
    binders: &'a [(&'a str, G)],
}

impl Run<'_> {
    /// Run the property and compare every S46 field the oracle predicts:
    /// `seed`, `property_seed`, `cases`, `failing_case`, `shrunk` (input,
    /// `shrink_steps`, `shrink_exhausted`), and the code of `fault`.
    fn check(&self, seed: u64, cases: u32, max_shrinks: u32, model: Model<'_>) -> TestCaseRecord {
        assert_eq!(
            self.p.def(self.prop).stable_id.as_str(),
            self.identity,
            "the property's identity"
        );
        let gens: Vec<G> = self.binders.iter().map(|(_, g)| g.clone()).collect();
        let names: Vec<&str> = self.binders.iter().map(|(n, _)| *n).collect();
        let config = TestConfig {
            cases,
            max_shrinks,
            ..TestConfig::with_seed(seed)
        };
        let record = run_test(self.p, self.prop, &config);
        let ctx = format!("{} with seed {seed}", self.identity);
        assert_eq!(record.kind, TestKind::Property, "{ctx}");
        assert_eq!(record.seed, Some(seed), "{ctx}");
        assert_eq!(
            record.property_seed.as_deref(),
            Some(oracle::seed_hex(seed, self.identity).as_str()),
            "{ctx}: property_seed"
        );
        match oracle::expect(seed, self.identity, &gens, cases, max_shrinks, model) {
            None => {
                assert_eq!(record.status, TestStatus::Tested, "{ctx}: {record:?}");
                assert_eq!(record.cases, Some(cases), "{ctx}");
                assert!(
                    record.failing_case.is_none()
                        && record.shrunk.is_none()
                        && record.fault.is_none(),
                    "{ctx}: {record:?}"
                );
            }
            Some(expected) => {
                assert_eq!(record.status, TestStatus::Failed, "{ctx}: {record:?}");
                assert_eq!(record.cases, Some(expected.index + 1), "{ctx}: cases");
                let case = record.failing_case.as_ref().expect("failing_case");
                assert_eq!(case.index, expected.index, "{ctx}: failing_case.index");
                assert_eq!(
                    Json::Object(case.input.clone()),
                    oracle::tuple(&names, &expected.input),
                    "{ctx}: failing_case.input"
                );
                let shrunk = record.shrunk.as_ref().expect("shrunk");
                assert_eq!(
                    Json::Object(shrunk.input.clone()),
                    oracle::tuple(&names, &expected.shrunk.values),
                    "{ctx}: shrunk.input"
                );
                assert_eq!(
                    shrunk.shrink_steps, expected.shrunk.steps,
                    "{ctx}: shrink_steps"
                );
                assert_eq!(
                    shrunk.shrink_exhausted, expected.shrunk.exhausted,
                    "{ctx}: shrink_exhausted"
                );
                let code = model(&expected.shrunk.values).expect("the shrunk tuple fails");
                assert_eq!(
                    record.fault.as_ref().expect("fault").code,
                    code,
                    "{ctx}: fault"
                );
            }
        }
        record
    }
}

/// Build a property that fails exactly on the tuple the oracle predicts for
/// the first fresh case at or after `from` satisfying `pick`, then check
/// that the runner fails it at that case, with that input, and with the
/// predicted shrinking (every candidate passes, so the shrunk tuple is the
/// original and `shrink_steps` is the number of candidates); and that
/// `--cases` one larger reproduces the record while one fewer never reaches
/// the case (stdlib 8.2).
fn replay_check(
    module: &str,
    name: &str,
    binders: &[(&str, G)],
    seed: u64,
    total: u32,
    from: usize,
    pick: &dyn Fn(&[V]) -> bool,
) {
    let identity = oracle::qualified(module, name);
    let gens: Vec<G> = binders.iter().map(|(_, g)| g.clone()).collect();
    let all = oracle::cases(seed, &identity, &gens, total);
    let index = (from..all.len())
        .find(|&i| pick(&all[i]) && !all[..i].contains(&all[i]))
        .unwrap_or_else(|| panic!("{identity}: no fresh tuple at or after case {from}"));
    let target = all[index].clone();
    // The property is CPL source checked by cpl-sema: the comparisons with
    // the predicted tuple are typed as the checker types them (each literal
    // against the erased type of its binder, semantics 3.6, so a `Positive`
    // binder's literal is a plain `Int`; a negative literal is a negation).
    let conjunction: Vec<String> = binders
        .iter()
        .zip(&target)
        .map(|((n, _), v)| format!("{n} == {}", source::literal(v)))
        .collect();
    let text = source::property(module, name, binders, &conjunction.join(" and "));
    let p = common::check_source(&text);
    let prop = common::def(&p, &identity);
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders,
    };
    let model = |t: &[V]| (t == target.as_slice()).then_some("assertion_failed");
    let full = run.check(seed, total, TestConfig::DEFAULT_MAX_SHRINKS, &model);
    assert_eq!(
        full.failing_case.as_ref().map(|c| c.index as usize),
        Some(index),
        "{identity}: the predicted case"
    );
    assert_eq!(full.symbol.as_str(), identity);
    assert_eq!(full.qualified_name, identity);
    let with_cases = |cases: usize| TestConfig {
        cases: cases as u32,
        ..TestConfig::with_seed(seed)
    };
    let exact = run_test(&p, prop, &with_cases(index + 1));
    assert_eq!(
        stable(&exact),
        stable(&full),
        "{identity}: --cases {}",
        index + 1
    );
    let short = run_test(&p, prop, &with_cases(index));
    assert_eq!(
        (short.status, short.cases),
        (TestStatus::Tested, Some(index as u32)),
        "{identity}: --cases {index}"
    );
}

// ===========================================================================
// Selection and order (semantics 7.2-7.3, S42; toolchain 1.7.2, 7.2, T43)
// ===========================================================================

/// Tests and one property whose names sort differently as strings and as
/// name texts.
fn ordering_fixture() -> Program {
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "ord");
    let (sub, cx_sub) = module_cx(&mut b, "ord.sub");
    for name in ["b", "a b", "a"] {
        add_test(&mut b, m, name, cx.h.block(vec![], None));
    }
    add_prop(&mut b, m, &cx, "a!", &[("flag", G::Bool)], |_, _| {
        cx.h.block(vec![], None)
    });
    add_test(&mut b, m, "a\"q", cx.h.block(vec![], None));
    add_test(&mut b, sub, "0", cx_sub.h.block(vec![], None));
    b.build()
}

/// The qualified names of [`ordering_fixture`] in byte order.
const ORDER: [&str; 6] = [
    "ord.\"a b\"",
    "ord.\"a!\"",
    "ord.\"a\"",
    "ord.\"a\\\"q\"",
    "ord.\"b\"",
    "ord.sub.\"0\"",
];

#[test]
fn tests_and_properties_run_in_the_string_order_of_their_qualified_names() {
    // semantics 7.3 ("in the order of their qualified names") and toolchain
    // 1.7.2 ("sorted by qualified name"). A qualified name is the string
    // `<module path>."<name>"` with the name canonically escaped (semantics
    // 1.4, toolchain 4.3), so this test reads the order as the byte order
    // of those strings: `ord."a b"` precedes `ord."a"` because ' ' < '"',
    // and tests and properties are interleaved by that one order.
    let mut sorted = ORDER;
    sorted.sort_unstable();
    assert_eq!(sorted, ORDER, "the expectation is itself in byte order");
    let p = ordering_fixture();
    assert_eq!(names(&p, &select_tests(&p, &TestSelector::all())), ORDER);
    let report = run_tests(&p, &TestSelector::all(), &cfg(1));
    let ran: Vec<&str> = report
        .results
        .iter()
        .map(|r| r.qualified_name.as_str())
        .collect();
    assert_eq!(ran, ORDER);
    assert_eq!((report.selected, report.tested, report.failed), (6, 6, 0));
    let kinds: Vec<TestKind> = report.results.iter().map(|r| r.kind).collect();
    assert_eq!(kinds[1], TestKind::Property);
    assert!(kinds
        .iter()
        .enumerate()
        .all(|(i, k)| i == 1 || *k == TestKind::Test));
}

#[test]
fn the_order_is_by_qualified_name_even_when_an_id_sorts_otherwise() {
    // semantics 7.3: "in the order of their qualified names", not of their
    // identities: `zzz_last` names `a."t1"`, which still runs first.
    let mut b = ProgramBuilder::new();
    let (bm, cb) = module_cx(&mut b, "b");
    let (am, ca) = module_cx(&mut b, "a");
    let second = add_test(&mut b, bm, "t2", cb.h.block(vec![], None));
    let first = add_test(&mut b, am, "t1", ca.h.block(vec![], None));
    b.set_stable_id(first, "zzz_last");
    let p = b.build();
    assert_eq!(select_tests(&p, &TestSelector::all()), [first, second]);
    let report = run_tests(&p, &TestSelector::all(), &cfg(1));
    let symbols: Vec<&str> = report.results.iter().map(|r| r.symbol.as_str()).collect();
    assert_eq!(symbols, ["zzz_last", "b.\"t2\""]);
}

#[test]
fn the_runner_defaults_are_the_specified_ones() {
    // S44: cases = 100; S45: max_shrinks = 1000; toolchain 1.7: fail_fast
    // false; 7.2: timeout_ms 0, no budget; S38: max_call_depth 10000.
    let config = TestConfig::with_seed(5);
    assert_eq!(
        (
            config.seed,
            config.cases,
            config.max_shrinks,
            config.fail_fast
        ),
        (5, 100, 1000, false)
    );
    assert_eq!(config.eval.time_budget, None);
    assert_eq!(config.eval.max_call_depth, 10_000);
    assert!(config.eval.check_contracts);
}

#[test]
fn the_name_filter_is_the_exact_name_text_of_a_test_or_property() {
    // toolchain 7.2 `--name TEXT`: "only the test or property with exactly
    // that name text"; the text is the literal's value, not its escaped
    // spelling and not a qualified name.
    let p = ordering_fixture();
    let by_name = |n: &str| {
        let sel = TestSelector {
            name: Some(n.to_string()),
            ..TestSelector::all()
        };
        names(&p, &select_tests(&p, &sel))
    };
    assert_eq!(by_name("a"), ["ord.\"a\""]);
    assert_eq!(by_name("a!"), ["ord.\"a!\""]);
    assert_eq!(by_name("a\"q"), ["ord.\"a\\\"q\""]);
    assert_eq!(by_name("0"), ["ord.sub.\"0\""]);
    for absent in ["a\\\"q", "A", "a ", " a", "ord.\"a\"", "\"a\"", ""] {
        assert!(by_name(absent).is_empty(), "{absent:?}");
    }
}

#[test]
fn namespace_name_and_id_filters_must_all_hold() {
    let mut b = ProgramBuilder::new();
    let (x, cx) = module_cx(&mut b, "sel.x");
    let (y, cy) = module_cx(&mut b, "sel.y");
    let prop = add_prop(&mut b, x, &cx, "annotated", &[("flag", G::Bool)], |_, _| {
        cx.h.block(vec![], None)
    });
    b.set_stable_id(prop, "prop_sel");
    let plain = add_test(&mut b, y, "plain", cy.h.block(vec![], None));
    let p = b.build();
    let sel = |ns: Option<&str>, name: Option<&str>, id: Option<&str>| TestSelector {
        namespace: ns.map(|n| n.split('.').map(str::to_string).collect()),
        name: name.map(str::to_string),
        id: id.map(|i| StableId::parse(i).expect("an identity")),
    };
    assert_eq!(select_tests(&p, &sel(None, None, Some("prop_sel"))), [prop]);
    assert_eq!(
        select_tests(&p, &sel(Some("sel"), Some("annotated"), Some("prop_sel"))),
        [prop]
    );
    assert!(select_tests(&p, &sel(Some("sel.y"), None, Some("prop_sel"))).is_empty());
    assert!(select_tests(&p, &sel(None, Some("plain"), Some("prop_sel"))).is_empty());
    // An `@id` is the identity (semantics S2, S42): the qualified name of an
    // annotated property is not its stable id, an unannotated test's is.
    assert!(select_tests(&p, &sel(None, None, Some("sel.x.\"annotated\""))).is_empty());
    assert_eq!(
        select_tests(&p, &sel(None, None, Some("sel.y.\"plain\""))),
        [plain]
    );
}

#[test]
fn an_empty_selection_runs_nothing_but_reports_the_seed_and_cases() {
    let p = ordering_fixture();
    for selector in ["nowhere", "ordx", "ord.subx", "ord.sub.deeper"] {
        let report = run_tests(
            &p,
            &TestSelector::parse(selector).expect("a module path"),
            &TestConfig {
                cases: 13,
                ..cfg(77)
            },
        );
        assert_eq!(
            (
                report.seed,
                report.cases,
                report.selected,
                report.tested,
                report.failed
            ),
            (77, 13, 0, 0, 0),
            "{selector}"
        );
        assert!(report.results.is_empty(), "{selector}");
    }
    // T43: a namespace selects its module and every module below it, and a
    // path with no module of its own still selects the modules below it.
    assert_eq!(
        select_tests(&p, &TestSelector::parse("ord.sub.*").expect("a wildcard")).len(),
        1
    );
    assert_eq!(
        select_tests(&p, &TestSelector::parse("ord").expect("a path")).len(),
        6
    );
}

#[test]
fn selector_segments_are_lower_identifiers_and_never_keywords() {
    // toolchain 7.2: a module path or a namespace wildcard; module path
    // segments are `LOWER_IDENT` (grammar 3.1, 1.3): `_x` is one, `_` is the
    // wildcard token; keywords and reserved words are never identifiers
    // (grammar 1.4); `self`, `result`, contextual `.work` words, and facet
    // names are ordinary `LOWER_IDENT`s in a module path.
    for ok in [
        "a",
        "tests.result",
        "tests.write",
        "tests.body",
        "tests._x2",
        "x.y.z.*",
    ] {
        assert!(TestSelector::parse(ok).is_ok(), "{ok} was rejected");
    }
    for bad in [
        "_",
        "tests._",
        "tests.test",
        "property.x",
        "tests.for",
        "tests.effects",
        "tests.loop",
        "tests.true",
        "tests.Pages",
        "tests.*.*",
        "tests.1x",
        "tests.x-y",
        "tests.\u{e9}",
        "tests.*x",
    ] {
        assert!(TestSelector::parse(bad).is_err(), "{bad} was accepted");
    }
}

// ===========================================================================
// Seeds and identities (S44; stdlib 8.2, S31; toolchain 1.7, T45)
// ===========================================================================

#[test]
fn the_stream_is_keyed_by_the_canonically_escaped_qualified_name() {
    // stdlib 8.2: the identity is the qualified name "exactly as the test
    // result reports it", "a name containing `"` or `\` is hashed in its
    // canonically escaped spelling".
    let name = "say \"hi\" \\ now\n\u{1}";
    let identity = "adv.seed.\"say \\\"hi\\\" \\\\ now\\n\\u{1}\"";
    assert_eq!(oracle::qualified("adv.seed", name), identity);
    for seed in [0, 9, u64::MAX] {
        assert_eq!(
            property_seed(seed, identity),
            oracle::seed_bytes(seed, identity)
        );
    }
    replay_check(
        "adv.seed",
        name,
        &[("x", G::Int), ("t", G::Text)],
        9,
        100,
        10,
        &|_| true,
    );
}

#[test]
fn the_stream_is_keyed_by_the_utf8_bytes_of_a_unicode_name() {
    replay_check(
        "adv.seed",
        "gr\u{f6}\u{df}e \u{4e2d} \u{1f600}",
        &[("n", G::Pos)],
        3,
        100,
        5,
        &|_| true,
    );
}

/// `assert x < 10` on the first binder.
fn x_below_ten_block(cx: &Cx, l: &[LocalId]) -> Block {
    let h = &cx.h;
    asserting(h, bin(h, B::Lt, cx.int(l[0]), h.int(10)), "x < 10")
}

/// [`x_below_ten_block`] as a property body.
fn x_below_ten(cx: &Cx, _: &mut PropertyBuilder, l: &[LocalId]) -> Block {
    x_below_ten_block(cx, l)
}

fn at_least_ten(t: &[V]) -> Option<&'static str> {
    (*t[0].int() >= BigInt::from(10)).then_some("assertion_failed")
}

#[test]
fn an_id_keys_the_stream_across_renames_and_moves() {
    // S44 / stdlib S31: the stream is keyed by the identity; an `@id`
    // survives renames and moves, a qualified name does not.
    let binders = [("x", G::Int), ("t", G::Text)];
    let seed = 21;
    let (p1, d1) = single_property("adv.a", "first", Some("prop_keep"), &binders, x_below_ten);
    let (p2, d2) = single_property(
        "adv.b.c",
        "second",
        Some("prop_keep"),
        &binders,
        x_below_ten,
    );
    let check = |p: &Program, d: DefId, identity: &str| {
        Run {
            p,
            prop: d,
            identity,
            binders: &binders,
        }
        .check(seed, 100, 1000, &at_least_ten)
    };
    let r1 = check(&p1, d1, "prop_keep");
    let r2 = check(&p2, d2, "prop_keep");
    assert_eq!(r1.status, TestStatus::Failed);
    assert_eq!(r1.property_seed, r2.property_seed);
    assert_eq!(r1.failing_case, r2.failing_case);
    assert_eq!(r1.shrunk, r2.shrunk);
    assert_eq!(r1.qualified_name, "adv.a.\"first\"");
    assert_eq!(r2.qualified_name, "adv.b.c.\"second\"");
    let (p3, d3) = single_property("adv.a", "first", None, &binders, x_below_ten);
    let (p4, d4) = single_property("adv.a", "renamed", None, &binders, x_below_ten);
    let r3 = check(&p3, d3, "adv.a.\"first\"");
    let r4 = check(&p4, d4, "adv.a.\"renamed\"");
    assert_ne!(r3.property_seed, r4.property_seed);
    assert_ne!(r1.property_seed, r3.property_seed);
}

#[test]
fn other_tests_and_properties_never_change_a_propertys_cases() {
    // S44: "adding, renaming (under an @id), or reordering tests never
    // changes another property's cases": one stream per property.
    let binders = [("xs", list(G::Int)), ("o", opt(G::Text))];
    let body = |cx: &Cx, _: &mut PropertyBuilder, l: &[LocalId]| {
        let h = &cx.h;
        let len = h.call_generic(
            cx.list_len,
            vec![Ty::Int],
            vec![h.local(l[0], Ty::list(Ty::Int))],
            Ty::Int,
        );
        asserting(h, bin(h, B::Lt, len, h.int(4)), "xs.len < 4")
    };
    let model = |t: &[V]| (t[0].items().len() >= 4).then_some("assertion_failed");
    let seed = 31;
    let (alone, prop) = single_property("adv.crowd", "p", None, &binders, body);
    let identity = oracle::qualified("adv.crowd", "p");
    let expected = Run {
        p: &alone,
        prop,
        identity: &identity,
        binders: &binders,
    }
    .check(seed, 100, 1000, &model);
    assert_eq!(expected.status, TestStatus::Failed);
    let mut b = ProgramBuilder::new();
    let (first, cf) = module_cx(&mut b, "adv");
    add_prop(&mut b, first, &cf, "before", &[("x", G::Int)], |_, l| {
        x_below_ten_block(&cf, l)
    });
    let (m, cx) = module_cx(&mut b, "adv.crowd");
    add_test(&mut b, m, "a", asserting(&cx.h, cx.h.bool(false), "false"));
    add_prop(
        &mut b,
        m,
        &cx,
        "o",
        &[("t", G::Text), ("n", G::Pos)],
        |_, _| asserting(&cx.h, cx.h.bool(false), "false"),
    );
    add_prop(&mut b, m, &cx, "p", &binders, |pb, l| body(&cx, pb, l));
    add_prop(&mut b, m, &cx, "q", &[("x", G::Int)], |_, l| {
        x_below_ten_block(&cx, l)
    });
    let crowded = b.build();
    for selector in [
        TestSelector::all(),
        TestSelector::parse("adv.crowd").unwrap(),
    ] {
        let report = run_tests(&crowded, &selector, &cfg(seed));
        let record = report
            .results
            .iter()
            .find(|r| r.qualified_name == identity)
            .expect("the property ran");
        assert_eq!(stable(record), stable(&expected));
    }
}

// ===========================================================================
// Draws (S44; stdlib 8.2-8.3): failing cases predicted by the oracle
// ===========================================================================

#[test]
fn int_draws_replay_the_stream_beyond_the_size_cap() {
    // Case i runs at size min(i, 100): a large value at a case beyond 100
    // has a bit length 64 + below(801), not 64 + below(8i + 1).
    for seed in [0, 1, u64::MAX] {
        replay_check(
            "adv.draw",
            "large ints",
            &[("x", G::Int)],
            seed,
            240,
            101,
            &|t| t[0].int().bits() > 64,
        );
    }
}

#[test]
fn uniform_draws_beyond_case_100_use_size_100() {
    // A large value hardly depends on the cap (below(793) and below(801)
    // draw the same ten bits); a uniform value below(2s + 1) - s and a list
    // length below(s + 1) do, so a target past case 100 made of those pins
    // the size at exactly 100.
    for seed in [3, 4] {
        replay_check(
            "adv.draw",
            "capped sizes",
            &[("x", G::Int), ("ys", list(G::Int))],
            seed,
            160,
            110,
            &|t| t[0].int().bits() <= 64,
        );
    }
}

#[test]
fn every_generator_draws_from_one_stream_in_clause_order() {
    let binders = [
        ("x", G::Int),
        ("flag", G::Bool),
        ("t", G::Text),
        ("os", list(opt(G::Pos))),
        ("r", G::Range(-7, 7)),
    ];
    for seed in [5, 6] {
        replay_check(
            "adv.draw",
            "all generators",
            &binders,
            seed,
            100,
            40,
            &|_| true,
        );
    }
}

#[test]
fn small_sizes_replay_exactly() {
    for seed in [11, 12, 13] {
        replay_check(
            "adv.draw",
            "small pairs",
            &[("a", G::Int), ("b", G::Int)],
            seed,
            30,
            1,
            &|_| true,
        );
    }
}

#[test]
fn the_large_bucket_starts_at_size_eight() {
    // stdlib 8.3: bucket 2 is a large value only when s >= 8; below that it
    // is uniform in [-s, s]. A target after case 8 is reached only when the
    // words consumed at sizes 7 and 8 are the specified ones.
    let binders = [("a", G::Int), ("b", G::Int), ("c", G::Int), ("d", G::Int)];
    for seed in 1..=6 {
        replay_check("adv.draw", "size eight", &binders, seed, 40, 9, &|_| true);
    }
}

#[test]
fn a_one_value_range_draws_nothing() {
    // below(1) is 0 and draws nothing (stdlib 8.3), so `gen.range(5, 5)`
    // leaves the stream untouched for the clauses after it.
    let binders = [
        ("r", G::Range(5, 5)),
        ("x", G::Int),
        ("s", G::Range(-3, -3)),
        ("y", G::Int),
    ];
    for seed in [2, 3] {
        replay_check("adv.draw", "fixed ranges", &binders, seed, 100, 20, &|_| {
            true
        });
    }
}

#[test]
fn wide_ranges_draw_whole_words_without_modulo() {
    // `gen.range(0, 5_000_000_000)` draws 33 bits as two words and retries
    // above the bound; a width of exactly 2^32 is one word, never retried.
    let binders = [
        ("r", G::Range(0, 5_000_000_000)),
        ("s", G::Range(-3, 4_294_967_292)),
    ];
    for seed in [4, 8] {
        replay_check("adv.draw", "wide ranges", &binders, seed, 40, 3, &|_| true);
    }
}

#[test]
fn ranges_wider_than_64_bits_draw_masked_words() {
    // stdlib 8.3: below(k) draws bits(bit length of k - 1): for a width of
    // 2^64 that is 64 bits in two words, for 2^64 + 1 it is 65 bits in three
    // words with the top word masked to one bit, for 2^100 + 1 it is 101
    // bits in four words; nothing is reduced modulo anything.
    let two64: i128 = 1 << 64;
    let far: i128 = -(1 << 90);
    let binders = [
        ("a", G::Range(0, two64 - 1)),
        ("b", G::Range(-5, two64 - 5)),
        ("c", G::Range(-(1 << 100), 0)),
        ("d", G::Range(far, far + 6)),
    ];
    for seed in [1, 2] {
        replay_check("adv.draw", "beyond 64 bits", &binders, seed, 30, 2, &|_| {
            true
        });
    }
}

#[test]
fn nested_lists_and_optional_texts_replay() {
    let binders = [("xss", list(list(G::Bool))), ("o", opt(G::Text))];
    replay_check("adv.draw", "nested", &binders, 7, 100, 15, &|_| true);
}

#[test]
fn positive_draws_are_one_plus_an_int_magnitude() {
    let binders = [("n", G::Pos), ("ns", list(G::Pos))];
    replay_check("adv.draw", "positives", &binders, 1, 100, 30, &|_| true);
}

#[test]
fn case_zero_runs_at_size_zero() {
    // S44: size = min(i, 100), so case 0 has size 0 and a list or text has
    // length below(1) = 0: the empty value, which has no shrink candidates.
    let (p, prop) = single_property(
        "adv.size",
        "lists are never empty",
        None,
        &[("xs", list(G::Int))],
        |cx, _, l| {
            let h = &cx.h;
            let empty = h.call_generic(
                cx.list_is_empty,
                vec![Ty::Int],
                vec![h.local(l[0], Ty::list(Ty::Int))],
                Ty::Bool,
            );
            asserting(h, not(h, empty), "not xs.is_empty()")
        },
    );
    let (q, text_prop) = single_property(
        "adv.size",
        "texts are never empty",
        None,
        &[("t", G::Text)],
        |cx, _, l| {
            let h = &cx.h;
            let empty = h.call(cx.text_is_empty, vec![h.local(l[0], Ty::Text)], Ty::Bool);
            asserting(h, not(h, empty), "not t.is_empty()")
        },
    );
    for seed in [0, 5, u64::MAX] {
        for (record, input) in [
            (run_test(&p, prop, &cfg(seed)), json!({"xs": []})),
            (run_test(&q, text_prop, &cfg(seed)), json!({"t": ""})),
        ] {
            assert_eq!(record.status, TestStatus::Failed);
            assert_eq!(record.cases, Some(1));
            assert_eq!(record.failing_case.as_ref().map(|c| c.index), Some(0));
            assert_eq!(failing_input(&record), input);
            assert_eq!(shrunk_input(&record), input);
            let shrunk = record.shrunk.as_ref().unwrap();
            assert_eq!((shrunk.shrink_steps, shrunk.shrink_exhausted), (0, false));
        }
    }
}

// ===========================================================================
// Shrinking (S45; stdlib 8.4, S32): exact minima and step counts
// ===========================================================================

#[test]
fn ints_shrink_toward_zero_from_above() {
    let binders = [("x", G::Int)];
    let (p, prop) = single_property("adv.shrink", "x below ten", None, &binders, x_below_ten);
    let identity = oracle::qualified("adv.shrink", "x below ten");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let mut minimal = 0;
    for seed in 1..=8 {
        let r = run.check(seed, 100, 1000, &at_least_ten);
        if r.status == TestStatus::Failed && !r.shrunk.as_ref().unwrap().shrink_exhausted {
            assert_eq!(shrunk_input(&r), json!({"x": 10}), "seed {seed}");
            minimal += 1;
        }
    }
    assert!(minimal >= 4, "{minimal}");
}

#[test]
fn ints_shrink_toward_zero_from_below() {
    let binders = [("x", G::Int)];
    let (p, prop) = single_property(
        "adv.shrink",
        "x above minus ten",
        None,
        &binders,
        |cx, _, l| {
            let h = &cx.h;
            asserting(
                h,
                bin(h, B::Gt, cx.int(l[0]), h.unary(UnaryOp::Neg, h.int(10))),
                "x > -10",
            )
        },
    );
    let identity = oracle::qualified("adv.shrink", "x above minus ten");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (*t[0].int() <= BigInt::from(-10)).then_some("assertion_failed");
    let mut minimal = 0;
    for seed in 1..=8 {
        let r = run.check(seed, 100, 1000, &model);
        if r.status == TestStatus::Failed && !r.shrunk.as_ref().unwrap().shrink_exhausted {
            assert_eq!(shrunk_input(&r), json!({"x": -10}), "seed {seed}");
            minimal += 1;
        }
    }
    assert!(minimal >= 4, "{minimal}");
}

#[test]
fn positives_shrink_toward_one_and_never_below_it() {
    // stdlib 8.4: Positive candidates are `>= 1` and `1` has none; toolchain
    // 1.7.2: every candidate also satisfies the binder type's predicates.
    let binders = [("n", G::Pos)];
    let (p, prop) = single_property("adv.shrink", "n below five", None, &binders, |cx, _, l| {
        let h = &cx.h;
        asserting(h, bin(h, B::Lt, cx.widened(l[0]), h.int(5)), "n < 5")
    });
    let identity = oracle::qualified("adv.shrink", "n below five");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (*t[0].int() >= BigInt::from(5)).then_some("assertion_failed");
    for seed in 1..=4 {
        let r = run.check(seed, 100, 1000, &model);
        if !r.shrunk.as_ref().unwrap().shrink_exhausted {
            assert_eq!(shrunk_input(&r), json!({"n": 5}), "seed {seed}");
        }
    }
    // A property that always fails: the minimum is 1, not 0.
    let (q, always) = single_property("adv.shrink", "always", None, &binders, |cx, _, _| {
        asserting(&cx.h, cx.h.bool(false), "false")
    });
    let identity = oracle::qualified("adv.shrink", "always");
    let run = Run {
        p: &q,
        prop: always,
        identity: &identity,
        binders: &binders,
    };
    for seed in 1..=4 {
        let r = run.check(seed, 100, 1000, &|_| Some("assertion_failed"));
        assert_eq!(shrunk_input(&r), json!({"n": 1}), "seed {seed}");
    }
    // Inside a list, Positive elements shrink to 1 as well.
    let binders = [("ns", list(G::Pos))];
    let (r_p, two) = single_property(
        "adv.shrink",
        "fewer than two",
        None,
        &binders,
        |cx, _, l| {
            let h = &cx.h;
            let len = h.call_generic(
                cx.list_len,
                vec![cx.pos.clone()],
                vec![h.local(l[0], Ty::list(cx.pos.clone()))],
                Ty::Int,
            );
            asserting(h, bin(h, B::Lt, len, h.int(2)), "ns.len < 2")
        },
    );
    let identity = oracle::qualified("adv.shrink", "fewer than two");
    let run = Run {
        p: &r_p,
        prop: two,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (t[0].items().len() >= 2).then_some("assertion_failed");
    for seed in 1..=4 {
        let r = run.check(seed, 100, 1000, &model);
        assert_eq!(shrunk_input(&r), json!({"ns": [1, 1]}), "seed {seed}");
    }
}

#[test]
fn lists_shrink_by_halves_then_removals_then_elements() {
    let binders = [("xs", list(G::Int))];
    let (p, prop) = single_property(
        "adv.shrink",
        "all below five",
        None,
        &binders,
        |cx, pb, l| {
            let h = &cx.h;
            let x = pb.local("x", Ty::Int, BindingKind::Forall, false);
            let all = h.forall(
                x,
                h.local(l[0], Ty::list(Ty::Int)),
                bin(h, B::Lt, cx.int(x), h.int(5)),
            );
            asserting(h, all, "forall x in xs: x < 5")
        },
    );
    let identity = oracle::qualified("adv.shrink", "all below five");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let five = BigInt::from(5);
    let model = |t: &[V]| {
        t[0].items()
            .iter()
            .any(|x| *x.int() >= five)
            .then_some("assertion_failed")
    };
    for seed in 1..=6 {
        let r = run.check(seed, 100, 1000, &model);
        if !r.shrunk.as_ref().unwrap().shrink_exhausted {
            assert_eq!(shrunk_input(&r), json!({"xs": [5]}), "seed {seed}");
        }
    }
    // Length-only failures shrink to the shortest list of zeros.
    let (q, short) = single_property(
        "adv.shrink",
        "shorter than three",
        None,
        &binders,
        |cx, _, l| {
            let h = &cx.h;
            let len = h.call_generic(
                cx.list_len,
                vec![Ty::Int],
                vec![h.local(l[0], Ty::list(Ty::Int))],
                Ty::Int,
            );
            asserting(h, bin(h, B::Lt, len, h.int(3)), "xs.len < 3")
        },
    );
    let identity = oracle::qualified("adv.shrink", "shorter than three");
    let run = Run {
        p: &q,
        prop: short,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (t[0].items().len() >= 3).then_some("assertion_failed");
    for seed in 1..=6 {
        let r = run.check(seed, 100, 1000, &model);
        if !r.shrunk.as_ref().unwrap().shrink_exhausted {
            assert_eq!(shrunk_input(&r), json!({"xs": [0, 0, 0]}), "seed {seed}");
        }
    }
}

#[test]
fn texts_shrink_by_halves_tails_and_the_letter_a() {
    let binders = [("t", G::Text)];
    let (p, prop) = single_property("adv.shrink", "no z", None, &binders, |cx, _, l| {
        let h = &cx.h;
        let has = h.call(
            cx.text_contains,
            vec![h.local(l[0], Ty::Text), h.text("z")],
            Ty::Bool,
        );
        asserting(h, not(h, has), "not std.text.contains(t, \"z\")")
    });
    let identity = oracle::qualified("adv.shrink", "no z");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| t[0].text().contains('z').then_some("assertion_failed");
    let mut failed = 0;
    for seed in 1..=8 {
        let r = run.check(seed, 100, 1000, &model);
        if r.status == TestStatus::Failed {
            failed += 1;
            assert_eq!(shrunk_input(&r), json!({"t": "z"}), "seed {seed}");
        }
    }
    assert!(failed >= 4, "{failed}");
    let (q, two) = single_property(
        "adv.shrink",
        "at most one scalar",
        None,
        &binders,
        |cx, _, l| {
            let h = &cx.h;
            let len = h.call(cx.text_len, vec![h.local(l[0], Ty::Text)], Ty::Int);
            asserting(h, bin(h, B::Lt, len, h.int(2)), "t.len < 2")
        },
    );
    let identity = oracle::qualified("adv.shrink", "at most one scalar");
    let run = Run {
        p: &q,
        prop: two,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (t[0].text().chars().count() >= 2).then_some("assertion_failed");
    for seed in 1..=6 {
        let r = run.check(seed, 100, 1000, &model);
        assert_eq!(shrunk_input(&r), json!({"t": "aa"}), "seed {seed}");
    }
}

#[test]
fn options_shrink_to_none_first_then_their_payload() {
    let binders = [("o", opt(G::Int))];
    let (p, prop) = single_property("adv.shrink", "small payload", None, &binders, |cx, _, l| {
        let h = &cx.h;
        let payload = h.call_generic(
            cx.unwrap_or,
            vec![Ty::Int],
            vec![h.local(l[0], h.option_ty(Ty::Int)), h.int(0)],
            Ty::Int,
        );
        asserting(h, bin(h, B::Lt, payload, h.int(3)), "o.unwrap_or(0) < 3")
    });
    let identity = oracle::qualified("adv.shrink", "small payload");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let three = BigInt::from(3);
    let model = |t: &[V]| match &t[0] {
        V::Some(x) if *x.int() >= three => Some("assertion_failed"),
        _ => None,
    };
    for seed in 1..=6 {
        let r = run.check(seed, 100, 1000, &model);
        if !r.shrunk.as_ref().unwrap().shrink_exhausted {
            assert_eq!(
                shrunk_input(&r),
                json!({"o": {"$type": "Option", "$variant": "Some", "$payload": [3]}}),
                "seed {seed}"
            );
        }
    }
}

#[test]
fn ranges_shrink_toward_their_origin() {
    // stdlib 8.4: the origin is `0.max(lo).min(hi)`, and every candidate
    // lies in [lo, hi].
    struct Case {
        name: &'static str,
        g: G,
        op: B,
        bound: i64,
        minimum: i64,
    }
    let cases = [
        Case {
            name: "positive range",
            g: G::Range(5, 1000),
            op: B::Lt,
            bound: 100,
            minimum: 100,
        },
        Case {
            name: "negative range",
            g: G::Range(-1000, -5),
            op: B::Gt,
            bound: -100,
            minimum: -100,
        },
        Case {
            name: "straddling above",
            g: G::Range(-50, 50),
            op: B::Lt,
            bound: 7,
            minimum: 7,
        },
        Case {
            name: "straddling below",
            g: G::Range(-50, 50),
            op: B::Gt,
            bound: -7,
            minimum: -7,
        },
    ];
    for case in cases {
        let binders = [("r", case.g.clone())];
        let (p, prop) = single_property("adv.range", case.name, None, &binders, |cx, _, l| {
            let h = &cx.h;
            asserting(
                h,
                bin(h, case.op, cx.int(l[0]), h.int(case.bound)),
                "r against the bound",
            )
        });
        let identity = oracle::qualified("adv.range", case.name);
        let run = Run {
            p: &p,
            prop,
            identity: &identity,
            binders: &binders,
        };
        let bound = BigInt::from(case.bound);
        let model = |t: &[V]| {
            let r = t[0].int();
            let holds = if case.op == B::Lt {
                *r < bound
            } else {
                *r > bound
            };
            (!holds).then_some("assertion_failed")
        };
        for seed in 1..=4 {
            let r = run.check(seed, 100, 1000, &model);
            assert_eq!(r.status, TestStatus::Failed, "{} seed {seed}", case.name);
            assert_eq!(
                shrunk_input(&r),
                json!({"r": case.minimum}),
                "{} seed {seed}",
                case.name
            );
        }
    }
}

#[test]
fn a_bool_shrinks_only_to_false_and_other_binders_still_shrink() {
    let binders = [("b", G::Bool), ("x", G::Int)];
    let (p, prop) = single_property("adv.shrink", "never true", None, &binders, |cx, _, l| {
        let h = &cx.h;
        asserting(h, not(h, h.local(l[0], Ty::Bool)), "not b")
    });
    let identity = oracle::qualified("adv.shrink", "never true");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (t[0] == V::Bool(true)).then_some("assertion_failed");
    for seed in 1..=4 {
        let r = run.check(seed, 100, 1000, &model);
        assert_eq!(shrunk_input(&r), json!({"b": true, "x": 0}), "seed {seed}");
    }
}

#[test]
fn a_full_pass_restarts_from_the_first_binder() {
    // S45: "after the last binder restarts from binder 1 until a full pass
    // accepts nothing". Failing when a >= b >= 5, the first pass ends at
    // (b0, 5); only a second pass brings `a` down to 5.
    let binders = [("a", G::Int), ("b", G::Int)];
    let (p, prop) = single_property("adv.shrink", "ordered pair", None, &binders, |cx, _, l| {
        let h = &cx.h;
        let both = bin(
            h,
            B::And,
            bin(h, B::Ge, cx.int(l[0]), cx.int(l[1])),
            bin(h, B::Ge, cx.int(l[1]), h.int(5)),
        );
        asserting(h, not(h, both), "not (a >= b and b >= 5)")
    });
    let identity = oracle::qualified("adv.shrink", "ordered pair");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let five = BigInt::from(5);
    let model =
        |t: &[V]| (t[0].int() >= t[1].int() && *t[1].int() >= five).then_some("assertion_failed");
    let mut restarted = 0;
    for seed in 1..=10 {
        let r = run.check(seed, 100, 1000, &model);
        if r.status != TestStatus::Failed || r.shrunk.as_ref().unwrap().shrink_exhausted {
            continue;
        }
        assert_eq!(shrunk_input(&r), json!({"a": 5, "b": 5}), "seed {seed}");
        let b0 = failing_input(&r)["b"].clone();
        if b0 != json!(5) {
            restarted += 1;
        }
    }
    assert!(restarted >= 2, "{restarted}");
}

#[test]
fn only_candidates_failing_with_the_original_code_are_accepted() {
    // S45, stdlib S32: a candidate failing with another code is discarded;
    // each rerun still counts as a step.
    let binders = [("x", G::Range(0, 1000))];
    let (p, prop) = single_property("adv.shrink", "two codes", None, &binders, |cx, _, l| {
        let h = &cx.h;
        h.block(
            vec![Stmt::Expr(h.if_(
                bin(h, B::Ge, cx.int(l[0]), h.int(20)),
                asserting(h, h.bool(false), "false"),
                Some(h.block(
                    vec![Stmt::Discard(bin(h, B::Div, h.int(1), h.int(0)))],
                    None,
                )),
            ))],
            None,
        )
    });
    let identity = oracle::qualified("adv.shrink", "two codes");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let twenty = BigInt::from(20);
    let model = |t: &[V]| {
        Some(if *t[0].int() >= twenty {
            "assertion_failed"
        } else {
            "division_by_zero"
        })
    };
    for seed in 0..12 {
        let r = run.check(seed, 100, 1000, &model);
        let original = failing_input(&r)["x"].as_i64().expect("an Int");
        let expected = if original >= 20 { 20 } else { 0 };
        assert_eq!(shrunk_input(&r), json!({"x": expected}), "seed {seed}");
    }
}

#[test]
fn a_negative_range_shrinks_toward_its_upper_bound() {
    // stdlib 8.4: with hi < 0 the origin `0.max(lo).min(hi)` is hi, so
    // candidates climb toward hi; the bounds lie far outside i64.
    let base: i128 = -(1 << 90);
    let binders = [("r", G::Range(base, base + 1000))];
    let (p, prop) = single_property("adv.range", "far below", None, &binders, |cx, _, l| {
        let h = &cx.h;
        asserting(
            h,
            bin(h, B::Gt, cx.int(l[0]), h.int(base + 500)),
            "r > base + 500",
        )
    });
    let identity = oracle::qualified("adv.range", "far below");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let limit = BigInt::from(base + 500);
    let model = |t: &[V]| (*t[0].int() <= limit).then_some("assertion_failed");
    let minimum: Json = serde_json::from_str(&(base + 500).to_string()).unwrap();
    for seed in 1..=4 {
        let r = run.check(seed, 100, 1000, &model);
        assert_eq!(r.status, TestStatus::Failed, "seed {seed}");
        assert_eq!(shrunk_input(&r), json!({"r": minimum}), "seed {seed}");
    }
}

#[test]
fn nested_generators_shrink_exactly() {
    // stdlib 8.4: "nested generators shrink their payloads": a list's
    // element candidates are the inner generator's, an option's payload
    // candidates follow `None`.
    let binders = [("xss", list(list(G::Int)))];
    let (p, prop) = single_property("adv.nested", "few elements", None, &binders, |cx, _, l| {
        let h = &cx.h;
        let lli = Ty::list(Ty::list(Ty::Int));
        let flat = h.call_generic(
            cx.flatten,
            vec![Ty::Int],
            vec![h.local(l[0], lli)],
            Ty::list(Ty::Int),
        );
        let len = h.call_generic(cx.list_len, vec![Ty::Int], vec![flat], Ty::Int);
        asserting(h, bin(h, B::Lt, len, h.int(5)), "flatten(xss).len < 5")
    });
    let identity = oracle::qualified("adv.nested", "few elements");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| {
        let total: usize = t[0].items().iter().map(|inner| inner.items().len()).sum();
        (total >= 5).then_some("assertion_failed")
    };
    for seed in 1..=4 {
        run.check(seed, 100, 1000, &model);
    }
    let binders = [("o", opt(list(G::Bool)))];
    let (q, prop) = single_property("adv.nested", "short payload", None, &binders, |cx, _, l| {
        let h = &cx.h;
        let lb = Ty::list(Ty::Bool);
        let payload = h.call_generic(
            cx.unwrap_or,
            vec![lb.clone()],
            vec![
                h.local(l[0], h.option_ty(lb.clone())),
                h.list(vec![], Ty::Bool),
            ],
            lb,
        );
        let len = h.call_generic(cx.list_len, vec![Ty::Bool], vec![payload], Ty::Int);
        asserting(h, bin(h, B::Lt, len, h.int(2)), "o.unwrap_or([]).len < 2")
    });
    let identity = oracle::qualified("adv.nested", "short payload");
    let run = Run {
        p: &q,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| match &t[0] {
        V::Some(xs) if xs.items().len() >= 2 => Some("assertion_failed"),
        _ => None,
    };
    for seed in 1..=4 {
        let r = run.check(seed, 100, 1000, &model);
        assert_eq!(
            shrunk_input(&r),
            json!({"o": {"$type": "Option", "$variant": "Some", "$payload": [[false, false]]}}),
            "seed {seed}"
        );
    }
}

#[test]
fn list_removals_try_indexes_in_ascending_order() {
    // stdlib 8.4: "v without element i for i ascending over every index";
    // with a failure that depends on which ends survive, the order decides
    // the shrunk list.
    let binders = [("xs", list(G::Int))];
    let (p, prop) = single_property("adv.order", "ends agree", None, &binders, |cx, _, l| {
        let h = &cx.h;
        let li = Ty::list(Ty::Int);
        let xs = || h.local(l[0], li.clone());
        let first = h.call_generic(
            cx.list_first,
            vec![Ty::Int],
            vec![xs()],
            h.option_ty(Ty::Int),
        );
        let last = h.call_generic(
            cx.list_last,
            vec![Ty::Int],
            vec![xs()],
            h.option_ty(Ty::Int),
        );
        let len = h.call_generic(cx.list_len, vec![Ty::Int], vec![xs()], Ty::Int);
        let both = bin(
            h,
            B::And,
            bin(h, B::Ge, len, h.int(2)),
            bin(h, B::Ne, first, last),
        );
        asserting(
            h,
            not(h, both),
            "not (xs.len >= 2 and xs.first() != xs.last())",
        )
    });
    let identity = oracle::qualified("adv.order", "ends agree");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| {
        let xs = t[0].items();
        (xs.len() >= 2 && xs.first() != xs.last()).then_some("assertion_failed")
    };
    for seed in 1..=8 {
        run.check(seed, 100, 1000, &model);
    }
}

#[test]
fn a_spent_budget_stops_inside_either_binders_candidates() {
    // S45: max_shrinks bounds the body reruns of the whole coordinate-wise
    // walk; whichever binder is being shrunk when it is reached, the tuple
    // reached so far is reported with shrink_exhausted.
    let binders = [("a", WIDE), ("b", WIDE)];
    let (p, prop) = single_property("adv.budget", "ordered pair", None, &binders, |cx, _, l| {
        let h = &cx.h;
        let both = bin(
            h,
            B::And,
            bin(h, B::Ge, cx.int(l[0]), cx.int(l[1])),
            bin(h, B::Ge, cx.int(l[1]), h.int(5)),
        );
        asserting(h, not(h, both), "not (a >= b and b >= 5)")
    });
    let identity = oracle::qualified("adv.budget", "ordered pair");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let five = BigInt::from(5);
    let model =
        |t: &[V]| (t[0].int() >= t[1].int() && *t[1].int() >= five).then_some("assertion_failed");
    for seed in [1, 2] {
        let full = run.check(seed, 100, 1000, &model);
        let steps = full.shrunk.as_ref().unwrap().shrink_steps;
        assert_eq!(shrunk_input(&full), json!({"a": 5, "b": 5}), "seed {seed}");
        for max in 0..steps {
            let r = run.check(seed, 100, max, &model);
            assert!(
                r.shrunk.as_ref().unwrap().shrink_exhausted,
                "seed {seed} max {max}"
            );
        }
    }
}

/// The callees of the counterexample tests, in module `adv.ce`.
#[derive(Clone, Copy)]
struct CalleeIds {
    /// `fn f(n: Int) -> Int ensures result < 10 { n }`.
    f: DefId,
    /// A loop with `invariant i < 3` running while `i < n`.
    g: DefId,
    /// `decreases n`, recursive down to 0.
    countdown: DefId,
    /// `requires n < 10`.
    bounded: DefId,
    /// `requires n != 10 ensures result < 10 { n }`.
    both: DefId,
    /// `fn via(n: Int) -> Int { f(n + 5) }`.
    via: DefId,
    /// `decreases n`, recursing on `n + 1` while `n > 0`.
    grow: DefId,
    /// `@id("fn_zero_args") fn zero() -> Int ensures result > 0 { 0 }`.
    zero: DefId,
}

type PropBody<'a> = &'a dyn Fn(&Cx, &CalleeIds, &mut PropertyBuilder, &[LocalId]) -> Block;
type TestBody<'a> = &'a dyn Fn(&Cx, &CalleeIds) -> Block;

/// A property of the counterexample module.
struct PropSpec<'a> {
    name: &'a str,
    id: Option<&'a str>,
    binders: &'a [(&'a str, G)],
    body: PropBody<'a>,
}

struct Callees {
    p: Program,
    props: Vec<DefId>,
    tests: Vec<DefId>,
}

fn callees(props: &[PropSpec<'_>], tests: &[(&str, TestBody<'_>)]) -> Callees {
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.ce");
    let h = cx.h;
    let mut f = FnBuilder::new("f", Ty::Int);
    let n = f.param("n", Ty::Int);
    let result = f.result();
    f.ensures(h.clause(bin(&h, B::Lt, cx.int(result), h.int(10)), "result < 10"));
    f.body(h.value(cx.int(n)));
    let f = b.add_fn(m, f);

    let mut g = FnBuilder::new("g", Ty::Int);
    let n = g.param("n", Ty::Int);
    let i = g.local("i", Ty::Int, BindingKind::Var, true);
    g.body(h.block(
        vec![
            Stmt::Let {
                local: i,
                value: h.int(0),
            },
            Stmt::While {
                cond: bin(&h, B::Lt, cx.int(i), cx.int(n)),
                invariants: vec![h.clause(bin(&h, B::Lt, cx.int(i), h.int(3)), "i < 3")],
                decreases: None,
                body: h.block(
                    vec![Stmt::Assign {
                        local: i,
                        value: bin(&h, B::Add, cx.int(i), h.int(1)),
                    }],
                    None,
                ),
            },
        ],
        Some(cx.int(i)),
    ));
    let g = b.add_fn(m, g);

    let countdown_id = b.next_def();
    let mut d = FnBuilder::new("countdown", Ty::Int);
    let n = d.param("n", Ty::Int);
    d.decreases(h.clause(cx.int(n), "n"));
    d.body(h.value(h.if_(
        bin(&h, B::Le, cx.int(n), h.int(0)),
        h.value(h.int(0)),
        Some(h.value(h.call(
            countdown_id,
            vec![bin(&h, B::Sub, cx.int(n), h.int(1))],
            Ty::Int,
        ))),
    )));
    let countdown = b.add_fn(m, d);
    assert_eq!(countdown, countdown_id);

    let mut r = FnBuilder::new("bounded", Ty::Int);
    let n = r.param("n", Ty::Int);
    r.requires(h.clause(bin(&h, B::Lt, cx.int(n), h.int(10)), "n < 10"));
    r.body(h.value(cx.int(n)));
    let bounded = b.add_fn(m, r);

    let mut r = FnBuilder::new("both", Ty::Int);
    let n = r.param("n", Ty::Int);
    let result = r.result();
    r.requires(h.clause(bin(&h, B::Ne, cx.int(n), h.int(10)), "n != 10"));
    r.ensures(h.clause(bin(&h, B::Lt, cx.int(result), h.int(10)), "result < 10"));
    r.body(h.value(cx.int(n)));
    let both = b.add_fn(m, r);

    let mut v = FnBuilder::new("via", Ty::Int);
    let n = v.param("n", Ty::Int);
    v.body(h.value(h.call(f, vec![bin(&h, B::Add, cx.int(n), h.int(5))], Ty::Int)));
    let via = b.add_fn(m, v);

    let grow_id = b.next_def();
    let mut d = FnBuilder::new("grow", Ty::Int);
    let n = d.param("n", Ty::Int);
    d.decreases(h.clause(cx.int(n), "n"));
    d.body(h.value(h.if_(
        bin(&h, B::Le, cx.int(n), h.int(0)),
        h.value(h.int(0)),
        Some(h.value(h.call(grow_id, vec![bin(&h, B::Add, cx.int(n), h.int(1))], Ty::Int))),
    )));
    let grow = b.add_fn(m, d);
    assert_eq!(grow, grow_id);

    let mut z = FnBuilder::new("zero", Ty::Int);
    let result = z.result();
    z.ensures(h.clause(bin(&h, B::Gt, cx.int(result), h.int(0)), "result > 0"));
    z.body(h.value(h.int(0)));
    let zero = b.add_fn(m, z);
    b.set_stable_id(zero, "fn_zero_args");

    let ids = CalleeIds {
        f,
        g,
        countdown,
        bounded,
        both,
        via,
        grow,
        zero,
    };
    let mut prop_ids = Vec::new();
    for spec in props {
        let id = add_prop(&mut b, m, &cx, spec.name, spec.binders, |pb, l| {
            (spec.body)(&cx, &ids, pb, l)
        });
        if let Some(stable) = spec.id {
            b.set_stable_id(id, stable);
        }
        prop_ids.push(id);
    }
    let test_ids = tests
        .iter()
        .map(|(name, body)| {
            let block = body(&cx, &ids);
            add_test(&mut b, m, name, block)
        })
        .collect();
    Callees {
        p: b.build(),
        props: prop_ids,
        tests: test_ids,
    }
}

/// `let _ = callee(x)` for the first binder.
fn calls(cx: &Cx, callee: DefId, l: &[LocalId]) -> Block {
    cx.h.block(
        vec![Stmt::Discard(cx.h.call(
            callee,
            vec![cx.int(l[0])],
            Ty::Int,
        ))],
        None,
    )
}

fn counterexample(record: &TestCaseRecord) -> Json {
    assert_eq!(record.counterexamples.len(), 1, "{record:?}");
    serde_json::to_value(&record.counterexamples[0]).expect("a diagnostic serializes")
}

/// The fields S46 fixes for every `contract_counterexample` (category and
/// severity: semantics 9.4 lists it under **contract**, severity error).
fn assert_counterexample(ce: &Json, symbol: &str, clause: &str, input: Json, found_by: &str) {
    assert_eq!(ce["code"], "contract_counterexample");
    assert_eq!(ce["category"], "contract");
    assert_eq!(ce["severity"], "error");
    assert_eq!(ce["symbol"], symbol);
    assert_eq!(ce["qualified_name"], symbol);
    assert_eq!(ce["facet"], "body");
    assert_eq!(ce["clause"], clause);
    assert_eq!(ce["input"], input);
    assert_eq!(ce["status"], "failed");
    assert_eq!(ce["found_by"], found_by);
}

const WIDE: G = G::Range(-1000, 1000);

#[test]
fn an_ensures_violation_found_by_a_property_is_a_counterexample_at_the_shrunk_tuple() {
    // S46: `symbol` = the declaration, `facet: "body"`, `clause`, `input` =
    // its arguments at the violation (here: of the shrunk tuple), `actual`,
    // `status: "failed"`, `found_by` = the property's identity (its @id).
    let binders = [("x", WIDE)];
    let fx = callees(
        &[PropSpec {
            name: "calls f",
            id: Some("prop_ce"),
            binders: &binders,
            body: &|cx, c, _, l| calls(cx, c.f, l),
        }],
        &[],
    );
    let run = Run {
        p: &fx.p,
        prop: fx.props[0],
        identity: "prop_ce",
        binders: &binders,
    };
    let ten = BigInt::from(10);
    let model = |t: &[V]| (*t[0].int() >= ten).then_some("contract_violation");
    for seed in 1..=4 {
        let record = run.check(seed, 100, 1000, &model);
        assert_eq!(shrunk_input(&record), json!({"x": 10}), "seed {seed}");
        let fault = record.fault.as_ref().unwrap();
        assert_eq!(fault.data["clause_kind"], "ensures");
        assert_eq!(fault.data["input"], json!({"n": 10}));
        assert_eq!(fault.data["actual"], json!(10));
        let ce = counterexample(&record);
        assert_counterexample(&ce, "adv.ce.f", "result < 10", json!({"n": 10}), "prop_ce");
        assert_eq!(ce["actual"], json!(10));
    }
}

#[test]
fn an_invariant_violation_in_a_callee_is_a_counterexample() {
    let binders = [("x", WIDE)];
    let fx = callees(
        &[PropSpec {
            name: "calls g",
            id: None,
            binders: &binders,
            body: &|cx, c, _, l| calls(cx, c.g, l),
        }],
        &[],
    );
    let identity = oracle::qualified("adv.ce", "calls g");
    let run = Run {
        p: &fx.p,
        prop: fx.props[0],
        identity: &identity,
        binders: &binders,
    };
    let three = BigInt::from(3);
    let model = |t: &[V]| (*t[0].int() >= three).then_some("contract_violation");
    for seed in 1..=3 {
        let record = run.check(seed, 100, 1000, &model);
        assert_eq!(shrunk_input(&record), json!({"x": 3}), "seed {seed}");
        let fault = record.fault.as_ref().unwrap();
        assert_eq!(fault.data["clause_kind"], "invariant");
        assert_eq!(fault.data["iteration"], json!(3));
        let ce = counterexample(&record);
        assert_counterexample(&ce, "adv.ce.g", "i < 3", json!({"n": 3}), &identity);
        assert_eq!(ce["actual"]["bindings"]["i"], json!(3));
    }
}

#[test]
fn a_decreases_violation_in_a_callee_is_a_counterexample() {
    let binders = [("x", G::Range(-50, 50))];
    let fx = callees(
        &[PropSpec {
            name: "calls countdown",
            id: None,
            binders: &binders,
            body: &|cx, c, _, l| calls(cx, c.countdown, l),
        }],
        &[],
    );
    let identity = oracle::qualified("adv.ce", "calls countdown");
    let run = Run {
        p: &fx.p,
        prop: fx.props[0],
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (*t[0].int() < BigInt::from(0)).then_some("contract_violation");
    for seed in 1..=3 {
        let record = run.check(seed, 100, 1000, &model);
        assert_eq!(shrunk_input(&record), json!({"x": -1}), "seed {seed}");
        assert_eq!(
            record.fault.as_ref().unwrap().data["clause_kind"],
            "decreases"
        );
        let ce = counterexample(&record);
        assert_counterexample(&ce, "adv.ce.countdown", "n", json!({"n": -1}), &identity);
        // S35: a negative measure at entry is `actual: {measure: v}`.
        assert_eq!(ce["actual"], json!({"measure": -1}));
    }
}

#[test]
fn a_recursive_decreases_violation_is_a_counterexample_at_the_inner_entry() {
    // S35: on entry the measure must be strictly below the nearest
    // activation's; the violation's `input` is the arguments of the entry
    // that violated it and `actual` is `{previous, measure}`; S46 copies
    // both into the counterexample. (A negative argument violates the
    // measure's `>= 0` instead, so the range stays nonnegative.)
    let binders = [("x", G::Range(0, 50))];
    let fx = callees(
        &[PropSpec {
            name: "calls grow",
            id: None,
            binders: &binders,
            body: &|cx, c, _, l| calls(cx, c.grow, l),
        }],
        &[],
    );
    let identity = oracle::qualified("adv.ce", "calls grow");
    let run = Run {
        p: &fx.p,
        prop: fx.props[0],
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (*t[0].int() > BigInt::from(0)).then_some("contract_violation");
    for seed in 1..=3 {
        let record = run.check(seed, 100, 1000, &model);
        assert_eq!(shrunk_input(&record), json!({"x": 1}), "seed {seed}");
        let ce = counterexample(&record);
        assert_counterexample(&ce, "adv.ce.grow", "n", json!({"n": 2}), &identity);
        assert_eq!(ce["actual"], json!({"measure": 2, "previous": 1}));
    }
}

#[test]
fn a_counterexample_against_an_annotated_function_without_parameters() {
    // S46 / S2: `symbol` is the violating declaration's identity (its @id)
    // with its `qualified_name`; `input` is its arguments: none.
    let fx = callees(
        &[],
        &[("calls zero", &|cx, c| {
            cx.h.block(
                vec![Stmt::Discard(cx.h.call(c.zero, vec![], Ty::Int))],
                None,
            )
        })],
    );
    let record = run_test(&fx.p, fx.tests[0], &cfg(1));
    let ce = counterexample(&record);
    assert_eq!(ce["symbol"], "fn_zero_args");
    assert_eq!(ce["qualified_name"], "adv.ce.zero");
    assert_eq!(ce["facet"], "body");
    assert_eq!(ce["clause"], "result > 0");
    assert_eq!(ce["input"], json!({}));
    assert_eq!(ce["actual"], json!(0));
    assert_eq!(ce["found_by"], "adv.ce.\"calls zero\"");
}

#[test]
fn a_violation_two_calls_deep_indicts_the_declaration_whose_clause_failed() {
    let fx = callees(
        &[],
        &[("calls via", &|cx, c| {
            // `via(5)` calls `f(10)`, whose ensures fails.
            cx.h.block(
                vec![Stmt::Discard(cx.h.call(c.via, vec![cx.h.int(5)], Ty::Int))],
                None,
            )
        })],
    );
    let record = run_test(&fx.p, fx.tests[0], &cfg(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.kind, TestKind::Test);
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.symbol.as_ref().unwrap().as_str(), "adv.ce.f");
    let frames: Vec<&str> = fault
        .backtrace
        .iter()
        .map(|f| f.qualified_name.as_str())
        .collect();
    assert_eq!(frames, ["adv.ce.f", "adv.ce.via", "adv.ce.\"calls via\""]);
    let ce = counterexample(&record);
    assert_counterexample(
        &ce,
        "adv.ce.f",
        "result < 10",
        json!({"n": 10}),
        "adv.ce.\"calls via\"",
    );
    assert_eq!(ce["actual"], json!(10));
}

#[test]
fn a_requires_violation_or_a_builtin_precondition_is_not_a_counterexample() {
    // S46: "A requires violation inside a test is not a counterexample
    // against the callee; it is reported only through the test's fault."
    let binders = [("x", WIDE)];
    let fx = callees(
        &[
            PropSpec {
                name: "calls bounded",
                id: None,
                binders: &binders,
                body: &|cx, c, _, l| calls(cx, c.bounded, l),
            },
            PropSpec {
                name: "takes x",
                id: None,
                binders: &binders,
                body: &|cx, _, _, l| {
                    let h = &cx.h;
                    let take = h.call_generic(
                        cx.list_take,
                        vec![Ty::Int],
                        vec![h.list(vec![h.int(1)], Ty::Int), cx.int(l[0])],
                        Ty::list(Ty::Int),
                    );
                    h.block(vec![Stmt::Discard(take)], None)
                },
            },
        ],
        &[],
    );
    let identity = oracle::qualified("adv.ce", "calls bounded");
    let run = Run {
        p: &fx.p,
        prop: fx.props[0],
        identity: &identity,
        binders: &binders,
    };
    let ten = BigInt::from(10);
    let model = |t: &[V]| (*t[0].int() >= ten).then_some("contract_violation");
    let record = run.check(2, 100, 1000, &model);
    assert_eq!(shrunk_input(&record), json!({"x": 10}));
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.data["clause_kind"], "requires");
    assert_eq!(fault.data["attributed_to"], "caller");
    assert_eq!(fault.data["caller"], json!(identity));
    assert!(record.counterexamples.is_empty());

    let identity = oracle::qualified("adv.ce", "takes x");
    let run = Run {
        p: &fx.p,
        prop: fx.props[1],
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (*t[0].int() < BigInt::from(0)).then_some("contract_violation");
    let record = run.check(2, 100, 1000, &model);
    assert_eq!(shrunk_input(&record), json!({"x": -1}));
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(
        fault.symbol.as_ref().unwrap().as_str(),
        "std.prelude.List.take"
    );
    assert_eq!(fault.data["clause"], "n >= 0");
    assert!(record.counterexamples.is_empty());
}

#[test]
fn a_requires_violation_shares_the_code_of_an_ensures_violation() {
    // S45 and stdlib S32 compare fault *codes* ("since the codes exist no
    // classification is needed beyond comparing them"): `both(x)` fails its
    // ensures for x in 11..=20 and its requires for x = 10, and both are
    // `contract_violation`. The range's origin is 10, so the first candidate
    // of any x > 10 is accepted and the reported fault is the requires
    // violation, which yields no counterexample (S46).
    let binders = [("x", G::Range(10, 20))];
    let fx = callees(
        &[PropSpec {
            name: "calls both",
            id: None,
            binders: &binders,
            body: &|cx, c, _, l| calls(cx, c.both, l),
        }],
        &[],
    );
    let identity = oracle::qualified("adv.ce", "calls both");
    let run = Run {
        p: &fx.p,
        prop: fx.props[0],
        identity: &identity,
        binders: &binders,
    };
    let mut ensures_first = 0;
    for seed in 0..10 {
        let record = run.check(seed, 100, 1000, &|_| Some("contract_violation"));
        assert_eq!(record.failing_case.as_ref().unwrap().index, 0);
        assert_eq!(shrunk_input(&record), json!({"x": 10}), "seed {seed}");
        let fault = record.fault.as_ref().unwrap();
        assert_eq!(fault.data["clause_kind"], "requires", "seed {seed}");
        assert_eq!(fault.data["input"], json!({"n": 10}));
        assert!(record.counterexamples.is_empty(), "seed {seed}");
        if failing_input(&record) != json!({"x": 10}) {
            ensures_first += 1;
            assert_eq!(record.shrunk.as_ref().unwrap().shrink_steps, 1);
        }
    }
    assert!(ensures_first >= 5, "{ensures_first}");
}

#[test]
fn a_propertys_own_loop_invariant_is_not_a_counterexample() {
    let binders = [("x", G::Range(0, 10))];
    let (p, prop) = single_property("adv.own", "own loop", None, &binders, |cx, pb, l| {
        let h = &cx.h;
        let i = pb.local("i", Ty::Int, BindingKind::Var, true);
        h.block(
            vec![
                Stmt::Let {
                    local: i,
                    value: h.int(0),
                },
                Stmt::While {
                    cond: bin(h, B::Lt, cx.int(i), cx.int(l[0])),
                    invariants: vec![h.clause(bin(h, B::Lt, cx.int(i), h.int(5)), "i < 5")],
                    decreases: None,
                    body: h.block(
                        vec![Stmt::Assign {
                            local: i,
                            value: bin(h, B::Add, cx.int(i), h.int(1)),
                        }],
                        None,
                    ),
                },
            ],
            None,
        )
    });
    let identity = oracle::qualified("adv.own", "own loop");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let model = |t: &[V]| (*t[0].int() >= BigInt::from(5)).then_some("contract_violation");
    for seed in 1..=3 {
        let record = run.check(seed, 100, 1000, &model);
        assert_eq!(shrunk_input(&record), json!({"x": 5}));
        let fault = record.fault.as_ref().unwrap();
        assert_eq!(fault.data["clause_kind"], "invariant");
        assert_eq!(fault.symbol.as_ref().unwrap().as_str(), identity);
        assert!(record.counterexamples.is_empty());
    }
}

// ===========================================================================
// The shrink budget (S45: max_shrinks, shrink_exhausted)
// ===========================================================================

#[test]
fn a_spent_budget_reports_the_tuple_reached_and_its_fault() {
    let binders = [("x", G::Range(0, 100_000))];
    let (p, prop) = single_property("adv.budget", "x below ten", None, &binders, x_below_ten);
    let identity = oracle::qualified("adv.budget", "x below ten");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    for seed in [1, 2] {
        let full = run.check(seed, 100, 1000, &at_least_ten);
        let steps = full.shrunk.as_ref().unwrap().shrink_steps;
        assert_eq!(shrunk_input(&full), json!({"x": 10}));
        assert!(!full.shrunk.as_ref().unwrap().shrink_exhausted);
        assert!(steps > 6, "{steps}");
        let mut budgets = vec![0, 1, 2, 3, 5, steps / 2, steps - 1];
        budgets.dedup();
        for max in budgets {
            // Fewer steps than shrinking needs: the budget is reached with a
            // rerun still wanted, so `shrink_exhausted` is true (S45).
            let record = run.check(seed, 100, max, &at_least_ten);
            let shrunk = record.shrunk.as_ref().unwrap();
            assert_eq!(
                (shrunk.shrink_steps, shrunk.shrink_exhausted),
                (max, true),
                "seed {seed} max {max}"
            );
            // "the reported fault is the one it raised": the reported tuple's.
            let fault = record.fault.as_ref().unwrap();
            assert_eq!(
                fault.data["bindings"][0]["value"],
                shrunk_input(&record)["x"],
                "seed {seed} max {max}"
            );
            if max == 0 {
                assert_eq!(shrunk_input(&record), failing_input(&record));
            }
        }
    }
}

// ===========================================================================
// Range setup (stdlib 8.2, S30; toolchain 1.7.2, 3.5)
// ===========================================================================

/// A property `adv.setup."setup"` with the `for` ranges `gens` builds and
/// an empty body.
fn ranges_property(gens: impl FnOnce(&Cx) -> Vec<(&'static str, GenExpr, Ty)>) -> (Program, DefId) {
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.setup");
    let mut pb = PropertyBuilder::new("setup");
    for (name, g, ty) in gens(&cx) {
        pb.binder(name, g, ty);
    }
    pb.body(cx.h.block(vec![], None));
    let prop = b.add_property(m, pb);
    (b.build(), prop)
}

fn range(lo: Expr, hi: Expr) -> GenExpr {
    GenExpr::Range(Box::new(lo), Box::new(hi))
}

fn div(h: &Hir, a: i64, b: i64) -> Expr {
    bin(h, B::Div, h.int(a), h.int(b))
}

/// The record of a property that fails at setup: zero cases, no case.
fn setup_fault(p: &Program, prop: DefId, cases: u32) -> Json {
    let record = run_test(p, prop, &TestConfig { cases, ..cfg(3) });
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.cases, Some(0));
    assert!(record.failing_case.is_none() && record.shrunk.is_none());
    assert_eq!(record.seed, Some(3));
    assert!(record.property_seed.is_some());
    serde_json::to_value(record.fault.as_ref().expect("a fault")).expect("serializes")
}

#[test]
fn range_bounds_are_checked_at_setup_even_with_zero_cases() {
    // stdlib 8.2 and 12 ("for x in gen.range(5, 1) (at property setup)"):
    // the ranges are evaluated once per property before case 0, as part of
    // its setup, so the fault occurs even when no case is to run.
    let (p, prop) = ranges_property(|cx| vec![("x", range(cx.h.int(5), cx.h.int(1)), Ty::Int)]);
    for cases in [0, 1, 100] {
        let fault = setup_fault(&p, prop, cases);
        assert_eq!(fault["code"], "contract_violation");
        assert_eq!(fault["clause_kind"], "requires");
        assert_eq!(fault["symbol"], "std.gen.range");
        assert_eq!(fault["clause"], "lo <= hi");
        assert_eq!(fault["input"], json!({"lo": 5, "hi": 1}));
    }
}

#[test]
fn range_setup_follows_clause_order_and_argument_order() {
    // The first clause's range is valid, the second's is not.
    let (p, prop) = ranges_property(|cx| {
        let h = &cx.h;
        vec![
            ("a", range(h.int(0), h.int(3)), Ty::Int),
            ("b", range(h.int(5), h.int(1)), Ty::Int),
        ]
    });
    let fault = setup_fault(&p, prop, 100);
    assert_eq!(fault["code"], "contract_violation");
    assert_eq!(fault["input"], json!({"lo": 5, "hi": 1}));
    // Clause order: the first clause's fault wins.
    let (p, prop) = ranges_property(|cx| {
        let h = &cx.h;
        vec![
            ("a", range(div(h, 1, 0), h.int(3)), Ty::Int),
            ("b", range(h.int(5), h.int(1)), Ty::Int),
        ]
    });
    let fault = setup_fault(&p, prop, 100);
    assert_eq!(fault["code"], "division_by_zero");
    assert_eq!(fault["input"], json!({"a": 1, "b": 0}));
    assert_eq!(fault["symbol"], "adv.setup.\"setup\"");
    // Left to right within a range (semantics 6.2): lo before hi.
    let (p, prop) = ranges_property(|cx| {
        let h = &cx.h;
        vec![(
            "a",
            range(bin(h, B::Rem, h.int(7), h.int(0)), div(h, 1, 0)),
            Ty::Int,
        )]
    });
    let fault = setup_fault(&p, prop, 100);
    assert_eq!(fault["input"], json!({"a": 7, "b": 0}));
    // Arguments are evaluated before the precondition (semantics 5.2).
    let (p, prop) = ranges_property(|cx| {
        let h = &cx.h;
        vec![("a", range(h.int(10), div(h, 2, 0)), Ty::Int)]
    });
    let fault = setup_fault(&p, prop, 100);
    assert_eq!(fault["code"], "division_by_zero");
    assert_eq!(fault["input"], json!({"a": 2, "b": 0}));
    // A range nested in another generator is evaluated at setup too.
    let (p, prop) = ranges_property(|cx| {
        let h = &cx.h;
        vec![(
            "xs",
            GenExpr::List(Box::new(range(h.int(3), h.int(2)))),
            Ty::list(Ty::Int),
        )]
    });
    let fault = setup_fault(&p, prop, 100);
    assert_eq!(fault["symbol"], "std.gen.range");
    assert_eq!(fault["input"], json!({"lo": 3, "hi": 2}));
}

#[test]
fn range_bounds_may_call_pure_functions_and_one_value_ranges_are_valid() {
    // stdlib S30: "a range expression is a contract expression ... so it may
    // call pure functions, as in gen.range(0, limit())"; lo == hi satisfies
    // `requires lo <= hi`.
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.setup");
    let h = cx.h;
    let mut f = FnBuilder::new("limit", Ty::Int);
    f.body(h.value(h.int(5)));
    let limit = b.add_fn(m, f);
    let call = || h.call(limit, vec![], Ty::Int);
    let mut pb = PropertyBuilder::new("up to the limit");
    let r = pb.binder("r", range(h.int(0), call()), Ty::Int);
    let s = pb.binder("s", range(call(), call()), Ty::Int);
    pb.body(h.block(
        vec![
            assert_(
                bin(
                    &h,
                    B::And,
                    bin(&h, B::Ge, cx.int(r), h.int(0)),
                    bin(&h, B::Le, cx.int(r), h.int(5)),
                ),
                "r >= 0 and r <= 5",
            ),
            assert_(bin(&h, B::Eq, cx.int(s), h.int(5)), "s == 5"),
        ],
        None,
    ));
    let ok = b.add_property(m, pb);
    let mut pb = PropertyBuilder::new("inverted limit");
    pb.binder("r", range(call(), h.int(0)), Ty::Int);
    pb.body(h.block(vec![], None));
    let inverted = b.add_property(m, pb);
    let p = b.build();
    let record = run_test(&p, ok, &cfg(8));
    assert_eq!(record.status, TestStatus::Tested, "{record:?}");
    assert_eq!(record.cases, Some(100));
    let fault = setup_fault(&p, inverted, 100);
    assert_eq!(fault["symbol"], "std.gen.range");
    assert_eq!(fault["input"], json!({"lo": 5, "hi": 0}));
}

#[test]
fn an_ensures_violation_in_a_range_bound_is_a_counterexample() {
    // toolchain 1.7.2: a fault while evaluating the ranges "fails the
    // property with that fault and zero cases"; S46: "If the fault is a
    // contract_violation with clause_kind in {ensures, invariant, decreases}
    // raised by a declaration other than the test itself, the runner
    // additionally emits contract_counterexample". The setup fault is the
    // property's fault, so the rule applies to it as to any other.
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.ce");
    let h = cx.h;
    let mut f = FnBuilder::new("f", Ty::Int);
    let n = f.param("n", Ty::Int);
    let result = f.result();
    f.ensures(h.clause(bin(&h, B::Lt, cx.int(result), h.int(10)), "result < 10"));
    f.body(h.value(cx.int(n)));
    let f = b.add_fn(m, f);
    let mut pb = PropertyBuilder::new("bound by f");
    pb.binder(
        "r",
        range(h.int(0), h.call(f, vec![h.int(20)], Ty::Int)),
        Ty::Int,
    );
    pb.body(h.block(vec![], None));
    let prop = b.add_property(m, pb);
    let p = b.build();
    let record = run_test(&p, prop, &cfg(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.cases, Some(0));
    assert!(record.failing_case.is_none() && record.shrunk.is_none());
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.code, "contract_violation");
    assert_eq!(fault.data["clause_kind"], "ensures");
    assert_eq!(fault.data["input"], json!({"n": 20}));
    let ce = counterexample(&record);
    assert_counterexample(
        &ce,
        "adv.ce.f",
        "result < 10",
        json!({"n": 20}),
        "adv.ce.\"bound by f\"",
    );
    assert_eq!(ce["actual"], json!(20));
}

/// `fn slow(n: Int) -> Int { var i = 0; while i < n { i = i + 1 }; 5 }` and
/// a property `for r in gen.range(0, slow(n)) { assert r <= 5 }`.
fn slow_bound_property(n: i64) -> (Program, DefId) {
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.setup");
    let h = cx.h;
    let mut f = FnBuilder::new("slow", Ty::Int);
    let bound = f.param("n", Ty::Int);
    let i = f.local("i", Ty::Int, BindingKind::Var, true);
    f.body(h.block(
        vec![
            Stmt::Let {
                local: i,
                value: h.int(0),
            },
            Stmt::While {
                cond: bin(&h, B::Lt, cx.int(i), cx.int(bound)),
                invariants: vec![],
                decreases: None,
                body: h.block(
                    vec![Stmt::Assign {
                        local: i,
                        value: bin(&h, B::Add, cx.int(i), h.int(1)),
                    }],
                    None,
                ),
            },
        ],
        Some(h.int(5)),
    ));
    let slow = b.add_fn(m, f);
    let mut pb = PropertyBuilder::new("slow bound");
    let r = pb.binder(
        "r",
        range(h.int(0), h.call(slow, vec![h.int(n)], Ty::Int)),
        Ty::Int,
    );
    pb.body(asserting(&h, bin(&h, B::Le, cx.int(r), h.int(5)), "r <= 5"));
    let prop = b.add_property(m, pb);
    (b.build(), prop)
}

#[test]
fn the_time_budget_does_not_cover_the_range_setup() {
    // toolchain 7.2 `--timeout-ms`: a "wall-clock budget per body
    // evaluation: each test, each property case, and each shrink rerun
    // separately"; semantics S38: a time budget "applies only where
    // [the toolchain] defines one". The ranges are evaluated once per
    // property before case 0 (stdlib 8.2), which is none of the three, so a
    // slow but finite bound is not cut off; every case body is instant.
    // The bound's loop grows until its setup takes long enough to matter,
    // whatever the build's speed (a release build is several times faster).
    let mut n = 300_000;
    let (p, prop, setup) = loop {
        let (p, prop) = slow_bound_property(n);
        let start = std::time::Instant::now();
        let unbudgeted = run_test(&p, prop, &TestConfig { cases: 3, ..cfg(1) });
        let setup = start.elapsed();
        assert_eq!(unbudgeted.status, TestStatus::Tested, "{unbudgeted:?}");
        if setup >= Duration::from_millis(40) || n >= 64_000_000 {
            break (p, prop, setup);
        }
        n *= 2;
    };
    assert!(
        setup >= Duration::from_millis(40),
        "the bound is too fast to matter: {setup:?}"
    );
    let mut config = TestConfig { cases: 3, ..cfg(1) };
    config.eval.time_budget = Some(setup / 8);
    let record = run_test(&p, prop, &config);
    assert_eq!(record.status, TestStatus::Tested, "{record:?}");
    assert_eq!(record.cases, Some(3));
}

// ===========================================================================
// Records (S46; cpl_diag::json::TestCaseRecord; toolchain 7.2)
// ===========================================================================

#[test]
fn property_records_carry_exactly_the_s46_fields() {
    let binders = [("x", G::Int)];
    let (p, prop) = single_property(
        "adv.rec",
        "x below ten",
        Some("prop_rec"),
        &binders,
        x_below_ten,
    );
    // One case at size 0 draws 0 or +-1 and passes.
    let tested =
        serde_json::to_value(run_test(&p, prop, &TestConfig { cases: 1, ..cfg(1) })).unwrap();
    assert_eq!(
        keys(&tested),
        [
            "cases",
            "duration_ms",
            "kind",
            "property_seed",
            "qualified_name",
            "seed",
            "status",
            "symbol"
        ]
    );
    assert_eq!(tested["status"], "tested");
    assert_eq!(tested["cases"], json!(1));
    let record = Run {
        p: &p,
        prop,
        identity: "prop_rec",
        binders: &binders,
    }
    .check(1, 100, 1000, &at_least_ten);
    assert_eq!(record.status, TestStatus::Failed);
    let failed = serde_json::to_value(&record).unwrap();
    assert_eq!(
        keys(&failed),
        [
            "cases",
            "duration_ms",
            "failing_case",
            "fault",
            "kind",
            "property_seed",
            "qualified_name",
            "seed",
            "shrunk",
            "status",
            "symbol"
        ]
    );
    assert_eq!(keys(&failed["failing_case"]), ["index", "input"]);
    assert_eq!(
        keys(&failed["shrunk"]),
        ["input", "shrink_exhausted", "shrink_steps"]
    );
    assert_eq!(failed["symbol"], "prop_rec");
    assert_eq!(failed["qualified_name"], "adv.rec.\"x below ten\"");
    assert_eq!(failed["kind"], "property");
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["seed"], json!(1));
    assert_eq!(
        failed["property_seed"],
        json!(oracle::seed_hex(1, "prop_rec"))
    );
    // The fault is a fault diagnostic object (semantics 6.7, 9.2).
    let fault = &failed["fault"];
    assert_eq!(fault["code"], "assertion_failed");
    assert_eq!(fault["category"], "test");
    assert_eq!(fault["severity"], "error");
    assert_eq!(fault["symbol"], "prop_rec");
    assert_eq!(fault["qualified_name"], "adv.rec.\"x below ten\"");
    assert_eq!(fault["clause"], "x < 10");
    assert_eq!(fault["bindings"][0]["name"], "x");
    assert_eq!(
        fault["bindings"][0]["value"],
        failed["shrunk"]["input"]["x"]
    );
}

#[test]
fn zero_cases_run_nothing_and_the_property_is_tested() {
    // S46: `cases` is the number of cases executed; with `--cases 0` none
    // is, and "every case ran to completion" holds vacuously.
    let (p, prop) = single_property(
        "adv.rec",
        "always fails",
        None,
        &[("x", G::Int)],
        |cx, _, _| asserting(&cx.h, cx.h.bool(false), "false"),
    );
    let record = run_test(&p, prop, &TestConfig { cases: 0, ..cfg(4) });
    assert_eq!(record.status, TestStatus::Tested);
    assert_eq!(record.cases, Some(0));
    assert_eq!(record.seed, Some(4));
    assert_eq!(
        record.property_seed.as_deref(),
        Some(oracle::seed_hex(4, "adv.rec.\"always fails\"").as_str())
    );
    assert!(record.failing_case.is_none() && record.shrunk.is_none() && record.fault.is_none());
    let report = run_tests(&p, &TestSelector::all(), &TestConfig { cases: 0, ..cfg(4) });
    assert_eq!(
        (report.cases, report.selected, report.tested, report.failed),
        (0, 1, 1, 0)
    );
    // One case is enough to fail it at case 0.
    let record = run_test(&p, prop, &TestConfig { cases: 1, ..cfg(4) });
    assert_eq!((record.status, record.cases), (TestStatus::Failed, Some(1)));
}

#[test]
fn the_test_result_of_a_property_carries_its_record() {
    let binders = [("x", G::Int)];
    let (p, prop) = single_property(
        "adv.rec",
        "x below ten",
        Some("prop_diag"),
        &binders,
        x_below_ten,
    );
    let failed = Run {
        p: &p,
        prop,
        identity: "prop_diag",
        binders: &binders,
    }
    .check(1, 100, 1000, &at_least_ten);
    let d = test_result_diagnostic(&failed);
    assert_eq!(d.code, "test_result");
    assert_eq!(d.category, Category::Test);
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.symbol.as_ref().map(StableId::as_str), Some("prop_diag"));
    assert_eq!(d.qualified_name.as_deref(), Some("adv.rec.\"x below ten\""));
    assert_eq!(d.data["status"], "failed");
    assert_eq!(d.data["seed"], json!(1));
    assert_eq!(
        d.data["property_seed"],
        json!(oracle::seed_hex(1, "prop_diag"))
    );
    assert_eq!(d.data["cases"], json!(failed.cases.unwrap()));
    assert_eq!(
        d.data["failing_case"]["index"],
        json!(failed.failing_case.as_ref().unwrap().index)
    );
    assert_eq!(d.data["shrunk"]["input"], shrunk_input(&failed));
    assert_eq!(d.data["fault"]["code"], "assertion_failed");
    let tested = run_test(&p, prop, &TestConfig { cases: 1, ..cfg(1) });
    let d = test_result_diagnostic(&tested);
    assert_eq!(d.severity, Severity::Info);
    assert_eq!(d.data["status"], "tested");
    assert_eq!(d.data["cases"], json!(1));
    assert!(d.data.get("failing_case").is_none() && d.data.get("fault").is_none());
}

// ===========================================================================
// assert (semantics 7.1, S41)
// ===========================================================================

#[test]
fn an_assert_in_a_called_function_blames_it_and_lists_its_bindings() {
    // S41: `symbol` is the enclosing declaration of the assert, `bindings`
    // every local binding and parameter visible there, with type and value,
    // innermost first, `var` bindings with `mutable: true` (8.3); "a failing
    // assert inside a function called by a test fails the test like any
    // fault".
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.assert");
    let h = cx.h;
    let mut f = FnBuilder::new("checked", Ty::Int);
    let n = f.param("n", Ty::Int);
    let doubled = f.local("doubled", Ty::Int, BindingKind::Let, false);
    let acc = f.local("acc", Ty::Int, BindingKind::Var, true);
    let later = f.local("later", Ty::Int, BindingKind::Let, false);
    f.body(h.block(
        vec![
            Stmt::Let {
                local: doubled,
                value: bin(&h, B::Mul, cx.int(n), h.int(2)),
            },
            Stmt::Let {
                local: acc,
                value: bin(&h, B::Add, cx.int(doubled), h.int(1)),
            },
            Stmt::Assign {
                local: acc,
                value: bin(&h, B::Add, cx.int(acc), h.int(1)),
            },
            assert_(bin(&h, B::Lt, cx.int(acc), h.int(10)), "acc < 10"),
            Stmt::Let {
                local: later,
                value: h.int(0),
            },
        ],
        Some(cx.int(acc)),
    ));
    let checked = b.add_fn(m, f);
    let mut t = TestBuilder::new("calls checked");
    let mine = t.local("mine", Ty::Int, BindingKind::Let, false);
    t.body(h.block(
        vec![
            Stmt::Let {
                local: mine,
                value: h.int(7),
            },
            Stmt::Discard(h.call(checked, vec![cx.int(mine)], Ty::Int)),
        ],
        None,
    ));
    let test = b.add_test(m, t);
    let p = b.build();
    let record = run_test(&p, test, &cfg(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert!(record.counterexamples.is_empty());
    let fault = serde_json::to_value(record.fault.as_ref().unwrap()).unwrap();
    assert_eq!(fault["code"], "assertion_failed");
    assert_eq!(fault["category"], "test");
    assert_eq!(fault["severity"], "error");
    assert_eq!(fault["symbol"], "adv.assert.checked");
    assert_eq!(fault["qualified_name"], "adv.assert.checked");
    assert_eq!(fault["facet"], "body");
    assert_eq!(fault["clause"], "acc < 10");
    assert_eq!(
        fault["bindings"],
        json!([
            {"name": "acc", "type": "Int", "kind": "var", "mutable": true, "value": 16},
            {"name": "doubled", "type": "Int", "kind": "let", "value": 14},
            {"name": "n", "type": "Int", "kind": "parameter", "value": 7},
        ])
    );
    let frames: Vec<&str> = fault["backtrace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["qualified_name"].as_str().unwrap())
        .collect();
    assert_eq!(
        frames,
        ["adv.assert.checked", "adv.assert.\"calls checked\""]
    );
}

#[test]
fn an_assert_in_nested_scopes_sees_exactly_the_bindings_in_scope() {
    // S41: a `let` inside an `if` branch and a `forall` binder are visible
    // at an assert inside them; bindings introduced after the assert are
    // not; innermost first.
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.assert");
    let h = cx.h;
    let mut t = TestBuilder::new("nested scopes");
    let a = t.local("a", Ty::Int, BindingKind::Let, false);
    let ok = t.local("ok", Ty::Bool, BindingKind::Let, false);
    let x = t.local("x", Ty::Int, BindingKind::Forall, false);
    let tens = t.local("tens", Ty::Int, BindingKind::Let, false);
    let after = t.local("after", Ty::Int, BindingKind::Let, false);
    let branch = h.block(
        vec![
            Stmt::Let {
                local: tens,
                value: bin(&h, B::Mul, cx.int(x), h.int(10)),
            },
            assert_(bin(&h, B::Lt, cx.int(tens), h.int(20)), "tens < 20"),
        ],
        Some(h.bool(true)),
    );
    let predicate = h.if_(
        bin(&h, B::Eq, cx.int(x), h.int(2)),
        branch,
        Some(h.value(h.bool(true))),
    );
    let items = h.list(vec![h.int(1), h.int(2), h.int(3)], Ty::Int);
    t.body(h.block(
        vec![
            Stmt::Let {
                local: a,
                value: h.int(1),
            },
            Stmt::Let {
                local: ok,
                value: h.forall(x, items, predicate),
            },
            Stmt::Let {
                local: after,
                value: h.int(5),
            },
        ],
        None,
    ));
    let test = b.add_test(m, t);
    let p = b.build();
    let record = run_test(&p, test, &cfg(1));
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.code, "assertion_failed");
    assert_eq!(fault.data["clause"], "tens < 20");
    assert_eq!(
        fault.data["bindings"],
        json!([
            {"name": "tens", "type": "Int", "kind": "let", "value": 20},
            {"name": "x", "type": "Int", "kind": "forall", "value": 2},
            {"name": "a", "type": "Int", "kind": "let", "value": 1},
        ])
    );
}

#[test]
fn a_property_assert_lists_its_lets_and_binders_at_the_shrunk_tuple() {
    let binders = [("x", G::Range(0, 100)), ("flag", G::Bool)];
    let (p, prop) = single_property("adv.assert", "let in body", None, &binders, |cx, pb, l| {
        let h = &cx.h;
        let y = pb.local("y", Ty::Int, BindingKind::Let, false);
        h.block(
            vec![
                Stmt::Let {
                    local: y,
                    value: bin(h, B::Add, cx.int(l[0]), h.int(1)),
                },
                assert_(bin(h, B::Lt, cx.int(y), h.int(5)), "y < 5"),
            ],
            None,
        )
    });
    let identity = oracle::qualified("adv.assert", "let in body");
    let run = Run {
        p: &p,
        prop,
        identity: &identity,
        binders: &binders,
    };
    let four = BigInt::from(4);
    let model = |t: &[V]| (*t[0].int() >= four).then_some("assertion_failed");
    for seed in 1..=3 {
        let record = run.check(seed, 100, 1000, &model);
        // `flag` does not matter, so its candidate `false` fails alike and
        // is accepted.
        assert_eq!(shrunk_input(&record), json!({"x": 4, "flag": false}));
        let fault = record.fault.as_ref().unwrap();
        assert_eq!(fault.symbol.as_ref().unwrap().as_str(), identity);
        assert_eq!(fault.facet, Some(cpl_diag::Facet::Body));
        assert_eq!(
            fault.data["bindings"],
            json!([
                {"name": "y", "type": "Int", "kind": "let", "value": 5},
                {"name": "flag", "type": "Bool", "kind": "for", "value": false},
                {"name": "x", "type": "Int", "kind": "for", "value": 4},
            ])
        );
    }
}

#[test]
fn an_assert_whose_expression_faults_reports_that_fault() {
    // 7.1: `assert e` evaluates `e`; a fault while evaluating it is that
    // fault, not `assertion_failed`.
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.assert");
    let h = cx.h;
    let test = add_test(
        &mut b,
        m,
        "faulting assert",
        asserting(&h, bin(&h, B::Eq, div(&h, 1, 0), h.int(0)), "1 / 0 == 0"),
    );
    let p = b.build();
    let record = run_test(&p, test, &cfg(1));
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.code, "division_by_zero");
    assert_eq!(
        fault.symbol.as_ref().unwrap().as_str(),
        "adv.assert.\"faulting assert\""
    );
    assert_eq!(fault.data["input"], json!({"a": 1, "b": 0}));
}

#[test]
fn the_run_seed_is_an_exact_decimal_u64_everywhere() {
    // toolchain 1.7.2 / stdlib 8.2: the run seed is reported as a decimal
    // integer, which for u64::MAX must not round through a float.
    let (p, prop) = single_property("adv.rec", "big seed", None, &[("x", G::Int)], x_below_ten);
    let report = run_tests(&p, &TestSelector::all(), &cfg(u64::MAX));
    let j = serde_json::to_value(&report).unwrap();
    let max: Json = serde_json::from_str("18446744073709551615").unwrap();
    assert_eq!(j["seed"], max);
    assert_eq!(j["results"][0]["seed"], max);
    assert_eq!(
        j["results"][0]["property_seed"],
        json!(oracle::seed_hex(u64::MAX, "adv.rec.\"big seed\""))
    );
    let _ = prop;
}

// ===========================================================================
// fail_fast and independence of tests (semantics 7.3; toolchain 7.2)
// ===========================================================================

#[test]
fn fail_fast_keeps_the_passing_prefix_and_stops_at_the_first_failure() {
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "ff");
    let h = cx.h;
    add_test(&mut b, m, "a", h.block(vec![], None));
    add_test(&mut b, m, "b", asserting(&h, h.bool(false), "false"));
    add_test(&mut b, m, "c", h.block(vec![], None));
    add_prop(&mut b, m, &cx, "d", &[("flag", G::Bool)], |_, _| {
        asserting(&h, h.bool(false), "false")
    });
    let (m2, cx2) = module_cx(&mut b, "ff2");
    add_prop(&mut b, m2, &cx2, "a", &[("flag", G::Bool)], |_, _| {
        asserting(&cx2.h, cx2.h.bool(false), "false")
    });
    add_test(&mut b, m2, "b", cx2.h.block(vec![], None));
    let p = b.build();
    let ff = TestSelector::parse("ff").unwrap();
    let statuses = |r: &cpl_diag::json::TestReport| -> Vec<(String, TestStatus)> {
        r.results
            .iter()
            .map(|x| (x.qualified_name.clone(), x.status))
            .collect()
    };
    // A failing test never prevents another test from running (7.3).
    let all = run_tests(&p, &ff, &cfg(1));
    assert_eq!(
        statuses(&all),
        [
            ("ff.\"a\"".to_string(), TestStatus::Tested),
            ("ff.\"b\"".to_string(), TestStatus::Failed),
            ("ff.\"c\"".to_string(), TestStatus::Tested),
            ("ff.\"d\"".to_string(), TestStatus::Failed),
        ]
    );
    assert_eq!((all.selected, all.tested, all.failed), (4, 2, 2));
    let fast = run_tests(
        &p,
        &ff,
        &TestConfig {
            fail_fast: true,
            ..cfg(1)
        },
    );
    assert_eq!(
        statuses(&fast),
        [
            ("ff.\"a\"".to_string(), TestStatus::Tested),
            ("ff.\"b\"".to_string(), TestStatus::Failed),
        ]
    );
    // `selected` is the size of the selection, run or not.
    assert_eq!((fast.selected, fast.tested, fast.failed), (4, 1, 1));
    // The first failure may be a property.
    let fast = run_tests(
        &p,
        &TestSelector::parse("ff2").unwrap(),
        &TestConfig {
            fail_fast: true,
            ..cfg(1)
        },
    );
    assert_eq!(
        statuses(&fast),
        [("ff2.\"a\"".to_string(), TestStatus::Failed)]
    );
    assert_eq!((fast.selected, fast.tested, fast.failed), (2, 0, 1));
}

#[test]
fn every_shrink_rerun_gets_a_fresh_time_budget() {
    // toolchain 7.2 `--timeout-ms`: the budget applies to "each test, each
    // property case, and each shrink rerun separately". A body that spins
    // forever when x > 5 times out on every such x; passing reruns finish
    // at once. Were the budget shared across reruns, a rerun started after
    // it expired would time out at once and x = 0 would be accepted.
    let binders = [("x", G::Range(0, 40))];
    let (p, prop) = single_property(
        "adv.time",
        "spins above five",
        None,
        &binders,
        |cx, _, l| {
            let h = &cx.h;
            h.block(
                vec![Stmt::While {
                    cond: bin(h, B::Gt, cx.int(l[0]), h.int(5)),
                    invariants: vec![],
                    decreases: None,
                    body: h.block(vec![], None),
                }],
                None,
            )
        },
    );
    let identity = oracle::qualified("adv.time", "spins above five");
    let gens = [binders[0].1.clone()];
    let model = |t: &[V]| (*t[0].int() > BigInt::from(5)).then_some("resource_exhaustion");
    let seed = 3;
    let expected =
        oracle::expect(seed, &identity, &gens, 100, 1000, &model).expect("a failing case");
    let mut config = cfg(seed);
    config.eval.time_budget = Some(Duration::from_millis(50));
    let record = run_test(&p, prop, &config);
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.failing_case.as_ref().unwrap().index, expected.index);
    assert_eq!(shrunk_input(&record), json!({"x": 6}));
    let shrunk = record.shrunk.as_ref().unwrap();
    assert_eq!(
        (shrunk.shrink_steps, shrunk.shrink_exhausted),
        (expected.shrunk.steps, expected.shrunk.exhausted)
    );
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.code, "resource_exhaustion");
    assert_eq!(fault.data["resource"], "time");
    assert_eq!(fault.data["limit"], json!(50));
}

#[test]
fn one_seed_gives_identical_reports_for_drawing_and_failing_properties() {
    // stdlib S31: the same toolchain, run seed, and property produce the
    // same cases; run_tests and run_test agree record for record.
    let mut b = ProgramBuilder::new();
    let (m, cx) = module_cx(&mut b, "adv.det");
    let mut ids = Vec::new();
    ids.push(add_prop(
        &mut b,
        m,
        &cx,
        "ints",
        &[("x", G::Int), ("t", G::Text)],
        |_, l| x_below_ten_block(&cx, l),
    ));
    ids.push(add_prop(
        &mut b,
        m,
        &cx,
        "nested",
        &[("xss", list(list(G::Int))), ("o", opt(G::Pos))],
        |_, l| {
            let h = &cx.h;
            let len = h.call_generic(
                cx.list_len,
                vec![Ty::list(Ty::Int)],
                vec![h.local(l[0], Ty::list(Ty::list(Ty::Int)))],
                Ty::Int,
            );
            asserting(h, bin(h, B::Lt, len, h.int(3)), "xss.len < 3")
        },
    ));
    let p = b.build();
    for seed in [0, 42, u64::MAX] {
        let a = run_tests(&p, &TestSelector::all(), &cfg(seed));
        let b = run_tests(&p, &TestSelector::all(), &cfg(seed));
        assert_eq!(a.results.len(), 2);
        for ((x, y), &id) in a.results.iter().zip(&b.results).zip(&ids) {
            assert_eq!(stable(x), stable(y), "seed {seed}");
            assert_eq!(
                stable(x),
                stable(&run_test(&p, id, &cfg(seed))),
                "seed {seed}"
            );
        }
    }
}
