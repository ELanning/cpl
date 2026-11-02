//! Adversarial tests of the test and property runner driven from CPL source
//! through the real checker (`cpl-sema`, a dev-dependency of this crate,
//! toolchain 1.2 and T50): semantics 7 (7.1-7.5: S41-S46), stdlib 8
//! (8.1-8.4: S28-S32), and toolchain 1.7.2 together with the `cpl test`
//! contract of toolchain 7.2 and the wire types of Appendix C.
//!
//! Every expectation is the specifications' answer, never the
//! implementation's:
//!
//! * `mod oracle` re-derives from the text of stdlib 8.2-8.4 and semantics
//!   S44-S45 alone each property's stream
//!   (`ChaCha20Rng::from_seed(blake3(seed_le(8) ++ identity))`), the size of
//!   every case (`min(i, 100)`), every generator's draw, every list of
//!   shrink candidates, and the greedy coordinate-wise shrinker with its
//!   budget. A test writes a property in CPL, states the property's failure
//!   condition in Rust (the *model*), and compares the record `run_test`
//!   returns with the oracle's first failing case, shrunk tuple, number of
//!   shrink steps, and `shrink_exhausted` ([`Prop::check`]).
//! * Identities are written out by hand from semantics 1.4, S2, and S40, so
//!   that the escaping of test names is checked rather than borrowed.
//! * Where a specification leaves a reading open, the comment at the test
//!   says which reading it encodes and why.

mod common;

use cpl_diag::json::{TestCaseRecord, TestKind, TestStatus};
use cpl_diag::{Category, Severity, StableId};
use cpl_eval::test::TestReport;
use cpl_eval::{
    property_seed, run_test, run_tests, seed_hex, select_tests, test_result_diagnostic, EvalConfig,
    TestConfig, TestSelector,
};
use cpl_hir::{DefId, Program};
use num_bigint::BigInt;
use oracle::{list, opt, range, range_big, Model, G, V};
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

    /// S44, stdlib 8.2: `blake3(seed_le(8) ++ identity_utf8)`.
    pub fn seed_bytes(run_seed: u64, identity: &str) -> [u8; 32] {
        let mut bytes = Vec::with_capacity(8 + identity.len());
        bytes.extend_from_slice(&run_seed.to_le_bytes());
        bytes.extend_from_slice(identity.as_bytes());
        *blake3::hash(&bytes).as_bytes()
    }

    /// `property_seed` as records carry it: 64 lowercase hex digits (T45).
    pub fn seed_hex(run_seed: u64, identity: &str) -> String {
        seed_bytes(run_seed, identity)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// A property's one stream and the primitive draws of stdlib 8.3.
    pub struct Stream(ChaCha20Rng);

    impl Stream {
        pub fn new(run_seed: u64, identity: &str) -> Stream {
            Stream(ChaCha20Rng::from_seed(seed_bytes(run_seed, identity)))
        }

        /// `u()`: the next `u32` word of the stream.
        fn u(&mut self) -> u32 {
            self.0.next_u32()
        }

        /// `bits(b)`: `ceil(b / 32)` words, the first least significant, of
        /// which the low `b` bits are kept (no word for `b == 0`).
        fn bits(&mut self, b: u64) -> BigUint {
            let mut x = BigUint::zero();
            for i in 0..b.div_ceil(32) {
                x += BigUint::from(self.u()) << (32 * i as usize);
            }
            x % (BigUint::one() << (b as usize))
        }

        /// `below(k)`: `bits(bit length of k - 1)` repeated until below `k`.
        fn below(&mut self, k: &BigUint) -> BigUint {
            assert!(!k.is_zero(), "below(k) needs k >= 1");
            let b = (k - BigUint::one()).bits();
            loop {
                let x = self.bits(b);
                if &x < k {
                    return x;
                }
            }
        }

        fn below64(&mut self, k: u64) -> u64 {
            self.below(&BigUint::from(k))
                .to_u64()
                .expect("a draw below a u64")
        }

        /// `sign()`: one word; odd is negative.
        fn negative(&mut self) -> bool {
            self.u() % 2 == 1
        }
    }

    /// A generator of stdlib 8.1, a `gen.range` with its bounds evaluated.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum G {
        Int,
        Pos,
        Bool,
        Text,
        List(Box<G>),
        Opt(Box<G>),
        Range(BigInt, BigInt),
    }

    pub fn list(g: G) -> G {
        G::List(Box::new(g))
    }

    pub fn opt(g: G) -> G {
        G::Opt(Box::new(g))
    }

    pub fn range(lo: i64, hi: i64) -> G {
        G::Range(BigInt::from(lo), BigInt::from(hi))
    }

    pub fn range_big(lo: &str, hi: &str) -> G {
        G::Range(
            lo.parse().expect("a decimal bound"),
            hi.parse().expect("a decimal bound"),
        )
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

        pub fn flag(&self) -> bool {
            match self {
                V::Bool(b) => *b,
                other => panic!("{other:?} is not a Bool"),
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

        pub fn some(&self) -> Option<&V> {
            match self {
                V::None => None,
                V::Some(x) => Some(x),
                other => panic!("{other:?} is not an option"),
            }
        }

        /// `len` of a list (elements) or a text (Unicode scalar values,
        /// semantics 6.4).
        pub fn length(&self) -> usize {
            match self {
                V::List(items) => items.len(),
                V::Text(t) => t.chars().count(),
                other => panic!("{other:?} has no length"),
            }
        }
    }

    /// One draw of `g` at size `s` (stdlib 8.3; within a row the draws happen
    /// in the order written).
    pub fn draw(st: &mut Stream, g: &G, s: u64) -> V {
        match g {
            G::Int => V::Int(draw_int(st, s)),
            G::Pos => V::Int(BigInt::one() + draw_int(st, s).abs()),
            G::Bool => V::Bool(st.u() % 2 == 1),
            G::Text => {
                let len = st.below64(s + 1);
                let mut text = String::new();
                for _ in 0..len {
                    let scalar = match st.u() % 4 {
                        0 | 1 => char::from(b'a' + st.below64(26) as u8),
                        2 => char::from(0x20 + st.below64(95) as u8),
                        _ => EXTRAS[st.below64(10) as usize],
                    };
                    text.push(scalar);
                }
                V::Text(text)
            }
            G::List(e) => {
                let len = st.below64(s + 1);
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
                let width = (hi - lo + BigInt::one()).to_biguint().expect("lo <= hi");
                V::Int(lo + BigInt::from(st.below(&width)))
            }
        }
    }

    /// `gen.int` at size `s`: bucket `u() % 8`.
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
                let length = 64 + st.below64(8 * s + 1);
                let top = BigUint::one() << ((length - 1) as usize);
                let magnitude = BigInt::from(top + st.bits(length - 1));
                if st.negative() {
                    -magnitude
                } else {
                    magnitude
                }
            }
            _ => BigInt::from(st.below64(2 * s + 1)) - BigInt::from(s),
        }
    }

    /// The shrink candidates of `v` (stdlib 8.4), in the table's order, each
    /// different from `v`; duplicates are kept ("candidates are not
    /// deduplicated").
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
                c.extend(candidates(e, x).into_iter().map(|y| V::Some(Box::new(y))));
                c
            }
            (G::Opt(_), V::None) => Vec::new(),
            (G::Range(lo, hi), V::Int(n)) => {
                let o = BigInt::zero().max(lo.clone()).min(hi.clone());
                let mut c = vec![o.clone(), &o + (n - &o) / 2];
                if n > &o {
                    c.push(n - 1);
                } else if n < &o {
                    c.push(n + 1);
                }
                c.into_iter()
                    .filter(|x| x >= lo && x <= hi)
                    .map(V::Int)
                    .collect()
            }
            _ => panic!("{v:?} is not a value of {g:?}"),
        };
        raw.into_iter().filter(|c| c != v).collect()
    }

    /// The refinement predicates of the binder's type hold (toolchain 1.7.2:
    /// every candidate is checked against them; only `Positive` has one).
    fn satisfies(g: &G, v: &V) -> bool {
        match (g, v) {
            (G::Pos, V::Int(n)) => n.is_positive(),
            (G::List(e), V::List(xs)) => xs.iter().all(|x| satisfies(e, x)),
            (G::Opt(e), V::Some(x)) => satisfies(e, x),
            _ => true,
        }
    }

    /// The fault code the body raises for a tuple, or `None` when it passes.
    pub type Model<'a> = &'a dyn Fn(&[V]) -> Option<&'static str>;

    /// The outcome of shrinking one failure.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Shrunk {
        pub values: Vec<V>,
        pub steps: u32,
        pub exhausted: bool,
    }

    /// S45, stdlib 8.4: greedy and coordinate-wise; the first candidate that
    /// fails with the original code is accepted and the walk continues with
    /// the same binder from it; after the last binder it restarts at the
    /// first until a full pass accepts nothing; every rerun is one step, and
    /// a rerun wanted once `max_shrinks` steps are spent exhausts the budget.
    pub fn shrink(gens: &[G], start: &[V], body: Model<'_>, max_shrinks: u32) -> Shrunk {
        let code = body(start).expect("the original tuple fails");
        let mut current = start.to_vec();
        let mut steps = 0;
        loop {
            let mut accepted_in_pass = false;
            for (j, g) in gens.iter().enumerate() {
                'same_binder: loop {
                    for candidate in candidates(g, &current[j]) {
                        if !satisfies(g, &candidate) {
                            continue;
                        }
                        if steps == max_shrinks {
                            return Shrunk {
                                values: current,
                                steps,
                                exhausted: true,
                            };
                        }
                        steps += 1;
                        let mut trial = current.clone();
                        trial[j] = candidate;
                        if body(&trial) == Some(code) {
                            current = trial;
                            accepted_in_pass = true;
                            continue 'same_binder;
                        }
                    }
                    break;
                }
            }
            if !accepted_in_pass {
                return Shrunk {
                    values: current,
                    steps,
                    exhausted: false,
                };
            }
        }
    }

    /// What a failing property's record must say.
    #[derive(Clone, Debug)]
    pub struct Expected {
        pub index: u32,
        pub input: Vec<V>,
        pub shrunk: Shrunk,
    }

    /// The first failing case among `cases` and its shrinking, or `None`
    /// when every case passes.
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

    /// Value JSON (semantics S40, toolchain 7.3).
    pub fn to_json(v: &V) -> Json {
        match v {
            V::Int(n) => serde_json::from_str(&n.to_string()).expect("an Int is a JSON number"),
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
// Helpers
// ===========================================================================

fn config(seed: u64) -> TestConfig {
    TestConfig::with_seed(seed)
}

fn big(n: i64) -> BigInt {
    BigInt::from(n)
}

/// A record as JSON with its wall time cleared, the one field two runs may
/// disagree on (toolchain 7.1, "Determinism").
fn stable(record: &TestCaseRecord) -> Json {
    let mut r = record.clone();
    r.duration_ms = 0;
    serde_json::to_value(r).expect("a record serializes")
}

/// A report as JSON with every wall time cleared.
fn stable_report(report: &TestReport) -> Json {
    let mut r = report.clone();
    for record in &mut r.results {
        record.duration_ms = 0;
    }
    serde_json::to_value(r).expect("a report serializes")
}

/// The parts of a property's record that its stream and shrinking decide
/// (not the spans of its fault, which move when the file is edited).
fn case_view(record: &TestCaseRecord) -> Json {
    json!({
        "status": record.status,
        "seed": record.seed,
        "property_seed": record.property_seed,
        "cases": record.cases,
        "failing_case": record.failing_case,
        "shrunk": record.shrunk,
        "fault_code": record.fault.as_ref().map(|f| f.code.clone()),
    })
}

fn fault_of(record: &TestCaseRecord) -> Json {
    let fault = record
        .fault
        .as_ref()
        .unwrap_or_else(|| panic!("{} has no fault", record.qualified_name));
    serde_json::to_value(fault).expect("a fault serializes")
}

/// The record's one counterexample (semantics S46).
fn counterexample(record: &TestCaseRecord) -> Json {
    assert_eq!(
        record.counterexamples.len(),
        1,
        "{}: exactly one contract_counterexample",
        record.qualified_name
    );
    serde_json::to_value(&record.counterexamples[0]).expect("a counterexample serializes")
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

/// The qualified names of a diagnostic's backtrace, innermost first.
fn frames(d: &Json) -> Vec<String> {
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

/// The keys of the outermost JSON object of `text`, in the order written.
fn keys_in_order(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut depth = 0;
    let mut want_key = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(d) = chars.next() {
                    match d {
                        '\\' => {
                            s.push(d);
                            if let Some(e) = chars.next() {
                                s.push(e);
                            }
                        }
                        '"' => break,
                        _ => s.push(d),
                    }
                }
                if depth == 1 && want_key {
                    keys.push(s);
                    want_key = false;
                }
            }
            '{' | '[' => {
                depth += 1;
                if depth == 1 && c == '{' {
                    want_key = true;
                }
            }
            '}' | ']' => depth -= 1,
            ',' if depth == 1 => want_key = true,
            _ => {}
        }
    }
    keys
}

/// The keys of a record as the wire form writes them (Appendix C: in the
/// field order of the type, optional keys omitted when absent).
fn record_keys(record: &TestCaseRecord) -> Vec<String> {
    keys_in_order(&serde_json::to_string(record).expect("a record serializes"))
}

fn by_name<'r>(report: &'r TestReport, qualified: &str) -> &'r TestCaseRecord {
    report
        .results
        .iter()
        .find(|r| r.qualified_name == qualified)
        .unwrap_or_else(|| panic!("the report has no {qualified}"))
}

fn names_of(p: &Program, defs: &[DefId]) -> Vec<String> {
    defs.iter().map(|&d| p.display_path(d)).collect()
}

fn selector(namespace: Option<&str>, name: Option<&str>, id: Option<&str>) -> TestSelector {
    TestSelector {
        namespace: namespace.map(|n| n.split('.').map(str::to_string).collect()),
        name: name.map(str::to_string),
        id: id.map(|i| StableId::parse(i).unwrap_or_else(|e| panic!("{i}: {e}"))),
    }
}

fn namespace(arg: &str) -> TestSelector {
    TestSelector::parse(arg).unwrap_or_else(|e| panic!("{arg}: {e}"))
}

/// A binding object without its value: what the hole report lists (8.3).
fn without_value(bindings: &Json) -> Json {
    Json::Array(
        bindings
            .as_array()
            .expect("bindings are an array")
            .iter()
            .map(|b| {
                let mut b = b.as_object().expect("a binding is an object").clone();
                b.remove("value");
                Json::Object(b)
            })
            .collect(),
    )
}

/// A checked property and what the oracle needs about it: its identity,
/// written out independently of the implementation, and its binders.
struct Prop<'a> {
    p: &'a Program,
    def: DefId,
    identity: String,
    binders: &'a [(&'a str, G)],
}

impl<'a> Prop<'a> {
    fn new(
        p: &'a Program,
        qualified: &str,
        identity: &str,
        binders: &'a [(&'a str, G)],
    ) -> Prop<'a> {
        let def = common::def(p, qualified);
        assert_eq!(
            p.def(def).stable_id.as_str(),
            identity,
            "{qualified}: its identity (semantics S2, S42)"
        );
        Prop {
            p,
            def,
            identity: identity.to_string(),
            binders,
        }
    }

    fn gens(&self) -> Vec<G> {
        self.binders.iter().map(|(_, g)| g.clone()).collect()
    }

    fn names(&self) -> Vec<&'a str> {
        self.binders.iter().map(|(n, _)| *n).collect()
    }

    fn expect(&self, config: &TestConfig, model: Model<'_>) -> Option<oracle::Expected> {
        oracle::expect(
            config.seed,
            &self.identity,
            &self.gens(),
            config.cases,
            config.max_shrinks,
            model,
        )
    }

    /// Run the property and compare every field of S46 the oracle predicts:
    /// `seed`, `property_seed`, `cases`, `failing_case`, `shrunk` (input,
    /// `shrink_steps`, `shrink_exhausted`), the code of `fault`, and, for a
    /// fault raised in the property's own body, the values of its `for`
    /// bindings (S41: the bindings visible at the fault, which are the
    /// reported tuple's).
    fn check(&self, config: &TestConfig, model: Model<'_>) -> TestCaseRecord {
        let record = run_test(self.p, self.def, config);
        let ctx = format!(
            "{} (seed {}, cases {}, max_shrinks {})",
            self.identity, config.seed, config.cases, config.max_shrinks
        );
        assert_eq!(record.kind, TestKind::Property, "{ctx}");
        assert_eq!(record.symbol.as_str(), self.identity, "{ctx}: symbol");
        assert_eq!(record.seed, Some(config.seed), "{ctx}: seed");
        assert_eq!(
            record.property_seed.as_deref(),
            Some(oracle::seed_hex(config.seed, &self.identity).as_str()),
            "{ctx}: property_seed"
        );
        let names = self.names();
        match self.expect(config, model) {
            None => {
                assert_eq!(
                    record.status,
                    TestStatus::Tested,
                    "{ctx}: {}",
                    stable(&record)
                );
                assert_eq!(record.cases, Some(config.cases), "{ctx}: cases");
                assert!(
                    record.failing_case.is_none()
                        && record.shrunk.is_none()
                        && record.fault.is_none()
                        && record.counterexamples.is_empty(),
                    "{ctx}: {}",
                    stable(&record)
                );
            }
            Some(e) => {
                assert_eq!(
                    record.status,
                    TestStatus::Failed,
                    "{ctx}: case {} must fail: {}",
                    e.index,
                    stable(&record)
                );
                assert_eq!(record.cases, Some(e.index + 1), "{ctx}: cases");
                let case = record.failing_case.as_ref().expect("failing_case");
                assert_eq!(case.index, e.index, "{ctx}: failing_case.index");
                assert_eq!(
                    Json::Object(case.input.clone()),
                    oracle::tuple(&names, &e.input),
                    "{ctx}: failing_case.input"
                );
                let shrunk = record.shrunk.as_ref().expect("shrunk");
                assert_eq!(
                    Json::Object(shrunk.input.clone()),
                    oracle::tuple(&names, &e.shrunk.values),
                    "{ctx}: shrunk.input"
                );
                assert_eq!(shrunk.shrink_steps, e.shrunk.steps, "{ctx}: shrink_steps");
                assert_eq!(
                    shrunk.shrink_exhausted, e.shrunk.exhausted,
                    "{ctx}: shrink_exhausted"
                );
                let code = model(&e.shrunk.values).expect("the shrunk tuple fails");
                let f = fault_of(&record);
                assert_eq!(f["code"], code, "{ctx}: the fault of the shrunk tuple");
                let own = f["symbol"] == self.identity.as_str()
                    && matches!(
                        f["code"].as_str(),
                        Some("assertion_failed" | "unresolved_hole")
                    );
                if own {
                    let bindings = f["bindings"].as_array().expect("bindings");
                    for (name, v) in names.iter().zip(&e.shrunk.values) {
                        let b = bindings
                            .iter()
                            .find(|b| b["name"] == *name && b["kind"] == "for")
                            .unwrap_or_else(|| panic!("{ctx}: no `for` binding {name}: {f}"));
                        assert_eq!(b["value"], oracle::to_json(v), "{ctx}: binding {name}");
                    }
                }
            }
        }
        record
    }
}

// ===========================================================================
// Order and selection (semantics 7.2-7.3, S42; toolchain 1.7.2, 7.2, T43)
// ===========================================================================

const ORD: &str = r#"module ord

test "B" {
    assert true
}

test "a" {
    assert true
}

test "a b" {
    assert true
}

property "a!"
    for flag in gen.bool
{
    assert flag == flag
}

test "a\"q" {
    assert true
}

test "a\\b" {
    assert true
}

test "\t" {
    assert true
}

test "\u{7f}" {
    assert true
}

test "~" {
    assert true
}

test "é" {
    assert true
}
"#;

const ORD_SUB: &str = r#"module ord.sub

test "0" {
    assert true
}
"#;

const ORD_X: &str = r#"module ord_x

@id("aaa_first")
test "z" {
    assert true
}
"#;

/// The qualified names of the ordering fixture, written out by hand
/// (semantics 1.4: `<module path>."<name>"`, the name escaped as a `TEXT`
/// literal is, S40: `"`, `\`, TAB, and DEL escaped, `é` literal), in the
/// order the runner must use: the byte order of these strings, in which a
/// TAB (`\t`) and a DEL (`\u{7f}`) sort by their backslash, before `a`,
/// while as raw text DEL would sort after `~`.
const RUN_ORDER: [&str; 12] = [
    "ord.\"B\"",
    "ord.\"\\t\"",
    "ord.\"\\u{7f}\"",
    "ord.\"a b\"",
    "ord.\"a!\"",
    "ord.\"a\"",
    "ord.\"a\\\"q\"",
    "ord.\"a\\\\b\"",
    "ord.\"~\"",
    "ord.\"é\"",
    "ord.sub.\"0\"",
    "ord_x.\"z\"",
];

fn ordering() -> Program {
    common::check_files(&[
        ("src/ord.cpl", ORD),
        ("src/ord/sub.cpl", ORD_SUB),
        ("src/ord_x.cpl", ORD_X),
    ])
}

/// semantics 7.3 and toolchain 1.7.2: every selected test and property runs
/// in the byte order of its qualified name as `SymbolPath::display` renders
/// it, tests and properties interleaved, `ord."a b"` and `ord."a!"` before
/// `ord."a"` (the toolchain's own example), escapes compared as escaped,
/// a module's tests before its submodules', and the order is by qualified
/// name, not by identity (`aaa_first` names the last one).
#[test]
fn checked_tests_and_properties_run_in_the_byte_order_of_their_qualified_names() {
    let mut sorted = RUN_ORDER;
    sorted.sort_unstable();
    assert_eq!(sorted, RUN_ORDER, "the expectation is itself in byte order");
    let p = ordering();
    assert_eq!(
        names_of(&p, &select_tests(&p, &TestSelector::all())),
        RUN_ORDER
    );
    let report = run_tests(&p, &TestSelector::all(), &config(3));
    let ran: Vec<&str> = report
        .results
        .iter()
        .map(|r| r.qualified_name.as_str())
        .collect();
    assert_eq!(ran, RUN_ORDER);
    assert_eq!((report.selected, report.tested, report.failed), (12, 12, 0));
    for (i, r) in report.results.iter().enumerate() {
        let kind = if i == 4 {
            TestKind::Property
        } else {
            TestKind::Test
        };
        assert_eq!(r.kind, kind, "{}", r.qualified_name);
    }
    // S42: the identity is the @id when there is one, else the qualified name.
    let symbols: Vec<&str> = report.results.iter().map(|r| r.symbol.as_str()).collect();
    let mut expected: Vec<&str> = RUN_ORDER.to_vec();
    expected[11] = "aaa_first";
    assert_eq!(symbols, expected);
}

/// toolchain 7.2 and 1.7.2: a selector is a module-path prefix compared
/// segment by segment (T43), `--name` the exact name text (the literal's
/// value, not its escaped spelling), `--id` the identity; all must hold.
#[test]
fn selection_by_namespace_name_and_identity_of_checked_tests() {
    let p = ordering();
    let pick = |sel: TestSelector| names_of(&p, &select_tests(&p, &sel));
    assert_eq!(pick(namespace("ord")), RUN_ORDER[..11]);
    assert_eq!(pick(namespace("ord.*")), RUN_ORDER[..11]);
    assert_eq!(pick(namespace("ord.sub.*")), ["ord.sub.\"0\""]);
    assert_eq!(pick(namespace("ord_x")), ["ord_x.\"z\""]);
    for none in ["orde", "ord.su", "ord.sub.deeper", "ordx", "zzz"] {
        assert!(pick(namespace(none)).is_empty(), "{none}");
    }
    let named = |n: &str| pick(selector(None, Some(n), None));
    assert_eq!(named("a\"q"), ["ord.\"a\\\"q\""]);
    assert_eq!(named("a\\b"), ["ord.\"a\\\\b\""]);
    assert_eq!(named("\u{e9}"), ["ord.\"é\""]);
    assert_eq!(named("a!"), ["ord.\"a!\""]);
    assert_eq!(named("z"), ["ord_x.\"z\""]);
    assert_eq!(named("\t"), ["ord.\"\\t\""]);
    assert_eq!(named("\u{7f}"), ["ord.\"\\u{7f}\""]);
    for absent in [
        "a\\\"q",
        "\"a\"",
        "ord.\"a\"",
        "A",
        "b",
        "a  b",
        "e\u{301}",
        "\\t",
    ] {
        assert!(named(absent).is_empty(), "{absent:?}");
    }
    let with_id = |i: &str| pick(selector(None, None, Some(i)));
    assert_eq!(with_id("aaa_first"), ["ord_x.\"z\""]);
    assert!(
        with_id("ord_x.\"z\"").is_empty(),
        "an annotated test's identity is its @id, not its qualified name"
    );
    assert_eq!(with_id("ord.\"a\\\"q\""), ["ord.\"a\\\"q\""]);
    assert_eq!(with_id("ord.\"\\u{7f}\""), ["ord.\"\\u{7f}\""]);
    assert!(pick(selector(Some("ord"), Some("z"), None)).is_empty());
    assert_eq!(
        pick(selector(Some("ord_x"), Some("z"), Some("aaa_first"))),
        ["ord_x.\"z\""]
    );
    let report = run_tests(&p, &namespace("ord.sub"), &config(1));
    assert_eq!((report.selected, report.tested, report.failed), (1, 1, 0));
    assert_eq!(report.results[0].qualified_name, "ord.sub.\"0\"");
}

// ===========================================================================
// Records (semantics S46; toolchain 1.7.2, 7.2, Appendix C)
// ===========================================================================

const SHAPE: &str = r#"module tests.shape

fn half(n: Int) -> Int
    ensures result * 2 == n
{
    n / 2
}

test "passes" {
    assert true
}

test "fails" {
    assert 1 == 2
}

test "finds a counterexample" {
    let _ = half(3)
}

property "always"
    for x in gen.int
{
    assert x == x
}

property "never"
    for x in gen.int
{
    assert false
}

property "half of odd"
    for n in gen.range(1, 9)
{
    let _ = half(n * 2 + 1)
}

property "setup"
    for x in gen.range(2, 1)
{
    assert x == x
}
"#;

fn shape(name: &str) -> String {
    format!("tests.shape.\"{name}\"")
}

/// semantics S46 and toolchain 7.2: a test's record has `symbol`,
/// `qualified_name`, `kind`, `status`, `duration_ms`, and `fault` when it
/// failed; a property's adds `seed`, `property_seed`, `cases`, and when a
/// case failed `failing_case` and `shrunk`; `counterexamples` only when
/// there are some; the keys come in the field order of the wire types
/// (toolchain 7.1, Appendix C), which is the order of 7.2's example.
#[test]
fn records_carry_exactly_the_s46_keys_in_declaration_order() {
    let p = common::check_source(SHAPE);
    let report = run_tests(&p, &TestSelector::all(), &config(5));
    let order: Vec<&str> = report
        .results
        .iter()
        .map(|r| r.qualified_name.as_str())
        .collect();
    let expected_order = [
        "always",
        "fails",
        "finds a counterexample",
        "half of odd",
        "never",
        "passes",
        "setup",
    ]
    .map(shape);
    assert_eq!(order, expected_order);
    const TEST: [&str; 5] = ["symbol", "qualified_name", "kind", "status", "duration_ms"];
    const PROPERTY: [&str; 8] = [
        "symbol",
        "qualified_name",
        "kind",
        "status",
        "duration_ms",
        "seed",
        "property_seed",
        "cases",
    ];
    let keys = |name: &str| record_keys(by_name(&report, &shape(name)));
    assert_eq!(keys("passes"), TEST);
    assert_eq!(keys("fails"), [&TEST[..], &["fault"]].concat());
    assert_eq!(
        keys("finds a counterexample"),
        [&TEST[..], &["fault", "counterexamples"]].concat()
    );
    assert_eq!(keys("always"), PROPERTY);
    assert_eq!(
        keys("never"),
        [&PROPERTY[..], &["failing_case", "shrunk", "fault"]].concat()
    );
    assert_eq!(
        keys("half of odd"),
        [
            &PROPERTY[..],
            &["failing_case", "shrunk", "fault", "counterexamples"]
        ]
        .concat()
    );
    // A fault at setup fails the property with zero cases: there is no
    // failing case and nothing to shrink (stdlib 8.2, toolchain 1.7.2).
    assert_eq!(keys("setup"), [&PROPERTY[..], &["fault"]].concat());
    let never = by_name(&report, &shape("never"));
    assert_eq!(
        keys_in_order(&serde_json::to_string(never.failing_case.as_ref().unwrap()).unwrap()),
        ["index", "input"]
    );
    assert_eq!(
        keys_in_order(&serde_json::to_string(never.shrunk.as_ref().unwrap()).unwrap()),
        ["input", "shrink_steps", "shrink_exhausted"]
    );
    assert_eq!(
        keys_in_order(&serde_json::to_string(&report).unwrap()),
        ["seed", "cases", "selected", "tested", "failed", "results"]
    );
    assert_eq!(
        (
            report.seed,
            report.cases,
            report.selected,
            report.tested,
            report.failed
        ),
        (5, 100, 7, 2, 5)
    );
    // run_tests leaves every receipt to its caller (toolchain 1.7.2).
    assert!(report.results.iter().all(|r| r.receipt.is_none()));
    // Appendix C: a wire type reads back what it wrote.
    let text = serde_json::to_string(&report).unwrap();
    let back: TestReport = serde_json::from_str(&text).expect("the report reads back");
    assert_eq!(back, report);
}

/// The values of the records of [`SHAPE`] (semantics S46, 7.3-7.5).
#[test]
fn record_values_of_tests_and_properties_s46() {
    let p = common::check_source(SHAPE);
    let report = run_tests(&p, &TestSelector::all(), &config(5));
    let always = by_name(&report, &shape("always"));
    assert_eq!(
        (always.status, always.cases, always.seed),
        (TestStatus::Tested, Some(100), Some(5))
    );
    // Every n in [1, 9] fails (half of an odd number), so case 0 fails, and
    // the origin 1 of `gen.range(1, 9)` is the first candidate (stdlib 8.4):
    // it fails too and nothing is below it. One step, unless case 0 drew 1.
    let odd = by_name(&report, &shape("half of odd"));
    assert_eq!(odd.status, TestStatus::Failed);
    assert_eq!(
        (odd.cases, odd.failing_case.as_ref().unwrap().index),
        (Some(1), 0)
    );
    let original = failing_input(odd)["n"].as_i64().expect("an Int");
    assert!((1..=9).contains(&original), "{original}");
    assert_eq!(shrunk_input(odd), json!({"n": 1}));
    let s = odd.shrunk.as_ref().unwrap();
    assert_eq!(
        (s.shrink_steps, s.shrink_exhausted),
        (u32::from(original != 1), false)
    );
    let f = fault_of(odd);
    assert_eq!(f["code"], "contract_violation");
    assert_eq!(f["clause_kind"], "ensures");
    assert_eq!(f["input"], json!({"n": 3}));
    assert_eq!(f["actual"], json!(1));
    let ce = counterexample(odd);
    assert_eq!(ce["symbol"], "tests.shape.half");
    assert_eq!(ce["input"], json!({"n": 3}));
    assert_eq!(ce["actual"], json!(1));
    assert_eq!(ce["found_by"], shape("half of odd").as_str());
    let plain = by_name(&report, &shape("finds a counterexample"));
    let ce = counterexample(plain);
    assert_eq!(ce["found_by"], shape("finds a counterexample").as_str());
    assert!(plain.seed.is_none() && plain.property_seed.is_none() && plain.cases.is_none());
    let setup = by_name(&report, &shape("setup"));
    assert_eq!((setup.status, setup.cases), (TestStatus::Failed, Some(0)));
    assert_eq!(fault_of(setup)["symbol"], "std.gen.range");
    // `--cases 7`: a passing property ran 7 cases, and the report says so.
    let seven = run_tests(
        &p,
        &TestSelector::all(),
        &TestConfig {
            cases: 7,
            ..config(5)
        },
    );
    assert_eq!(seven.cases, 7);
    assert_eq!(by_name(&seven, &shape("always")).cases, Some(7));
    // `--cases 0`: no case runs, so a property is tested with zero cases
    // (every case it ran completed), while a range's setup still runs and
    // fails its property (stdlib 8.2).
    let none = run_tests(
        &p,
        &TestSelector::all(),
        &TestConfig {
            cases: 0,
            ..config(5)
        },
    );
    let never = by_name(&none, &shape("never"));
    assert_eq!((never.status, never.cases), (TestStatus::Tested, Some(0)));
    assert!(never.failing_case.is_none() && never.fault.is_none());
    let setup = by_name(&none, &shape("setup"));
    assert_eq!((setup.status, setup.cases), (TestStatus::Failed, Some(0)));
}

/// semantics S46 and toolchain 1.7.2: every record is one `test_result`
/// diagnostic (category `test`), `info` when tested and `error` when
/// failed, with no span, `symbol` and `qualified_name` the identity, and
/// every other field of the record flattened into the payload. S46 names no
/// facet for it (reading: none, since 9.2 has `facet` only "when a facet is
/// involved").
#[test]
fn every_record_is_one_test_result_diagnostic_s46() {
    let p = common::check_source(SHAPE);
    let report = run_tests(&p, &TestSelector::all(), &config(5));
    for record in &report.results {
        let ctx = &record.qualified_name;
        let d = test_result_diagnostic(record);
        let tested = record.status == TestStatus::Tested;
        assert_eq!(d.code.0, "test_result", "{ctx}");
        assert_eq!(d.category, Category::Test, "{ctx}");
        let severity = if tested {
            Severity::Info
        } else {
            Severity::Error
        };
        assert_eq!(d.severity, severity, "{ctx}");
        assert!(d.span.is_none(), "{ctx}: a test_result has no span");
        assert_eq!(
            d.symbol.as_ref().map(StableId::as_str),
            Some(record.symbol.as_str()),
            "{ctx}"
        );
        assert_eq!(d.qualified_name.as_deref(), Some(ctx.as_str()));
        assert!(d.facet.is_none(), "{ctx}");
        let mut payload = serde_json::to_value(record)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        payload.remove("symbol");
        payload.remove("qualified_name");
        assert_eq!(
            Json::Object(d.data.clone()),
            Json::Object(payload),
            "{ctx}: the payload is the record"
        );
        let j = serde_json::to_value(d.to_json(p.source_map())).unwrap();
        assert_eq!(j["code"], "test_result");
        assert_eq!(j["category"], "test");
        assert_eq!(j["severity"], if tested { "info" } else { "error" });
        assert_eq!(j["status"], if tested { "tested" } else { "failed" });
        assert_eq!(j["symbol"], record.symbol.as_str());
        assert!(j.get("span").is_none());
    }
}

/// The run seed is a decimal `u64` everywhere (toolchain 1.7.2, 8.2), its
/// largest value included: the wire form writes it as an integer.
#[test]
fn the_largest_run_seed_is_written_as_an_exact_integer() {
    let p = common::check_source(SHAPE);
    let report = run_tests(&p, &namespace("tests.shape"), &config(u64::MAX));
    let text = serde_json::to_string(&report).unwrap();
    assert!(
        text.starts_with("{\"seed\":18446744073709551615,"),
        "{text}"
    );
    let always = by_name(&report, &shape("always"));
    assert_eq!(always.seed, Some(u64::MAX));
    assert!(serde_json::to_string(always)
        .unwrap()
        .contains("\"seed\":18446744073709551615,"));
}

// ===========================================================================
// Seeds and identities (semantics S44; stdlib 8.2, S31; toolchain T45)
// ===========================================================================

const SEEDS_SRC: &str = r#"module tests.seeds

property "p"
    for x in gen.int
{
    assert x == x
}

@id("prop_seed_a1")
property "q"
    for x in gen.int
{
    assert x == x
}

property "nl\nq\"b\\t\tu\u{1}e\u{e9}"
    for x in gen.int
{
    assert x == x
}

property "cr\rnul\u{0}"
    for x in gen.int
{
    assert x == x
}
"#;

/// The qualified name of the third property of [`SEEDS_SRC`] as semantics
/// S40 escapes it: LF, `"`, `\`, TAB, and U+0001 escaped, `é` literal.
const ESCAPED: &str = "tests.seeds.\"nl\\nq\\\"b\\\\t\\tu\\u{1}e\u{e9}\"";

/// `(run seed, identity, qualified name, property_seed)`. The digests are
/// blake3 of the eight little-endian bytes of the seed followed by the UTF-8
/// of the identity; they are written out so that a derivation that agrees
/// with itself but not with S44 (big-endian bytes, the unescaped name, the
/// qualified name of an annotated property) cannot pass.
const KNOWN_SEEDS: [(u64, &str, &str, &str); 6] = [
    (
        1,
        "tests.seeds.\"p\"",
        "tests.seeds.\"p\"",
        "82d670e1b7e90d843e01252b17bf25b47cb4db9e681d0e9527b3ccd401a72b9c",
    ),
    (
        0,
        "tests.seeds.\"p\"",
        "tests.seeds.\"p\"",
        "66006de4e204fcbfc55518f000b1c56e52fcf44050800c3f49b293abc1b2db58",
    ),
    (
        u64::MAX,
        "tests.seeds.\"p\"",
        "tests.seeds.\"p\"",
        "f1a051f460f7413421a6d0e8a318fc93c2533c98d183b3b1d7fbee1966574a31",
    ),
    (
        0x0102_0304_0506_0708,
        "tests.seeds.\"p\"",
        "tests.seeds.\"p\"",
        "071480f3666e76576465560eaefffcded112c2402c68f5440d63502ba043a415",
    ),
    (
        1,
        "prop_seed_a1",
        "tests.seeds.\"q\"",
        "f82bfb99e0cd76a5834db9b3804f4403b25643cdfe2a38c8d6c85c1158bde765",
    ),
    (
        7,
        ESCAPED,
        ESCAPED,
        "e1a17ecebef4b9102bfc4371fe1034f01cad2925a9effeba5176e3cef4f696cd",
    ),
];

/// semantics S44, stdlib 8.2, toolchain T45: `property_seed` is
/// `blake3(seed_le(8) ++ identity_utf8)` as 64 lowercase hex digits, the
/// identity being the `@id` value or else the qualified name in its
/// canonically escaped spelling.
#[test]
fn the_property_seed_is_blake3_of_the_le_run_seed_and_the_identity_s44() {
    let p = common::check_source(SEEDS_SRC);
    for (seed, identity, qualified, hex) in KNOWN_SEEDS {
        let mut bytes = seed.to_le_bytes().to_vec();
        bytes.extend_from_slice(identity.as_bytes());
        assert_eq!(blake3::hash(&bytes).to_hex().as_str(), hex, "the table");
        let def = common::def(&p, qualified);
        assert_eq!(p.def(def).stable_id.as_str(), identity);
        let record = run_test(&p, def, &config(seed));
        assert_eq!(record.status, TestStatus::Tested, "{qualified}");
        assert_eq!(record.qualified_name, qualified);
        assert_eq!(record.symbol.as_str(), identity);
        assert_eq!(record.seed, Some(seed));
        assert_eq!(record.property_seed.as_deref(), Some(hex), "{qualified}");
        assert_eq!(seed_hex(&property_seed(seed, identity)), hex);
    }
    // The escaped spelling is hashed, not the name text.
    let unescaped = "tests.seeds.\"nl\nq\"b\\t\tu\u{1}e\u{e9}\"";
    assert_ne!(oracle::seed_hex(7, unescaped), KNOWN_SEEDS[5].3);
    // CR and NUL are spelled `\r` and `\u{0}` (S40: the minimal number of
    // lowercase hex digits) in the identity that is hashed.
    let q = "tests.seeds.\"cr\\rnul\\u{0}\"";
    let record = run_test(&p, common::def(&p, q), &config(9));
    assert_eq!(record.qualified_name, q);
    assert_eq!(
        record.property_seed.as_deref(),
        Some(oracle::seed_hex(9, q).as_str())
    );
}

const MOVED_A: &str = r#"module tests.ida

@id("prop_move_k2")
property "before"
    for x in gen.int
    for xs in gen.list(gen.int)
{
    assert x < 20 or xs.len < 2
}
"#;

const MOVED_B: &str = r#"module tests.idb

test "unrelated" {
    assert true
}

@id("prop_move_k2")
property "after the move"
    for x in gen.int
    for xs in gen.list(gen.int)
{
    assert x < 20 or xs.len < 2
}
"#;

const MOVED_C: &str = r#"module tests.ida

property "before"
    for x in gen.int
    for xs in gen.list(gen.int)
{
    assert x < 20 or xs.len < 2
}
"#;

/// stdlib S31: a property that carries an `@id` keeps its case sequence
/// under the same run seed across renames and moves; one without is keyed
/// by its qualified name.
#[test]
fn an_id_keeps_the_cases_across_a_rename_and_a_move_s31() {
    let a = common::check_source(MOVED_A);
    let b = common::check_source(MOVED_B);
    let c = common::check_source(MOVED_C);
    let binders = [("x", G::Int), ("xs", list(G::Int))];
    let model =
        |t: &[V]| (*t[0].int() >= big(20) && t[1].length() >= 2).then_some("assertion_failed");
    let pa = Prop::new(&a, "tests.ida.\"before\"", "prop_move_k2", &binders);
    let pb = Prop::new(&b, "tests.idb.\"after the move\"", "prop_move_k2", &binders);
    let pc = Prop::new(&c, "tests.ida.\"before\"", "tests.ida.\"before\"", &binders);
    let mut failures = 0;
    for seed in [1, 2, 3, 4] {
        let ra = pa.check(&config(seed), &model);
        let rb = pb.check(&config(seed), &model);
        assert_eq!(case_view(&ra), case_view(&rb), "seed {seed}");
        let rc = pc.check(&config(seed), &model);
        assert_ne!(rc.property_seed, ra.property_seed);
        failures += usize::from(ra.status == TestStatus::Failed);
    }
    assert!(
        failures > 0,
        "some seed must fail for the comparison to mean something"
    );
}

// ===========================================================================
// Every generator form against the oracle (stdlib 8.1-8.4, S44-S45)
// ===========================================================================

const GENS: &str = r#"module tests.gens

fn limit() -> Int {
    3
}

property "int at least fifty"
    for x in gen.int
{
    assert x < 50
}

property "int at most minus fifty"
    for x in gen.int
{
    assert x > -50
}

property "int large"
    for x in gen.int
{
    assert x < 1000000
}

property "int large negative"
    for x in gen.int
{
    assert x > -1000000
}

property "positive seven"
    for n in gen.positive_int
{
    assert n < 7
}

property "bool and int"
    for b in gen.bool
    for x in gen.int
{
    assert not b or x < 3
}

property "text three"
    for t in gen.text
{
    assert t.len < 3
}

property "text z"
    for t in gen.text
{
    assert not std.text.contains(t, "z")
}

property "list four"
    for xs in gen.list(gen.int)
{
    assert xs.len < 4
}

property "list seven"
    for xs in gen.list(gen.int)
{
    assert not xs.contains(7)
}

property "option five"
    for o in gen.option(gen.int)
{
    assert o.unwrap_or(0) < 5
}

property "nested options"
    for o in gen.option(gen.option(gen.bool))
{
    assert o != Some(Some(true))
}

property "range seven"
    for r in gen.range(-20, 20)
{
    assert r < 7
}

property "negative range"
    for r in gen.range(-20, -2)
{
    assert r > -9
}

property "high range"
    for r in gen.range(10, 1000)
{
    assert r < 500
}

property "range from a function"
    for r in gen.range(limit() - 6, limit())
{
    assert r < 2
}

property "nested lists"
    for xss in gen.list(gen.list(gen.int))
{
    assert flatten(xss).len < 3
}

property "list of options"
    for xs in gen.list(gen.option(gen.positive_int))
{
    assert xs.len < 2
}

property "option of list"
    for o in gen.option(gen.list(gen.bool))
{
    assert o.unwrap_or([]).len < 2
}

property "list of ranges"
    for xs in gen.list(gen.range(1, 3))
{
    assert xs.len < 3
}

property "list of texts"
    for ts in gen.list(gen.text)
{
    assert ts.len < 2
}

property "sum"
    for a in gen.int
    for b in gen.int
{
    assert a + b < 30
}

property "three binders"
    for xs in gen.list(gen.bool)
    for n in gen.positive_int
    for t in gen.text
{
    assert xs.len < n or t.len < 2
}

property "wide range"
    for r in gen.range(0, 100000000000000000000000)
{
    assert r < 5
}

property "wide negative range"
    for r in gen.range(-100000000000000000000000, -1)
{
    assert r > -5
}

property "two to the 64 values"
    for r in gen.range(0, 18446744073709551615)
{
    assert r < 5
}

property "one less than two to the 64 values"
    for r in gen.range(0, 18446744073709551614)
{
    assert r < 5
}

property "one value range"
    for a in gen.range(5, 5)
    for x in gen.int
{
    assert x < 3
}

property "option of range"
    for o in gen.option(gen.range(-5, 5))
{
    assert o.unwrap_or(0) < 3
}

property "list of positives"
    for ns in gen.list(gen.positive_int)
{
    assert not ns.contains(3)
}

property "list of a constant"
    for xs in gen.list(gen.range(7, 7))
{
    assert xs.len < 5
}

property "qualified spelling"
    for xs in std.gen.list(std.gen.range(0, 9))
{
    assert xs.len < 3
}
"#;

const SEEDS: [u64; 5] = [1, 2, 3, 77, 0xfeed_beef];

/// Check the property `name` of [`GENS`] against the oracle for every seed
/// of [`SEEDS`]; at least one seed must fail (otherwise nothing shrank).
fn gens_case(name: &str, binders: &[(&str, G)], model: Model<'_>) -> Vec<TestCaseRecord> {
    let p = common::check_source(GENS);
    let qualified = format!("tests.gens.\"{name}\"");
    let prop = Prop::new(&p, &qualified, &qualified, binders);
    let records: Vec<TestCaseRecord> = SEEDS
        .iter()
        .map(|&seed| prop.check(&config(seed), model))
        .collect();
    assert!(
        records.iter().any(|r| r.status == TestStatus::Failed),
        "{qualified}: no seed fails, so nothing shrank"
    );
    records
}

#[test]
fn gen_int_shrinks_down_to_the_first_failing_value() {
    gens_case("int at least fifty", &[("x", G::Int)], &|t: &[V]| {
        (*t[0].int() >= big(50)).then_some("assertion_failed")
    });
}

#[test]
fn gen_int_shrinks_up_to_the_first_failing_negative_value() {
    gens_case("int at most minus fifty", &[("x", G::Int)], &|t: &[V]| {
        (*t[0].int() <= big(-50)).then_some("assertion_failed")
    });
}

/// From size 8 `gen.int` draws values of 64 to `64 + 8s` bits (stdlib 8.3).
/// Shrinking one toward the bound costs two steps per halving (the
/// candidate 0 passes, `v / 2` fails), so for a long one the default budget
/// of 1000 steps runs out first (`shrink_exhausted`, S45).
#[test]
fn gen_int_large_values_are_exact_and_exhaust_the_budget() {
    let up = gens_case("int large", &[("x", G::Int)], &|t: &[V]| {
        (*t[0].int() >= big(1_000_000)).then_some("assertion_failed")
    });
    let down = gens_case("int large negative", &[("x", G::Int)], &|t: &[V]| {
        (*t[0].int() <= big(-1_000_000)).then_some("assertion_failed")
    });
    for records in [up, down] {
        // A uniform draw is at most 100 in magnitude, so every failure is a
        // large draw of more than 64 bits, and some search runs out of steps.
        for r in records.iter().filter(|r| r.status == TestStatus::Failed) {
            let x = failing_input(r)["x"].to_string();
            assert!(x.trim_start_matches('-').len() >= 19, "{x}");
        }
        assert!(records
            .iter()
            .any(|r| r.shrunk.as_ref().is_some_and(|s| s.shrink_exhausted)));
    }
}

#[test]
fn gen_positive_int_shrinks_toward_one() {
    gens_case("positive seven", &[("n", G::Pos)], &|t: &[V]| {
        (*t[0].int() >= big(7)).then_some("assertion_failed")
    });
}

#[test]
fn gen_bool_and_gen_int_shrink_coordinate_wise() {
    gens_case(
        "bool and int",
        &[("b", G::Bool), ("x", G::Int)],
        &|t: &[V]| (t[0].flag() && *t[1].int() >= big(3)).then_some("assertion_failed"),
    );
}

#[test]
fn gen_text_shrinks_by_halves_tails_and_the_letter_a() {
    gens_case("text three", &[("t", G::Text)], &|t: &[V]| {
        (t[0].length() >= 3).then_some("assertion_failed")
    });
    let records = gens_case("text z", &[("t", G::Text)], &|t: &[V]| {
        t[0].text().contains('z').then_some("assertion_failed")
    });
    // Shrinking works on scalars, not bytes: some original holds a scalar
    // outside ASCII, and every shrunk text is the minimum "z".
    assert!(records
        .iter()
        .filter_map(|r| r.failing_case.as_ref())
        .any(|c| c.input["t"].as_str().is_some_and(|t| !t.is_ascii())));
    for r in &records {
        assert_eq!(shrunk_input(r), json!({"t": "z"}));
    }
}

#[test]
fn gen_list_shrinks_by_halves_removals_then_elements() {
    gens_case("list four", &[("xs", list(G::Int))], &|t: &[V]| {
        (t[0].length() >= 4).then_some("assertion_failed")
    });
    gens_case("list seven", &[("xs", list(G::Int))], &|t: &[V]| {
        t[0].items()
            .contains(&V::Int(big(7)))
            .then_some("assertion_failed")
    });
}

#[test]
fn gen_option_shrinks_to_none_first_then_its_payload() {
    gens_case("option five", &[("o", opt(G::Int))], &|t: &[V]| {
        t[0].some()
            .is_some_and(|x| *x.int() >= big(5))
            .then_some("assertion_failed")
    });
    gens_case(
        "nested options",
        &[("o", opt(opt(G::Bool)))],
        &|t: &[V]| {
            (t[0] == V::Some(Box::new(V::Some(Box::new(V::Bool(true))))))
                .then_some("assertion_failed")
        },
    );
}

#[test]
fn gen_range_shrinks_toward_its_origin() {
    gens_case("range seven", &[("r", range(-20, 20))], &|t: &[V]| {
        (*t[0].int() >= big(7)).then_some("assertion_failed")
    });
    // The origin of [-20, -2] is its upper bound, -2.
    gens_case("negative range", &[("r", range(-20, -2))], &|t: &[V]| {
        (*t[0].int() <= big(-9)).then_some("assertion_failed")
    });
    // The origin of [10, 1000] is its lower bound, 10.
    gens_case("high range", &[("r", range(10, 1000))], &|t: &[V]| {
        (*t[0].int() >= big(500)).then_some("assertion_failed")
    });
}

/// stdlib S30: a range is a contract expression evaluated once before case
/// 0, and it may call pure functions: `gen.range(limit() - 6, limit())` is
/// `gen.range(-3, 3)`.
#[test]
fn gen_range_bounds_may_call_pure_functions() {
    gens_case(
        "range from a function",
        &[("r", range(-3, 3))],
        &|t: &[V]| (*t[0].int() >= big(2)).then_some("assertion_failed"),
    );
}

/// stdlib 8.3: `below(k)` for `k > 2^64` draws `ceil(b / 32)` words and
/// keeps the low `b` bits; a value beyond 64 bits is exact in value JSON.
#[test]
fn gen_range_wider_than_64_bits_draws_and_shrinks_exactly() {
    gens_case(
        "wide range",
        &[("r", range_big("0", "100000000000000000000000"))],
        &|t: &[V]| (*t[0].int() >= big(5)).then_some("assertion_failed"),
    );
    gens_case(
        "wide negative range",
        &[("r", range_big("-100000000000000000000000", "-1"))],
        &|t: &[V]| (*t[0].int() <= big(-5)).then_some("assertion_failed"),
    );
    // `below(2^64)` keeps all 64 bits of two words (accepting every draw);
    // `below(2^64 - 1)` keeps them too and redraws only `2^64 - 1` itself.
    gens_case(
        "two to the 64 values",
        &[("r", range_big("0", "18446744073709551615"))],
        &|t: &[V]| (*t[0].int() >= big(5)).then_some("assertion_failed"),
    );
    gens_case(
        "one less than two to the 64 values",
        &[("r", range_big("0", "18446744073709551614"))],
        &|t: &[V]| (*t[0].int() >= big(5)).then_some("assertion_failed"),
    );
}

/// stdlib 8.3: `below(1)` draws nothing, so a one-value range consumes no
/// word and the next clause draws exactly what the oracle predicts.
#[test]
fn a_one_value_range_draws_nothing() {
    gens_case(
        "one value range",
        &[("a", range(5, 5)), ("x", G::Int)],
        &|t: &[V]| (*t[1].int() >= big(3)).then_some("assertion_failed"),
    );
}

#[test]
fn nested_generators_draw_and_shrink_exactly() {
    gens_case(
        "nested lists",
        &[("xss", list(list(G::Int)))],
        &|t: &[V]| {
            let total: usize = t[0].items().iter().map(V::length).sum();
            (total >= 3).then_some("assertion_failed")
        },
    );
    gens_case(
        "list of options",
        &[("xs", list(opt(G::Pos)))],
        &|t: &[V]| (t[0].length() >= 2).then_some("assertion_failed"),
    );
    gens_case(
        "option of list",
        &[("o", opt(list(G::Bool)))],
        &|t: &[V]| {
            t[0].some()
                .is_some_and(|l| l.length() >= 2)
                .then_some("assertion_failed")
        },
    );
    gens_case(
        "list of ranges",
        &[("xs", list(range(1, 3)))],
        &|t: &[V]| (t[0].length() >= 3).then_some("assertion_failed"),
    );
    gens_case("list of texts", &[("ts", list(G::Text))], &|t: &[V]| {
        (t[0].length() >= 2).then_some("assertion_failed")
    });
}

#[test]
fn generators_nest_inside_options_and_lists_of_positives() {
    gens_case(
        "option of range",
        &[("o", opt(range(-5, 5)))],
        &|t: &[V]| {
            t[0].some()
                .is_some_and(|x| *x.int() >= big(3))
                .then_some("assertion_failed")
        },
    );
    // A `Positive` element shrinks toward 1, never below (stdlib 8.4).
    gens_case(
        "list of positives",
        &[("ns", list(G::Pos))],
        &|t: &[V]| {
            t[0].items()
                .contains(&V::Int(big(3)))
                .then_some("assertion_failed")
        },
    );
}

/// The elements of `gen.list(gen.range(7, 7))` draw nothing and have no
/// candidate (the origin is 7 itself), so only the length shrinks.
#[test]
fn a_list_of_a_constant_shrinks_only_its_length() {
    gens_case(
        "list of a constant",
        &[("xs", list(range(7, 7)))],
        &|t: &[V]| (t[0].length() >= 5).then_some("assertion_failed"),
    );
    let p = common::check_source(GENS);
    let record = run_test(
        &p,
        common::def(&p, "tests.gens.\"list of a constant\""),
        &config(1),
    );
    assert_eq!(shrunk_input(&record), json!({"xs": [7, 7, 7, 7, 7]}));
}

#[test]
fn two_and_three_binders_shrink_coordinate_wise_with_restarts() {
    gens_case("sum", &[("a", G::Int), ("b", G::Int)], &|t: &[V]| {
        (t[0].int() + t[1].int() >= big(30)).then_some("assertion_failed")
    });
    gens_case(
        "three binders",
        &[("xs", list(G::Bool)), ("n", G::Pos), ("t", G::Text)],
        &|t: &[V]| {
            (BigInt::from(t[0].length()) >= *t[1].int() && t[2].length() >= 2)
                .then_some("assertion_failed")
        },
    );
}

/// stdlib 8.1: `std.gen.<name>` is the same generator as `gen.<name>`.
#[test]
fn the_qualified_spelling_of_a_generator_is_the_same_generator() {
    gens_case(
        "qualified spelling",
        &[("xs", list(range(0, 9)))],
        &|t: &[V]| (t[0].length() >= 3).then_some("assertion_failed"),
    );
}

const NAMES: &str = r#"module tests.names

fn n() -> Int {
    3
}

property "binders named like prelude names"
    for gen in gen.int
    for flatten in gen.list(gen.int)
{
    assert gen < 10 or flatten.len < 2
}

property "a binder named like a function"
    for n in gen.range(0, n())
{
    assert n < 2
}
"#;

/// S43 (scope 7 of 2.1) and S8: `for` binders are visible in the body and in
/// no range, so a range still resolves the module alias `gen` and a module
/// function `n` that binders of those names hide in the body (a local may
/// hide module-level and universe names, with no diagnostic).
#[test]
fn ranges_resolve_the_names_binders_hide_in_the_body_s43() {
    let result = common::check_source_result(NAMES);
    assert!(result.diagnostics.is_empty(), "{}", common::render(&result));
    let p = common::checked(result);
    let q = "tests.names.\"binders named like prelude names\"";
    let binders = [("gen", G::Int), ("flatten", list(G::Int))];
    let prop = Prop::new(&p, q, q, &binders);
    let model =
        |t: &[V]| (*t[0].int() >= big(10) && t[1].length() >= 2).then_some("assertion_failed");
    let failures = SEEDS
        .iter()
        .filter(|&&seed| prop.check(&config(seed), &model).status == TestStatus::Failed)
        .count();
    assert!(failures > 0);
    let q = "tests.names.\"a binder named like a function\"";
    let binders = [("n", range(0, 3))];
    let prop = Prop::new(&p, q, q, &binders);
    for seed in SEEDS {
        let record = prop.check(&config(seed), &|t: &[V]| {
            (*t[0].int() >= big(2)).then_some("assertion_failed")
        });
        assert_eq!(shrunk_input(&record), json!({"n": 2}));
    }
}

const LIMITS: &str = r#"module tests.limits

pub fn limit() -> Int {
    4
}
"#;

const USES: &str = r#"module tests.uses

use tests.limits.{limit}

type Positive = Int where self > 0

fn total(xs: List<Positive>, o: Option<Positive>) -> Int {
    flatten([xs]).len + o.unwrap_or(1)
}

property "imported bound"
    for r in gen.range(0, limit())
{
    assert r < 3
}

property "qualified bound"
    for r in gen.range(0, tests.limits.limit() / 0)
{
    assert r < 3
}

property "structural positives"
    for xs in gen.list(gen.positive_int)
    for o in gen.option(gen.positive_int)
{
    assert total(xs, o) < 6
}
"#;

/// A range resolves names as any contract expression of its module does:
/// an imported function and a fully qualified one (semantics 2.1, 2.4), and
/// its clause text keeps the spelling as written. A drawn `Positive`
/// (`std.gen.Positive`) inside a list or an option is the module's own
/// `Positive` (stdlib S29, semantics S55), so the binders pass straight to
/// a function declared over the module's type.
#[test]
fn ranges_see_imports_and_qualified_names_and_draws_are_structural() {
    let p = common::check_files(&[
        ("src/tests/limits.cpl", LIMITS),
        ("src/tests/uses.cpl", USES),
    ]);
    let q = "tests.uses.\"imported bound\"";
    let binders = [("r", range(0, 4))];
    let prop = Prop::new(&p, q, q, &binders);
    for seed in SEEDS {
        let record = prop.check(&config(seed), &|t: &[V]| {
            (*t[0].int() >= big(3)).then_some("assertion_failed")
        });
        assert_eq!(shrunk_input(&record), json!({"r": 3}));
    }
    let f = setup_fault(&p, "tests.uses.\"qualified bound\"");
    assert_eq!(f["code"], "division_by_zero");
    assert_eq!(f["clause"], "gen.range(0, tests.limits.limit() / 0)");
    assert_eq!(f["input"], json!({"a": 4, "b": 0}));
    let q = "tests.uses.\"structural positives\"";
    let binders = [("xs", list(G::Pos)), ("o", opt(G::Pos))];
    let prop = Prop::new(&p, q, q, &binders);
    let model = |t: &[V]| {
        let extra = t[1].some().map_or(big(1), |x| x.int().clone());
        (BigInt::from(t[0].length()) + extra >= big(6)).then_some("assertion_failed")
    };
    let failures = SEEDS
        .iter()
        .filter(|&&seed| prop.check(&config(seed), &model).status == TestStatus::Failed)
        .count();
    assert!(failures > 0);
}

// ===========================================================================
// Sizes, case 0, and the shrink budget (stdlib 8.2, 8.4; semantics S45)
// ===========================================================================

const SIZES: &str = r#"module tests.sizes

property "case zero"
    for xs in gen.list(gen.int)
    for t in gen.text
    for x in gen.int
    for n in gen.positive_int
    for o in gen.option(gen.int)
    for r in gen.range(-3, 3)
    for b in gen.bool
{
    assert false
}

property "length one hundred"
    for xs in gen.list(gen.bool)
{
    assert xs.len < 100
}

property "never longer than one hundred"
    for xs in gen.list(gen.bool)
    for t in gen.text
{
    assert xs.len <= 100 and t.len <= 100
}

property "empty list at case zero"
    for xs in gen.list(gen.int)
{
    assert false
}

property "empty text at case zero"
    for t in gen.text
{
    assert false
}

property "fixed and flag"
    for x in gen.range(4, 4)
    for b in gen.bool
{
    assert false
}
"#;

fn sizes(name: &str) -> String {
    format!("tests.sizes.\"{name}\"")
}

fn always_fails(_: &[V]) -> Option<&'static str> {
    Some("assertion_failed")
}

/// stdlib 8.2: case 0 runs at size 0, where a list and a text are empty
/// (`below(1)` is 0), `gen.int` gives -1, 0, or 1, and `gen.positive_int`
/// 1 or 2. With `max_shrinks` 0 the reported tuple is case 0 itself.
#[test]
fn case_zero_is_drawn_at_size_zero() {
    // S67: `for` binders are never diagnosed as unused.
    let result = common::check_source_result(SIZES);
    assert!(result.diagnostics.is_empty(), "{}", common::render(&result));
    let p = common::checked(result);
    let binders = [
        ("xs", list(G::Int)),
        ("t", G::Text),
        ("x", G::Int),
        ("n", G::Pos),
        ("o", opt(G::Int)),
        ("r", range(-3, 3)),
        ("b", G::Bool),
    ];
    let q = sizes("case zero");
    let prop = Prop::new(&p, &q, &q, &binders);
    for seed in 0..24 {
        let record = prop.check(
            &TestConfig {
                max_shrinks: 0,
                ..config(seed)
            },
            &always_fails,
        );
        assert_eq!(record.failing_case.as_ref().unwrap().index, 0);
        let input = failing_input(&record);
        assert_eq!(input["xs"], json!([]), "seed {seed}");
        assert_eq!(input["t"], json!(""), "seed {seed}");
        let x = input["x"].as_i64().unwrap();
        assert!((-1..=1).contains(&x), "seed {seed}: {x}");
        let n = input["n"].as_i64().unwrap();
        assert!((1..=2).contains(&n), "seed {seed}: {n}");
        assert_eq!(shrunk_input(&record), input);
        // With the default budget every coordinate reaches its minimum.
        let record = prop.check(&config(seed), &always_fails);
        assert_eq!(
            shrunk_input(&record),
            json!({"xs": [], "t": "", "x": 0, "n": 1,
                   "o": {"$type": "Option", "$variant": "None"}, "r": 0, "b": false}),
            "seed {seed}"
        );
    }
}

/// stdlib 8.2: case `i` runs at size `min(i, 100)`: no list is ever longer
/// than 100, and a list of exactly 100 needs a case of index 100 or more.
#[test]
fn the_size_stops_growing_at_one_hundred() {
    let p = common::check_source(SIZES);
    let cases = TestConfig {
        cases: 400,
        ..config(1)
    };
    let q = sizes("never longer than one hundred");
    let binders = [("xs", list(G::Bool)), ("t", G::Text)];
    let prop = Prop::new(&p, &q, &q, &binders);
    let record = prop.check(&cases, &|t: &[V]| {
        (t[0].length() > 100 || t[1].length() > 100).then_some("assertion_failed")
    });
    assert_eq!(
        (record.status, record.cases),
        (TestStatus::Tested, Some(400))
    );
    let q = sizes("length one hundred");
    let binders = [("xs", list(G::Bool))];
    let prop = Prop::new(&p, &q, &q, &binders);
    let mut failures = 0;
    for seed in 1..=6 {
        let record = prop.check(
            &TestConfig {
                cases: 400,
                ..config(seed)
            },
            &|t: &[V]| (t[0].length() >= 100).then_some("assertion_failed"),
        );
        if record.status == TestStatus::Failed {
            failures += 1;
            assert!(record.failing_case.as_ref().unwrap().index >= 100);
            assert_eq!(failing_input(&record)["xs"].as_array().unwrap().len(), 100);
        }
    }
    assert!(
        failures > 0,
        "some seed draws a list of 100 within 400 cases"
    );
}

/// S45, stdlib 8.4: with `max_shrinks` 0, `shrink_exhausted` is true exactly
/// when the failing value has a candidate; an empty list, an empty text,
/// `false`, and the only value of a one-value range have none.
#[test]
fn a_budget_of_zero_is_exhausted_exactly_when_a_candidate_exists() {
    let p = common::check_source(SIZES);
    let zero = |seed: u64| TestConfig {
        max_shrinks: 0,
        ..config(seed)
    };
    for (name, binders) in [
        ("empty list at case zero", vec![("xs", list(G::Int))]),
        ("empty text at case zero", vec![("t", G::Text)]),
    ] {
        let q = sizes(name);
        let prop = Prop::new(&p, &q, &q, &binders);
        for seed in 0..8 {
            for budget in [zero(seed), config(seed)] {
                let record = prop.check(&budget, &always_fails);
                let s = record.shrunk.as_ref().unwrap();
                assert_eq!((s.shrink_steps, s.shrink_exhausted), (0, false), "{q}");
                assert_eq!(record.failing_case.as_ref().unwrap().index, 0);
            }
        }
    }
    let q = sizes("fixed and flag");
    let binders = [("x", range(4, 4)), ("b", G::Bool)];
    let prop = Prop::new(&p, &q, &q, &binders);
    let mut seen = [false, false];
    for seed in 0..24 {
        let record = prop.check(&zero(seed), &always_fails);
        let flag = failing_input(&record)["b"].as_bool().unwrap();
        let s = record.shrunk.as_ref().unwrap();
        assert_eq!(
            (s.shrink_steps, s.shrink_exhausted),
            (0, flag),
            "seed {seed}"
        );
        seen[usize::from(flag)] = true;
        // The full search: `true` shrinks to `false` in one step, after
        // which nothing has a candidate, so a budget of exactly that one
        // step is not exhausted (the last allowed step ended the search).
        for budget in [
            config(seed),
            TestConfig {
                max_shrinks: 1,
                ..config(seed)
            },
        ] {
            let record = prop.check(&budget, &always_fails);
            let s = record.shrunk.as_ref().unwrap();
            assert_eq!(
                (s.shrink_steps, s.shrink_exhausted),
                (u32::from(flag), false)
            );
            assert_eq!(shrunk_input(&record), json!({"x": 4, "b": false}));
        }
    }
    assert_eq!(seen, [true, true], "both draws of case 0 occur");
}

/// Sweep `max_shrinks` from 0 to one past the full search of property
/// `name` of [`GENS`]: `shrink_steps` is `min(k, n)`, `shrink_exhausted` is
/// `k < n` (true while another rerun was wanted, false when the last
/// allowed step also ended the search), the reported tuple is the one
/// reached and the fault the one it raised. The sweep uses the first
/// `wanted` run seeds whose full search (per the oracle) takes from 2 to
/// 200 steps within the default budget.
fn budget_sweep(name: &str, binders: &[(&str, G)], model: Model<'_>, wanted: usize) {
    let p = common::check_source(GENS);
    let q = format!("tests.gens.\"{name}\"");
    let prop = Prop::new(&p, &q, &q, binders);
    let seeds: Vec<(u64, oracle::Expected)> = (1..500)
        .filter_map(|seed| prop.expect(&config(seed), model).map(|e| (seed, e)))
        .filter(|(_, e)| !e.shrunk.exhausted && (2..=200).contains(&e.shrunk.steps))
        .take(wanted)
        .collect();
    assert_eq!(seeds.len(), wanted, "{q}: too few seeds to sweep");
    for (seed, full) in seeds {
        let n = full.shrunk.steps;
        for k in 0..=n + 1 {
            let record = prop.check(
                &TestConfig {
                    max_shrinks: k,
                    ..config(seed)
                },
                model,
            );
            let s = record.shrunk.as_ref().unwrap();
            assert_eq!(s.shrink_steps, k.min(n), "{q}: seed {seed}, budget {k}");
            assert_eq!(s.shrink_exhausted, k < n, "{q}: seed {seed}, budget {k}");
            if k == 0 {
                assert_eq!(shrunk_input(&record), failing_input(&record));
            }
            if k >= n {
                assert_eq!(
                    shrunk_input(&record),
                    oracle::tuple(&prop.names(), &full.shrunk.values)
                );
            }
        }
    }
}

#[test]
fn every_shrink_budget_from_zero_past_the_full_search_s45() {
    budget_sweep(
        "int at least fifty",
        &[("x", G::Int)],
        &|t: &[V]| (*t[0].int() >= big(50)).then_some("assertion_failed"),
        3,
    );
    budget_sweep(
        "sum",
        &[("a", G::Int), ("b", G::Int)],
        &|t: &[V]| (t[0].int() + t[1].int() >= big(30)).then_some("assertion_failed"),
        2,
    );
    budget_sweep(
        "list four",
        &[("xs", list(G::Int))],
        &|t: &[V]| (t[0].length() >= 4).then_some("assertion_failed"),
        2,
    );
}

// ===========================================================================
// The same-code rule and faults inside shrinking (S45, S46; stdlib S32)
// ===========================================================================

const CODES: &str = r#"module tests.codes

fn guard(n: Int) -> Int
    requires n != 3
    ensures result < 5
{
    n
}

property "guarded"
    for n in gen.range(0, 40)
{
    let _ = guard(n)
}
"#;

/// S45 compares fault *codes*: a `requires` and an `ensures` violation are
/// both `contract_violation`, so shrinking an `ensures` failure may accept
/// a candidate that violates `requires` (and the reverse), and the record
/// reports the fault of the shrunk tuple: a counterexample only when that
/// fault is the `ensures` violation (S46: "a requires violation inside a
/// test is not a counterexample against the callee"), with `caller` the
/// property's qualified name for the `requires` one (5.2).
#[test]
fn a_requires_violation_may_replace_an_ensures_violation_while_shrinking() {
    let p = common::check_source(CODES);
    let q = "tests.codes.\"guarded\"";
    let binders = [("n", range(0, 40))];
    let prop = Prop::new(&p, q, q, &binders);
    let model = |t: &[V]| {
        let n = t[0].int();
        (*n == big(3) || *n >= big(5)).then_some("contract_violation")
    };
    let mut ends = [false, false];
    for seed in 0..40 {
        let record = prop.check(&config(seed), &model);
        let original = failing_input(&record)["n"].as_i64().unwrap();
        let shrunk = shrunk_input(&record)["n"].as_i64().unwrap();
        let f = fault_of(&record);
        assert_eq!(f["code"], "contract_violation");
        assert_eq!(f["symbol"], "tests.codes.guard");
        if shrunk == 3 {
            assert_eq!(f["clause_kind"], "requires", "seed {seed}");
            assert_eq!(f["clause"], "n != 3");
            assert_eq!(f["attributed_to"], "caller");
            assert_eq!(f["caller"], q);
            assert_eq!(f["input"], json!({"n": 3}));
            assert!(record.counterexamples.is_empty(), "seed {seed}");
            if original != 3 {
                ends[0] = true;
            }
        } else {
            assert_eq!(shrunk, 5, "seed {seed}");
            assert_eq!(f["clause_kind"], "ensures");
            assert_eq!(f["clause"], "result < 5");
            let ce = counterexample(&record);
            assert_eq!(ce["input"], json!({"n": 5}));
            assert_eq!(ce["actual"], json!(5));
            assert_eq!(ce["found_by"], q);
            ends[1] = true;
        }
    }
    assert_eq!(
        ends,
        [true, true],
        "an ensures failure shrinks into a requires one for some seed and stays one for another"
    );
}

const LOOPS: &str = r#"module tests.loops

fn count_up(n: Int) -> Int {
    var i = 0
    while i < n
        invariant i < 3
    {
        i = i + 1
    }
    i
}

property "counts"
    for n in gen.range(0, 10)
{
    assert count_up(n) == n
}

@id("prop_count_c4")
property "counts again"
    for n in gen.range(0, 10)
{
    assert count_up(n) == n
}

property "own loop"
    for n in gen.range(0, 6)
{
    var i = 0
    while i < n
        invariant i < 2
    {
        i = i + 1
    }
}
"#;

/// S46: an invariant violation in a callee, found by a property, is a
/// counterexample at the shrunk tuple, with `iteration` and `actual =
/// {bindings}` (5.3) as the violation raised them.
#[test]
fn a_property_shrinks_to_the_smallest_invariant_counterexample() {
    let p = common::check_source(LOOPS);
    let q = "tests.loops.\"counts\"";
    let binders = [("n", range(0, 10))];
    let prop = Prop::new(&p, q, q, &binders);
    let model = |t: &[V]| (*t[0].int() >= big(3)).then_some("contract_violation");
    let mut failures = 0;
    for seed in [1, 2, 3, 4] {
        let record = prop.check(&config(seed), &model);
        if record.status == TestStatus::Tested {
            continue;
        }
        failures += 1;
        assert_eq!(shrunk_input(&record), json!({"n": 3}));
        let f = fault_of(&record);
        assert_eq!(f["clause_kind"], "invariant");
        assert_eq!(f["attributed_to"], "body");
        assert_eq!(f["symbol"], "tests.loops.count_up");
        assert_eq!(f["facet"], "contract");
        assert_eq!(f["iteration"], json!(3));
        let ce = counterexample(&record);
        assert_eq!(ce["code"], "contract_counterexample");
        assert_eq!(ce["symbol"], "tests.loops.count_up");
        assert_eq!(ce["facet"], "body");
        assert_eq!(ce["clause"], "i < 3");
        assert_eq!(ce["clause_kind"], "invariant");
        assert_eq!(ce["iteration"], json!(3));
        assert_eq!(ce["input"], json!({"n": 3}));
        assert_eq!(ce["actual"], json!({"bindings": {"i": 3, "n": 3}}));
        assert_eq!(ce["status"], "failed");
        assert_eq!(ce["found_by"], q);
        assert_eq!(
            ce["span"],
            common::location_of(LOOPS, "src/tests/loops.cpl", "invariant i < 3")
        );
    }
    assert!(failures > 0);
}

/// S46: `found_by` is the finding property's identity, its `@id` when it
/// has one. A property's own loop invariant is not a counterexample (the
/// violation is the property's own), and the loop of a property has no
/// `input` (5.3), its `bindings` holding the `for` binder.
#[test]
fn found_by_is_the_identity_and_a_propertys_own_invariant_is_no_counterexample() {
    let p = common::check_source(LOOPS);
    let binders = [("n", range(0, 10))];
    let prop = Prop::new(
        &p,
        "tests.loops.\"counts again\"",
        "prop_count_c4",
        &binders,
    );
    let model = |t: &[V]| (*t[0].int() >= big(3)).then_some("contract_violation");
    let record = prop.check(&config(1), &model);
    let ce = counterexample(&record);
    assert_eq!(ce["found_by"], "prop_count_c4");
    assert_eq!(ce["symbol"], "tests.loops.count_up");
    let q = "tests.loops.\"own loop\"";
    let own_binders = [("n", range(0, 6))];
    let own = Prop::new(&p, q, q, &own_binders);
    let record = own.check(&config(1), &|t: &[V]| {
        (*t[0].int() >= big(2)).then_some("contract_violation")
    });
    assert_eq!(shrunk_input(&record), json!({"n": 2}));
    let f = fault_of(&record);
    assert_eq!(f["clause_kind"], "invariant");
    assert_eq!(f["attributed_to"], "body");
    assert_eq!(f["symbol"], q);
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "i < 2");
    assert_eq!(f["iteration"], json!(2));
    assert_eq!(f["actual"], json!({"bindings": {"i": 2, "n": 2}}));
    assert!(f.get("input").is_none(), "{f}");
    assert!(record.counterexamples.is_empty());
}

const DEPTH: &str = r#"module tests.depth

fn leaf() -> Int {
    [1, 2].take(1).len
}

fn branch() -> Int {
    leaf()
}

fn down(n: Int) -> Int {
    if n <= 0 {
        0
    } else {
        down(n - 1)
    }
}

test "no call" {
    assert [1, 2].take(1).len == 1
}

test "one call" {
    assert leaf() == 1
}

test "two calls" {
    assert branch() == 1
}

property "deep"
    for n in gen.range(0, 120)
{
    assert down(n) == 0
}

property "bound calls a function"
    for x in gen.range(0, leaf())
{
    assert x >= 0
}
"#;

/// semantics S72: the depth limit counts CPL function calls only; a test
/// body and a builtin callee are frames but not calls, so with
/// `max_call_depth = 1` a test may call a function that calls `take`, but
/// not a function that calls another function (S38: `resource_exhaustion`,
/// `resource: "stack"`, blamed on the enclosing declaration).
#[test]
fn the_call_depth_limit_counts_function_calls_only_in_tests_s72() {
    let p = common::check_source(DEPTH);
    let mut cfg = config(1);
    cfg.eval.max_call_depth = 1;
    let one = run_test(&p, common::def(&p, "tests.depth.\"one call\""), &cfg);
    assert_eq!(one.status, TestStatus::Tested, "{}", stable(&one));
    let two = run_test(&p, common::def(&p, "tests.depth.\"two calls\""), &cfg);
    assert_eq!(two.status, TestStatus::Failed);
    let f = fault_of(&two);
    assert_eq!(f["code"], "resource_exhaustion");
    assert_eq!(f["category"], "internal");
    assert_eq!(f["resource"], "stack");
    assert_eq!(f["limit"], json!(1));
    assert_eq!(f["symbol"], "tests.depth.branch");
    assert_eq!(
        frames(&f),
        ["tests.depth.branch", "tests.depth.\"two calls\""]
    );
    let bound = common::def(&p, "tests.depth.\"bound calls a function\"");
    assert_eq!(run_test(&p, bound, &cfg).status, TestStatus::Tested);
    // The boundary `max_call_depth = 0`: a body without calls runs (a
    // builtin is no call), any call is refused, also one a range makes at
    // setup, which is the property's fault (S71).
    cfg.eval.max_call_depth = 0;
    let none = run_test(&p, common::def(&p, "tests.depth.\"no call\""), &cfg);
    assert_eq!(none.status, TestStatus::Tested, "{}", stable(&none));
    let one = run_test(&p, common::def(&p, "tests.depth.\"one call\""), &cfg);
    let f = fault_of(&one);
    assert_eq!(f["resource"], "stack");
    assert_eq!(f["limit"], json!(0));
    assert_eq!(f["symbol"], "tests.depth.\"one call\"");
    let record = run_test(&p, bound, &cfg);
    assert_eq!((record.status, record.cases), (TestStatus::Failed, Some(0)));
    let f = fault_of(&record);
    assert_eq!(f["code"], "resource_exhaustion");
    assert_eq!(f["resource"], "stack");
    assert_eq!(f["symbol"], "tests.depth.\"bound calls a function\"");
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "gen.range(0, leaf())");
}

/// The configured depth limit holds in every case and shrink rerun, and a
/// `resource_exhaustion` shrinks like any fault (S45): to the smallest `n`
/// that needs more than 50 active calls. The refused call never became
/// active, so the backtrace holds the 50 activations of `down` and the
/// property (6.7: every active declaration is a frame).
#[test]
fn a_stack_exhaustion_shrinks_to_the_depth_boundary() {
    let p = common::check_source(DEPTH);
    let q = "tests.depth.\"deep\"";
    let binders = [("n", range(0, 120))];
    let prop = Prop::new(&p, q, q, &binders);
    let model = |t: &[V]| (*t[0].int() >= big(50)).then_some("resource_exhaustion");
    let mut failures = 0;
    for seed in [1, 2, 3] {
        let mut cfg = config(seed);
        cfg.eval.max_call_depth = 50;
        let record = prop.check(&cfg, &model);
        if record.status == TestStatus::Tested {
            continue;
        }
        failures += 1;
        assert_eq!(shrunk_input(&record), json!({"n": 50}));
        let f = fault_of(&record);
        assert_eq!(f["resource"], "stack");
        assert_eq!(f["limit"], json!(50));
        assert_eq!(f["symbol"], "tests.depth.down");
        let names = frames(&f);
        assert_eq!(names.len(), 51, "{names:?}");
        assert!(names[..50].iter().all(|n| n == "tests.depth.down"));
        assert_eq!(names[50], q);
    }
    assert!(failures > 0);
}

// ===========================================================================
// The setup of `for` ranges (stdlib 8.2, 12; semantics 5.1, S70, S71)
// ===========================================================================

const SETUP: &str = r#"module tests.setup

property "lo before hi"
    for x in gen.range(7 / 0, 9 % 0)
{
    assert x == x
}

property "a faulting hi before the range check"
    for x in gen.range(10, 1 / 0)
{
    assert x == x
}

property "first clause first"
    for a in gen.range(0, 3 / 0)
    for b in gen.range(4 % 0, 5)
{
    assert a == b
}

property "an empty range before a later fault"
    for a in gen.range(5, 1)
    for b in gen.range(1 / 0, 2)
{
    assert a == b
}

property "an empty range after a generator without setup"
    for a in gen.bool
    for b in gen.list(gen.range(2, 1))
{
    assert a == a
}

property "an empty range in an option"
    for o in gen.option(gen.range(0, -1))
{
    assert o == o
}

@id("prop_setup_e1")
property "annotated empty range"
    for x in gen.range(3, 2)
{
    assert x == x
}

@id("prop_setup_f1")
property "annotated faulting bound"
    for x in gen.range(0, 2 / 0)
{
    assert x == x
}
"#;

fn setup(name: &str) -> String {
    format!("tests.setup.\"{name}\"")
}

/// The fault of a property that fails at setup: zero cases, no tuple
/// (stdlib 8.2, toolchain 1.7.2), even with `--cases 0`.
fn setup_fault(p: &Program, qualified: &str) -> Json {
    let def = common::def(p, qualified);
    let mut first = None;
    for cases in [100, 0] {
        let record = run_test(p, def, &TestConfig { cases, ..config(1) });
        assert_eq!(
            record.status,
            TestStatus::Failed,
            "{qualified}, {cases} cases"
        );
        assert_eq!(record.cases, Some(0), "{qualified}");
        assert!(record.failing_case.is_none() && record.shrunk.is_none());
        assert!(record.property_seed.is_some());
        let f = fault_of(&record);
        if let Some(previous) = &first {
            assert_eq!(
                &f, previous,
                "{qualified}: the setup runs whatever --cases is"
            );
        }
        first = Some(f);
    }
    first.unwrap()
}

/// S71: a fault raised while a range evaluates is the property's (`facet:
/// "contract"`, `clause` the range's canonical text, `symbol` its identity);
/// the setup runs in clause order with `lo` before `hi`, and `lo <= hi` is
/// checked after both bounds of its range (stdlib 8.2).
#[test]
fn setup_runs_in_clause_order_and_lo_before_hi_s71() {
    let p = common::check_source(SETUP);
    for (name, clause, input) in [
        (
            "lo before hi",
            "gen.range(7 / 0, 9 % 0)",
            json!({"a": 7, "b": 0}),
        ),
        (
            "a faulting hi before the range check",
            "gen.range(10, 1 / 0)",
            json!({"a": 1, "b": 0}),
        ),
        (
            "first clause first",
            "gen.range(0, 3 / 0)",
            json!({"a": 3, "b": 0}),
        ),
    ] {
        let q = setup(name);
        let f = setup_fault(&p, &q);
        assert_eq!(f["code"], "division_by_zero", "{q}");
        assert_eq!(f["category"], "runtime");
        assert_eq!(f["symbol"], q.as_str());
        assert_eq!(f["qualified_name"], q.as_str());
        assert_eq!(f["facet"], "contract");
        assert_eq!(f["clause"], clause, "{q}");
        assert_eq!(f["input"], input, "{q}");
        assert_eq!(frames(&f), [q.as_str()]);
    }
}

/// stdlib 8.2, 12, S71: the generator's own `requires lo <= hi` keeps the
/// builtin shape (`symbol std.gen.range`, `clause lo <= hi`, `caller` the
/// property) and stops the setup of later clauses; `caller` is a qualified
/// name (semantics 5.2, 9.3), also for an annotated property, whose own
/// faults carry its `@id` as `symbol`.
#[test]
fn an_empty_range_is_the_generators_own_precondition() {
    let p = common::check_source(SETUP);
    for (name, lo, hi) in [
        ("an empty range before a later fault", 5, 1),
        ("an empty range after a generator without setup", 2, 1),
        ("an empty range in an option", 0, -1),
        ("annotated empty range", 3, 2),
    ] {
        let q = setup(name);
        let f = setup_fault(&p, &q);
        assert_eq!(f["code"], "contract_violation", "{q}");
        assert_eq!(f["clause_kind"], "requires");
        assert_eq!(f["attributed_to"], "caller");
        assert_eq!(f["symbol"], "std.gen.range");
        assert_eq!(f["qualified_name"], "std.gen.range");
        assert_eq!(f["facet"], "contract");
        assert_eq!(f["clause"], "lo <= hi");
        assert_eq!(f["input"], json!({"lo": lo, "hi": hi}), "{q}");
        assert_eq!(f["caller"], q.as_str(), "{q}");
        assert_eq!(f["status"], "failed");
        assert_eq!(frames(&f), ["std.gen.range".to_string(), q.clone()]);
    }
    let annotated = run_test(
        &p,
        common::def(&p, &setup("annotated empty range")),
        &config(1),
    );
    assert_eq!(annotated.symbol.as_str(), "prop_setup_e1");
    assert_eq!(
        annotated.property_seed.as_deref(),
        Some(oracle::seed_hex(1, "prop_setup_e1").as_str())
    );
    let q = setup("annotated faulting bound");
    let f = setup_fault(&p, &q);
    assert_eq!(f["code"], "division_by_zero");
    assert_eq!(f["symbol"], "prop_setup_f1");
    assert_eq!(f["qualified_name"], q.as_str());
    assert_eq!(f["clause"], "gen.range(0, 2 / 0)");
}

const SETUP_CE: &str = r#"module tests.setup_ce

fn bump(n: Int) -> Int
    ensures result > n
{
    n
}

property "bumped bound"
    for x in gen.range(bump(4), 10)
{
    assert x == x
}

property "qualified range"
    for x in std.gen.range(-1 / 0, 2)
{
    assert x == x
}
"#;

/// S46, S70, S71: an `ensures` violation of a function a range calls at
/// setup is claimed by the range (`symbol` the property, `clause` the
/// range's text) while `clause_kind`, `attributed_to`, `input`, `actual`,
/// and `span` stay those of the violation, and it is a counterexample
/// against the function, built from the violation as raised.
#[test]
fn an_ensures_violation_at_setup_is_claimed_by_the_range_and_is_a_counterexample() {
    let p = common::check_source(SETUP_CE);
    let q = "tests.setup_ce.\"bumped bound\"";
    let record = run_test(&p, common::def(&p, q), &config(1));
    assert_eq!((record.status, record.cases), (TestStatus::Failed, Some(0)));
    let f = fault_of(&record);
    assert_eq!(f["code"], "contract_violation");
    assert_eq!(f["clause_kind"], "ensures");
    assert_eq!(f["attributed_to"], "body");
    assert_eq!(f["symbol"], q);
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "gen.range(bump(4), 10)");
    assert_eq!(f["input"], json!({"n": 4}));
    assert_eq!(f["actual"], json!(4));
    let ensures = common::location_of(SETUP_CE, "src/tests/setup_ce.cpl", "ensures result > n");
    assert_eq!(f["span"], ensures);
    assert_eq!(frames(&f), ["tests.setup_ce.bump", q]);
    let ce = counterexample(&record);
    assert_eq!(ce["code"], "contract_counterexample");
    assert_eq!(ce["severity"], "error");
    assert_eq!(ce["symbol"], "tests.setup_ce.bump");
    assert_eq!(ce["qualified_name"], "tests.setup_ce.bump");
    assert_eq!(ce["facet"], "body");
    assert_eq!(ce["clause"], "result > n");
    assert_eq!(ce["clause_kind"], "ensures");
    assert_eq!(ce["input"], json!({"n": 4}));
    assert_eq!(ce["actual"], json!(4));
    assert_eq!(ce["status"], "failed");
    assert_eq!(ce["found_by"], q);
    assert_eq!(ce["span"], ensures);
    // The range's text is its canonical text as written: the qualified
    // spelling of the generator stays (S71, toolchain 5.2).
    let f = fault_of(&run_test(
        &p,
        common::def(&p, "tests.setup_ce.\"qualified range\""),
        &config(1),
    ));
    assert_eq!(f["code"], "division_by_zero");
    assert_eq!(f["clause"], "std.gen.range(-1 / 0, 2)");
    assert_eq!(f["input"], json!({"a": -1, "b": 0}));
}

const SETUP_CALLS: &str = r#"module tests.setup_calls

fn crash() -> Int {
    1 / 0
}

fn todo_bound() -> Int {
    ?bound
}

fn asserting() -> Int {
    let n = 2
    assert n == 3
    n
}

property "a body fault in a called function"
    for x in gen.range(0, crash())
{
    assert x == x
}

property "a hole in a called function"
    for x in gen.range(0, todo_bound())
{
    assert x == x
}

property "an assert in a called function"
    for x in gen.range(asserting(), 5)
{
    assert x == x
}

property "a text literal"
    for x in gen.range("a\nb".len / 0, 5)
{
    assert x == x
}

property "a comment"
    for x in gen.range(0, match 0 {
        // the zero arm divides by zero
        0 => 1 / 0
        _ => 1
    })
{
    assert x == x
}

property "a block with a let"
    for x in gen.range(0, if true {
        let m = 1
        m / 0
    } else {
        0
    })
{
    assert x == x
}

property "line breaks"
    for x in gen.range(1
        / 0, 5)
{
    assert x == x
}
"#;

fn setup_calls(name: &str) -> String {
    format!("tests.setup_calls.\"{name}\"")
}

/// S71: S34 applies to every fault raised while a range evaluates, "the
/// bounds of gen.range, and every call they make", unless an inner clause
/// claimed it (S70): a body fault, a hole, or a failed `assert` in a
/// function a bound calls is reported with `symbol` the property, `facet:
/// "contract"`, and `clause` the range's text, its other fields as raised.
/// For the `assert` this replaces the assert's own `clause` (reading: S70
/// rewrites exactly `symbol`, `facet`, and `clause`, and an assert is no
/// contract clause that could claim the fault first).
#[test]
fn faults_in_functions_a_range_calls_are_the_propertys_s71() {
    let p = common::check_source(SETUP_CALLS);
    let q = setup_calls("a body fault in a called function");
    let f = setup_fault(&p, &q);
    assert_eq!(f["code"], "division_by_zero");
    assert_eq!(f["symbol"], q.as_str());
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "gen.range(0, crash())");
    assert_eq!(f["input"], json!({"a": 1, "b": 0}));
    assert_eq!(
        frames(&f),
        ["tests.setup_calls.crash".to_string(), q.clone()]
    );
    let q = setup_calls("a hole in a called function");
    let f = setup_fault(&p, &q);
    assert_eq!(f["code"], "unresolved_hole");
    assert_eq!(f["category"], "hole");
    assert_eq!(f["hole"], "bound");
    assert_eq!(f["expected_type"], "Int");
    assert_eq!(f["bindings"], json!([]));
    assert_eq!(f["symbol"], q.as_str());
    assert_eq!(f["qualified_name"], q.as_str());
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "gen.range(0, todo_bound())");
    assert_eq!(
        frames(&f),
        ["tests.setup_calls.todo_bound".to_string(), q.clone()]
    );
    let q = setup_calls("an assert in a called function");
    let f = setup_fault(&p, &q);
    assert_eq!(f["code"], "assertion_failed");
    assert_eq!(f["symbol"], q.as_str());
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "gen.range(asserting(), 5)");
    assert_eq!(
        f["bindings"],
        json!([{"name": "n", "type": "Int", "kind": "let", "value": 2}])
    );
    assert_eq!(
        frames(&f),
        ["tests.setup_calls.asserting".to_string(), q.clone()]
    );
}

/// 9.2, toolchain 5.2 and T56: a range's `clause` is its canonical text on
/// one line: a text literal keeps its escapes, comments are dropped, source
/// line breaks inside an expression are not reproduced, and a block's `let`
/// and its tail are joined by one space.
#[test]
fn a_range_clause_is_its_canonical_text_on_one_line_t56() {
    let p = common::check_source(SETUP_CALLS);
    for (name, clause, input) in [
        (
            "a text literal",
            "gen.range(\"a\\nb\".len / 0, 5)",
            json!({"a": 3, "b": 0}),
        ),
        (
            "a comment",
            "gen.range(0, match 0 { 0 => 1 / 0 _ => 1 })",
            json!({"a": 1, "b": 0}),
        ),
        (
            "a block with a let",
            "gen.range(0, if true { let m = 1 m / 0 } else { 0 })",
            json!({"a": 1, "b": 0}),
        ),
        (
            "line breaks",
            "gen.range(1 / 0, 5)",
            json!({"a": 1, "b": 0}),
        ),
    ] {
        let q = setup_calls(name);
        let f = setup_fault(&p, &q);
        assert_eq!(f["code"], "division_by_zero", "{q}");
        assert_eq!(f["symbol"], q.as_str());
        assert_eq!(f["clause"], clause, "{q}");
        assert_eq!(f["input"], input, "{q}");
    }
}

// ===========================================================================
// Time budgets (toolchain 1.7, 1.7.2, 7.2; semantics S38)
// ===========================================================================

const BUDGET_TESTS: &str = r#"module tests.budget

fn forever(n: Int) -> Int {
    var i = n
    while i >= 0 {
        i = i + 1
    }
    i
}

test "a spins" {
    while true {
    }
}

test "b calls forever" {
    let _ = forever(0)
}

test "c passes" {
    assert true
}
"#;

const BUDGET_PROPS: &str = r#"module tests.budget_props

fn slow_zero() -> Int {
    var i = 0
    while i < 200000 {
        i = i + 1
    }
    i - i
}

property "slow bound"
    for x in gen.range(slow_zero(), 2)
{
    assert x >= 0
}

property "spins from four"
    for n in gen.range(0, 10)
{
    var i = 0
    while i < 3 {
        i = i + 1
    }
    if n >= 4 {
        while true {
        }
    }
}

property "spins at zero"
    for n in gen.range(0, 10)
{
    if n == 0 {
        while true {
        }
    }
    assert n < 3
}
"#;

fn budget_program() -> Program {
    common::check_files(&[
        ("src/tests/budget.cpl", BUDGET_TESTS),
        ("src/tests/budget_props.cpl", BUDGET_PROPS),
    ])
}

fn with_budget(seed: u64, ms: u64) -> TestConfig {
    let mut c = config(seed);
    c.eval.time_budget = Some(Duration::from_millis(ms));
    c
}

/// toolchain 7.2 `--timeout-ms`: each test body has the budget; exceeding
/// it is `resource_exhaustion` (`resource: "time"`, `limit` the budget in
/// milliseconds, the unit of the flag), blamed on the enclosing declaration
/// (1.7.3); the failing test does not stop the others (semantics 7.3), and
/// its `duration_ms` (wall time, S46) covers the budget it spent.
#[test]
fn a_test_that_never_ends_times_out_and_the_others_still_run() {
    let p = budget_program();
    let cfg = with_budget(1, 50);
    let report = run_tests(&p, &namespace("tests.budget"), &cfg);
    assert_eq!((report.selected, report.tested, report.failed), (3, 1, 2));
    let spins = by_name(&report, "tests.budget.\"a spins\"");
    assert_eq!(spins.status, TestStatus::Failed);
    assert!(spins.duration_ms >= 50, "{}", spins.duration_ms);
    let f = fault_of(spins);
    assert_eq!(f["code"], "resource_exhaustion");
    assert_eq!(f["category"], "internal");
    assert_eq!(f["resource"], "time");
    assert_eq!(f["limit"], json!(50));
    assert_eq!(f["symbol"], "tests.budget.\"a spins\"");
    assert_eq!(f["facet"], "body");
    let calls = by_name(&report, "tests.budget.\"b calls forever\"");
    let f = fault_of(calls);
    assert_eq!(f["resource"], "time");
    assert_eq!(f["symbol"], "tests.budget.forever");
    assert_eq!(f["facet"], "body");
    assert_eq!(
        frames(&f),
        ["tests.budget.forever", "tests.budget.\"b calls forever\""]
    );
    assert_eq!(
        by_name(&report, "tests.budget.\"c passes\"").status,
        TestStatus::Tested
    );
}

/// toolchain 7.2: the budget applies to "each property case, and each
/// shrink rerun separately". A candidate below 4 runs a short loop (a time
/// check at every iteration); were the budget shared with the rerun before
/// it, which spun until it ran out, those candidates would fail with the
/// same code and the tuple would shrink to 0 instead of 4.
#[test]
fn every_property_case_and_shrink_rerun_gets_a_fresh_time_budget() {
    let p = budget_program();
    let q = "tests.budget_props.\"spins from four\"";
    let binders = [("n", range(0, 10))];
    let prop = Prop::new(&p, q, q, &binders);
    let model = |t: &[V]| (*t[0].int() >= big(4)).then_some("resource_exhaustion");
    for seed in [1, 2] {
        let record = prop.check(&with_budget(seed, 100), &model);
        assert_eq!(shrunk_input(&record), json!({"n": 4}), "seed {seed}");
        let f = fault_of(&record);
        assert_eq!(f["resource"], "time");
        assert_eq!(f["limit"], json!(100));
        assert_eq!(f["symbol"], q);
        assert_eq!(f["facet"], "body");
    }
}

/// S45: a shrink rerun that runs out of time raises `resource_exhaustion`,
/// another code than the original `assertion_failed`, so the candidate is
/// discarded and the walk goes on; a property whose first failing case is
/// the timeout keeps that fault.
#[test]
fn a_shrink_candidate_that_times_out_is_discarded_for_its_other_code() {
    let p = budget_program();
    let q = "tests.budget_props.\"spins at zero\"";
    let binders = [("n", range(0, 10))];
    let prop = Prop::new(&p, q, q, &binders);
    let model = |t: &[V]| {
        let n = t[0].int();
        if *n == big(0) {
            Some("resource_exhaustion")
        } else {
            (*n >= big(3)).then_some("assertion_failed")
        }
    };
    let mut past_a_timeout = false;
    for seed in 1..=4 {
        let record = prop.check(&with_budget(seed, 30), &model);
        let original = failing_input(&record)["n"].as_i64().unwrap();
        if original >= 3 {
            assert_eq!(shrunk_input(&record), json!({"n": 3}), "seed {seed}");
            assert_eq!(fault_of(&record)["code"], "assertion_failed");
            past_a_timeout = true;
        } else {
            assert_eq!(original, 0, "seed {seed}");
            assert_eq!(fault_of(&record)["resource"], "time");
        }
    }
    assert!(
        past_a_timeout,
        "some seed shrinks past a timed-out candidate"
    );
}

/// toolchain 1.7.2 and 7.2: a property's range setup runs without the time
/// budget; the premise, that `slow_zero` alone outlasts the budget, is
/// checked first.
#[test]
fn a_propertys_range_setup_runs_without_the_time_budget() {
    let p = budget_program();
    let tight = EvalConfig {
        time_budget: Some(Duration::from_millis(1)),
        ..EvalConfig::default()
    };
    let out = common::try_run_with(&p, "tests.budget_props.slow_zero", &[], &tight)
        .expect("slow_zero runs");
    let f = common::fault(&p, &out);
    assert_eq!(f["code"], "resource_exhaustion", "the premise: {f}");
    assert_eq!(f["resource"], "time");
    let record = run_test(
        &p,
        common::def(&p, "tests.budget_props.\"slow bound\""),
        &TestConfig {
            eval: tight,
            ..config(1)
        },
    );
    assert_eq!(record.status, TestStatus::Tested, "{}", stable(&record));
    assert_eq!(record.cases, Some(100));
}

// ===========================================================================
// `assert` (semantics 7.1, S41; 9.2 `clause`)
// ===========================================================================

const ASSERTS: &str = r#"module tests.asserts

fn checked_half(n: Int) -> Int {
    let h = n / 2
    assert h * 2 == n
    h
}

test "spacing" {
    let a = 1
    var b = [a]
    b = b.push(2)
    assert   a+1 ==  3
}

test "parentheses" {
    assert (1 + 1) == 3
}

test "blocks" {
    let x = 0
    assert if x > 0 { true } else { false }
}

test "arms" {
    let x = 0
    assert match x {
        0 => false
        _ => true
    }
}

test "commented" {
    let x = 0
    assert match x {
        // zero is the failing case
        0 => false
        _ => true
    }
}

test "let in a block" {
    let x = 1
    assert if x > 0 {
        let m = x
        m > 10
    } else {
        false
    }
}

test "two lines" {
    let x = 0
    assert (x == 1
        or x == 2)
}

test "called" {
    let _ = checked_half(7)
}

test "faulting assert" {
    let zero = 0
    assert 1 / zero == 0
}
"#;

fn asserts(name: &str) -> String {
    format!("tests.asserts.\"{name}\"")
}

/// S41: `assertion_failed` (category `test`) carries `clause` (the canonical
/// text of the expression, 9.2: canonical spacing, parentheses as written),
/// `symbol` (the enclosing declaration, `facet: "body"`), and `bindings`:
/// every visible binding with its type and value JSON, innermost first,
/// `var`s with `mutable: true` (8.3).
#[test]
fn an_assert_reports_its_canonical_clause_and_its_bindings_s41() {
    let p = common::check_source(ASSERTS);
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    assert_eq!((report.selected, report.tested, report.failed), (9, 0, 9));
    let q = asserts("spacing");
    let f = fault_of(by_name(&report, &q));
    assert_eq!(f["code"], "assertion_failed");
    assert_eq!(f["category"], "test");
    assert_eq!(f["severity"], "error");
    assert_eq!(f["clause"], "a + 1 == 3");
    assert_eq!(f["symbol"], q.as_str());
    assert_eq!(f["qualified_name"], q.as_str());
    assert_eq!(f["facet"], "body");
    assert_eq!(
        f["bindings"],
        json!([
            {"name": "b", "type": "List<Int>", "kind": "var", "mutable": true, "value": [1, 2]},
            {"name": "a", "type": "Int", "kind": "let", "value": 1},
        ])
    );
    assert_eq!(frames(&f), [q.as_str()]);
    assert!(
        f["backtrace"][0].get("span").is_none(),
        "the runner entered the test"
    );
    assert!(by_name(&report, &q).counterexamples.is_empty());
    let f = fault_of(by_name(&report, &asserts("parentheses")));
    assert_eq!(f["clause"], "(1 + 1) == 3");
    assert_eq!(f["bindings"], json!([]));
}

/// 9.2 and toolchain T56: a clause holding an `if` or a `match` is its
/// canonical text with every line break and the indentation after it
/// replaced by one space.
#[test]
fn an_assert_clause_with_blocks_is_one_line_t56() {
    let p = common::check_source(ASSERTS);
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    let f = fault_of(by_name(&report, &asserts("blocks")));
    assert_eq!(f["code"], "assertion_failed");
    assert_eq!(f["clause"], "if x > 0 { true } else { false }");
    let f = fault_of(by_name(&report, &asserts("arms")));
    assert_eq!(f["code"], "assertion_failed");
    assert_eq!(f["clause"], "match x { 0 => false _ => true }");
    // Canonical text has no comments (toolchain 5.2).
    let f = fault_of(by_name(&report, &asserts("commented")));
    assert_eq!(f["clause"], "match x { 0 => false _ => true }");
    // A block's `let` and its tail are joined by one space (T56's example);
    // the block's binding is out of scope at the assert (S41: visible).
    let f = fault_of(by_name(&report, &asserts("let in a block")));
    assert_eq!(f["clause"], "if x > 0 { let m = x m > 10 } else { false }");
    assert_eq!(
        f["bindings"],
        json!([{"name": "x", "type": "Int", "kind": "let", "value": 1}])
    );
    // Source line breaks are not reproduced; parentheses stay as written.
    let f = fault_of(by_name(&report, &asserts("two lines")));
    assert_eq!(f["clause"], "(x == 1 or x == 2)");
}

/// 7.1: a failing `assert` inside a function called by a test fails the
/// test like any fault; the fault names the function and its bindings. An
/// assert whose expression faults reports that fault: an assert is a body
/// statement, not a contract clause (5.1), so the fault keeps `facet:
/// "body"` and gains no `clause` (S34 applies to contract expressions only).
#[test]
fn an_assert_in_a_called_function_blames_that_function() {
    let p = common::check_source(ASSERTS);
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    let called = by_name(&report, &asserts("called"));
    let f = fault_of(called);
    assert_eq!(f["code"], "assertion_failed");
    assert_eq!(f["symbol"], "tests.asserts.checked_half");
    assert_eq!(f["facet"], "body");
    assert_eq!(f["clause"], "h * 2 == n");
    assert_eq!(
        f["bindings"],
        json!([
            {"name": "h", "type": "Int", "kind": "let", "value": 3},
            {"name": "n", "type": "Int", "kind": "parameter", "value": 7},
        ])
    );
    assert_eq!(
        frames(&f),
        ["tests.asserts.checked_half".to_string(), asserts("called")]
    );
    assert!(called.counterexamples.is_empty());
    let f = fault_of(by_name(&report, &asserts("faulting assert")));
    assert_eq!(f["code"], "division_by_zero");
    assert_eq!(f["input"], json!({"a": 1, "b": 0}));
    assert_eq!(f["symbol"], asserts("faulting assert").as_str());
    assert_eq!(f["facet"], "body");
    assert!(f.get("clause").is_none(), "{f}");
}

// ===========================================================================
// Holes in test bodies (semantics 8.1-8.3, S41, S47, S64)
// ===========================================================================

const TWINS: &str = r#"module tests.twins

test "assert twin" {
    let a = 1
    var b = 2
    b = b + a
    let o: Option<Bool> = Some(true)
    match o {
        Some(v) => {
            let c = [v]
            assert c.len == 2
        }
        None => {
            assert false
        }
    }
}

test "hole twin" {
    let a = 1
    var b = 2
    b = b + a
    let o: Option<Bool> = Some(true)
    match o {
        Some(v) => {
            let c = [v]
            assert ?twin
        }
        None => {
            assert false
        }
    }
}

test "after blocks assert" {
    let a = 1
    if a > 0 {
        let inner = 5
        assert inner > 0
    }
    var i = 0
    while i < 2 {
        let step = 1
        i = i + step
    }
    let o: Option<Int> = Some(4)
    match o {
        Some(v) => {
            assert v == 4
        }
        None => {}
    }
    assert a == 0
}

test "after blocks hole" {
    let a = 1
    if a > 0 {
        let inner = 5
        assert inner > 0
    }
    var i = 0
    while i < 2 {
        let step = 1
        i = i + step
    }
    let o: Option<Int> = Some(4)
    match o {
        Some(v) => {
            assert v == 4
        }
        None => {}
    }
    assert ?after
}

test "forall hole" {
    let xs = [1, 2]
    assert forall v in xs: v > ?h
}

property "assert twin property"
    for x in gen.range(0, 3)
    for n in gen.positive_int
    for o in gen.option(gen.positive_int)
{
    let a = x + 1
    assert a < 0
}

property "hole twin property"
    for x in gen.range(0, 3)
    for n in gen.positive_int
    for o in gen.option(gen.positive_int)
{
    let a = x + 1
    assert ?twin_p
}
"#;

/// S41 and 6.7: `assertion_failed` and `unresolved_hole` list the bindings
/// visible at the fault "innermost first as in the hole report (8.3)", with
/// their values; a hole standing where an assert's expression stands sees
/// exactly the assert's bindings, so all three lists agree: the checker's
/// hole report, and the two runtime faults without their values.
#[test]
fn runtime_bindings_are_the_checkers_hole_report_bindings_s41() {
    let result = common::check_source_result(TWINS);
    let reported = |hole: &str| -> Json {
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.code.0 == "hole" && d.data.get("hole") == Some(&json!(hole)))
            .unwrap_or_else(|| panic!("no hole report for ?{hole}"));
        assert_eq!(d.severity, Severity::Info, "development mode (S47)");
        d.data["bindings"].clone()
    };
    let twin = reported("twin");
    let twin_p = reported("twin_p");
    let after = reported("after");
    let forall = reported("h");
    let p = common::checked(result);
    let report = run_tests(&p, &TestSelector::all(), &config(4));
    let q = |name: &str| format!("tests.twins.\"{name}\"");
    let a = fault_of(by_name(&report, &q("assert twin")));
    let h = fault_of(by_name(&report, &q("hole twin")));
    assert_eq!(a["code"], "assertion_failed");
    assert_eq!(h["code"], "unresolved_hole");
    assert_eq!(h["hole"], "twin");
    assert_eq!(h["expected_type"], "Bool");
    assert_eq!(without_value(&a["bindings"]), twin);
    assert_eq!(without_value(&h["bindings"]), twin);
    assert_eq!(a["bindings"], h["bindings"]);
    assert_eq!(
        h["bindings"],
        json!([
            {"name": "c", "type": "List<Bool>", "kind": "let", "value": [true]},
            {"name": "v", "type": "Bool", "kind": "pattern", "value": true},
            {"name": "o", "type": "Option<Bool>", "kind": "let",
             "value": {"$type": "Option", "$variant": "Some", "$payload": [true]}},
            {"name": "b", "type": "Int", "kind": "var", "mutable": true, "value": 3},
            {"name": "a", "type": "Int", "kind": "let", "value": 1},
        ])
    );
    // The properties fail on every case; every candidate fails alike, so
    // each binder reaches its minimum (S45): x 0, n 1, o None.
    let ap = by_name(&report, &q("assert twin property"));
    let hp = by_name(&report, &q("hole twin property"));
    let minimum = json!({"x": 0, "n": 1, "o": {"$type": "Option", "$variant": "None"}});
    assert_eq!(shrunk_input(ap), minimum);
    assert_eq!(shrunk_input(hp), minimum);
    let a = fault_of(ap);
    let h = fault_of(hp);
    assert_eq!(h["code"], "unresolved_hole");
    assert_eq!(without_value(&a["bindings"]), twin_p);
    assert_eq!(without_value(&h["bindings"]), twin_p);
    let names: Vec<&str> = h["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a", "o", "n", "x"]);
    assert_eq!(h["bindings"][0]["value"], json!(1));
    assert_eq!(
        h["bindings"][1]["value"],
        json!({"$type": "Option", "$variant": "None"})
    );
    assert_eq!(h["bindings"][2]["value"], json!(1));
    assert_eq!(h["bindings"][3]["value"], json!(0));
    assert_eq!(h["bindings"][2]["kind"], "for");
    // Bindings of blocks, loop bodies, and match arms that have ended are no
    // longer visible.
    let a = fault_of(by_name(&report, &q("after blocks assert")));
    let h = fault_of(by_name(&report, &q("after blocks hole")));
    assert_eq!(a["code"], "assertion_failed");
    assert_eq!(h["code"], "unresolved_hole");
    assert_eq!(without_value(&a["bindings"]), after);
    assert_eq!(without_value(&h["bindings"]), after);
    assert_eq!(
        a["bindings"],
        json!([
            {"name": "o", "type": "Option<Int>", "kind": "let",
             "value": {"$type": "Option", "$variant": "Some", "$payload": [4]}},
            {"name": "i", "type": "Int", "kind": "var", "mutable": true, "value": 2},
            {"name": "a", "type": "Int", "kind": "let", "value": 1},
        ])
    );
    assert_eq!(a["bindings"], h["bindings"]);
    // A `forall` binder is visible in its predicate (8.3 kind `forall`), with
    // the element being tested as its value.
    let h = fault_of(by_name(&report, &q("forall hole")));
    assert_eq!(without_value(&h["bindings"]), forall);
    assert_eq!(
        h["bindings"],
        json!([
            {"name": "v", "type": "Int", "kind": "forall", "value": 1},
            {"name": "xs", "type": "List<Int>", "kind": "let", "value": [1, 2]},
        ])
    );
}

const UNFINISHED: &str = r#"module tests.unfinished

fn twice(n: Int) -> Int {
    n * 2
}

test "unfinished test" {
    let n = twice(2)
    assert n == ?expected
}

property "unfinished property"
    for x in gen.int
{
    assert twice(x) == ?want
}

test "finished" {
    assert twice(3) == 6
}
"#;

/// S47 and S64: a hole in a test or property body is the diagnostic `hole`
/// (info in development, warning under `--release`, never
/// `hole_in_release`: test bodies are not part of the linked program), so
/// the program checks in both modes, and the test fails with
/// `unresolved_hole` only when it reaches the hole (7.3); the other tests
/// run as usual.
#[test]
fn a_hole_in_a_test_body_fails_only_that_test_in_both_modes_s47() {
    let manifest = common::manifest("t");
    let files = [
        ("cpl.toml", manifest.as_str()),
        ("src/tests/unfinished.cpl", UNFINISHED),
    ];
    let project = common::load_tree(&common::tree(&files));
    let development = cpl_sema::check(&project);
    assert!(
        !development.has_errors(),
        "{}",
        common::render(&development)
    );
    assert_eq!(
        common::codes_of(&development, Severity::Info),
        ["hole", "hole"]
    );
    assert!(common::codes_of(&development, Severity::Warning).is_empty());
    let release = cpl_sema::check_with(
        &project,
        &cpl_sema::CheckConfig {
            release: true,
            ..cpl_sema::CheckConfig::default()
        },
    );
    assert!(!release.has_errors(), "{}", common::render(&release));
    let mut warnings = common::codes_of(&release, Severity::Warning);
    warnings.sort();
    assert_eq!(warnings, ["hole", "hole", "project_lock_missing"]);
    let binders = [("x", G::Int)];
    let model = |_: &[V]| Some("unresolved_hole");
    for p in [development.program, release.program] {
        let report = run_tests(&p, &TestSelector::all(), &config(6));
        assert_eq!((report.selected, report.tested, report.failed), (3, 1, 2));
        let q = "tests.unfinished.\"unfinished test\"";
        let f = fault_of(by_name(&report, q));
        assert_eq!(f["code"], "unresolved_hole");
        assert_eq!(f["category"], "hole");
        assert_eq!(f["hole"], "expected");
        assert_eq!(f["expected_type"], "Int");
        assert_eq!(f["symbol"], q);
        assert_eq!(f["facet"], "body");
        assert_eq!(
            f["bindings"],
            json!([{"name": "n", "type": "Int", "kind": "let", "value": 4}])
        );
        let qp = "tests.unfinished.\"unfinished property\"";
        let prop = Prop::new(&p, qp, qp, &binders);
        let record = prop.check(&config(6), &model);
        assert_eq!(shrunk_input(&record), json!({"x": 0}));
        assert_eq!(fault_of(&record)["hole"], "want");
        assert!(record.counterexamples.is_empty());
    }
}

/// `examples/holes` with a test and a property added to its module (the
/// example itself has no tests, semantics 11): `summarize` and `paginate`
/// raise `unresolved_hole` with the hole report's expected type and their
/// parameters' values (6.7; a generic parameter is encoded by its value,
/// toolchain 7.3), and a property over `paginate` fails at case 0 and
/// shrinks every binder to its minimum.
#[test]
fn the_holes_example_with_tests_added() {
    let dir = common::example_dir("holes");
    let manifest = std::fs::read_to_string(dir.join("cpl.toml")).expect("the manifest");
    let pages = std::fs::read_to_string(dir.join("src/holes/pages.cpl")).expect("the module");
    let added = r#"
test "summarize a page" {
    let page = Page { id: PageId("p"), text: "t" }
    assert summarize(page) == ""
}

property "paginate keeps items"
    for items in gen.list(gen.int)
    for size in gen.positive_int
{
    assert flatten(paginate(items, size)) == items
}
"#;
    let text = format!("{pages}{added}");
    let result = common::check_files_result(&[
        ("cpl.toml", manifest.as_str()),
        ("src/holes/pages.cpl", text.as_str()),
    ]);
    let mut codes: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.code.0.to_string())
        .collect();
    codes.sort();
    assert_eq!(
        codes,
        ["hole", "hole", "verification_result"],
        "the example's diagnostics and nothing more:\n{}",
        common::render(&result)
    );
    let p = common::checked(result);
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    assert_eq!((report.selected, report.tested, report.failed), (2, 0, 2));
    let t = "holes.pages.\"summarize a page\"";
    let f = fault_of(by_name(&report, t));
    assert_eq!(f["code"], "unresolved_hole");
    assert_eq!(f["hole"], "summary");
    assert_eq!(f["symbol"], "holes.pages.summarize");
    assert_eq!(f["facet"], "body");
    assert_eq!(f["expected_type"], "Text");
    let page = json!({"$type": "holes.pages.Page",
                      "id": {"$type": "holes.pages.PageId", "$payload": ["p"]},
                      "text": "t"});
    assert_eq!(
        f["bindings"],
        json!([{"name": "page", "type": "Page", "kind": "parameter", "value": page}])
    );
    assert_eq!(frames(&f), ["holes.pages.summarize", t]);
    assert_eq!(
        f["backtrace"][0]["span"],
        common::location_of(&text, "src/holes/pages.cpl", "summarize(page)")
    );
    let q = "holes.pages.\"paginate keeps items\"";
    let binders = [("items", list(G::Int)), ("size", G::Pos)];
    let prop = Prop::new(&p, q, q, &binders);
    for seed in [1, 2, 3] {
        let record = prop.check(&config(seed), &|_: &[V]| Some("unresolved_hole"));
        assert_eq!(record.failing_case.as_ref().unwrap().index, 0);
        assert_eq!(shrunk_input(&record), json!({"items": [], "size": 1}));
        let f = fault_of(&record);
        assert_eq!(f["hole"], "build_pages");
        assert_eq!(f["symbol"], "pgn_7c2d");
        assert_eq!(f["qualified_name"], "holes.pages.paginate");
        assert_eq!(f["expected_type"], "List<List<T>>");
        assert_eq!(
            f["bindings"],
            json!([
                {"name": "size", "type": "Positive", "kind": "parameter", "value": 1},
                {"name": "items", "type": "List<T>", "kind": "parameter", "value": []},
            ])
        );
        assert_eq!(frames(&f), ["holes.pages.paginate", q]);
        assert!(record.counterexamples.is_empty());
    }
}

// ===========================================================================
// Contract violations inside tests (semantics 5.2-5.4, 6.10, S34, S46)
// ===========================================================================

const CALLS: &str = r#"module tests.calls

fn recip(n: Int) -> Int
    requires n != 0
{
    1 / n
}

@id("fn_half_k9")
fn half(n: Int) -> Int
    ensures result * 2 == n
{
    n / 2
}

fn sum_to(n: Int) -> Int {
    var i = 0
    var total = 0
    while i < n
        invariant total <= 10
    {
        i = i + 1
        total = total + i
    }
    total
}

fn step_down(n: Int) -> Int {
    var k = n
    while k != 0
        decreases k
    {
        k = k - 2
    }
    k
}

fn stuck(n: Int) -> Int {
    var k = n
    while k > 0
        decreases k
    {
        k = k * 1
    }
    k
}

fn again(n: Int) -> Int
    decreases n
{
    if n <= 0 {
        0
    } else {
        again(n)
    }
}

fn zero_ok(n: Int) -> Int
    ensures 10 / result > 0
{
    n
}

fn first_of(xs: List<Int>) -> List<Int> {
    xs.take(-1)
}

fn never_ok(n: Int) -> Int {
    var i = n
    while i > 0
        invariant i < 0
    {
        i = i - 1
    }
    i
}

fn exits_cleanly(n: Int) -> Int {
    var k = n
    while k > 0
        decreases k - 1
    {
        k = k - 1
    }
    k
}

fn early(n: Int) -> Int
    ensures result >= 0
{
    if n < 0 {
        return n
    }
    n
}

fn parsed(t: Text) -> Result<Int, ConstraintError>
    ensures result.is_ok()
{
    let n = std.text.parse_int(t)?
    Ok(n)
}

fn positive_only(n: Int) -> Int
    requires n > 0
{
    n
}

fn checks_result(n: Int) -> Int
    ensures positive_only(result) > 0
{
    n
}

fn seven() -> Int
    ensures result == 8
{
    7
}

@id("test_recip_z1")
test "calls recip" {
    let _ = recip(0)
}

@id("test_half_q3")
test "calls half" {
    let _ = half(3)
}

test "calls sum_to" {
    assert sum_to(5) == 15
}

test "calls step_down" {
    let _ = step_down(3)
}

test "calls stuck" {
    let _ = stuck(2)
}

test "calls again" {
    let _ = again(2)
}

test "calls zero_ok" {
    let _ = zero_ok(0)
}

test "calls first_of" {
    let _ = first_of([4, 5])
}

test "calls never_ok" {
    let _ = never_ok(0)
}

test "calls exits_cleanly" {
    assert exits_cleanly(3) == 0
}

test "calls early" {
    let _ = early(-2)
}

test "calls parsed" {
    let _ = parsed("x")
}

test "calls checks_result" {
    let _ = checks_result(0)
}

test "calls seven" {
    let _ = seven()
}
"#;

const CALLS_FILE: &str = "src/tests/calls.cpl";

fn calls(name: &str) -> String {
    format!("tests.calls.\"{name}\"")
}

fn calls_report() -> (Program, TestReport) {
    let p = common::check_source(CALLS);
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    assert_eq!((report.selected, report.tested, report.failed), (14, 1, 13));
    (p, report)
}

/// 5.2 step 3 and S46: a `requires` violation of a function a test calls is
/// the caller's fault, `caller` the test's *qualified name* (5.2, 9.3: a
/// qualified name, also when the test has an `@id`), and no counterexample.
#[test]
fn a_requires_violation_names_the_calling_test_by_qualified_name() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls recip"));
    assert_eq!(record.symbol.as_str(), "test_recip_z1");
    let f = fault_of(record);
    assert_eq!(f["code"], "contract_violation");
    assert_eq!(f["category"], "contract");
    assert_eq!(f["clause_kind"], "requires");
    assert_eq!(f["attributed_to"], "caller");
    assert_eq!(f["symbol"], "tests.calls.recip");
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "n != 0");
    assert_eq!(f["input"], json!({"n": 0}));
    assert_eq!(f["caller"], calls("calls recip").as_str());
    assert_eq!(f["status"], "failed");
    assert_eq!(
        f["span"],
        common::location_of(CALLS, CALLS_FILE, "requires n != 0")
    );
    assert_eq!(
        frames(&f),
        ["tests.calls.recip".to_string(), calls("calls recip")]
    );
    assert!(record.counterexamples.is_empty());
}

/// S46: an `ensures` violation in a called declaration yields
/// `contract_counterexample` with `symbol` that declaration's identity,
/// `facet: "body"`, the clause, `input`, `actual`, `status: "failed"`, and
/// `found_by` the test's identity, spanning the violated clause.
#[test]
fn an_ensures_counterexample_names_both_identities() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls half"));
    let f = fault_of(record);
    assert_eq!(f["clause_kind"], "ensures");
    assert_eq!(f["attributed_to"], "body");
    assert_eq!(f["symbol"], "fn_half_k9");
    assert_eq!(f["qualified_name"], "tests.calls.half");
    assert_eq!(f["actual"], json!(1));
    let ce = counterexample(record);
    assert_eq!(ce["code"], "contract_counterexample");
    assert_eq!(ce["category"], "contract");
    assert_eq!(ce["severity"], "error");
    assert_eq!(ce["symbol"], "fn_half_k9");
    assert_eq!(ce["qualified_name"], "tests.calls.half");
    assert_eq!(ce["facet"], "body");
    assert_eq!(ce["clause"], "result * 2 == n");
    assert_eq!(ce["clause_kind"], "ensures");
    assert_eq!(ce["input"], json!({"n": 3}));
    assert_eq!(ce["actual"], json!(1));
    assert_eq!(ce["status"], "failed");
    assert_eq!(ce["found_by"], "test_half_q3");
    assert!(ce.get("iteration").is_none());
    assert_eq!(
        ce["span"],
        common::location_of(CALLS, CALLS_FILE, "ensures result * 2 == n")
    );
}

/// 5.3 and S46: an invariant violation in a callee carries `iteration` (the
/// completed iterations) and `actual = {bindings}` with every binding
/// visible at the loop, parameters included; it is a counterexample.
#[test]
fn an_invariant_violation_in_a_callee_is_a_counterexample_with_its_iteration() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls sum_to"));
    let f = fault_of(record);
    assert_eq!(f["code"], "contract_violation");
    assert_eq!(f["clause_kind"], "invariant");
    assert_eq!(f["attributed_to"], "body");
    assert_eq!(f["symbol"], "tests.calls.sum_to");
    assert_eq!(f["iteration"], json!(5));
    assert_eq!(f["input"], json!({"n": 5}));
    let actual = json!({"bindings": {"i": 5, "n": 5, "total": 15}});
    assert_eq!(f["actual"], actual);
    let ce = counterexample(record);
    assert_eq!(ce["clause"], "total <= 10");
    assert_eq!(ce["clause_kind"], "invariant");
    assert_eq!(ce["iteration"], json!(5));
    assert_eq!(ce["input"], json!({"n": 5}));
    assert_eq!(ce["actual"], actual);
    assert_eq!(ce["found_by"], calls("calls sum_to").as_str());
    assert_eq!(
        ce["span"],
        common::location_of(CALLS, CALLS_FILE, "invariant total <= 10")
    );
}

/// 5.3: a loop measure is checked before each body after the condition
/// held: a negative one carries `measure` only, one that did not decrease
/// `previous` and `measure`; both are counterexamples (S46).
#[test]
fn loop_decreases_violations_in_a_callee_are_counterexamples() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls step_down"));
    let f = fault_of(record);
    assert_eq!(f["clause_kind"], "decreases");
    assert_eq!(f["symbol"], "tests.calls.step_down");
    assert_eq!(f["iteration"], json!(2));
    let negative = json!({"bindings": {"k": -1, "n": 3}, "measure": -1});
    assert_eq!(f["actual"], negative);
    let ce = counterexample(record);
    assert_eq!(ce["clause"], "k");
    assert_eq!(ce["clause_kind"], "decreases");
    assert_eq!(ce["iteration"], json!(2));
    assert_eq!(ce["input"], json!({"n": 3}));
    assert_eq!(ce["actual"], negative);
    assert_eq!(
        ce["span"],
        common::location_after(CALLS, CALLS_FILE, "fn step_down(", "decreases k")
    );
    let record = by_name(&report, &calls("calls stuck"));
    let f = fault_of(record);
    assert_eq!(f["clause_kind"], "decreases");
    assert_eq!(f["iteration"], json!(1));
    let flat = json!({"bindings": {"k": 2, "n": 2}, "previous": 2, "measure": 2});
    assert_eq!(f["actual"], flat);
    let ce = counterexample(record);
    assert_eq!(ce["actual"], flat);
    assert_eq!(ce["iteration"], json!(1));
    assert_eq!(
        ce["span"],
        common::location_after(CALLS, CALLS_FILE, "fn stuck(", "decreases k")
    );
}

/// S35: a header measure that does not decrease at the inner entry is a
/// `decreases` violation with `actual: {measure, previous}` and `input`; it
/// has no `iteration` (that is for loops) and it is a counterexample.
#[test]
fn a_header_decreases_violation_is_a_counterexample_without_iteration() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls again"));
    let f = fault_of(record);
    assert_eq!(f["clause_kind"], "decreases");
    assert_eq!(f["attributed_to"], "body");
    assert_eq!(f["symbol"], "tests.calls.again");
    assert_eq!(f["input"], json!({"n": 2}));
    assert_eq!(f["actual"], json!({"measure": 2, "previous": 2}));
    let ce = counterexample(record);
    assert_eq!(ce["clause"], "n");
    assert_eq!(ce["clause_kind"], "decreases");
    assert_eq!(ce["input"], json!({"n": 2}));
    assert_eq!(ce["actual"], json!({"measure": 2, "previous": 2}));
    assert!(ce.get("iteration").is_none());
    assert_eq!(
        ce["span"],
        common::location_of(CALLS, CALLS_FILE, "decreases n")
    );
}

/// S34: a clause that faults reports that fault (`facet: "contract"`,
/// `clause`, `symbol` the clause's owner), not a `contract_violation`, so
/// it is no counterexample (S46 needs a `contract_violation`).
#[test]
fn a_faulting_ensures_is_its_fault_and_no_counterexample_s34() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls zero_ok"));
    let f = fault_of(record);
    assert_eq!(f["code"], "division_by_zero");
    assert_eq!(f["symbol"], "tests.calls.zero_ok");
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "10 / result > 0");
    assert_eq!(f["input"], json!({"a": 10, "b": 0}));
    assert!(record.counterexamples.is_empty());
}

/// 6.10: a builtin's violated precondition names the stub (`symbol`
/// `std.prelude.List.take`, its clause and span), `input` with the receiver
/// as `self`, and `caller` the innermost active declaration, the function
/// the test called; no counterexample.
#[test]
fn a_builtin_precondition_inside_a_called_function_names_that_function() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls first_of"));
    let f = fault_of(record);
    assert_eq!(f["code"], "contract_violation");
    assert_eq!(f["clause_kind"], "requires");
    assert_eq!(f["symbol"], "std.prelude.List.take");
    assert_eq!(f["clause"], "n >= 0");
    assert_eq!(f["input"], json!({"self": [4, 5], "n": -1}));
    assert_eq!(f["caller"], "tests.calls.first_of");
    assert_eq!(f["span"]["file"], "std/prelude.cpl");
    assert_eq!(
        frames(&f),
        [
            "std.prelude.List.take".to_string(),
            "tests.calls.first_of".to_string(),
            calls("calls first_of")
        ]
    );
    assert!(record.counterexamples.is_empty());
}

/// 5.3: the invariants run once before the first test of the condition, so
/// a loop whose body never runs still checks them (iteration 0).
#[test]
fn an_invariant_is_checked_on_entry_even_when_the_body_never_runs() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls never_ok"));
    let f = fault_of(record);
    assert_eq!(f["clause_kind"], "invariant");
    assert_eq!(f["iteration"], json!(0));
    assert_eq!(f["input"], json!({"n": 0}));
    assert_eq!(f["actual"], json!({"bindings": {"i": 0, "n": 0}}));
    let ce = counterexample(record);
    assert_eq!(ce["clause"], "i < 0");
    assert_eq!(ce["iteration"], json!(0));
}

/// 5.3: the measure is evaluated before each body and "never evaluated
/// after the loop exits", where `k - 1` would be -1.
#[test]
fn a_loop_measure_is_never_evaluated_after_the_loop_exits() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls exits_cleanly"));
    assert_eq!(record.status, TestStatus::Tested, "{}", stable(record));
}

/// 5.2 step 5: every exit evaluates the `ensures` clauses, `return e` and a
/// `?` that propagates an `Err` included (the propagated value is the exit
/// value; a `Result`-returning function's `ensures` sees the whole
/// `Result`). The `message` of a parser's `ConstraintError` is informative
/// only (stdlib 1.5), so only its `type_name` is compared.
#[test]
fn ensures_run_on_every_exit_return_and_propagation_included() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls early"));
    let f = fault_of(record);
    assert_eq!(f["clause_kind"], "ensures");
    assert_eq!(f["symbol"], "tests.calls.early");
    assert_eq!(f["input"], json!({"n": -2}));
    assert_eq!(f["actual"], json!(-2));
    assert_eq!(counterexample(record)["actual"], json!(-2));
    let record = by_name(&report, &calls("calls parsed"));
    let f = fault_of(record);
    assert_eq!(f["clause_kind"], "ensures");
    assert_eq!(f["clause"], "result.is_ok()");
    assert_eq!(f["input"], json!({"t": "x"}));
    let ce = counterexample(record);
    for actual in [&f["actual"], &ce["actual"]] {
        assert_eq!(actual["$type"], "Result");
        assert_eq!(actual["$variant"], "Err");
        assert_eq!(actual["$payload"][0]["$type"], "ConstraintError");
        assert_eq!(actual["$payload"][0]["type_name"], "Int");
    }
}

/// S70: a `requires` violation raised by a function an `ensures` clause
/// calls is claimed by that clause (`symbol` its owner, `clause` its text),
/// while `clause_kind`, `attributed_to`, `caller`, `input`, `span`, and the
/// backtrace describe the violation as raised; its kind is `requires`, so it
/// is no counterexample (S46).
#[test]
fn a_requires_violation_inside_an_ensures_is_claimed_by_it_and_is_no_counterexample() {
    let (_, report) = calls_report();
    let record = by_name(&report, &calls("calls checks_result"));
    let f = fault_of(record);
    assert_eq!(f["code"], "contract_violation");
    assert_eq!(f["clause_kind"], "requires");
    assert_eq!(f["attributed_to"], "caller");
    assert_eq!(f["symbol"], "tests.calls.checks_result");
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "positive_only(result) > 0");
    assert_eq!(f["caller"], "tests.calls.checks_result");
    assert_eq!(f["input"], json!({"n": 0}));
    assert_eq!(
        f["span"],
        common::location_of(CALLS, CALLS_FILE, "requires n > 0")
    );
    assert_eq!(
        frames(&f),
        [
            "tests.calls.positive_only".to_string(),
            "tests.calls.checks_result".to_string(),
            calls("calls checks_result")
        ]
    );
    assert!(record.counterexamples.is_empty());
}

/// 5.2-5.3 and S46: the `input` of a function without parameters is `{}`.
#[test]
fn a_counterexample_against_a_function_without_parameters_has_an_empty_input() {
    let (_, report) = calls_report();
    let ce = counterexample(by_name(&report, &calls("calls seven")));
    assert_eq!(ce["symbol"], "tests.calls.seven");
    assert_eq!(ce["clause"], "result == 8");
    assert_eq!(ce["input"], json!({}));
    assert_eq!(ce["actual"], json!(7));
}

const REFINE: &str = r#"module tests.refine

type Even = Int where is_even(self)

fn is_even(n: Int) -> Bool
    ensures result == (n % 2 == 0)
{
    true
}

test "checks an odd number" {
    let _ = check<Even>(3)
}

test "checks an even number" {
    assert check<Even>(4).is_ok()
}

property "checks every int"
    for n in gen.int
{
    let _ = check<Even>(n)
}
"#;

/// 6.10, S70, and S46: an `ensures` violation of a function that a
/// refinement predicate calls during `check<R>` is claimed by the predicate
/// (`symbol` the refinement, `clause` its text) and is a counterexample
/// against the function, found by the test or property; shrinking keeps
/// the same code, so the property's minimum is the odd value nearest 0.
#[test]
fn an_ensures_violation_inside_a_refinement_check_is_a_counterexample() {
    let p = common::check_source(REFINE);
    let report = run_tests(&p, &namespace("tests.refine"), &config(1));
    assert_eq!((report.selected, report.tested, report.failed), (3, 1, 2));
    let t = "tests.refine.\"checks an odd number\"";
    let record = by_name(&report, t);
    let f = fault_of(record);
    assert_eq!(f["code"], "contract_violation");
    assert_eq!(f["clause_kind"], "ensures");
    assert_eq!(f["attributed_to"], "body");
    assert_eq!(f["symbol"], "tests.refine.Even");
    assert_eq!(f["facet"], "contract");
    assert_eq!(f["clause"], "is_even(self)");
    assert_eq!(f["input"], json!({"n": 3}));
    assert_eq!(f["actual"], json!(true));
    let names = frames(&f);
    assert_eq!(
        names.first().map(String::as_str),
        Some("tests.refine.is_even")
    );
    assert_eq!(names.last().map(String::as_str), Some(t));
    let ce = counterexample(record);
    assert_eq!(ce["symbol"], "tests.refine.is_even");
    assert_eq!(ce["clause"], "result == (n % 2 == 0)");
    assert_eq!(ce["input"], json!({"n": 3}));
    assert_eq!(ce["actual"], json!(true));
    assert_eq!(ce["found_by"], t);
    let q = "tests.refine.\"checks every int\"";
    let binders = [("n", G::Int)];
    let prop = Prop::new(&p, q, q, &binders);
    let model = |t: &[V]| (t[0].int() % big(2) != big(0)).then_some("contract_violation");
    for seed in SEEDS {
        let record = prop.check(&config(seed), &model);
        let n = shrunk_input(&record)["n"].as_i64().expect("a small Int");
        assert!(n == 1 || n == -1, "seed {seed}: {n}");
        assert_eq!(counterexample(&record)["input"], json!({"n": n}));
    }
}

// ===========================================================================
// examples/reader (semantics 11; toolchain 12, 12.3)
// ===========================================================================

const PRESERVES: &str = "tests.pagination.\"pagination preserves input\"";
const SHORT_TAIL: &str = "tests.pagination.\"pagination preserves a short tail\"";
const EMPTY_INPUT: &str = "tests.pagination.\"pagination handles empty input\"";
const PAGES_FILE: &str = "src/reader/pages.cpl";

/// `examples/reader` with `from` replaced by `to` (exactly once) in
/// `src/reader/pages.cpl`, checked; returns the program and the edited text.
fn reader_with(from: &str, to: &str) -> (Program, String) {
    let dir = common::example_dir("reader");
    let read = |path: &str| {
        std::fs::read_to_string(dir.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    };
    let original = read(PAGES_FILE);
    assert_eq!(
        original.matches(from).count(),
        1,
        "the edit's anchor occurs once"
    );
    let pages = original.replace(from, to);
    let manifest = read("cpl.toml");
    let images = read("src/reader/images.cpl");
    let tests = read("src/tests/pagination.cpl");
    let p = common::check_files(&[
        ("cpl.toml", manifest.as_str()),
        (PAGES_FILE, pages.as_str()),
        ("src/reader/images.cpl", images.as_str()),
        ("src/tests/pagination.cpl", tests.as_str()),
    ]);
    (p, pages)
}

fn reader_binders() -> [(&'static str, G); 2] {
    [("items", list(G::Int)), ("size", G::Pos)]
}

/// semantics 11, toolchain 12.3: the design's three tests are `tested` for
/// any run seed (the design's `paginate` is correct, so the property holds
/// on every case the oracle draws), the property runs 100 cases, more when
/// asked even though sizes stop at 100, and its seed is keyed by its
/// qualified name.
#[test]
fn the_designs_tests_pass_for_every_seed_tried() {
    let p = common::reader();
    let binders = reader_binders();
    let prop = Prop::new(&p, PRESERVES, PRESERVES, &binders);
    for seed in [0, 2, 3, 99, 0x5eed, u64::MAX] {
        let report = common::run_all_tests(&p, seed);
        assert_eq!(
            (report.selected, report.tested, report.failed),
            (3, 3, 0),
            "seed {seed}"
        );
        let record = by_name(&report, PRESERVES);
        assert_eq!(
            stable(record),
            stable(&prop.check(&config(seed), &|_: &[V]| None))
        );
    }
    let record = prop.check(
        &TestConfig {
            cases: 250,
            ..config(8)
        },
        &|_: &[V]| None,
    );
    assert_eq!(record.cases, Some(250));
    let pick = |sel: TestSelector| names_of(&p, &select_tests(&p, &sel));
    assert_eq!(
        pick(namespace("tests")),
        [EMPTY_INPUT, SHORT_TAIL, PRESERVES]
    );
    assert_eq!(pick(namespace("tests.pagination.*")).len(), 3);
    assert!(pick(namespace("reader")).is_empty());
    assert_eq!(
        pick(selector(None, Some("pagination preserves input"), None)),
        [PRESERVES]
    );
}

/// The design's property against `paginate` with `rest = rest.drop(size +
/// 1)`: the loop loses one item per page, so the first invariant fails
/// after the first iteration exactly when `items.len > size`. The oracle
/// predicts the first failing case and its shrinking; the minimum is
/// `items = [0, 0]`, `size = 1`, and the counterexample is the design's
/// shape against `pgn_7c2d` with the loop's `iteration` and `bindings`.
#[test]
fn the_designs_property_finds_an_invariant_counterexample_in_a_mutated_paginate() {
    let (p, pages) = reader_with(
        "        rest = rest.drop(size)\n",
        "        rest = rest.drop(size + 1)\n",
    );
    let binders = reader_binders();
    let prop = Prop::new(&p, PRESERVES, PRESERVES, &binders);
    let model =
        |t: &[V]| (BigInt::from(t[0].length()) > *t[1].int()).then_some("contract_violation");
    let span = common::location_of(
        &pages,
        PAGES_FILE,
        "invariant flatten(pages) ++ rest == items",
    );
    let actual = json!({"bindings": {"items": [0, 0], "pages": [[0]], "rest": [], "size": 1}});
    let mut failures = 0;
    for seed in [1, 2, 3, 4] {
        let record = prop.check(&config(seed), &model);
        if record.status == TestStatus::Tested {
            continue;
        }
        failures += 1;
        assert_eq!(shrunk_input(&record), json!({"items": [0, 0], "size": 1}));
        let f = fault_of(&record);
        assert_eq!(f["clause_kind"], "invariant");
        assert_eq!(f["attributed_to"], "body");
        assert_eq!(f["symbol"], "pgn_7c2d");
        assert_eq!(f["qualified_name"], "reader.pages.paginate");
        assert_eq!(f["facet"], "contract");
        assert_eq!(f["clause"], "flatten(pages) ++ rest == items");
        assert_eq!(f["iteration"], json!(1));
        assert_eq!(f["input"], json!({"items": [0, 0], "size": 1}));
        assert_eq!(f["actual"], actual);
        assert_eq!(f["span"], span);
        assert_eq!(frames(&f), ["reader.pages.paginate", PRESERVES]);
        let ce = counterexample(&record);
        assert_eq!(ce["symbol"], "pgn_7c2d");
        assert_eq!(ce["facet"], "body");
        assert_eq!(ce["clause"], "flatten(pages) ++ rest == items");
        assert_eq!(ce["clause_kind"], "invariant");
        assert_eq!(ce["iteration"], json!(1));
        assert_eq!(ce["input"], json!({"items": [0, 0], "size": 1}));
        assert_eq!(ce["actual"], actual);
        assert_eq!(ce["status"], "failed");
        assert_eq!(ce["found_by"], PRESERVES);
        assert_eq!(ce["span"], span);
    }
    assert!(failures > 0);
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    assert_eq!(by_name(&report, EMPTY_INPUT).status, TestStatus::Tested);
    let tail = by_name(&report, SHORT_TAIL);
    let ce = counterexample(tail);
    assert_eq!(ce["input"], json!({"items": [1, 2, 3], "size": 2}));
    assert_eq!(
        ce["actual"],
        json!({"bindings": {"items": [1, 2, 3], "pages": [[1, 2]], "rest": [], "size": 2}})
    );
    assert_eq!(ce["iteration"], json!(1));
    assert_eq!(ce["found_by"], SHORT_TAIL);
}

/// The design's property against `paginate` with `decreases size -
/// rest.len`: the measure is negative the first time the loop is entered
/// (5.3: checked before the first body, `iteration` 0, `actual` with the
/// bindings and the negative `measure`), which happens exactly when
/// `items.len > size`.
#[test]
fn the_designs_property_finds_a_decreases_counterexample_in_a_mutated_paginate() {
    let (p, pages) = reader_with(
        "        decreases rest.len\n",
        "        decreases size - rest.len\n",
    );
    let binders = reader_binders();
    let prop = Prop::new(&p, PRESERVES, PRESERVES, &binders);
    let model =
        |t: &[V]| (BigInt::from(t[0].length()) > *t[1].int()).then_some("contract_violation");
    let actual = json!({"bindings": {"items": [0, 0], "pages": [], "rest": [0, 0], "size": 1},
                        "measure": -1});
    let mut failures = 0;
    for seed in [1, 2, 3, 4] {
        let record = prop.check(&config(seed), &model);
        if record.status == TestStatus::Tested {
            continue;
        }
        failures += 1;
        assert_eq!(shrunk_input(&record), json!({"items": [0, 0], "size": 1}));
        let ce = counterexample(&record);
        assert_eq!(ce["symbol"], "pgn_7c2d");
        assert_eq!(ce["clause"], "size - rest.len");
        assert_eq!(ce["clause_kind"], "decreases");
        assert_eq!(ce["iteration"], json!(0));
        assert_eq!(ce["input"], json!({"items": [0, 0], "size": 1}));
        assert_eq!(ce["actual"], actual);
        assert_eq!(
            ce["span"],
            common::location_of(&pages, PAGES_FILE, "decreases size - rest.len")
        );
    }
    assert!(failures > 0);
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    let ce = counterexample(by_name(&report, SHORT_TAIL));
    assert_eq!(
        ce["actual"],
        json!({"bindings": {"items": [1, 2, 3], "pages": [], "rest": [1, 2, 3], "size": 2},
               "measure": -1})
    );
    assert_eq!(ce["iteration"], json!(0));
}

/// The design's `paginate` with its final push made unconditional (`if
/// true`): for empty `items` it returns `[[]]`, which violates the second
/// `ensures` (a `forall` clause) and no other clause; the design's
/// empty-input test and its property (whose case 0, at size 0, always draws
/// `items = []`, stdlib 8.2-8.3) both find that counterexample, and the
/// property shrinks `size` to 1.
#[test]
fn the_designs_tests_find_a_forall_ensures_counterexample_for_an_empty_page() {
    let (p, pages) = reader_with("    if not rest.is_empty() {\n", "    if true {\n");
    let binders = reader_binders();
    let prop = Prop::new(&p, PRESERVES, PRESERVES, &binders);
    let model = |t: &[V]| (t[0].length() == 0).then_some("contract_violation");
    let clause = "forall page in result: page.len > 0 and page.len <= size";
    let span = common::location_of(&pages, PAGES_FILE, &format!("ensures {clause}"));
    for seed in [1, 2, 3] {
        let record = prop.check(&config(seed), &model);
        assert_eq!(record.failing_case.as_ref().unwrap().index, 0);
        assert_eq!(shrunk_input(&record), json!({"items": [], "size": 1}));
        let ce = counterexample(&record);
        assert_eq!(ce["symbol"], "pgn_7c2d");
        assert_eq!(ce["clause"], clause);
        assert_eq!(ce["clause_kind"], "ensures");
        assert_eq!(ce["input"], json!({"items": [], "size": 1}));
        assert_eq!(ce["actual"], json!([[]]));
        assert_eq!(ce["span"], span);
    }
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    assert_eq!((report.selected, report.tested, report.failed), (3, 1, 2));
    assert_eq!(by_name(&report, SHORT_TAIL).status, TestStatus::Tested);
    let ce = counterexample(by_name(&report, EMPTY_INPUT));
    assert_eq!(ce["clause"], clause);
    assert_eq!(ce["input"], json!({"items": [], "size": 2}));
    assert_eq!(ce["actual"], json!([[]]));
    assert_eq!(ce["found_by"], EMPTY_INPUT);
}

/// The design's property against `paginate` with `rest.take(size - 2)`: for
/// `size >= 2` the loop loses two items per page (an invariant violation),
/// for `size = 1` `take` gets -1 (the builtin's own `requires`, 6.10). Both
/// are `contract_violation`, so shrinking crosses from one to the other
/// (S45), and the minimum `[0, 0]`, `1` reports the builtin's precondition
/// with `paginate` as `caller` and no counterexample (S46).
#[test]
fn shrinking_the_designs_property_can_end_in_a_builtin_precondition() {
    let (p, _) = reader_with(
        "        pages = pages.push(rest.take(size))\n",
        "        pages = pages.push(rest.take(size - 2))\n",
    );
    let binders = reader_binders();
    let prop = Prop::new(&p, PRESERVES, PRESERVES, &binders);
    let model =
        |t: &[V]| (BigInt::from(t[0].length()) > *t[1].int()).then_some("contract_violation");
    let mut crossed = false;
    for seed in 1..=6 {
        let record = prop.check(&config(seed), &model);
        if record.status == TestStatus::Tested {
            continue;
        }
        assert_eq!(shrunk_input(&record), json!({"items": [0, 0], "size": 1}));
        let f = fault_of(&record);
        assert_eq!(f["clause_kind"], "requires");
        assert_eq!(f["symbol"], "std.prelude.List.take");
        assert_eq!(f["clause"], "n >= 0");
        assert_eq!(f["input"], json!({"self": [0, 0], "n": -1}));
        assert_eq!(f["caller"], "reader.pages.paginate");
        assert!(record.counterexamples.is_empty(), "seed {seed}");
        crossed |= failing_input(&record)["size"]
            .as_i64()
            .is_none_or(|s| s >= 2);
    }
    assert!(crossed, "some seed starts from an invariant violation");
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    let ce = counterexample(by_name(&report, SHORT_TAIL));
    assert_eq!(ce["clause_kind"], "invariant");
    assert_eq!(
        ce["actual"],
        json!({"bindings": {"items": [1, 2, 3], "pages": [[]], "rest": [3], "size": 2}})
    );
}

// ===========================================================================
// fail_fast, independence, and determinism (toolchain 1.7.2, 7.1; S44, S31)
// ===========================================================================

const FF: &str = r#"module tests.ff

test "a passes" {
    assert true
}

test "b fails" {
    assert 1 == 0
}

property "c fails"
    for x in gen.int
{
    assert false
}

test "d passes" {
    assert true
}
"#;

const FF2: &str = r#"module tests.ff2

test "a passes" {
    assert true
}

property "b fails"
    for xs in gen.list(gen.int)
{
    assert xs.len < 2
}

test "c passes" {
    assert true
}

test "d fails" {
    assert false
}
"#;

fn qualified_names(report: &TestReport) -> Vec<&str> {
    report
        .results
        .iter()
        .map(|r| r.qualified_name.as_str())
        .collect()
}

/// toolchain 1.7.2: with `fail_fast` the run stops after the first failing
/// record in run order; `selected` still counts the whole selection and
/// `tested` the records that ran and passed; the records that ran are the
/// ones a full run produces.
#[test]
fn fail_fast_stops_after_the_first_failing_record_in_run_order() {
    let p = common::check_files(&[("src/tests/ff.cpl", FF), ("src/tests/ff2.cpl", FF2)]);
    let fast = |c: TestConfig| TestConfig {
        fail_fast: true,
        ..c
    };
    for module in ["tests.ff", "tests.ff2"] {
        let full = run_tests(&p, &namespace(module), &config(2));
        assert_eq!(
            (full.selected, full.tested, full.failed),
            (4, 2, 2),
            "{module}"
        );
        let stopped = run_tests(&p, &namespace(module), &fast(config(2)));
        assert_eq!(
            (stopped.selected, stopped.tested, stopped.failed),
            (4, 1, 1),
            "{module}"
        );
        assert_eq!(
            qualified_names(&stopped),
            [
                format!("{module}.\"a passes\""),
                format!("{module}.\"b fails\"")
            ]
        );
        for (a, b) in stopped.results.iter().zip(&full.results) {
            assert_eq!(stable(a), stable(b));
        }
    }
    let both = run_tests(&p, &TestSelector::all(), &fast(config(2)));
    assert_eq!((both.selected, both.tested, both.failed), (8, 1, 1));
    assert_eq!(
        qualified_names(&both),
        ["tests.ff.\"a passes\"", "tests.ff.\"b fails\""]
    );
    let passing = run_tests(
        &p,
        &selector(None, Some("a passes"), None),
        &fast(config(2)),
    );
    assert_eq!(
        (passing.selected, passing.tested, passing.failed),
        (2, 2, 0)
    );
}

const INDEP_A: &str = r#"module tests.indep

property "p"
    for xs in gen.list(gen.int)
    for n in gen.positive_int
{
    assert xs.len < n
}

property "q"
    for t in gen.text
    for b in gen.bool
{
    assert t.len < 3 or not b
}
"#;

const INDEP_B: &str = r#"module tests.indep

test "a first" {
    assert true
}

property "o"
    for x in gen.int
{
    assert x < 5
}

property "p"
    for xs in gen.list(gen.int)
    for n in gen.positive_int
{
    assert xs.len < n
}

test "p2" {
    assert false
}

property "q"
    for t in gen.text
    for b in gen.bool
{
    assert t.len < 3 or not b
}

property "r"
    for xs in gen.list(gen.bool)
{
    assert xs.len < 2
}
"#;

const INDEP_OTHER: &str = r#"module tests.other

property "p"
    for xs in gen.list(gen.int)
    for n in gen.positive_int
{
    assert xs.len < n
}
"#;

/// S44 and stdlib S31: one stream per property keyed by its identity, so
/// adding, removing, or reordering other tests and properties, and how the
/// property is selected, never changes its cases or its shrinking.
#[test]
fn a_propertys_record_depends_on_nothing_but_its_identity_and_the_seed() {
    let a = common::check_source(INDEP_A);
    let b = common::check_files(&[
        ("src/tests/indep.cpl", INDEP_B),
        ("src/tests/other.cpl", INDEP_OTHER),
    ]);
    let p_binders = [("xs", list(G::Int)), ("n", G::Pos)];
    let q_binders = [("t", G::Text), ("b", G::Bool)];
    let p_model =
        |t: &[V]| (BigInt::from(t[0].length()) >= *t[1].int()).then_some("assertion_failed");
    let q_model = |t: &[V]| (t[0].length() >= 3 && t[1].flag()).then_some("assertion_failed");
    let p = "tests.indep.\"p\"";
    let q = "tests.indep.\"q\"";
    for seed in [1, 2, 3] {
        let in_a = run_tests(&a, &TestSelector::all(), &config(seed));
        let in_b = run_tests(&b, &TestSelector::all(), &config(seed));
        for (name, binders, model) in [
            (p, &p_binders[..], &p_model as Model<'_>),
            (q, &q_binders[..], &q_model as Model<'_>),
        ] {
            let ra = by_name(&in_a, name);
            let rb = by_name(&in_b, name);
            assert_eq!(case_view(ra), case_view(rb), "{name}, seed {seed}");
            Prop::new(&a, name, name, binders).check(&config(seed), model);
            let alone = run_test(&b, common::def(&b, name), &config(seed));
            assert_eq!(stable(&alone), stable(rb), "{name}: run_test");
            let short = name
                .trim_start_matches("tests.indep.\"")
                .trim_end_matches('"');
            let by_filter = run_tests(
                &b,
                &selector(Some("tests.indep"), Some(short), None),
                &config(seed),
            );
            assert_eq!(by_filter.results.len(), 1);
            assert_eq!(stable(&by_filter.results[0]), stable(rb), "{name}: --name");
        }
        // Another module's property of the same name is another identity.
        assert_ne!(
            by_name(&in_b, "tests.other.\"p\"").property_seed,
            by_name(&in_b, p).property_seed
        );
    }
}

/// toolchain 7.1 "Determinism" and the property-runner row of toolchain 12:
/// two runs with one seed give identical reports, wall times aside.
#[test]
fn two_runs_with_one_seed_give_identical_reports() {
    let b = common::check_files(&[
        ("src/tests/indep.cpl", INDEP_B),
        ("src/tests/other.cpl", INDEP_OTHER),
    ]);
    let setup = common::check_source(SETUP);
    let gens = common::check_source(GENS);
    for p in [&b, &setup, &gens] {
        for seed in [5, 6] {
            let first = run_tests(p, &TestSelector::all(), &config(seed));
            let second = run_tests(p, &TestSelector::all(), &config(seed));
            assert!(first.failed > 0);
            assert_eq!(stable_report(&first), stable_report(&second), "seed {seed}");
        }
    }
}

// ===========================================================================
// What the checker must reject (stdlib 8.1, S28; semantics S14, S42, S43)
// ===========================================================================

fn errors_of(source: &str) -> Vec<String> {
    common::codes_of(&common::check_source_result(source), Severity::Error)
}

/// S43, stdlib S30: a `for` range sees no binder, not even its own or a
/// later one; a range that calls a pure function is accepted.
#[test]
fn a_for_range_cannot_mention_a_binder_s43() {
    for range in ["gen.range(0, n)", "gen.list(gen.range(n, 3))"] {
        let source = format!(
            "module tests.bad\n\nproperty \"dependent\"\n    for n in gen.positive_int\n    for xs in {range}\n{{\n    assert true\n}}\n"
        );
        assert_eq!(errors_of(&source), ["unresolved_name"], "{range}");
    }
    let own = "module tests.bad\n\nproperty \"own\"\n    for n in gen.range(0, n)\n{\n    assert true\n}\n";
    assert_eq!(errors_of(own), ["unresolved_name"]);
    let later = "module tests.bad\n\nproperty \"later\"\n    for xs in gen.range(0, m)\n    for m in gen.int\n{\n    assert true\n}\n";
    assert_eq!(errors_of(later), ["unresolved_name"]);
    let pure = "module tests.ok\n\nfn three() -> Int {\n    3\n}\n\nproperty \"pure\"\n    for x in gen.range(0, three())\n{\n    assert x <= 3\n}\n";
    assert!(errors_of(pure).is_empty());
}

/// S8: a `for` binder may not reuse the name of an earlier binder, and a
/// local of the body may not reuse a binder's name (`shadowed_binding`).
#[test]
fn binders_cannot_be_shadowed_s8() {
    let twice = "module tests.bad\n\nproperty \"twice\"\n    for x in gen.int\n    for x in gen.bool\n{\n    assert true\n}\n";
    assert_eq!(errors_of(twice), ["shadowed_binding"]);
    let shadow = "module tests.bad\n\nproperty \"shadow\"\n    for x in gen.int\n{\n    let x = 1\n    assert x == 1\n}\n";
    assert_eq!(errors_of(shadow), ["shadowed_binding"]);
}

/// 5.1 and S64: a `for` range is a contract expression: a hole in it is
/// `hole_in_contract` alone (no `hole` report beside it), a `?` in it
/// `propagate_in_contract` alone (it takes precedence over
/// `propagate_outside_function`).
#[test]
fn a_for_range_is_a_contract_expression_5_1() {
    let hole = common::check_source_result(
        "module tests.bad\n\nproperty \"h\"\n    for x in gen.range(0, ?h)\n{\n    assert x == x\n}\n",
    );
    assert_eq!(
        common::codes_of(&hole, Severity::Error),
        ["hole_in_contract"]
    );
    assert!(
        common::codes_of(&hole, Severity::Info).is_empty(),
        "no hole report beside it (S64)"
    );
    let propagate = "module tests.bad\n\nfn f() -> Result<Int, Text> {\n    Ok(1)\n}\n\nproperty \"q\"\n    for x in gen.range(0, f()?)\n{\n    assert x == x\n}\n";
    assert_eq!(errors_of(propagate), ["propagate_in_contract"]);
}

/// S14 and stdlib S28: a generator appears only as a `for` range or as an
/// argument of a `std.gen` function; `gen.int` is a property, not a call;
/// nothing of `std.gen` can be imported; `Gen` is not nameable.
#[test]
fn generators_appear_only_in_for_clauses_s14() {
    let wrap = |body: &str| format!("module tests.bad\n\n{body}");
    let cases = [
        (
            "test \"t\" {\n    let g = gen.int\n}\n",
            "generator_outside_for",
        ),
        (
            "property \"p\"\n    for x in gen.int()\n{\n    assert true\n}\n",
            "property_called",
        ),
        (
            "use std.gen.{int}\n\ntest \"t\" {\n    assert true\n}\n",
            "generator_import",
        ),
        (
            "use std.gen.{Gen}\n\ntest \"t\" {\n    assert true\n}\n",
            "generator_import",
        ),
        (
            "test \"t\" {\n    let xs: List<Gen<Int>> = []\n    assert xs.len == 0\n}\n",
            "unresolved_type",
        ),
    ];
    for (body, code) in cases {
        let errors = errors_of(&wrap(body));
        assert!(!errors.is_empty(), "{body}: accepted");
        assert!(
            errors.iter().all(|e| e == code),
            "{body}: expected only {code}, got {errors:?}"
        );
    }
    let equality = errors_of(&wrap("test \"t\" {\n    assert gen.int == gen.int\n}\n"));
    assert!(!equality.is_empty(), "comparing generators is rejected");
    assert!(
        equality.iter().all(|e| e == "generator_outside_for"),
        "{equality:?}"
    );
    let not_a_generator = errors_of(&wrap(
        "property \"p\"\n    for x in 5\n{\n    assert true\n}\n",
    ));
    assert!(!not_a_generator.is_empty(), "a for range must be a Gen<T>");
}

/// S42 and S3: a test or property name is non-empty and unique in its
/// module's test namespace, which tests and properties share and which
/// spans the module's files; the same name in two modules, or a test named
/// like a function, is fine.
#[test]
fn test_names_are_non_empty_and_unique_per_module_s42() {
    assert_eq!(
        errors_of("module tests.bad\n\ntest \"\" {\n    assert true\n}\n"),
        ["empty_test_name"]
    );
    let shared = "module tests.bad\n\ntest \"a\" {\n    assert true\n}\n\nproperty \"a\"\n    for x in gen.bool\n{\n    assert x == x\n}\n";
    assert_eq!(errors_of(shared), ["duplicate_test_name"]);
    let first = "module tests.dup\n\ntest \"a\" {\n    assert true\n}\n";
    let second = "module tests.dup\n\ntest \"a\" {\n    assert true\n}\n";
    let result = common::check_files_result(&[
        ("src/tests/dup.cpl", first),
        ("src/tests/dup/more.cpl", second),
    ]);
    assert_eq!(
        common::codes_of(&result, Severity::Error),
        ["duplicate_test_name"]
    );
    let p = common::check_files(&[
        (
            "src/tests/one.cpl",
            "module tests.one\n\nfn a() -> Int {\n    1\n}\n\ntest \"a\" {\n    assert a() == 1\n}\n",
        ),
        ("src/tests/two.cpl", "module tests.two\n\ntest \"a\" {\n    assert true\n}\n"),
    ]);
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    assert_eq!(
        qualified_names(&report),
        ["tests.one.\"a\"", "tests.two.\"a\""]
    );
    assert_eq!(report.tested, 2);
    assert!(p.lookup(&common::path("tests.one.a")).is_some());
}
