//! The test runner (toolchain 1.7.2, 7.2; semantics 7.2-7.3, S42, S44,
//! S46) on checked programs: selection and selectors, the property seed,
//! records, the `test_result` diagnostic, fail-fast, per-test time budgets,
//! and `contract_counterexample` from plain tests. The design's tests on
//! `examples/reader` are `tests/examples.rs`.

use crate::common;
use cpl_diag::json::{TestKind, TestStatus};
use cpl_diag::{Severity, StableId};
use cpl_eval::test::{property_seed, seed_hex, test_result_diagnostic, TestConfig, TestSelector};
use cpl_eval::{run_test, run_tests, select_tests};
use cpl_hir::{DefId, Program};
use serde_json::json;
use std::time::Duration;

#[test]
fn the_property_seed_is_blake3_of_the_seed_and_identity_s44() {
    for (seed, id) in [
        (0u64, "pgn_7c2d"),
        (1, "tests.pagination.\"pagination preserves input\""),
        (u64::MAX, "x"),
    ] {
        let mut bytes = seed.to_le_bytes().to_vec();
        bytes.extend_from_slice(id.as_bytes());
        assert_eq!(property_seed(seed, id), *blake3::hash(&bytes).as_bytes());
    }
    // Little-endian: 1 is the byte 0x01 followed by seven zeros.
    let mut bytes = vec![1, 0, 0, 0, 0, 0, 0, 0];
    bytes.extend_from_slice(b"p");
    assert_eq!(property_seed(1, "p"), *blake3::hash(&bytes).as_bytes());
    let hex = seed_hex(&property_seed(7, "p"));
    assert_eq!(hex.len(), 64);
    assert!(hex
        .bytes()
        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    assert_eq!(
        hex,
        blake3::Hash::from(property_seed(7, "p"))
            .to_hex()
            .to_string()
    );
}

/// Tests in five modules, for selection: (module, test name).
const SELECTION: [(&str, &str); 6] = [
    ("tests.pagination", "b"),
    ("tests.pagination", "a"),
    ("tests.pagination.deep", "c"),
    ("tests.other", "a"),
    ("testsx", "d"),
    ("zeta", "e"),
];

/// The selection fixture, checked; the tests in [`SELECTION`] order.
fn selection_fixture() -> (Program, Vec<DefId>) {
    let mut files: Vec<(String, String)> = Vec::new();
    for (module, name) in SELECTION {
        let path = format!("src/{}.cpl", module.replace('.', "/"));
        let id = if name == "e" {
            "@id(\"test_zeta\")\n"
        } else {
            ""
        };
        let test = format!("{id}test \"{name}\" {{\n    assert true\n}}\n");
        match files.iter_mut().find(|(p, _)| *p == path) {
            Some((_, text)) => text.push_str(&format!("\n{test}")),
            None => files.push((path, format!("module {module}\n\n{test}"))),
        }
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let p = common::check_files(&refs);
    let ids = SELECTION
        .iter()
        .map(|(module, name)| common::def(&p, &format!("{module}.\"{name}\"")))
        .collect();
    (p, ids)
}

#[test]
fn selection_is_a_namespace_prefix_sorted_by_qualified_name() {
    let (p, ids) = selection_fixture();
    let names = |sel: &TestSelector| -> Vec<String> {
        select_tests(&p, sel)
            .iter()
            .map(|&d| p.display_path(d))
            .collect()
    };
    assert_eq!(
        names(&TestSelector::all()),
        [
            "tests.other.\"a\"",
            "tests.pagination.\"a\"",
            "tests.pagination.\"b\"",
            "tests.pagination.deep.\"c\"",
            "testsx.\"d\"",
            "zeta.\"e\"",
        ]
    );
    // Segment-wise prefixes: `tests` is not a prefix of `testsx`.
    assert_eq!(names(&TestSelector::parse("tests").unwrap()).len(), 4);
    assert_eq!(
        names(&TestSelector::parse("tests.pagination.*").unwrap()),
        names(&TestSelector::parse("tests.pagination").unwrap())
    );
    assert_eq!(
        names(&TestSelector::parse("tests.pagination").unwrap()).len(),
        3
    );
    assert!(names(&TestSelector::parse("tests.pag").unwrap()).is_empty());
    let by_name = TestSelector {
        name: Some("a".to_string()),
        ..TestSelector::all()
    };
    assert_eq!(
        names(&by_name),
        ["tests.other.\"a\"", "tests.pagination.\"a\""]
    );
    let by_id = TestSelector {
        id: Some(StableId::parse("test_zeta").unwrap()),
        ..TestSelector::all()
    };
    assert_eq!(select_tests(&p, &by_id), [ids[5]]);
    let by_path_id = TestSelector {
        id: Some(StableId::parse("tests.pagination.\"b\"").unwrap()),
        ..TestSelector::all()
    };
    assert_eq!(select_tests(&p, &by_path_id), [ids[0]]);
    let both = TestSelector {
        namespace: Some(vec!["tests".to_string()]),
        name: Some("a".to_string()),
        id: None,
    };
    assert_eq!(names(&both).len(), 2);
    // An empty selection is a report with nothing selected (empty_test_selection is the CLI's).
    let none = TestSelector::parse("nothing.here").unwrap();
    let report = run_tests(&p, &none, &TestConfig::with_seed(0));
    assert_eq!((report.selected, report.tested, report.failed), (0, 0, 0));
    assert!(report.results.is_empty());
}

#[test]
fn selectors_are_module_paths_or_namespace_wildcards_t43() {
    for ok in [
        "tests",
        "tests.pagination",
        "tests.pagination.*",
        "a_b.c2",
        "_x.y",
    ] {
        let sel = TestSelector::parse(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        let expected: Vec<String> = ok
            .trim_end_matches(".*")
            .split('.')
            .map(str::to_string)
            .collect();
        assert_eq!(sel.namespace, Some(expected));
        assert_eq!((sel.name, sel.id), (None, None));
    }
    for bad in [
        "",
        "*",
        ".*",
        "tests.",
        ".tests",
        "Tests.x",
        "tests..x",
        "tests.*.x",
        "tests.**",
        "tests .x",
        " tests",
        "tests.fn",
        "tests.\"x\"",
        "tests.x()",
        "tests/x",
    ] {
        assert!(TestSelector::parse(bad).is_err(), "{bad:?} was accepted");
    }
    let wire: cpl_diag::json::TestSelectorSpec = TestSelector::parse("a.b").unwrap().into();
    assert_eq!(wire.namespace, Some(vec!["a".to_string(), "b".to_string()]));
}

const M: &str = r#"module m

pub fn half(n: Int) -> Int
    ensures result * 2 == n
{
    n / 2
}

pub fn recip(n: Int) -> Int
    requires n != 0
{
    1 / n
}

// half's ensures fails inside a clause.
pub fn guarded(n: Int) -> Int
    requires half(n) >= 0
{
    n
}
"#;

const PLAIN: &str = r#"module tests.plain

use m.{half, recip, guarded}

test "passing" {
    assert 1 == 1
}

test "asserts" {
    assert 1 == 2
}

@id("test_half_5")
test "calls half" {
    let _ = half(5)
}

test "own invariant" {
    var i = 0
    while i < 3
        invariant i < 2
    {
        i = i + 1
    }
}

test "calls recip" {
    let _ = recip(0)
}

test "nested in clause" {
    let _ = guarded(5)
}

test "spins" {
    while true {
    }
}
"#;

/// A module of plain tests and the functions they call.
struct Plain {
    p: Program,
    passing: DefId,
    asserts: DefId,
    calls_half: DefId,
    own_invariant: DefId,
    calls_recip: DefId,
    nested_in_clause: DefId,
    spins: DefId,
}

fn plain_fixture() -> Plain {
    let p = common::check_files(&[("src/m.cpl", M), ("src/tests/plain.cpl", PLAIN)]);
    let t = |name: &str| common::def(&p, &format!("tests.plain.\"{name}\""));
    Plain {
        passing: t("passing"),
        asserts: t("asserts"),
        calls_half: t("calls half"),
        own_invariant: t("own invariant"),
        calls_recip: t("calls recip"),
        nested_in_clause: t("nested in clause"),
        spins: t("spins"),
        p,
    }
}

fn record_json(r: &cpl_diag::json::TestCaseRecord) -> serde_json::Value {
    serde_json::to_value(r).unwrap()
}

#[test]
fn a_plain_test_is_tested_or_failed_with_its_fault() {
    let fx = plain_fixture();
    let config = TestConfig::with_seed(3);
    let ok = run_test(&fx.p, fx.passing, &config);
    assert_eq!(ok.status, TestStatus::Tested);
    assert_eq!(ok.kind, TestKind::Test);
    let failed = run_test(&fx.p, fx.asserts, &config);
    assert_eq!(failed.status, TestStatus::Failed);
    let fault = failed.fault.as_ref().unwrap();
    assert_eq!(fault.code, "assertion_failed");
    assert_eq!(fault.data["clause"], "1 == 2");
    assert_eq!(
        fault.symbol.as_ref().unwrap().as_str(),
        "tests.plain.\"asserts\""
    );
    assert!(failed.counterexamples.is_empty());
    // The record's JSON has exactly the S46 keys of a test (plus kind).
    let j = record_json(&failed);
    let keys: Vec<&str> = j.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "duration_ms",
            "fault",
            "kind",
            "qualified_name",
            "status",
            "symbol"
        ]
    );
}

#[test]
fn the_test_result_diagnostic_carries_the_record() {
    let fx = plain_fixture();
    let config = TestConfig::with_seed(3);
    let d = test_result_diagnostic(&run_test(&fx.p, fx.passing, &config));
    assert_eq!(d.code, "test_result");
    assert_eq!(d.severity, Severity::Info);
    assert_eq!(
        d.symbol.as_ref().unwrap().as_str(),
        "tests.plain.\"passing\""
    );
    assert_eq!(d.qualified_name.as_deref(), Some("tests.plain.\"passing\""));
    assert_eq!(d.data["status"], "tested");
    assert_eq!(d.data["kind"], "test");
    assert!(d.data.get("symbol").is_none() && d.data.get("qualified_name").is_none());
    let d = test_result_diagnostic(&run_test(&fx.p, fx.asserts, &config));
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.data["status"], "failed");
    assert_eq!(d.data["fault"]["code"], "assertion_failed");
    assert!(d.message.contains("assertion failed"), "{}", d.message);
}

#[test]
fn an_ensures_violation_in_a_callee_is_a_counterexample_s46() {
    let fx = plain_fixture();
    let config = TestConfig::with_seed(3);
    let record = run_test(&fx.p, fx.calls_half, &config);
    assert_eq!(record.status, TestStatus::Failed);
    assert_eq!(record.symbol.as_str(), "test_half_5");
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.code, "contract_violation");
    assert_eq!(fault.data["clause_kind"], "ensures");
    assert_eq!(record.counterexamples.len(), 1);
    let ce = serde_json::to_value(&record.counterexamples[0]).unwrap();
    assert_eq!(ce["code"], "contract_counterexample");
    assert_eq!(ce["category"], "contract");
    assert_eq!(ce["severity"], "error");
    assert_eq!(ce["symbol"], "m.half");
    assert_eq!(ce["qualified_name"], "m.half");
    assert_eq!(ce["facet"], "body");
    assert_eq!(ce["clause"], "result * 2 == n");
    assert_eq!(ce["clause_kind"], "ensures");
    assert_eq!(ce["input"], json!({"n": 5}));
    assert_eq!(ce["actual"], json!(2));
    assert_eq!(ce["status"], "failed");
    assert_eq!(ce["found_by"], "test_half_5");
    assert_eq!(
        ce["span"],
        common::location_of(M, "src/m.cpl", "ensures result * 2 == n")
    );
    assert!(ce.get("backtrace").is_none());
}

#[test]
fn counterexamples_only_indict_another_declaration() {
    let fx = plain_fixture();
    let config = TestConfig::with_seed(3);
    // The test's own loop invariant: no counterexample.
    let record = run_test(&fx.p, fx.own_invariant, &config);
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.data["clause_kind"], "invariant");
    assert_eq!(fault.data["iteration"], json!(2));
    assert_eq!(fault.data["actual"], json!({"bindings": {"i": 2}}));
    assert!(fault.data.get("input").is_none());
    assert!(record.counterexamples.is_empty());
    // A requires violation is reported through the fault only.
    let record = run_test(&fx.p, fx.calls_recip, &config);
    assert_eq!(
        record.fault.as_ref().unwrap().data["clause_kind"],
        "requires"
    );
    assert_eq!(
        record.fault.as_ref().unwrap().data["caller"],
        "tests.plain.\"calls recip\""
    );
    assert!(record.counterexamples.is_empty());
    // An ensures violation inside another declaration's clause: the fault is
    // rewritten for the clause (S34), the counterexample keeps the violation.
    let record = run_test(&fx.p, fx.nested_in_clause, &config);
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.symbol.as_ref().unwrap().as_str(), "m.guarded");
    assert_eq!(fault.data["clause"], "half(n) >= 0");
    assert_eq!(fault.data["clause_kind"], "ensures");
    assert_eq!(record.counterexamples.len(), 1);
    assert_eq!(
        record.counterexamples[0].symbol.as_ref().unwrap().as_str(),
        "m.half"
    );
    assert_eq!(record.counterexamples[0].data["clause"], "result * 2 == n");
}

#[test]
fn fail_fast_stops_after_the_first_failure() {
    let fx = plain_fixture();
    let sel = TestSelector::parse("tests.plain").unwrap();
    let mut config = TestConfig::with_seed(3);
    config.eval.time_budget = Some(Duration::from_millis(20));
    let all = run_tests(&fx.p, &sel, &config);
    assert_eq!((all.selected, all.tested, all.failed), (7, 1, 6));
    config.fail_fast = true;
    let fast = run_tests(&fx.p, &sel, &config);
    // "asserts" is first in qualified-name order and fails.
    assert_eq!((fast.selected, fast.tested, fast.failed), (7, 0, 1));
    assert_eq!(fast.results.len(), 1);
    assert_eq!(fast.results[0].qualified_name, "tests.plain.\"asserts\"");
}

#[test]
fn the_time_budget_applies_to_each_test() {
    let fx = plain_fixture();
    let mut config = TestConfig::with_seed(3);
    config.eval.time_budget = Some(Duration::from_millis(20));
    let record = run_test(&fx.p, fx.spins, &config);
    assert_eq!(record.status, TestStatus::Failed);
    let fault = record.fault.as_ref().unwrap();
    assert_eq!(fault.code, "resource_exhaustion");
    assert_eq!(fault.data["resource"], "time");
    assert_eq!(fault.data["limit"], json!(20));
    // A later test gets its own budget.
    assert_eq!(
        run_test(&fx.p, fx.passing, &config).status,
        TestStatus::Tested
    );
}
