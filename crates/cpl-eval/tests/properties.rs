//! The property runner (the "property runner" row of `spec/toolchain.md`
//! 12; toolchain 1.7.2; semantics S44-S46, S71; stdlib 8.2-8.4) on programs
//! checked by the real checker.
//!
//! * A deliberately wrong `paginate` (the design's, with the final push of
//!   the partial page removed) yields a `contract_counterexample` for a
//!   fixed seed, with the shrunk inputs derived below from the shrink rules
//!   of S45 and stdlib 8.4; the failing case itself is re-derived from the
//!   draws of stdlib 8.3 ([`draws`]).
//! * Determinism: two runs with one seed give identical reports.
//! * The runner's rules on small properties: `--cases N` prefixes, the
//!   record of S46, each generator's shrink minimum, the same-code rule,
//!   the shrink budget, and the setup of `gen.range` bounds, including the
//!   S71 shape of a fault raised while a range is evaluated and the one-line
//!   clause text of a range (toolchain 5.2, T56).

mod common;

use cpl_diag::json::{TestCaseRecord, TestKind, TestStatus};
use cpl_eval::test::TestReport;
use cpl_eval::{property_seed, run_test, run_tests, seed_hex, TestConfig, TestSelector};
use cpl_hir::{DefId, Program};
use serde_json::{json, Value as Json};

fn config(seed: u64) -> TestConfig {
    TestConfig::with_seed(seed)
}

/// A record as JSON with the wall time cleared (the one field that may
/// differ between two runs).
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

fn shrunk_input(record: &TestCaseRecord) -> Json {
    Json::Object(record.shrunk.as_ref().expect("shrunk").input.clone())
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

fn fault(record: &TestCaseRecord) -> Json {
    serde_json::to_value(record.fault.as_ref().expect("a fault")).expect("a fault serializes")
}

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

// ===========================================================================
// The design's property against a deliberately wrong paginate
// ===========================================================================

const PRESERVES_INPUT: &str = "tests.pagination.\"pagination preserves input\"";

/// `examples/reader` with the design's `paginate` made wrong: the
/// `if not rest.is_empty() { pages = pages.push(rest) }` that pushes the
/// final partial page is removed. The loop runs while `rest.len > size`, so
/// for every non-empty input the last `1..=size` items stay in `rest` and
/// are lost: the first `ensures` (`flatten(result) == items`) fails exactly
/// when `items` is not empty, whatever `size` is; the other two `ensures`
/// still hold, and the empty input still passes. Everything else, the
/// `@id("pgn_7c2d")` included, is the example's text.
fn wrong_reader() -> Program {
    let dir = common::example_dir("reader");
    let read = |path: &str| std::fs::read_to_string(dir.join(path)).expect("an example file");
    let pages = read("src/reader/pages.cpl");
    let tail = "    if not rest.is_empty() {\n        pages = pages.push(rest)\n    }\n\n";
    assert!(
        pages.contains(tail),
        "the design's paginate pushes its tail"
    );
    let wrong = pages.replace(tail, "");
    let manifest = read("cpl.toml");
    let images = read("src/reader/images.cpl");
    let tests = read("src/tests/pagination.cpl");
    common::check_files(&[
        ("cpl.toml", manifest.as_str()),
        ("src/reader/pages.cpl", wrong.as_str()),
        ("src/reader/images.cpl", images.as_str()),
        ("src/tests/pagination.cpl", tests.as_str()),
    ])
}

/// The draws of stdlib 8.3 for `for items in gen.list(gen.int)` and
/// `for size in gen.positive_int`, re-derived from the text: one
/// `ChaCha20Rng::from_seed(property_seed)` stream per property (S44,
/// stdlib 8.2), case `i` at size `min(i, 100)`, the clauses drawing in
/// clause order.
mod draws {
    use num_bigint::{BigInt, BigUint};
    use num_traits::{One, Signed, ToPrimitive, Zero};
    use rand_chacha::rand_core::{Rng, SeedableRng};
    use rand_chacha::ChaCha20Rng;

    pub struct Stream(ChaCha20Rng);

    impl Stream {
        pub fn new(seed: [u8; 32]) -> Stream {
            Stream(ChaCha20Rng::from_seed(seed))
        }

        /// `u()`: the next `u32` word.
        fn u(&mut self) -> u32 {
            self.0.next_u32()
        }

        /// `bits(b)`: `ceil(b / 32)` words, little-endian, the low `b` bits.
        fn bits(&mut self, b: u64) -> BigUint {
            let mut x = BigUint::zero();
            for i in 0..b.div_ceil(32) {
                x += BigUint::from(self.u()) << (32 * i as usize);
            }
            x % (BigUint::one() << (b as usize))
        }

        /// `below(k)`: `bits(bit length of k - 1)` until it is below `k`.
        fn below(&mut self, k: u64) -> u64 {
            let k = BigUint::from(k);
            let b = (&k - BigUint::one()).bits();
            loop {
                let x = self.bits(b);
                if x < k {
                    return x.to_u64().expect("below a u64");
                }
            }
        }

        /// `gen.int` at size `s`.
        pub fn int(&mut self, s: u64) -> BigInt {
            match self.u() % 8 {
                0 => BigInt::zero(),
                1 => {
                    if self.u() % 2 == 1 {
                        -BigInt::one()
                    } else {
                        BigInt::one()
                    }
                }
                2 if s >= 8 => {
                    let length = 64 + self.below(8 * s + 1);
                    let top = BigUint::one() << ((length - 1) as usize);
                    let magnitude = BigInt::from(top + self.bits(length - 1));
                    if self.u() % 2 == 1 {
                        -magnitude
                    } else {
                        magnitude
                    }
                }
                _ => BigInt::from(self.below(2 * s + 1)) - BigInt::from(s),
            }
        }

        /// `gen.positive_int`: `1 + v.abs()` for a `gen.int` draw `v`.
        pub fn positive(&mut self, s: u64) -> BigInt {
            BigInt::one() + self.int(s).abs()
        }

        /// `gen.list(gen.int)`: a length `below(s + 1)`, then the elements.
        pub fn ints(&mut self, s: u64) -> Vec<BigInt> {
            let len = self.below(s + 1);
            (0..len).map(|_| self.int(s)).collect()
        }
    }

    /// An `Int` as value JSON: a number with every digit.
    fn int_json(n: &BigInt) -> serde_json::Value {
        serde_json::from_str(&n.to_string()).expect("an Int is a JSON number")
    }

    /// The first case of `pagination preserves input` whose `items` is not
    /// empty, the one the wrong `paginate` fails: its index and its tuple as
    /// the `input` object of `failing_case`.
    pub fn first_nonempty(run_seed: u64) -> (u32, serde_json::Value) {
        // S44: blake3(run seed as 8 little-endian bytes ++ the identity).
        let mut key = run_seed.to_le_bytes().to_vec();
        key.extend_from_slice(super::PRESERVES_INPUT.as_bytes());
        let mut st = Stream::new(*blake3::hash(&key).as_bytes());
        for i in 0..100u32 {
            let s = u64::from(i.min(100));
            let items = st.ints(s);
            let size = st.positive(s);
            if !items.is_empty() {
                let items: Vec<_> = items.iter().map(int_json).collect();
                return (
                    i,
                    serde_json::json!({"items": items, "size": int_json(&size)}),
                );
            }
        }
        panic!("no non-empty case in 100");
    }
}

#[test]
fn a_wrong_paginate_yields_a_contract_counterexample() {
    let p = wrong_reader();
    let property = common::def(&p, PRESERVES_INPUT);

    // ---- seed 1 -----------------------------------------------------------
    //
    // The draws (stdlib 8.3, `draws`): case 0 runs at size 0, where
    // `gen.list` draws the length `below(1) = 0`, so `items = []`, which the
    // wrong paginate still handles; case 1 (size 1) draws `items = [-1]` and
    // `size = 1`, and fails with the `contract_violation` of the first
    // `ensures` (`flatten([]) != [-1]`). `cases` is therefore 2 (S46).
    //
    // Shrinking (S45, stdlib 8.4): binders in clause order, `items` first;
    // each candidate is one body rerun (a step); the first candidate that
    // fails with the same code (`contract_violation`) is accepted and the
    // walk continues with the same binder from it; a candidate equal to the
    // value is never produced; duplicates are not removed.
    //
    //   pass 1, items = [-1]: the candidates are `[]`, the first half
    //     `take(0) = []`, (the second half `drop(0) = [-1]` equals the value
    //     and is not a candidate), without element 0 `[]`, then element 0
    //     replaced by each `gen.int` candidate of -1: `0`, `-1 / 2 = 0`,
    //     `-1 + 1 = 0`, giving `[0]`, `[0]`, `[0]`.
    //       step 1 `[]`, step 2 `[]`, step 3 `[]`: the empty input passes;
    //       step 4 `[0]` fails with contract_violation: accepted.
    //   pass 1, items = [0]: `[]`, `[]`, (second half equals the value),
    //     `[]`; `0` has no `gen.int` candidate (every rule yields 0).
    //       steps 5, 6, 7 pass: nothing accepted; on to `size`.
    //   pass 1, size = 1: `gen.positive_int` candidates `1`, `1 + 0 / 2`,
    //     and `0` are equal to the value or below 1: none, no step.
    //   pass 2 (pass 1 accepted a candidate): items = [0] costs steps 8, 9,
    //     10 and accepts nothing; size = 1 has no candidate. A full pass
    //     accepted nothing: shrinking stops after 10 steps, within the
    //     budget of 1000 (`shrink_exhausted: false`).
    assert_eq!(
        draws::first_nonempty(1),
        (1, json!({"items": [-1], "size": 1}))
    );
    let record = run_test(&p, property, &config(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.kind, TestKind::Property);
    assert_eq!(record.seed, Some(1));
    assert_eq!(
        record.property_seed.as_deref(),
        Some(seed_hex(&property_seed(1, PRESERVES_INPUT)).as_str())
    );
    assert_eq!(record.cases, Some(2));
    let failing = record.failing_case.as_ref().expect("a failing case");
    assert_eq!(failing.index, 1);
    assert_eq!(failing_input(&record), json!({"items": [-1], "size": 1}));
    let shrunk = record.shrunk.as_ref().expect("shrunk");
    assert_eq!(shrunk_input(&record), json!({"items": [0], "size": 1}));
    assert_eq!((shrunk.shrink_steps, shrunk.shrink_exhausted), (10, false));

    // The record's fault is the one the shrunk tuple raised: paginate's
    // first ensures, blamed on paginate's `@id` (S46).
    let d = fault(&record);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "ensures");
    assert_eq!(d["attributed_to"], "body");
    assert_eq!(d["symbol"], "pgn_7c2d");
    assert_eq!(d["qualified_name"], "reader.pages.paginate");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "flatten(result) == items");
    assert_eq!(d["input"], json!({"items": [0], "size": 1}));
    assert_eq!(d["actual"], json!([]));
    assert_eq!(frames(&d), ["reader.pages.paginate", PRESERVES_INPUT]);

    // The counterexample against the body (S46, toolchain 1.7.3): the
    // design's example shape.
    assert_eq!(record.counterexamples.len(), 1);
    let ce = serde_json::to_value(&record.counterexamples[0]).unwrap();
    assert_eq!(ce["code"], "contract_counterexample");
    assert_eq!(ce["category"], "contract");
    assert_eq!(ce["severity"], "error");
    assert_eq!(ce["symbol"], "pgn_7c2d");
    assert_eq!(ce["qualified_name"], "reader.pages.paginate");
    assert_eq!(ce["facet"], "body");
    assert_eq!(ce["clause"], "flatten(result) == items");
    assert_eq!(ce["clause_kind"], "ensures");
    assert_eq!(ce["input"], json!({"items": [0], "size": 1}));
    assert_eq!(ce["actual"], json!([]));
    assert_eq!(ce["status"], "failed");
    assert_eq!(ce["found_by"], PRESERVES_INPUT);
    assert!(ce.get("iteration").is_none());
    // It spans the violated clause, from its keyword.
    let pages = std::fs::read_to_string(common::example_dir("reader").join("src/reader/pages.cpl"))
        .expect("the example");
    assert_eq!(
        ce["span"],
        common::location_of(
            &pages,
            "src/reader/pages.cpl",
            "ensures flatten(result) == items"
        )
    );

    // ---- seeds 2 and 3 ------------------------------------------------------
    //
    // seed 2: the first non-empty case is case 2 with `items = [0]`,
    //   `size = 1`. items: `[]`, `[]`, `[]` pass (steps 1-3), no element
    //   candidate; size 1: none. Pass 1 accepted nothing: 3 steps, and the
    //   shrunk tuple is the original one.
    // seed 3: case 2 with `items = [1]`, `size = 3`. items: `[]`, `[]`, `[]`
    //   pass (steps 1-3), `[0]` (0 is the first `gen.int` candidate of 1)
    //   fails: accepted (step 4); from `[0]` steps 5-7 accept nothing.
    //   size 3: its first candidate `1` fails: accepted (step 8); 1 has no
    //   candidate. Pass 2: steps 9-11 on `[0]`, nothing accepted: 11 steps.
    for (seed, original, steps) in [
        (2, json!({"items": [0], "size": 1}), 3),
        (3, json!({"items": [1], "size": 3}), 11),
    ] {
        assert_eq!(draws::first_nonempty(seed), (2, original.clone()));
        let record = run_test(&p, property, &config(seed));
        assert_eq!(record.status, TestStatus::Failed, "seed {seed}");
        assert_eq!(record.cases, Some(3));
        assert_eq!(record.failing_case.as_ref().unwrap().index, 2);
        assert_eq!(failing_input(&record), original, "seed {seed}");
        assert_eq!(shrunk_input(&record), json!({"items": [0], "size": 1}));
        let s = record.shrunk.as_ref().unwrap();
        assert_eq!(
            (s.shrink_steps, s.shrink_exhausted),
            (steps, false),
            "seed {seed}"
        );
        assert_eq!(record.counterexamples.len(), 1);
    }

    // The plain tests: the empty input still passes; the short tail is lost.
    let report = run_tests(&p, &TestSelector::all(), &config(1));
    assert_eq!((report.selected, report.tested, report.failed), (3, 1, 2));
    let empty = common::def(&p, "tests.pagination.\"pagination handles empty input\"");
    assert_eq!(run_test(&p, empty, &config(1)).status, TestStatus::Tested);
    let short = common::def(&p, "tests.pagination.\"pagination preserves a short tail\"");
    let short = run_test(&p, short, &config(1));
    assert_eq!(short.status, TestStatus::Failed);
    assert!(short.seed.is_none() && short.shrunk.is_none());
    let ce = &short.counterexamples[0];
    assert_eq!(ce.data["input"], json!({"items": [1, 2, 3], "size": 2}));
    assert_eq!(ce.data["actual"], json!([[1, 2]]));
    assert_eq!(
        ce.data["found_by"],
        "tests.pagination.\"pagination preserves a short tail\""
    );
}

#[test]
fn two_runs_with_one_seed_give_identical_reports() {
    let wrong = wrong_reader();
    let props = props();
    for (p, selector) in [
        (&wrong, TestSelector::all()),
        (&props, TestSelector::parse("tests.props").unwrap()),
    ] {
        for seed in [1, 42] {
            let a = run_tests(p, &selector, &config(seed));
            let b = run_tests(p, &selector, &config(seed));
            assert!(a.failed > 0, "the runs have failures to shrink");
            assert_eq!(stable_report(&a), stable_report(&b), "seed {seed}");
        }
    }
    // Another seed draws other cases.
    let sel = TestSelector::parse("tests.props").unwrap();
    let a = run_tests(&props, &sel, &config(42));
    assert_eq!(a.results.len(), 13);
    let c = run_tests(&props, &sel, &config(43));
    assert!(a
        .results
        .iter()
        .zip(&c.results)
        .any(|(x, y)| x.failing_case != y.failing_case));
}

// ===========================================================================
// Small properties: the runner's rules
// ===========================================================================

/// Thirteen properties in `tests.props`, each failing (or not) for a
/// reason a test below states.
const PROPS: &str = r#"module tests.props

property "ints below ten"
    for x in gen.int
{
    assert x < 10
}

@id("prop_ints")
property "ints below ten again"
    for x in gen.int
{
    assert x < 10
}

property "short lists"
    for xs in gen.list(gen.int)
{
    assert xs.len < 3
}

property "small positives"
    for n in gen.positive_int
{
    assert n < 5
}

property "short texts"
    for t in gen.text
{
    assert t.len < 2
}

property "options are none"
    for o in gen.option(gen.int)
{
    assert o.is_none()
}

property "range below a hundred"
    for r in gen.range(5, 1000)
{
    assert r < 100
}

property "same code"
    for x in gen.range(0, 1000)
{
    if x >= 20 {
        assert false
    } else {
        let _ = 1 / 0
    }
}

property "empty range"
    for x in gen.range(5, 1)
{
    assert x == x
}

property "faulting bound"
    for x in gen.range(1 / 0, 5)
{
    assert x == x
}

property "always"
    for x in gen.int
    for flag in gen.bool
    for t in gen.text
{
    assert x == x
}

property "pair"
    for a in gen.int
    for b in gen.int
{
    assert a < 10 or b < 10
}

property "positive draws"
    for ns in gen.list(gen.positive_int)
{
    assert ns.len >= 0
}
"#;

fn props() -> Program {
    common::check_source(PROPS)
}

fn prop(p: &Program, name: &str) -> DefId {
    common::def(p, &format!("tests.props.\"{name}\""))
}

#[test]
fn a_passing_property_runs_every_case() {
    let p = props();
    let always = prop(&p, "always");
    let record = run_test(&p, always, &config(11));
    assert_eq!(record.status, TestStatus::Tested);
    assert_eq!(record.cases, Some(100));
    assert_eq!(record.seed, Some(11));
    assert!(record.fault.is_none() && record.failing_case.is_none());
    let record = run_test(
        &p,
        always,
        &TestConfig {
            cases: 7,
            ..config(11)
        },
    );
    assert_eq!(record.cases, Some(7));
    // Drawn Positive values are checked against their predicate, and pass.
    assert_eq!(
        run_test(&p, prop(&p, "positive draws"), &config(11)).status,
        TestStatus::Tested
    );
}

#[test]
fn cases_n_changes_no_case_a_smaller_n_runs() {
    let p = props();
    let ints = prop(&p, "ints below ten");
    for seed in [1, 2, 3, 4, 5] {
        let full = run_test(&p, ints, &config(seed));
        assert_eq!(full.status, TestStatus::Failed, "seed {seed}");
        let index = full.failing_case.as_ref().unwrap().index;
        assert_eq!(full.cases, Some(index + 1));
        // Exactly enough cases reach the same failing case and shrink alike.
        let prefix = run_test(
            &p,
            ints,
            &TestConfig {
                cases: index + 1,
                ..config(seed)
            },
        );
        assert_eq!(stable(&prefix), stable(&full));
        // One case fewer never reaches it.
        let short = run_test(
            &p,
            ints,
            &TestConfig {
                cases: index,
                ..config(seed)
            },
        );
        assert_eq!(short.status, TestStatus::Tested);
        assert_eq!(short.cases, Some(index));
    }
}

#[test]
fn the_record_of_a_failing_property_s46() {
    let p = props();
    let record = run_test(&p, prop(&p, "ints below ten"), &config(9));
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.seed, Some(9));
    let expected = seed_hex(&property_seed(9, "tests.props.\"ints below ten\""));
    assert_eq!(record.property_seed.as_deref(), Some(expected.as_str()));
    let original = failing_input(&record)["x"].clone();
    assert!(original.as_i64().is_none_or(|x| x >= 10), "{original}");
    assert_eq!(shrunk_input(&record), json!({"x": 10}));
    let shrunk = record.shrunk.as_ref().unwrap();
    assert!(!shrunk.shrink_exhausted);
    assert!(shrunk.shrink_steps > 0);
    // The fault is the one the shrunk tuple raised; the binder is a `for`
    // binding (semantics 8.3, S41).
    let d = fault(&record);
    assert_eq!(d["code"], "assertion_failed");
    assert_eq!(d["clause"], "x < 10");
    assert_eq!(d["symbol"], "tests.props.\"ints below ten\"");
    assert_eq!(d["facet"], "body");
    assert_eq!(
        d["bindings"],
        json!([{"name": "x", "type": "Int", "kind": "for", "value": 10}])
    );
    assert!(record.counterexamples.is_empty());
    // An @id keys the stream (S44): its seed is derived from the @id.
    let record = run_test(&p, prop(&p, "ints below ten again"), &config(9));
    assert_eq!(record.symbol.as_str(), "prop_ints");
    assert_eq!(
        record.qualified_name,
        "tests.props.\"ints below ten again\""
    );
    let expected = seed_hex(&property_seed(9, "prop_ints"));
    assert_eq!(record.property_seed.as_deref(), Some(expected.as_str()));
    assert_eq!(shrunk_input(&record), json!({"x": 10}));
}

#[test]
fn shrinking_reaches_the_minimal_counterexample_of_each_generator() {
    let p = props();
    for seed in [1, 7, 100] {
        let c = config(seed);
        let shrunk = |name: &str| shrunk_input(&run_test(&p, prop(&p, name), &c));
        assert_eq!(shrunk("ints below ten"), json!({"x": 10}));
        assert_eq!(shrunk("short lists"), json!({"xs": [0, 0, 0]}));
        // Positive candidates stay >= 1, and the minimum is the boundary.
        assert_eq!(shrunk("small positives"), json!({"n": 5}));
        assert_eq!(shrunk("short texts"), json!({"t": "aa"}));
        assert_eq!(
            shrunk("options are none"),
            json!({"o": {"$type": "Option", "$variant": "Some", "$payload": [0]}})
        );
        assert_eq!(shrunk("range below a hundred"), json!({"r": 100}));
        // Coordinate-wise: the first binder, then the second.
        assert_eq!(shrunk("pair"), json!({"a": 10, "b": 10}));
    }
}

#[test]
fn only_a_failure_with_the_same_code_is_accepted_s45() {
    let p = props();
    let same_code = prop(&p, "same code");
    let mut checked = 0;
    for seed in 0..40 {
        let record = run_test(&p, same_code, &config(seed));
        let original = failing_input(&record)["x"].as_i64().unwrap();
        let d = fault(&record);
        if original >= 20 {
            // Candidates below 20 divide by zero instead; they are rejected.
            assert_eq!(shrunk_input(&record), json!({"x": 20}), "seed {seed}");
            assert_eq!(d["code"], "assertion_failed");
            checked += 1;
        } else {
            assert_eq!(shrunk_input(&record), json!({"x": 0}), "seed {seed}");
            assert_eq!(d["code"], "division_by_zero");
        }
    }
    assert!(checked > 30);
}

#[test]
fn the_shrink_budget_counts_body_reruns() {
    let p = props();
    let ints = prop(&p, "ints below ten");
    let full = run_test(&p, ints, &config(9));
    let original = failing_input(&full);
    let none = run_test(
        &p,
        ints,
        &TestConfig {
            max_shrinks: 0,
            ..config(9)
        },
    );
    let s = none.shrunk.as_ref().unwrap();
    assert_eq!((s.shrink_steps, s.shrink_exhausted), (0, true));
    assert_eq!(shrunk_input(&none), original);
    let one = run_test(
        &p,
        ints,
        &TestConfig {
            max_shrinks: 1,
            ..config(9)
        },
    );
    let s = one.shrunk.as_ref().unwrap();
    // The first candidate, 0, passes and is discarded; the budget is spent.
    assert_eq!((s.shrink_steps, s.shrink_exhausted), (1, true));
    assert_eq!(shrunk_input(&one), original);
    let steps = full.shrunk.as_ref().unwrap().shrink_steps;
    let exact = run_test(
        &p,
        ints,
        &TestConfig {
            max_shrinks: steps,
            ..config(9)
        },
    );
    assert_eq!(shrunk_input(&exact), json!({"x": 10}));
}

#[test]
fn nested_lists_shrink_to_a_minimal_total() {
    let p = common::check_source(
        "module tests.nested\n\nproperty \"few elements\"\n    for xss in gen.list(gen.list(gen.int))\n{\n    assert flatten(xss).len < 5\n}\n",
    );
    let few = common::def(&p, "tests.nested.\"few elements\"");
    for seed in [1, 2, 3, 4] {
        let record = run_test(&p, few, &config(seed));
        assert_eq!(record.status, TestStatus::Failed);
        let shrunk = shrunk_input(&record);
        let inner = shrunk["xss"].as_array().unwrap();
        let all: Vec<&Json> = inner.iter().flat_map(|l| l.as_array().unwrap()).collect();
        assert_eq!(all.len(), 5, "{shrunk}");
        assert!(all.iter().all(|x| **x == json!(0)), "{shrunk}");
        // An empty inner list would have been removed (the failure persists without it).
        assert!(
            inner.iter().all(|l| !l.as_array().unwrap().is_empty()),
            "{shrunk}"
        );
        assert!(!record.shrunk.as_ref().unwrap().shrink_exhausted);
    }
}

// ===========================================================================
// The setup of `gen.range` bounds (stdlib 8.2, 12; semantics 5.1, S71)
// ===========================================================================

/// The setup fault of a property that must fail before case 0.
fn setup_fault(p: &Program, prop: DefId) -> Json {
    let record = run_test(p, prop, &config(1));
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.cases, Some(0));
    assert!(record.failing_case.is_none() && record.shrunk.is_none());
    assert!(record.property_seed.is_some());
    fault(&record)
}

#[test]
fn range_bounds_are_evaluated_before_case_zero() {
    let p = props();
    // `lo > hi` is the generator's own precondition: the builtin shape of
    // stdlib 12 (`symbol std.gen.range`, `clause lo <= hi`, `caller` the
    // property), checked after both bounds.
    let empty = prop(&p, "empty range");
    let d = setup_fault(&p, empty);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["symbol"], "std.gen.range");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "lo <= hi");
    assert_eq!(d["input"], json!({"lo": 5, "hi": 1}));
    assert_eq!(d["caller"], "tests.props.\"empty range\"");
    assert_eq!(d["span"]["file"], "std/gen.cpl");
    assert_eq!(frames(&d), ["std.gen.range", "tests.props.\"empty range\""]);
    // The generator's frame carries the span of the `for` clause.
    let clause = "for x in gen.range(5, 1)";
    let line = PROPS
        .lines()
        .position(|l| l.trim() == clause)
        .expect("the clause's line") as u32
        + 1;
    assert_eq!(
        d["backtrace"][0]["span"],
        json!({"file": "src/tests/props.cpl", "line": line, "col": 5, "end_line": line,
               "end_col": 5 + clause.len()})
    );
    assert!(run_test(&p, empty, &config(1)).counterexamples.is_empty());

    // A fault raised while a bound evaluates is the property's (S71).
    let d = setup_fault(&p, prop(&p, "faulting bound"));
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "tests.props.\"faulting bound\"");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "gen.range(1 / 0, 5)");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
}

const RANGES: &str = r#"module tests.ranges

fn f(n: Int) -> Int
    requires 10 / n > 0
{
    n
}

fn g(n: Int) -> Int
    requires n > 0
{
    n
}

property "division"
    for x in gen.range(1 / 0, 5)
{
    assert x == x
}

property "nested"
    for b in gen.bool
    for xs in gen.list(gen.range(0, 1 % 0))
{
    assert b == b
}

property "builtin"
    for x in gen.range([1].take(-1).len, 5)
{
    assert x == x
}

property "inner clause"
    for x in gen.range(f(0), 5)
{
    assert x == x
}

property "callee requires"
    for x in gen.range(0, g(0))
{
    assert x == x
}

property "canonical"
    for x in gen.range((1)/0 ,5)
{
    assert x == x
}

property "blocks"
    for x in gen.range(if true { 1 / 0 } else { 0 }, 5)
{
    assert x == x
}

property "arms"
    for x in gen.range(0, match 0 {
        0 => 1 / 0
        _ => 1
    })
{
    assert x == x
}
"#;

#[test]
fn a_fault_in_a_range_is_a_fault_of_that_contract_expression() {
    // A property's `for` range is a contract expression (semantics 5.1,
    // 7.4), so a fault raised while it is evaluated is reported per S34 and
    // S71: as that fault, with facet contract, the clause (the range's
    // canonical text, as every other clause reports its expression without
    // the keyword), and the property as symbol.
    let p = common::check_source(RANGES);
    let prop = |name: &str| common::def(&p, &format!("tests.ranges.\"{name}\""));

    let d = setup_fault(&p, prop("division"));
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["input"], json!({"a": 1, "b": 0}));
    assert_eq!(d["symbol"], "tests.ranges.\"division\"");
    assert_eq!(d["qualified_name"], "tests.ranges.\"division\"");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "gen.range(1 / 0, 5)");
    assert_eq!(frames(&d), ["tests.ranges.\"division\""]);

    // The clause is the whole range of the `for` clause, not its bound.
    let d = setup_fault(&p, prop("nested"));
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["clause"], "gen.list(gen.range(0, 1 % 0))");
    assert_eq!(d["facet"], "contract");

    // A builtin's violated precondition inside a bound is a nested
    // contract_violation: S34 names the property and its clause, the
    // violation keeps its kind, attribution, caller, and input (S70).
    let d = setup_fault(&p, prop("builtin"));
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["attributed_to"], "caller");
    assert_eq!(d["symbol"], "tests.ranges.\"builtin\"");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["clause"], "gen.range([1].take(-1).len, 5)");
    assert_eq!(d["input"], json!({"self": [1], "n": -1}));
    assert_eq!(d["caller"], "tests.ranges.\"builtin\"");
    assert_eq!(d["span"]["file"], "std/prelude.cpl");
    assert_eq!(
        frames(&d),
        ["std.prelude.List.take", "tests.ranges.\"builtin\""]
    );

    // A fault inside a clause evaluated within the range is reported by
    // that innermost clause, which claims it first (S70).
    let d = setup_fault(&p, prop("inner clause"));
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["symbol"], "tests.ranges.f");
    assert_eq!(d["clause"], "10 / n > 0");
    assert_eq!(d["facet"], "contract");
    assert_eq!(
        frames(&d),
        ["tests.ranges.f", "tests.ranges.\"inner clause\""]
    );

    // A callee's own requires violation raised while the range is evaluated.
    let callee = prop("callee requires");
    let d = setup_fault(&p, callee);
    assert_eq!(d["code"], "contract_violation");
    assert_eq!(d["clause_kind"], "requires");
    assert_eq!(d["symbol"], "tests.ranges.\"callee requires\"");
    assert_eq!(d["clause"], "gen.range(0, g(0))");
    assert_eq!(d["input"], json!({"n": 0}));
    assert_eq!(d["caller"], "tests.ranges.\"callee requires\"");
    assert!(
        run_test(&p, callee, &config(1)).counterexamples.is_empty(),
        "a requires violation is no counterexample (S46)"
    );
}

#[test]
fn the_range_clause_is_the_canonical_text_of_the_source() {
    // The clause is the formatter's canonical text of the range (toolchain
    // 5.2): spacing normalized, parentheses kept as written.
    let p = common::check_source(RANGES);
    let d = setup_fault(&p, common::def(&p, "tests.ranges.\"canonical\""));
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["clause"], "gen.range((1) / 0, 5)");
    assert_eq!(d["facet"], "contract");
    assert_eq!(d["symbol"], "tests.ranges.\"canonical\"");
}

#[test]
fn a_range_clause_with_blocks_is_joined_onto_one_line_t56() {
    // toolchain 5.2, T56 [D-246]: the facet text of a range holding an `if`
    // or a `match` keeps the blocks' layout over several lines; its clause
    // text, every `clause` field, is that text with each line break and the
    // indentation after it replaced by one space, as `Clause::text` is.
    let p = common::check_source(RANGES);
    let d = setup_fault(&p, common::def(&p, "tests.ranges.\"blocks\""));
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["clause"], "gen.range(if true { 1 / 0 } else { 0 }, 5)");
    let d = setup_fault(&p, common::def(&p, "tests.ranges.\"arms\""));
    assert_eq!(d["code"], "division_by_zero");
    assert_eq!(d["clause"], "gen.range(0, match 0 { 0 => 1 / 0 _ => 1 })");
    assert_eq!(d["symbol"], "tests.ranges.\"arms\"");
    let clause = d["clause"].as_str().unwrap();
    assert!(!clause.contains('\n'), "{clause}");
}
