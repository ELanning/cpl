//! Measured `improve` requirements and proved behavior preservation together.
use cpl_diag::Facet;
use cpl_evolve::{change::*, verify::VerifyConfig, Snapshot};
use cpl_project::{LoadOptions, VirtualTree};
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: &str = r#"module m
@id("fn_sum")
pub pure fn sum(n: Int) -> Int
    requires n >= 0
    ensures result >= 0
{
    var total = 0
    var i = 0
    while i <= n
        invariant i >= 0
        invariant total >= 0
        decreases n - i + 1
    {
        total = total + i
        i = i + 1
    }
    total
}
@id("test_sum")
test "sum" { assert sum(4) == 10 }
"#;

fn snapshot(source: &str) -> Snapshot {
    let tree = VirtualTree {
        files: BTreeMap::from([
            (
                "cpl.toml".into(),
                b"[project]\nname = \"improve\"\nedition = \"0.1\"\n".to_vec(),
            ),
            ("src/m.cpl".into(), source.as_bytes().to_vec()),
        ]),
    };
    Snapshot::new(tree, &LoadOptions::default()).unwrap_or_else(|d| panic!("{}", d.message))
}

fn change(s: &Snapshot, body: &str, improve: Vec<Improvement>) -> Candidate {
    let id = s.resolve("m.sum").unwrap();
    let c = Change {
        name: "faster".into(),
        actor: "optimizer".into(),
        edits: vec![Edit {
            symbol: "fn_sum".into(),
            expect: BTreeMap::from([(
                Facet::Body,
                s.index().version(id).facets[&Facet::Body].to_string(),
            )]),
            operation: Operation::ReplaceBody { body: body.into() },
        }],
        reads: vec![],
        contract: ChangeContract {
            improve,
            ..ChangeContract::default()
        },
        verification: VerifyConfig::default(),
    };
    prepare(s, &c, None, &LoadOptions::default()).unwrap_or_else(|d| panic!("{}", d.message))
}

fn improvement(c: &Candidate) -> &Value {
    c.report
        .obligations
        .iter()
        .find(|o| o["requirement"] == "time_complexity")
        .expect("a time_complexity requirement")
}

#[test]
fn a_closed_form_improves_the_time_complexity_of_a_loop() {
    let s = snapshot(SOURCE);
    let c = change(
        &s,
        "{\n    n * (n + 1) / 2\n}",
        vec![Improvement::Named("time_complexity".into())],
    );
    let o = improvement(&c);
    assert_eq!(o["status"], "measured", "{o:#}");
    assert_eq!(o["symbol"], "fn_sum");
    let rows = o["evidence"]["measurements"].as_array().unwrap();
    assert!(rows.len() >= 5, "{o:#}");
    // The loop grows linearly, the closed form not at all.
    let exponent = &o["evidence"]["growth_exponent"];
    assert!(exponent["before"].as_f64().unwrap() > 0.8, "{exponent}");
    assert!(exponent["after"].as_f64().unwrap() < 0.2, "{exponent}");
    assert!(c.report.accepted, "{:#?}", c.report.obligations);
}

#[test]
fn a_constant_factor_or_a_slowdown_is_not_a_complexity_improvement() {
    let s = snapshot(SOURCE);
    // Half the iterations: faster, but still linear.
    let halved = "{\n    var total = 0\n    var i = 0\n    while i + 1 <= n\n        invariant i >= 0\n        invariant total >= 0\n        decreases n - i + 1\n    {\n        total = total + i + i + 1\n        i = i + 2\n    }\n    if i == n { total + n } else { total }\n}";
    let c = change(
        &s,
        halved,
        vec![Improvement::Target {
            symbol: "fn_sum".into(),
            measure: "time_complexity".into(),
        }],
    );
    let o = improvement(&c);
    assert_eq!(o["status"], "failed", "{o:#}");
    assert_eq!(o["reason"], "no_asymptotic_improvement", "{o:#}");
    assert!(!c.report.accepted);
    // The same halving does satisfy a request for fewer steps.
    let c = change(
        &s,
        halved,
        vec![Improvement::Target {
            symbol: "fn_sum".into(),
            measure: "steps".into(),
        }],
    );
    let o = &c.report.obligations[0];
    assert_eq!(o["status"], "measured", "{o:#}");
    // A quadratic version is slower.
    let quadratic = "{\n    var total = 0\n    var i = 0\n    while i <= n\n        invariant i >= 0\n        invariant total >= 0\n        decreases n - i + 1\n    {\n        var j = 0\n        while j < i\n            invariant j >= 0\n            decreases i - j\n        {\n            total = total + 1\n            j = j + 1\n        }\n        i = i + 1\n    }\n    total\n}";
    let c = change(
        &s,
        quadratic,
        vec![Improvement::Named("time_complexity".into())],
    );
    let o = improvement(&c);
    assert_eq!(o["status"], "failed", "{o:#}");
    assert!(
        o["reason"].as_str().unwrap().starts_with("slower at size"),
        "{o:#}"
    );
}

#[test]
fn a_change_that_alters_results_is_not_an_improvement() {
    let s = snapshot(SOURCE);
    let c = change(
        &s,
        "{\n    n * n\n}",
        vec![Improvement::Named("time_complexity".into())],
    );
    let o = improvement(&c);
    assert_eq!(o["status"], "failed", "{o:#}");
    assert_eq!(o["reason"], "behavior_changed");
}
