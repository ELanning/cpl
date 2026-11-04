use cpl_diag::{Facet, StableId};
use cpl_evolve::{
    change::*,
    explore::{self, ExploreConfig},
    migration, query,
    verify::{self, VerifyConfig},
    Snapshot,
};
use cpl_project::{LoadOptions, VirtualTree};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const SOURCE: &str = r#"module m
@id("fn_calc")
@intent("Calculate the next integer")
@preserve("Positive inputs keep their behavior")
pub pure fn calc(n: Int) -> Int { n + 1 }
@id("fn_caller")
pure fn caller(n: Int) -> Int { calc(n) }
@id("test_calc")
test "calc result" { assert caller(1) == 2 }
@id("test_other")
test "independent" { assert 2 + 2 == 4 }
"#;
fn tree(source: &str) -> VirtualTree {
    VirtualTree {
        files: BTreeMap::from([
            (
                "cpl.toml".into(),
                b"[project]\nname = \"evolution\"\nedition = \"0.1\"\n".to_vec(),
            ),
            ("src/m.cpl".into(), source.as_bytes().to_vec()),
        ]),
    }
}
fn snapshot(source: &str) -> Snapshot {
    Snapshot::new(tree(source), &LoadOptions::default())
        .unwrap_or_else(|d| panic!("{}: {:?}", d.message, d.data))
}
fn body(s: &Snapshot, text: &str) -> Change {
    let id = s.resolve("m.calc").unwrap();
    Change {
        name: "optimize".into(),
        actor: "optimizer".into(),
        edits: vec![Edit {
            symbol: "fn_calc".into(),
            expect: BTreeMap::from([(
                Facet::Body,
                s.index().version(id).facets[&Facet::Body].to_string(),
            )]),
            operation: Operation::ReplaceBody { body: text.into() },
        }],
        reads: vec![],
        contract: ChangeContract {
            acknowledge: vec![Acknowledgement {
                symbol: "fn_calc".into(),
                statement: "Positive inputs keep their behavior".into(),
            }],
            ..ChangeContract::default()
        },
        verification: VerifyConfig::default(),
    }
}
fn prepared(s: &Snapshot, c: &Change) -> Candidate {
    prepare(s, c, None, &LoadOptions::default())
        .unwrap_or_else(|d| panic!("{}: {:?}", d.message, d.data))
}
fn id(s: &str) -> StableId {
    StableId::parse(s).unwrap()
}

#[test]
fn queries_expose_resolved_syntax_metadata_and_typed_expressions() {
    let s = snapshot(SOURCE);
    let symbol = query::query(&s, "symbol fn_calc", None).unwrap();
    assert_eq!(
        symbol["symbol"]["metadata"]["intent"][0],
        "Calculate the next integer"
    );
    assert!(!symbol["typed_expressions"].as_array().unwrap().is_empty());
    let callers = query::query(&s, "callers of m.calc", None).unwrap();
    assert_eq!(callers["symbols"][0]["version"]["id"], "fn_caller");
    let affected = query::query(&s, "affected by m.calc", None).unwrap();
    assert_eq!(affected["symbols"].as_array().unwrap().len(), 3);
    assert!(query::query(&s, "guess what I mean", None).is_err());
}

#[test]
fn facet_patches_rebase_over_renames_and_reject_stale_bodies() {
    let s = snapshot(SOURCE);
    let change = body(&s, "{ 1 + n }");
    let renamed = snapshot(
        &SOURCE
            .replace("fn calc(", "fn compute(")
            .replace("{ calc(n) }", "{ compute(n) }"),
    );
    let result = prepared(&renamed, &change);
    assert!(result.report.accepted);
    assert!(
        String::from_utf8_lossy(&result.snapshot.tree.files["src/m.cpl"]).contains("fn compute")
    );
    let stale = snapshot(&SOURCE.replace("n + 1", "n + 2"));
    assert_eq!(
        prepare(&stale, &change, None, &LoadOptions::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "change_stale"
    );
    assert_eq!(s.tree, tree(SOURCE));
}

#[test]
fn failing_candidates_and_unproved_change_contracts_are_not_accepted() {
    let s = snapshot(SOURCE);
    let result = prepared(&s, &body(&s, "{ n + 2 }"));
    assert!(!result.report.accepted);
    assert_eq!(result.report.evidence.tests.len(), 1);
    // Equivalence is proved symbolically.
    let mut change = body(&s, "{ 1 + n }");
    change.contract.preserve_behavior = true;
    let result = prepared(&s, &change);
    assert!(result.report.accepted, "{:#?}", result.report.obligations);
    assert_eq!(result.report.obligations[0]["status"], "proved");
    assert_eq!(
        result.report.obligations[0]["functions"][0]["evidence"]["by"],
        "symbolic_equivalence"
    );
    // A difference is refuted with the input that shows it.
    let mut change = body(&s, "{ if n == 7 { n } else { n + 1 } }");
    change.contract.preserve_behavior = true;
    let result = prepared(&s, &change);
    assert!(!result.report.accepted);
    let function = &result.report.obligations[0]["functions"][0];
    assert_eq!(function["status"], "failed", "{function}");
    assert_eq!(function["evidence"]["inputs"]["n"], 7);
    // Outside the fragment: unknown, and not accepted on test evidence.
    let mut change = body(&s, "{ (\"ab\" ++ \"c\").len + n - 2 }");
    change.contract.preserve_behavior = true;
    let result = prepared(&s, &change);
    assert!(!result.report.accepted);
    assert_eq!(result.report.obligations[0]["status"], "unknown");
    assert!(prepare(&s, &body(&s, "{ true }"), None, &LoadOptions::default()).is_err());
    let mut empty = body(&s, "{ n + 1 }");
    empty.edits[0].expect.clear();
    assert!(prepare(&s, &empty, None, &LoadOptions::default()).is_err());
}

#[test]
fn authority_and_preservation_are_enforced_before_acceptance() {
    let s = snapshot(SOURCE);
    let mut change = body(&s, "{ 1 + n }");
    let selection = Selection {
        symbol: "fn_calc".into(),
        facets: BTreeSet::from([Facet::Body]),
    };
    let mut policy = Policy {
        grants: vec![Grant {
            actor: "optimizer".into(),
            write: vec![selection.clone()],
            forbid: vec![],
            add: vec![],
            remove: vec![],
            read: None,
        }],
        regions: vec![],
    };
    assert!(
        prepare(&s, &change, Some(&policy), &LoadOptions::default())
            .unwrap()
            .report
            .accepted
    );
    policy.grants[0].forbid.push(selection.clone());
    assert_eq!(
        prepare(&s, &change, Some(&policy), &LoadOptions::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "change_write_denied"
    );
    policy.grants[0].forbid.clear();
    policy.regions.push(Region {
        namespace: "m".into(),
        owner: "another_agent".into(),
    });
    assert!(prepare(&s, &change, Some(&policy), &LoadOptions::default()).is_err());
    change.contract.preserve.push(selection);
    assert_eq!(
        prepare(&s, &change, None, &LoadOptions::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "change_preserve_violation"
    );
}

#[test]
fn rename_updates_imports_and_calls_but_keeps_strings_and_local_names() {
    let mut tree = tree(SOURCE);
    tree.files.insert("src/client.cpl".into(),b"module client\nuse m.calc\npure fn run() -> Int { calc(4) }\ntest \"calc is a name\" { assert run() == 5 }\n".to_vec());
    let s = Snapshot::new(tree, &LoadOptions::default()).unwrap();
    let mut change = body(&s, "{ n + 1 }");
    change.contract.acknowledge.clear();
    change.edits[0].operation = Operation::Rename {
        name: "compute".into(),
    };
    change.edits[0].expect = BTreeMap::from([(
        Facet::Name,
        s.index().version(s.resolve("fn_calc").unwrap()).facets[&Facet::Name].to_string(),
    )]);
    let result = prepared(&s, &change);
    assert!(result.report.accepted);
    let text = String::from_utf8_lossy(&result.snapshot.tree.files["src/client.cpl"]);
    assert!(text.contains("use m.compute"));
    assert!(text.contains("compute(4)"));
    assert!(text.contains("\"calc is a name\""));
}

#[test]
fn incremental_verification_tracks_transitive_type_and_contract_references() {
    let s = snapshot(SOURCE);
    let result = prepared(&s, &body(&s, "{ 1 + n }"));
    let evidence = &result.report.evidence;
    assert_eq!(evidence.impact.tests, BTreeSet::from([id("test_calc")]));
    assert!(!evidence.impact.affected.contains(&id("test_other")));
    assert!(verify::recheck(&result.snapshot, Some(&s), evidence).is_ok());
    let mut forged = evidence.clone();
    forged.tests.clear();
    forged.digest = forged.calculate_digest();
    assert!(verify::recheck(&result.snapshot, Some(&s), &forged).is_err());
    let options = VerifyConfig {
        cases: 0,
        ..VerifyConfig::default()
    };
    assert!(verify::verify(&s, None, &options).is_err());
    // A postcondition outside the verifier's fragment (text equality) stays unknown.
    let with_contract = snapshot(&SOURCE.replace(
        "{ n + 1 }",
        "\nensures result.to_text() != \"0\"\n{ n + 1 }",
    ));
    let proof = verify::verify(
        &with_contract,
        None,
        &VerifyConfig {
            require_proofs: true,
            ..VerifyConfig::default()
        },
    )
    .unwrap();
    assert!(!proof.accepted);
    assert!(proof.unresolved() > 0);
}

#[test]
fn conflicting_facets_and_dependent_assumptions_have_different_statuses() {
    let s = snapshot(SOURCE);
    let a = snapshot(&SOURCE.replace("n + 1", "n + 2"));
    let b = snapshot(&SOURCE.replace("n + 1", "n + 3"));
    let options = LoadOptions::default();
    let merged = cpl_evolve::merge::merge(&s, &a, &b, &VerifyConfig::default(), &options).unwrap();
    assert_eq!(merged.report.status, "conflict");
    assert!(merged
        .report
        .conflicts
        .iter()
        .any(|c| c["status"] == "conflict" && c["facet"] == "body"));
    // A dependent change composes structurally; its interaction stays visible as unknown, and the
    // composition's own test evidence decides acceptance.
    let b = snapshot(&SOURCE.replace("{ calc(n) }", "{ calc(n) - 1 }"));
    let merged = cpl_evolve::merge::merge(&s, &a, &b, &VerifyConfig::default(), &options).unwrap();
    assert!(merged.report.conflicts.is_empty());
    assert!(merged
        .report
        .interactions
        .iter()
        .all(|c| c["status"] == "unknown"));
    assert!(!merged.report.interactions.is_empty());
}

const MACHINE: &str = r#"module m
program App { n: Int } where self.n <= 2
enum Event {
    Add
    Reset
}
initial fn start() -> App { App { n: 0 } }
machine update(state: App, event: Event) -> App {
    match event {
        Add => App { n: state.n + 1 }
        Reset => App { n: 0 }
    }
}
"#;
fn event(name: &str) -> Value {
    json!({"$type":"m.Event","$variant":name})
}
#[test]
fn breadth_first_exploration_finds_shortest_trace_and_reports_bounds() {
    let s = snapshot(MACHINE);
    let initial = json!({"$type":"m.App","n":0});
    let events = vec![event("Reset"), event("Add")];
    let result = explore::explore(
        &s,
        "m.update",
        Some(&initial),
        Some(&events),
        &ExploreConfig::default(),
    )
    .unwrap();
    assert_eq!(result.result, "counterexample");
    assert_eq!(
        result.counterexample.unwrap()["events"],
        json!([event("Add"), event("Add"), event("Add")])
    );
    let result = explore::explore(
        &s,
        "m.update",
        Some(&initial),
        Some(&events),
        &ExploreConfig {
            depth: 2,
            ..ExploreConfig::default()
        },
    )
    .unwrap();
    assert_eq!(result.result, "bounded_pass");
    let result = explore::explore(
        &s,
        "m.update",
        Some(&initial),
        Some(&events),
        &ExploreConfig {
            max_states: 1,
            ..ExploreConfig::default()
        },
    )
    .unwrap();
    assert_eq!(result.result, "incomplete");
    let result = explore::explore(
        &s,
        "m.update",
        Some(&initial),
        Some(&[event("Reset")]),
        &ExploreConfig::default(),
    )
    .unwrap();
    assert_eq!(result.result, "exhaustive");
}

const ACCOUNT: &str = r#"module m
program Account {
    charged: Int
    refunded: Int
    open: Bool
}
enum Action {
    Charge
    Refund
    Close
    Toggle(Bool)
}
initial fn start() -> Account { Account { charged: 0, refunded: 0, open: true } }
machine step(state: Account, event: Action) -> Account {
    match event {
        Charge => if state.open and state.charged < 2 { Account { ..state, charged: state.charged + 1 } } else { state }
        Refund => if state.refunded < 3 { Account { ..state, refunded: state.refunded + 1 } } else { state }
        Close => Account { ..state, open: false }
        Toggle(flag) => Account { ..state, open: flag }
    }
}
pure fn refunds_bounded(state: Account) -> Bool { state.refunded <= state.charged }
pure fn closed_with_charge(state: Account) -> Bool { not state.open and state.charged == 2 }
pure fn impossible(state: Account) -> Bool { state.charged > 2 }
pure fn wrong(n: Int) -> Bool { n > 0 }
"#;

#[test]
fn exploration_defaults_to_the_initializer_and_a_derived_alphabet() {
    let s = snapshot(ACCOUNT);
    let result = explore::explore(&s, "m.step", None, None, &ExploreConfig::default()).unwrap();
    assert_eq!(result.initial_source, "initializer");
    assert_eq!(result.alphabet_source, "derived");
    // Charge, Refund, Close, Toggle(false), Toggle(true).
    assert_eq!(result.event_alphabet.len(), 5);
    assert_eq!(result.result, "exhaustive");
    // An invariant that is not a construction invariant: refunds never exceed charges.
    let config = ExploreConfig {
        invariants: vec!["m.refunds_bounded".into()],
        ..ExploreConfig::default()
    };
    let result = explore::explore(&s, "m.step", None, None, &config).unwrap();
    assert_eq!(result.result, "counterexample");
    let counterexample = result.counterexample.unwrap();
    assert_eq!(counterexample["invariant"], "m.refunds_bounded");
    assert_eq!(
        counterexample["events"],
        json!([{"$type": "m.Action", "$variant": "Refund"}])
    );
    // A reachability question with a shortest witness.
    let config = ExploreConfig {
        goal: Some("m.closed_with_charge".into()),
        ..ExploreConfig::default()
    };
    let result = explore::explore(&s, "m.step", None, None, &config).unwrap();
    assert_eq!(result.result, "witness");
    assert_eq!(
        result.witness.unwrap()["events"].as_array().unwrap().len(),
        3
    );
    let config = ExploreConfig {
        goal: Some("m.impossible".into()),
        ..ExploreConfig::default()
    };
    let result = explore::explore(&s, "m.step", None, None, &config).unwrap();
    assert_eq!(result.result, "unreachable");
    let config = ExploreConfig {
        invariants: vec!["m.wrong".into()],
        ..ExploreConfig::default()
    };
    assert_eq!(
        explore::explore(&s, "m.step", None, None, &config)
            .err()
            .unwrap()
            .code
            .as_str(),
        "explore_predicate_invalid"
    );
    // An Int payload has no finite alphabet: payload samples must be supplied.
    let infinite = snapshot(&ACCOUNT.replace("Toggle(Bool)", "Toggle(Int)").replace(
        "Toggle(flag) => Account { ..state, open: flag }",
        "Toggle(n) => Account { ..state, open: n > 0 }",
    ));
    assert_eq!(
        explore::explore(&infinite, "m.step", None, None, &ExploreConfig::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "explore_alphabet_infinite"
    );
}

const PROTOCOL: &str = r#"module m
@protocol("connection", "closed", "initial_terminal")
type Closed(Int)
@protocol("connection", "open", "state")
type Open(Int)
@transition("connection", "closed", "open")
@message("Client", "Server")
pure fn connect(state: Closed) -> Open { match state { Closed(n) => Open(n) } }
@transition("connection", "open", "closed")
pure fn close(state: Open) -> Closed { match state { Open(n) => Closed(n) } }
test "protocol" { assert close(connect(Closed(1))) == Closed(1) }
"#;
#[test]
fn protocol_graphs_and_typed_endpoints_reject_invalid_programs() {
    snapshot(PROTOCOL);
    for invalid in [
        PROTOCOL.replace("close(connect(Closed(1)))", "close(Closed(1))"),
        PROTOCOL.replace("\"open\", \"closed\"", "\"closed\", \"open\""),
        PROTOCOL.replace("@transition(\"connection\", \"open\", \"closed\")", ""),
    ] {
        assert!(Snapshot::new(tree(&invalid), &LoadOptions::default()).is_err());
    }
}

const SCHEMAS: &str = r#"module m
@id("ty_user1")
@schema("user", "1")
struct UserV1 { name: Text }
@id("ty_user2")
@schema("user", "2")
struct UserV2 { name: Text
active: Bool } where self.active
@id("fn_migrate")
@migration("user", "1", "2")
pure fn upgrade(old: UserV1) -> UserV2 { UserV2 { name: old.name, active: true } }
"#;
#[test]
fn migrations_validate_versions_fingerprints_purity_and_output_invariants() {
    let s = snapshot(SCHEMAS);
    let cfg = VerifyConfig::default();
    let stored = migration::pack(
        &s,
        "m.UserV1",
        &json!({"$type":"m.UserV1","name":"Ada"}),
        &cfg,
    )
    .unwrap();
    let output = migration::migrate(&s, "m.upgrade", &stored, &cfg).unwrap();
    assert_eq!(output.version, "2");
    assert_eq!(output.value["active"], true);
    let mut stale = stored.clone();
    stale.schema_digest = "forged".into();
    assert!(migration::migrate(&s, "m.upgrade", &stale, &cfg).is_err());
    let failing = snapshot(&SCHEMAS.replace("active: true", "active: old.name == \"ok\""));
    assert!(migration::migrate(&failing, "m.upgrade", &stored, &cfg).is_err());
    assert!(Snapshot::new(
        tree(&SCHEMAS.replace("pure fn upgrade", "mutate fn upgrade")),
        &LoadOptions::default()
    )
    .is_err());
    assert!(migration::pack(
        &s,
        "m.UserV2",
        &json!({"$type":"m.UserV2","name":"Ada","active":false}),
        &cfg
    )
    .is_err());
    let changed = snapshot(&SCHEMAS.replace(
        "struct UserV1 { name: Text }",
        "struct UserV1 { name: Text\nextra: Int }",
    ));
    assert!(migration::require_migrations(&s, &changed).is_err());
}

#[test]
fn values_migrate_along_the_shortest_declared_path_and_verification_guards_schemas() {
    let source = format!(
        "{SCHEMAS}@id(\"ty_user3\")\n@schema(\"user\", \"3\")\nstruct UserV3 {{ name: Text\nactive: Bool\nrole: Text }}\n\
         @id(\"fn_migrate3\")\n@migration(\"user\", \"2\", \"3\")\npure fn promote(old: UserV2) -> UserV3 {{ UserV3 {{ name: old.name, active: old.active, role: \"member\" }} }}\n"
    );
    let s = snapshot(&source);
    let cfg = VerifyConfig::default();
    let stored = migration::pack(
        &s,
        "m.UserV1",
        &json!({"$type":"m.UserV1","name":"Ada"}),
        &cfg,
    )
    .unwrap();
    let (output, steps) = migration::migrate_to(&s, &stored, "3", &cfg).unwrap();
    assert_eq!(steps, vec!["m.upgrade", "m.promote"]);
    assert_eq!(output.version, "3");
    assert_eq!(output.value["role"], "member");
    let (same, steps) = migration::migrate_to(&s, &stored, "1", &cfg).unwrap();
    assert!(steps.is_empty());
    assert_eq!(same, stored);
    assert_eq!(
        migration::migrate_to(&s, &output, "1", &cfg)
            .unwrap_err()
            .code
            .as_str(),
        "migration_path_missing"
    );
    // An in-place schema edit is caught by incremental verification, not only by transactions.
    let edited = snapshot(
        &source
            .replace("role: Text }", "role: Text\nlevel: Int }")
            .replace("role: \"member\" }", "role: \"member\", level: 0 }"),
    );
    let evidence = verify::verify(&edited, Some(&s), &cfg).unwrap();
    assert!(!evidence.accepted);
    assert_eq!(evidence.schemas[0]["status"], "changed_in_place");
    assert_eq!(evidence.summary["schemas"], "incompatible");
    let allowed = verify::verify(
        &edited,
        Some(&s),
        &VerifyConfig {
            require_migrations: false,
            ..cfg.clone()
        },
    )
    .unwrap();
    assert!(allowed.accepted, "the issue is reported but not required");
    assert!(!allowed.schemas.is_empty());
    let unchanged = verify::verify(&s, Some(&s), &cfg).unwrap();
    assert_eq!(unchanged.summary["schemas"], "compatible");
    assert_eq!(unchanged.summary["scope"]["affected"], 0);
}

#[test]
fn transaction_can_add_a_version_and_migration_as_one_checked_change() {
    let old = SCHEMAS.split("@id(\"ty_user2\")").next().unwrap();
    let s = snapshot(old);
    let user =
        "@id(\"ty_user2\")\n@schema(\"user\", \"2\")\nstruct UserV2 { name: Text\nactive: Bool }";
    let upgrade="@id(\"fn_migrate\")\n@migration(\"user\", \"1\", \"2\")\npure fn upgrade(old: UserV1) -> UserV2 { UserV2 { name: old.name, active: true } }";
    let mut change = Change {
        name: "schema".into(),
        actor: "schema_owner".into(),
        edits: vec![],
        reads: vec![],
        contract: ChangeContract::default(),
        verification: VerifyConfig::default(),
    };
    for (symbol, source) in [("ty_user2", user), ("fn_migrate", upgrade)] {
        change.edits.push(Edit {
            symbol: symbol.into(),
            expect: BTreeMap::new(),
            operation: Operation::AddDeclaration {
                file: "src/m.cpl".into(),
                module: "m".into(),
                source: source.into(),
            },
        });
    }
    assert!(prepared(&s, &change).report.accepted);
    change.edits.pop();
    assert_eq!(
        prepare(&s, &change, None, &LoadOptions::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "migration_required"
    );
}

#[test]
fn holes_are_structured_requests_and_fill_hole_keeps_precedence() {
    let source = r#"module m
@id("fn_scale")
pure fn scale(n: Int) -> Int
    requires n >= 0
{ 3 * ?h }
@id("test_scale")
test "scale" { assert scale(1) == 6 }
"#;
    let s = snapshot(source);
    let holes = query::query(&s, "holes", None).unwrap();
    let hole = &holes["holes"][0];
    assert_eq!(hole["hole"], "h");
    assert_eq!(hole["symbol"], "fn_scale");
    assert_eq!(hole["expected_type"], "Int");
    assert_eq!(hole["bindings"][0]["name"], "n");
    assert_eq!(hole["requires"][0], "n >= 0");
    let id = s.resolve("fn_scale").unwrap();
    let operation = Operation::FillHole {
        hole: "h".into(),
        expression: "n + 1".into(),
    };
    assert_eq!(
        operation_facets(&s, id, &operation).unwrap(),
        vec![Facet::Body]
    );
    let change = Change {
        name: "fill".into(),
        actor: "synthesizer".into(),
        edits: vec![Edit {
            symbol: "fn_scale".into(),
            expect: BTreeMap::from([(
                Facet::Body,
                s.index().version(id).facets[&Facet::Body].to_string(),
            )]),
            operation,
        }],
        reads: vec![],
        contract: ChangeContract::default(),
        verification: VerifyConfig::default(),
    };
    let filled = prepared(&s, &change);
    assert!(filled.report.accepted, "{:#?}", filled.report.evidence);
    assert!(
        String::from_utf8_lossy(&filled.snapshot.tree.files["src/m.cpl"]).contains("3 * (n + 1)")
    );
    assert_eq!(filled.report.evidence.holes, 0);
    let mut missing = change.clone();
    missing.edits[0].operation = Operation::FillHole {
        hole: "other".into(),
        expression: "n".into(),
    };
    assert_eq!(
        prepare(&s, &missing, None, &LoadOptions::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "change_hole_missing"
    );
    let mut invalid = change;
    invalid.edits[0].operation = Operation::FillHole {
        hole: "h".into(),
        expression: "n +".into(),
    };
    assert_eq!(
        prepare(&s, &invalid, None, &LoadOptions::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "change_syntax"
    );
    let tests = query::query(&s, "tests of fn_scale", None).unwrap();
    assert_eq!(tests["symbols"][0]["version"]["id"], "test_scale");
}

#[test]
fn protocol_queries_return_the_checked_state_graph() {
    let s = snapshot(PROTOCOL);
    let graph = query::query(&s, "protocol connection", None).unwrap();
    assert_eq!(graph["initial"], json!(["closed"]));
    assert_eq!(graph["terminal"], json!(["closed"]));
    let transitions = graph["transitions"].as_array().unwrap();
    assert_eq!(transitions.len(), 2);
    let connect = transitions
        .iter()
        .find(|t| t["function"] == "m.connect")
        .unwrap();
    assert_eq!(
        (connect["from"].as_str(), connect["to"].as_str()),
        (Some("closed"), Some("open"))
    );
    assert_eq!(
        (connect["sender"].as_str(), connect["receiver"].as_str()),
        (Some("Client"), Some("Server"))
    );
    assert!(query::query(&s, "protocol nothing", None).is_err());
    assert_eq!(graph["unanswered"], json!([]));
    // A purchase where the server, having received a payment decision, never answers the customer.
    let purchase = r#"module m
@protocol("purchase", "start", "initial")
type Start(Int)
@protocol("purchase", "ordered", "state")
type Ordered(Int)
@protocol("purchase", "authorizing", "state")
type Authorizing(Int)
@protocol("purchase", "done", "terminal")
type Done(Int)
@transition("purchase", "start", "ordered")
@message("Customer", "Server")
pure fn order(s: Start) -> Ordered { match s { Start(n) => Ordered(n) } }
@transition("purchase", "ordered", "authorizing")
@message("Server", "Payment")
pure fn authorize(s: Ordered) -> Authorizing { match s { Ordered(n) => Authorizing(n) } }
@transition("purchase", "authorizing", "done")
@message("Server", "Customer")
pure fn confirm(s: Authorizing) -> Done { match s { Authorizing(n) => Done(n) } }
"#;
    let graph = query::query(&snapshot(purchase), "protocol purchase", None).unwrap();
    assert_eq!(
        graph["unanswered"],
        json!([{"transition": "m.authorize", "state": "authorizing", "receiver": "Payment"}])
    );
}

#[test]
fn replacing_a_preserved_declaration_requires_an_acknowledgement_or_a_revision() {
    let s = snapshot(SOURCE);
    let mut change = body(&s, "{ 1 + n }");
    let acknowledged = prepared(&s, &change);
    assert_eq!(
        acknowledged.report.attestations,
        vec![
            json!({"symbol": "fn_calc", "statement": "Positive inputs keep their behavior",
            "status": "attested", "by": "optimizer"})
        ]
    );
    change.contract.acknowledge.clear();
    let missing = prepare(&s, &change, None, &LoadOptions::default())
        .err()
        .unwrap();
    assert_eq!(missing.code.as_str(), "change_preserve_unacknowledged");
    assert_eq!(missing.data["statements"][0]["symbol"], "fn_calc");
    change.contract.acknowledge.push(Acknowledgement {
        symbol: "fn_calc".into(),
        statement: "Something else".into(),
    });
    assert_eq!(
        prepare(&s, &change, None, &LoadOptions::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "change_acknowledgement_invalid"
    );
    // Revising the statement in the same change records the revision instead.
    let id = s.resolve("fn_calc").unwrap();
    let revised = Change {
        edits: vec![Edit {
            symbol: "fn_calc".into(),
            expect: s.index().version(id).facets.iter().map(|(f, h)| (*f, h.to_string())).collect(),
            operation: Operation::ReplaceDeclaration {
                source: "@id(\"fn_calc\")\n@intent(\"Calculate the next integer\")\n@preserve(\"All inputs keep their behavior\")\npub pure fn calc(n: Int) -> Int { 1 + n }".into(),
            },
        }],
        contract: ChangeContract::default(),
        ..change.clone()
    };
    let result = prepared(&s, &revised);
    assert_eq!(result.report.attestations[0]["status"], "revised");
    assert_eq!(
        result.report.attestations[0]["revision"],
        "All inputs keep their behavior"
    );
    // A rename changes no behavior and needs no acknowledgement.
    let mut rename = change;
    rename.contract.acknowledge.clear();
    rename.edits[0].operation = Operation::Rename {
        name: "compute".into(),
    };
    rename.edits[0].expect = BTreeMap::from([(
        Facet::Name,
        s.index().version(id).facets[&Facet::Name].to_string(),
    )]);
    assert!(prepared(&s, &rename).report.attestations.is_empty());
}

#[test]
fn declarations_are_removed_only_when_unused_and_permitted() {
    let source =
        format!("{SOURCE}/// Unused.\n@id(\"fn_unused\")\npure fn unused() -> Int {{ 0 }}\n");
    let s = snapshot(&source);
    let removal = |symbol: &str| {
        let id = s.resolve(symbol).unwrap();
        Change {
            name: "cleanup".into(),
            actor: "janitor".into(),
            edits: vec![Edit {
                symbol: symbol.into(),
                expect: s
                    .index()
                    .version(id)
                    .facets
                    .iter()
                    .map(|(f, h)| (*f, h.to_string()))
                    .collect(),
                operation: Operation::RemoveDeclaration,
            }],
            reads: vec![],
            contract: ChangeContract::default(),
            verification: VerifyConfig::default(),
        }
    };
    let removed = prepared(&s, &removal("fn_unused"));
    assert!(removed.report.accepted);
    let text = String::from_utf8_lossy(&removed.snapshot.tree.files["src/m.cpl"]).to_string();
    assert_eq!(text, SOURCE, "docs, annotations, and the line go with it");
    // A declaration that is still referenced cannot be removed: the candidate does not check.
    assert_eq!(
        prepare(&s, &removal("fn_calc"), None, &LoadOptions::default())
            .err()
            .unwrap()
            .code
            .as_str(),
        "evolution_invalid_program"
    );
    let mut policy = Policy {
        grants: vec![Grant {
            actor: "janitor".into(),
            write: vec![],
            forbid: vec![],
            add: vec![],
            remove: vec![],
            read: None,
        }],
        regions: vec![],
    };
    let denied = prepare(
        &s,
        &removal("fn_unused"),
        Some(&policy),
        &LoadOptions::default(),
    );
    assert_eq!(denied.err().unwrap().code.as_str(), "change_remove_denied");
    policy.grants[0].remove.push("m".into());
    assert!(
        prepare(
            &s,
            &removal("fn_unused"),
            Some(&policy),
            &LoadOptions::default()
        )
        .unwrap()
        .report
        .accepted
    );
    policy.grants[0].forbid.push(Selection {
        symbol: "fn_unused".into(),
        facets: BTreeSet::from([Facet::Docs]),
    });
    assert!(prepare(
        &s,
        &removal("fn_unused"),
        Some(&policy),
        &LoadOptions::default()
    )
    .is_err());
    let json = serde_json::to_value(&removal("fn_unused").edits[0].operation).unwrap();
    assert_eq!(json, json!({"kind": "remove_declaration"}));
}

#[test]
fn permissions_report_exactly_what_an_actor_may_change() {
    let s = snapshot(SOURCE);
    let policy = Policy {
        grants: vec![Grant {
            actor: "optimizer".into(),
            write: vec![Selection {
                symbol: "m.*".into(),
                facets: BTreeSet::from([Facet::Body, Facet::Contract, Facet::Identity]),
            }],
            forbid: vec![Selection {
                symbol: "fn_caller".into(),
                facets: BTreeSet::from([Facet::Body]),
            }],
            add: vec!["m.extra".into()],
            remove: vec![],
            read: None,
        }],
        regions: vec![],
    };
    let view = permissions(&s, &policy, "optimizer");
    assert_eq!(view["readable"], "unrestricted");
    let calc = view["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["symbol"] == "fn_calc")
        .unwrap();
    // Identity is never writable, whatever a grant says.
    assert_eq!(calc["writable"], json!(["contract", "body"]));
    assert!(!view["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["symbol"] == "fn_caller"
            && e["writable"].as_array().unwrap().contains(&json!("body"))));
    assert_eq!(view["add"], json!(["m.extra"]));
    let mut owned = policy.clone();
    owned.regions.push(Region {
        namespace: "m".into(),
        owner: "someone_else".into(),
    });
    let view = permissions(&s, &owned, "optimizer");
    assert_eq!(view["symbols"], json!([]));
    assert_eq!(view["add"], json!([]));
    assert_eq!(permissions(&s, &policy, "stranger")["symbols"], json!([]));
}

#[test]
fn read_grants_bound_what_changed_code_may_depend_on() {
    let source = format!("{SOURCE}@id(\"fn_helper\")\npure fn helper(n: Int) -> Int {{ n }}\n");
    let s = snapshot(&source);
    let mut policy = Policy {
        grants: vec![Grant {
            actor: "optimizer".into(),
            write: vec![Selection {
                symbol: "fn_calc".into(),
                facets: BTreeSet::from([Facet::Body]),
            }],
            forbid: vec![],
            add: vec![],
            remove: vec![],
            read: Some(vec!["dependencies(fn_calc)".into()]),
        }],
        regions: vec![],
    };
    // calc depends on nothing in the project: a self-contained rewrite is allowed.
    assert!(
        prepare(
            &s,
            &body(&s, "{ 1 + n }"),
            Some(&policy),
            &LoadOptions::default()
        )
        .unwrap()
        .report
        .accepted
    );
    let coupled = body(&s, "{ helper(n) + 1 }");
    let denied = prepare(&s, &coupled, Some(&policy), &LoadOptions::default())
        .err()
        .unwrap();
    assert_eq!(denied.code.as_str(), "change_read_denied");
    assert_eq!(denied.data["target"], "fn_helper");
    policy.grants[0]
        .read
        .as_mut()
        .unwrap()
        .push("m.helper".into());
    assert!(
        prepare(&s, &coupled, Some(&policy), &LoadOptions::default())
            .unwrap()
            .report
            .accepted
    );
    let view = permissions(&s, &policy, "optimizer");
    assert_eq!(view["readable"], json!(["fn_calc", "fn_helper"]));
    policy.grants[0].read = Some(vec!["m.nothing".into()]);
    assert!(prepare(&s, &coupled, Some(&policy), &LoadOptions::default()).is_err());
}
