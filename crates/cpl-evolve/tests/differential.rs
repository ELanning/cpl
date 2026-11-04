use cpl_diag::Facet;
use cpl_evolve::{change::*, differential::Differential, verify::VerifyConfig, Snapshot};
use cpl_project::{LoadOptions, VirtualTree};
use std::collections::BTreeMap;

fn snapshot(source: &str) -> Snapshot {
    let tree = VirtualTree {
        files: BTreeMap::from([
            (
                "cpl.toml".into(),
                b"[project]\nname = \"differential\"\nedition = \"0.1\"\n".to_vec(),
            ),
            ("src/m.cpl".into(), source.as_bytes().to_vec()),
        ]),
    };
    Snapshot::new(tree, &LoadOptions::default())
        .unwrap_or_else(|d| panic!("{}: {:?}", d.message, d.data))
}

/// Replace the body of `symbol` and compare old and new on generated inputs.
fn change(s: &Snapshot, symbol: &str, body: &str, spec: Differential) -> Candidate {
    let id = s.resolve(symbol).unwrap();
    let c = Change {
        name: "rewrite".into(),
        actor: "optimizer".into(),
        edits: vec![Edit {
            symbol: symbol.into(),
            expect: BTreeMap::from([(
                Facet::Body,
                s.index().version(id).facets[&Facet::Body].to_string(),
            )]),
            operation: Operation::ReplaceBody { body: body.into() },
        }],
        reads: vec![],
        contract: ChangeContract {
            differential: vec![spec],
            ..ChangeContract::default()
        },
        verification: VerifyConfig::default(),
    };
    prepare(s, &c, None, &LoadOptions::default())
        .unwrap_or_else(|d| panic!("{}: {:?}", d.message, d.data))
}
fn spec(symbol: &str) -> Differential {
    Differential {
        symbol: symbol.into(),
        cases: None,
        filter: None,
    }
}

const SOURCE: &str = r#"module m
@id("fn_calc")
pub pure fn calc(n: Int) -> Int { n + 1 }
@id("test_calc")
test "calc" { assert calc(1) == 2 }
@id("fn_positive")
pure fn positive(n: Int) -> Bool { n > 0 }
@id("fn_root")
pure fn root(n: Int) -> Int
    requires n >= 0
{ n / 2 }
"#;

#[test]
fn an_equivalent_rewrite_agrees_on_every_generated_case() {
    let s = snapshot(SOURCE);
    let c = change(&s, "fn_calc", "{ 1 + n }", spec("fn_calc"));
    assert!(c.report.accepted, "{:#?}", c.report.behavior);
    let record = &c.report.behavior[0];
    assert_eq!(record["status"], "tested");
    assert_eq!(record["method"], "differential");
    assert_eq!(record["cases_passed"], 100);
    assert_eq!(record["equivalence"], "unknown");
}

#[test]
fn a_behavior_change_the_tests_miss_has_a_counterexample() {
    let s = snapshot(SOURCE);
    let body = "{ if n < -5 { n } else { n + 1 } }";
    let unchecked = prepare(
        &s,
        &Change {
            contract: ChangeContract::default(),
            ..change(&s, "fn_calc", body, spec("fn_calc")).report.request
        },
        None,
        &LoadOptions::default(),
    )
    .unwrap();
    assert!(unchecked.report.accepted, "the unit test alone passes");
    let c = change(&s, "fn_calc", body, spec("fn_calc"));
    assert!(!c.report.accepted);
    let failure = &c.report.behavior[0];
    assert_eq!(failure["status"], "failed");
    // The generated input shrinks to the boundary of the changed region.
    assert_eq!(failure["arguments"], serde_json::json!([-6]), "{failure}");
    assert_eq!(failure["before"]["value"], -5);
    assert_eq!(failure["after"]["value"], -6);
}

#[test]
fn preconditions_and_filters_bound_the_compared_domain() {
    let s = snapshot(SOURCE);
    // Negative inputs violate root's requires and are discarded, so this rewrite agrees.
    let c = change(
        &s,
        "fn_root",
        "{ if n < 0 { 0 - 1 } else { n / 2 } }",
        spec("fn_root"),
    );
    assert!(c.report.accepted, "{:#?}", c.report.behavior);
    assert!(c.report.behavior[0]["discarded"].as_u64().unwrap() > 0);
    // Changing behavior outside "existing positive behavior" is allowed with that filter.
    let body = "{ if n > 0 { n + 1 } else { 0 } }";
    let filtered = Differential {
        filter: Some("m.positive".into()),
        ..spec("fn_calc")
    };
    assert!(change(&s, "fn_calc", body, filtered).report.accepted);
    assert!(!change(&s, "fn_calc", body, spec("fn_calc")).report.accepted);
    let wrong = Differential {
        filter: Some("m.calc".into()),
        ..spec("fn_calc")
    };
    let id = s.resolve("fn_calc").unwrap();
    let request = Change {
        name: "rewrite".into(),
        actor: "optimizer".into(),
        edits: vec![Edit {
            symbol: "fn_calc".into(),
            expect: BTreeMap::from([(
                Facet::Body,
                s.index().version(id).facets[&Facet::Body].to_string(),
            )]),
            operation: Operation::ReplaceBody { body: body.into() },
        }],
        reads: vec![],
        contract: ChangeContract {
            differential: vec![wrong],
            ..ChangeContract::default()
        },
        verification: VerifyConfig::default(),
    };
    let error = prepare(&s, &request, None, &LoadOptions::default())
        .err()
        .unwrap();
    assert_eq!(error.code.as_str(), "differential_filter_invalid");
}

#[test]
fn user_types_refinements_and_faults_are_generated_and_compared() {
    let source = r#"module m
type Small = Int where self >= 0 and self < 10
enum Shape {
    Square(Small)
    Rect(Small, Small)
    Dot
}
struct Tagged {
    shape: Shape
    label: Text
}
@id("fn_area")
pure fn area(t: Tagged) -> Int {
    match t.shape {
        Square(s) => s * s
        Rect(a, b) => a * b
        Dot => 0
    }
}
@id("fn_ratio")
pure fn ratio(n: Int) -> Int { 100 / n }
"#;
    let s = snapshot(source);
    let rewrite = "{\n    match t.shape {\n        Square(s) => s * s\n        Rect(a, b) => b * a\n        Dot => 0 * 1\n    }\n}";
    let c = change(&s, "fn_area", rewrite, spec("fn_area"));
    assert!(c.report.accepted, "{:#?}", c.report.behavior);
    // A refinement outside its range is discarded at the boundary, not compared.
    assert_eq!(c.report.behavior[0]["cases_passed"], 100);
    // Both versions divide by zero at 0: the same fault is agreement. The candidate is still
    // not accepted: its division obligation fails, with `n = 0` as the counterexample.
    let c = change(&s, "fn_ratio", "{ (100 * 1) / n }", spec("fn_ratio"));
    assert_eq!(
        c.report.behavior[0]["status"], "tested",
        "{:#?}",
        c.report.behavior
    );
    let division = c
        .report
        .evidence
        .obligations
        .iter()
        .find(|o| o.kind == "division")
        .expect("a division obligation");
    assert_eq!(division.status, cpl_diag::json::ObligationStatus::Failed);
    assert_eq!(division.evidence.as_ref().unwrap()["model"]["n"], 0);
    assert!(!c.report.accepted);
    let c = change(
        &s,
        "fn_ratio",
        "{ if n == 0 { 0 } else { 100 / n } }",
        spec("fn_ratio"),
    );
    assert_eq!(c.report.behavior[0]["status"], "failed");
    assert!(c.report.behavior[0]["before"]["fault"].is_object());
    assert_eq!(c.report.behavior[0]["arguments"], serde_json::json!([0]));
}

#[test]
fn an_empty_domain_is_untested_rather_than_passing() {
    let source = r#"module m
@id("fn_never")
pure fn never(n: Int) -> Int
    requires n == 123456789123456789
{ n }
"#;
    let s = snapshot(source);
    let c = change(
        &s,
        "fn_never",
        "{ n + 0 }",
        Differential {
            cases: Some(5),
            ..spec("fn_never")
        },
    );
    assert!(!c.report.accepted);
    assert_eq!(c.report.behavior[0]["status"], "untested");
    assert_eq!(c.report.behavior[0]["discarded"], 50);
}
