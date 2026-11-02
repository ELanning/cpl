//! The programs and configurations on which `m2_hooks.rs` compares the generic runner of toolchain 1.7.4 (T77) with
//! the Milestone 1 runner. The goldens beside this file (`runner_*.json`) were written by the Milestone 1 runner of
//! `crates/cpl-eval/src/test.rs`, before it became generic over `TestEngine`, and are never regenerated: they are the
//! evidence that the refactor is byte-identical (toolchain 1.7.2, 1.7.4).

use crate::common;
use cpl_eval::test::TestReport;
use cpl_eval::{TestConfig, TestSelector};
use cpl_hir::Program;
use serde_json::{Map, Value as Json};

/// One run of the runner: a selector and a configuration over one program.
pub struct Case {
    pub name: String,
    pub selector: TestSelector,
    pub config: TestConfig,
}

/// `examples/reader` as `cpl test` loads it.
pub fn reader() -> Program {
    common::reader()
}

/// `examples/reader` with the design's `paginate` made wrong (the final push of the partial page removed), as
/// `crates/cpl-eval/tests/properties.rs` builds it: the design's property then fails on every non-empty input and
/// shrinks, and its failure is a `contract_counterexample` against `paginate`.
pub fn wrong_reader() -> Program {
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

/// Small properties and tests that reach every path of the runner: shrinking of every generator, the same-code rule,
/// the refinement check of drawn values and candidates, the setup of `gen.range` bounds (a fault in a bound, S71, and
/// the generator's own `requires lo <= hi`), counterexamples of `ensures`, `invariant`, and `decreases` violations in
/// callees (S46), a builtin precondition, and passing tests.
pub const PROPS: &str = r#"module tests.props

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

property "nested lists"
    for xss in gen.list(gen.list(gen.option(gen.bool)))
{
    assert xss.len < 2 or xss.first().is_none()
}
"#;

/// Faults in `gen.range` bounds (semantics S71), as `crates/cpl-eval/tests/properties.rs` writes them.
pub const RANGES: &str = r#"module tests.ranges

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

fn limit() -> Int {
    7
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

property "computed bounds"
    for x in gen.range(g(2), limit())
    for ys in gen.list(gen.range(0, limit()))
{
    assert x + ys.len < 9
}
"#;

/// Contract violations in callees, which are counterexamples (semantics S46), and plain tests.
pub const COUNTER: &str = r#"module tests.counter

fn grow(n: Int) -> Int
    ensures result > n
{
    if n > 3 { n } else { n + 1 }
}

fn count_up(n: Int) -> Int {
    var i = 0
    while i < n
        invariant i < 5
    {
        i = i + 1
    }
    i
}

fn down(n: Int) -> Int
    decreases n
{
    if n <= 0 { 0 } else { down(n + 1) }
}

test "ensures counterexample" {
    assert grow(10) == 10
}

property "invariant counterexample"
    for n in gen.int
{
    assert count_up(n) == n or n < 0
}

property "decreases counterexample"
    for n in gen.positive_int
{
    assert down(n) == 0
}

property "grows"
    for n in gen.range(0, 50)
{
    assert grow(n) > n
}

test "builtin precondition" {
    let xs = [1, 2, 3]
    assert xs.take(-1).len == 0
}

test "passes" {
    assert 1 + 1 == 2
}

test "a hole" {
    let x = 1
    let y: Int = ?answer
    assert y == x
}
"#;

/// The three modules above in one project.
pub fn props() -> Program {
    common::check_files(&[
        ("src/tests/props.cpl", PROPS),
        ("src/tests/ranges.cpl", RANGES),
        ("src/tests/counter.cpl", COUNTER),
    ])
}

/// Seeds 1..=8 with the default configuration, every test of the program.
pub fn seeds() -> Vec<Case> {
    (1..=8)
        .map(|seed| Case {
            name: format!("seed {seed}"),
            selector: TestSelector::all(),
            config: TestConfig::with_seed(seed),
        })
        .collect()
}

/// [`seeds`] and three configurations that reach the shrink budget, a smaller case count, and `fail_fast`.
pub fn props_cases() -> Vec<Case> {
    let mut cases = seeds();
    let mut budget = TestConfig::with_seed(3);
    budget.max_shrinks = 3;
    cases.push(Case {
        name: "seed 3, max_shrinks 3".to_string(),
        selector: TestSelector::all(),
        config: budget,
    });
    let mut few = TestConfig::with_seed(5);
    few.cases = 7;
    cases.push(Case {
        name: "seed 5, cases 7".to_string(),
        selector: TestSelector::all(),
        config: few,
    });
    let mut fast = TestConfig::with_seed(2);
    fast.fail_fast = true;
    cases.push(Case {
        name: "seed 2, fail_fast, tests.props".to_string(),
        selector: TestSelector::parse("tests.props").expect("a namespace"),
        config: fast,
    });
    cases
}

/// A report as JSON with every wall time cleared (`duration_ms`, the one field that may differ between two runs).
pub fn stable_report(report: &TestReport) -> Json {
    let mut r = report.clone();
    for record in &mut r.results {
        record.duration_ms = 0;
    }
    serde_json::to_value(r).expect("a report serializes")
}

/// The golden text of a set of reports: one object keyed by case name, pretty-printed, with a final line feed.
pub fn golden_text(reports: Vec<(String, Json)>) -> String {
    let mut m = Map::new();
    for (name, report) in reports {
        m.insert(name, report);
    }
    let mut text = serde_json::to_string_pretty(&Json::Object(m)).expect("JSON serializes");
    text.push('\n');
    text
}

/// The path of a golden file beside this one.
pub fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("m2_hooks")
        .join(name)
}
