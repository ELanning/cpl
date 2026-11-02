//! The `cpl` integration tests of `spec/toolchain.md` 12.4, through the
//! built binary: every `cpl check`, `cpl test`, and `cpl run` row of the
//! 12.3 table (exit codes and the exact diagnostics multiset of each check
//! row), `cpl inspect` on several targets and include sets, `cpl prove` on
//! `paginate` and on declarations without `@prove`, `cpl diff` between
//! commits of the temporary repository (and against the working tree),
//! `cpl work check` on `pagination.work`, the reserved commands, golden
//! `--json` outputs (normalized as 12.4 says), the mutation tests of 12.4
//! item 3 (`fmt --check` on a de-formatted copy, `id assign` then `check`,
//! a contract edit between commits is `contract_changed`, overlapping
//! writes are `work_overlapping_writes`), usage errors, the one-object
//! `--json` rule, `--jsonl`, determinism, and the stable human lines of
//! every command (item 4). Tests that need git skip themselves when it is
//! absent.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

/// The commit of a committed fixture.
fn head(fx: &Fixture) -> &str {
    fx.commit.as_deref().expect("a committed fixture")
}

/// The `module` of every entry of a `modules` list.
fn module_names(modules: &Value) -> Vec<String> {
    modules
        .as_array()
        .expect("modules")
        .iter()
        .map(|m| m["module"].as_str().expect("module").to_string())
        .collect()
}

// ===========================================================================
// cpl check (12.3 rows)
// ===========================================================================

#[test]
fn check_reader_row() {
    let Some(fx) = Fixture::committed("check-reader") else {
        return;
    };
    let run = cpl(&fx.reader, &["check", "--json"]);
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["command"], "check");
    assert_eq!(env["status"], "ok");
    assert_eq!(env["revision"]["commit"], head(&fx));
    assert_eq!(env["revision"]["state"], "clean");
    assert_eq!(kinds(&env), pairs(&[("info", "verification_result")]));
    let v = with_code(&env, "verification_result")[0];
    assert_eq!(v["symbol"], "pgn_7c2d");
    assert_eq!(v["qualified_name"], "reader.pages.paginate");
    assert_eq!(v["facet"], "verification_policy");
    assert_eq!(v["status"], "unknown");
    assert_eq!(v["reason"], "verifier_unavailable");
    assert_eq!(v["span"]["file"], "src/reader/pages.cpl");
    assert_eq!(
        (v["span"]["line"].clone(), v["span"]["col"].clone()),
        (json!(45), json!(8))
    );
    let receipt = &v["receipt"];
    assert_eq!(receipt["source_revision"], head(&fx));
    assert!(
        receipt.get("dirty").is_none(),
        "a clean checkout is not dirty"
    );
    assert_eq!(receipt["check_configuration"], json!({ "release": false }));
    assert_eq!(receipt["assumptions"], json!([]));
    assert_eq!(receipt["toolchain"]["cpl"], "0.1.0");
    assert_eq!(receipt["toolchain"]["edition"], "0.1");
    assert_eq!(module_names(&receipt["modules"]), ["reader.pages"]);
    assert!(receipt.get("fixtures").is_none() && receipt.get("environment").is_none());
    assert_eq!(env["release"], false);
    assert_eq!(
        env["summary"],
        json!({ "errors": 0, "warnings": 0, "holes": 0, "definitions": 21, "modules": 3, "files": 3 })
    );
    assert_eq!(env["holes"], json!([]));
    assert_eq!(
        env["verification"],
        json!([{ "symbol": "pgn_7c2d", "qualified_name": "reader.pages.paginate",
                 "status": "unknown", "reason": "verifier_unavailable" }])
    );
    check_json_golden("check_reader.json", &run, &fx, false);
}

#[test]
fn check_holes_row() {
    let Some(fx) = Fixture::committed("check-holes") else {
        return;
    };
    let run = cpl(&fx.holes, &["check", "--json"]);
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["status"], "ok");
    assert_eq!(
        kinds(&env),
        pairs(&[
            ("info", "hole"),
            ("info", "hole"),
            ("info", "verification_result")
        ])
    );
    let holes = with_code(&env, "hole");
    let build = holes[0];
    assert_eq!(build["hole"], "build_pages");
    assert_eq!(build["qualified_name"], "holes.pages.paginate");
    assert_eq!(build["symbol"], "pgn_7c2d");
    assert_eq!(build["facet"], "body");
    assert_eq!(build["expected_type"], "List<List<T>>");
    assert_eq!(
        build["bindings"],
        json!([{ "name": "size", "type": "Positive", "kind": "parameter" },
               { "name": "items", "type": "List<T>", "kind": "parameter" }])
    );
    assert_eq!(build["capabilities"], json!([]));
    assert_eq!(build["requires"], json!([]));
    assert_eq!(build["ensures"].as_array().map(Vec::len), Some(3));
    assert_eq!(build["ensures"][0], "flatten(result) == items");
    assert_eq!(build["invariants"], json!([]));
    let summary = holes[1];
    assert_eq!(summary["hole"], "summary");
    assert_eq!(summary["expected_type"], "Text");
    assert_eq!(summary["qualified_name"], "holes.pages.summarize");
    assert_eq!(
        summary["bindings"],
        json!([{ "name": "page", "type": "Page", "kind": "parameter" }])
    );
    assert_eq!(summary["ensures"], json!([]));
    for hole in &holes {
        let deps = &hole["dependencies"];
        assert_eq!(deps["source_revision"], head(&fx));
        assert!(deps.get("dirty").is_none());
        assert_eq!(deps["toolchain"]["cpl"], "0.1.0");
        assert_eq!(module_names(&deps["modules"]), ["holes.pages"]);
        for symbol in deps["symbols"].as_array().expect("symbols") {
            for key in ["id", "path", "provisional", "facets", "definition"] {
                assert!(symbol.get(key).is_some(), "{key} in {symbol}");
            }
        }
    }
    let symbol_ids = |hole: &Value| -> Vec<String> {
        hole["dependencies"]["symbols"]
            .as_array()
            .expect("symbols")
            .iter()
            .map(|s| s["id"].as_str().expect("id").to_string())
            .collect()
    };
    let build_deps = symbol_ids(build);
    assert!(
        build_deps.contains(&"holes.pages.Positive".to_string()),
        "{build_deps:?}"
    );
    assert!(
        build_deps.contains(&"std.prelude.flatten".to_string()),
        "{build_deps:?}"
    );
    assert!(symbol_ids(summary).contains(&"holes.pages.Page".to_string()));
    // `holes` holds the same payloads, without the envelope fields.
    let entries = env["holes"].as_array().expect("holes");
    assert_eq!(entries.len(), 2);
    for (hole, entry) in holes.iter().zip(entries) {
        for key in [
            "hole",
            "expected_type",
            "bindings",
            "capabilities",
            "requires",
            "ensures",
            "invariants",
            "dependencies",
        ] {
            assert_eq!(hole[key], entry[key], "{key}");
        }
        assert!(entry.get("code").is_none() && entry.get("symbol").is_none());
    }
    assert_eq!(
        env["summary"],
        json!({ "errors": 0, "warnings": 0, "holes": 2, "definitions": 5, "modules": 1, "files": 1 })
    );
    check_json_golden("check_holes.json", &run, &fx, false);
}

#[test]
fn check_release_reader_row() {
    let Some(fx) = Fixture::committed("check-release-reader") else {
        return;
    };
    let run = cpl(&fx.reader, &["check", "--release", "--json"]);
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["status"], "unsuccessful");
    assert_eq!(env["release"], true);
    assert_eq!(
        kinds(&env),
        pairs(&[
            ("error", "verification_unsatisfied"),
            ("warning", "project_lock_missing"),
            ("info", "verification_result"),
        ])
    );
    let unsatisfied = with_code(&env, "verification_unsatisfied")[0];
    assert_eq!(unsatisfied["qualified_name"], "reader.pages.paginate");
    let v = with_code(&env, "verification_result")[0];
    assert_eq!(
        v["receipt"]["check_configuration"],
        json!({ "release": true })
    );
    assert_eq!(env["summary"]["errors"], 1);
    assert_eq!(env["summary"]["warnings"], 1);
    check_json_golden("check_release_reader.json", &run, &fx, false);
}

#[test]
fn check_release_holes_row() {
    let Some(fx) = Fixture::committed("check-release-holes") else {
        return;
    };
    let run = cpl(&fx.holes, &["check", "--release", "--json"]);
    run.expect_code(1);
    let env = run.json();
    assert_eq!(
        kinds(&env),
        pairs(&[
            ("error", "hole_in_release"),
            ("error", "hole_in_release"),
            ("error", "verification_unsatisfied"),
            ("warning", "project_lock_missing"),
            ("info", "verification_result"),
        ])
    );
    let names: Vec<&Value> = with_code(&env, "hole_in_release")
        .iter()
        .map(|d| &d["hole"])
        .collect();
    assert_eq!(names, [&json!("build_pages"), &json!("summary")]);
    // CheckResult::holes still lists both holes.
    assert_eq!(env["holes"].as_array().map(Vec::len), Some(2));
    assert_eq!(env["summary"]["holes"], 2);
    check_json_golden("check_release_holes.json", &run, &fx, false);
}

#[test]
fn check_human_output_has_the_stable_lines() {
    let fx = Fixture::new("check-human");
    let run = cpl(&fx.reader, &["check"]);
    run.expect_code(0);
    assert_eq!(
        run.stdout,
        "check: 0 errors, 0 warnings, 0 holes, 21 definitions in 3 modules\n"
    );
    let lines: Vec<&str> = run.stderr.lines().collect();
    assert_eq!(
        lines[0],
        "info[verification_result]: paginate: unknown (verifier unavailable)"
    );
    assert_eq!(lines[1], "  --> src/reader/pages.cpl:45:8");

    let run = cpl(&fx.holes, &["check"]);
    run.expect_code(0);
    assert_eq!(
        run.stdout,
        "check: 0 errors, 0 warnings, 2 holes, 5 definitions in 1 module\n"
    );
    assert!(
        run.stderr.contains(
            "info[hole]: unfinished expression ?build_pages\n  --> src/holes/pages.cpl:20:5\n"
        ),
        "{}",
        run.stderr
    );
    assert!(run
        .stderr
        .contains("info[hole]: unfinished expression ?summary\n  --> src/holes/pages.cpl:24:5\n"));

    let run = cpl(&fx.holes, &["check", "--release"]);
    run.expect_code(1);
    assert_eq!(
        run.stdout,
        "check: 3 errors, 1 warning, 2 holes, 5 definitions in 1 module\n"
    );
    let firsts: Vec<&str> = run
        .stderr
        .lines()
        .filter(|l| !l.starts_with(' ') && !l.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    assert!(firsts
        .iter()
        .any(|l| l.starts_with("error[hole_in_release]: ")));
    assert!(firsts
        .iter()
        .any(|l| l.starts_with("error[verification_unsatisfied]: ")));
    assert!(firsts.contains(&"warning[project_lock_missing]: cpl.lock is missing: a release check needs the locked build inputs; run `cpl check --lock`"));
}

#[test]
fn check_jsonl_streams_the_diagnostics_then_the_envelope() {
    let fx = Fixture::new("check-jsonl");
    let run = cpl(&fx.holes, &["check", "--jsonl"]);
    run.expect_code(0);
    let lines = run.json_lines();
    let env = cpl(&fx.holes, &["check", "--json"]).json();
    let diagnostics = env["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(lines.len(), diagnostics.len() + 1);
    assert_eq!(&lines[..diagnostics.len()], &diagnostics[..]);
    let tail = lines.last().expect("the envelope line");
    assert!(tail.get("diagnostics").is_none());
    let mut expected = env.clone();
    expected
        .as_object_mut()
        .expect("object")
        .remove("diagnostics");
    assert_eq!(tail, &expected);
    // The tail keeps the envelope's key order.
    let raw = run.stdout.lines().last().expect("tail");
    let at = |key: &str| raw.find(&format!("\"{key}\":")).expect(key);
    assert!(at("command") < at("toolchain"));
    assert!(at("toolchain") < at("revision"));
    assert!(at("revision") < at("status"));
    assert!(at("status") < at("release"));
    assert!(at("release") < at("summary"));
    assert!(at("summary") < at("holes"));
    assert!(at("holes") < at("verification"));
}

#[test]
fn deny_warnings_turns_a_warning_into_failure() {
    let fx = Fixture::new("check-deny-warnings");
    let path = "reader/src/reader/images.cpl";
    let text = fx.read(path).replace(
        "use std.foreign.{ForeignError}\n",
        "use std.foreign.{ForeignError}\nuse std.text.{trim}\n",
    );
    fx.write(path, &text);
    let run = cpl(&fx.reader, &["check", "--json"]);
    run.expect_code(0);
    let env = run.json();
    assert!(kinds(&env).contains(&("warning".to_string(), "unused_import".to_string())));
    assert_eq!(env["status"], "ok");
    let run = cpl(&fx.reader, &["check", "--deny-warnings", "--json"]);
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["status"], "unsuccessful");
    // Severities are unchanged.
    assert_eq!(with_code(&env, "unused_import")[0]["severity"], "warning");
}

#[test]
fn lint_id_missing_and_unknown_lints() {
    let fx = Fixture::new("check-lint");
    let run = cpl(&fx.reader, &["check", "--lint", "id_missing", "--json"]);
    run.expect_code(0);
    let env = run.json();
    let missing = with_code(&env, "id_missing");
    // Every public declaration without @id: 9 in reader.pages, 3 in reader.images.
    assert_eq!(missing.len(), 12, "{missing:?}");
    assert!(missing.iter().all(|d| d["severity"] == "warning"));
    let run = cpl(&fx.reader, &["check", "--lint", "no_such_lint", "--json"]);
    run.expect_code(2);
    assert_eq!(
        kinds(&run.json()),
        pairs(&[("error", "usage_bad_argument")])
    );
}

#[test]
fn check_lock_writes_a_lock_that_release_reads() {
    let fx = Fixture::new("check-lock");
    cpl(&fx.reader, &["check", "--lock"]).expect_code(0);
    let lock = fx.read("reader/cpl.lock");
    assert!(
        lock.starts_with("# Generated by cpl. Do not edit.\n"),
        "{lock}"
    );
    assert!(lock.contains("\"src/reader/pages.cpl\" = \""), "{lock}");
    assert!(lock.contains("\"cpl.toml\" = \""), "{lock}");
    let run = cpl(&fx.reader, &["check", "--release", "--json"]);
    run.expect_code(1);
    assert_eq!(
        kinds(&run.json()),
        pairs(&[
            ("error", "verification_unsatisfied"),
            ("info", "verification_result")
        ])
    );
    // A changed source makes the lock stale.
    let path = "reader/src/tests/pagination.cpl";
    let text = fx.read(path).replace("[1, 2, 3], 2)", "[1, 2, 3],  2)");
    fx.write(path, &text);
    let run = cpl(&fx.reader, &["check", "--release", "--json"]);
    let env = run.json();
    let stale = with_code(&env, "project_lock_stale");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0]["paths"], json!(["src/tests/pagination.cpl"]));
    // A failed check writes no lock.
    let before = fx.read("reader/cpl.lock");
    cpl(&fx.reader, &["check", "--release", "--lock"]).expect_code(1);
    assert_eq!(fx.read("reader/cpl.lock"), before);
}

#[test]
fn unversioned_and_dirty_revisions() {
    let fx = Fixture::new("check-unversioned");
    let env = cpl(&fx.holes, &["check", "--json"]).json();
    assert_eq!(env["revision"]["commit"], Value::Null);
    assert_eq!(env["revision"]["state"], "worktree");
    let hole = with_code(&env, "hole")[0];
    assert_eq!(hole["dependencies"]["source_revision"], "unversioned");
    assert!(hole["dependencies"].get("dirty").is_none());

    let Some(fx) = Fixture::committed("check-dirty") else {
        return;
    };
    let path = "holes/src/holes/pages.cpl";
    let text = fx.read(path).replace("?summary", "?summary_text");
    fx.write(path, &text);
    let env = cpl(&fx.holes, &["check", "--json"]).json();
    assert_eq!(env["revision"]["commit"], head(&fx));
    assert_eq!(env["revision"]["state"], "worktree");
    for hole in with_code(&env, "hole") {
        assert_eq!(hole["dependencies"]["source_revision"], head(&fx));
        assert_eq!(hole["dependencies"]["dirty"], true);
    }
    let v = with_code(&env, "verification_result")[0];
    assert_eq!(v["receipt"]["dirty"], true);
}

// ===========================================================================
// cpl test (12.3 row, runner behavior)
// ===========================================================================

const TESTS: [&str; 3] = [
    "tests.pagination.\"pagination handles empty input\"",
    "tests.pagination.\"pagination preserves a short tail\"",
    "tests.pagination.\"pagination preserves input\"",
];

#[test]
fn test_reader_with_seed_1() {
    let Some(fx) = Fixture::committed("test-reader") else {
        return;
    };
    let run = cpl(&fx.reader, &["test", "--seed", "1", "--json"]);
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["command"], "test");
    assert_eq!(env["status"], "ok");
    for (key, value) in [
        ("seed", 1),
        ("cases", 100),
        ("selected", 3),
        ("tested", 3),
        ("failed", 0),
    ] {
        assert_eq!(env[key], value, "{key}");
    }
    let results = env["results"].as_array().expect("results");
    let names: Vec<&str> = results
        .iter()
        .map(|r| r["qualified_name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, TESTS);
    for record in results {
        assert_eq!(record["status"], "tested");
        let receipt = &record["receipt"];
        assert_eq!(receipt["source_revision"], head(&fx));
        let modules = module_names(&receipt["modules"]);
        assert!(modules.contains(&"reader.pages".to_string()), "{modules:?}");
        assert!(
            modules.contains(&"tests.pagination".to_string()),
            "{modules:?}"
        );
        assert_eq!(
            receipt["check_configuration"],
            json!({ "cases": 100, "max_shrinks": 1000, "max_call_depth": 10000, "seed": 1 })
        );
        assert_eq!(receipt["fixtures"], json!([]));
        assert_eq!(receipt["assumptions"], json!([]));
        assert!(receipt["environment"]["os"].is_string());
        assert!(receipt["environment"]["arch"].is_string());
    }
    assert_eq!(results[0]["kind"], "test");
    assert!(results[0].get("seed").is_none() && results[0].get("cases").is_none());
    let property = &results[2];
    assert_eq!(property["kind"], "property");
    assert_eq!(property["cases"], 100);
    assert_eq!(property["seed"], 1);
    let seed = property["property_seed"].as_str().expect("property_seed");
    assert_eq!(seed.len(), 64);
    assert!(seed
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    // One test_result diagnostic per record, in completion order.
    let diagnostics = env["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(kinds(&env), pairs(&[("info", "test_result"); 3]));
    for (d, record) in diagnostics.iter().zip(results) {
        assert_eq!(d["symbol"], record["symbol"]);
        assert_eq!(d["qualified_name"], record["qualified_name"]);
        assert_eq!(d["status"], "tested");
        assert_eq!(d["receipt"], record["receipt"]);
        assert!(d.get("span").is_none());
    }
    let receipt = &env["receipt"];
    assert_eq!(receipt["receipt"], "test");
    assert_eq!(receipt["status"], "tested");
    assert_eq!(
        receipt["summary"],
        json!({ "selected": 3, "tested": 3, "failed": 0 })
    );
    assert_eq!(receipt["configuration"]["seed"], 1);
    assert_eq!(
        receipt["configuration"]["selector"],
        json!({ "namespace": null, "name": null, "id": null })
    );
    assert_eq!(receipt["tests"].as_array().map(Vec::len), Some(3));
    assert_eq!(
        module_names(&receipt["core"]["modules"]),
        ["reader.pages", "tests.pagination"]
    );
    let tested: Vec<&str> = receipt["tested"]
        .as_array()
        .expect("tested")
        .iter()
        .map(|v| v["path"].as_str().expect("path"))
        .collect();
    assert!(tested.contains(&"reader.pages.paginate"), "{tested:?}");
    assert!(tested.contains(&"std.prelude.flatten"), "{tested:?}");
    assert_eq!(receipt["inputs"]["lock"], Value::Null);
    assert!(receipt["inputs"]["sources"]["cpl.toml"].is_string());
    check_json_golden("test_reader_seed1.json", &run, &fx, false);
}

#[test]
fn test_human_output_lines() {
    let fx = Fixture::new("test-human");
    let run = cpl(&fx.reader, &["test", "--seed", "1"]);
    run.expect_code(0);
    let expected: String = TESTS
        .iter()
        .map(|t| format!("ok       {t}\n"))
        .chain(["test: 3 tested, 0 failed; seed 1\n".to_string()])
        .collect();
    assert_eq!(run.stdout, expected);
    assert_eq!(run.stderr, "", "the test_result lines are the stdout lines");
}

#[test]
fn test_without_a_seed_reports_the_os_seed() {
    let fx = Fixture::new("test-os-seed");
    let env = cpl(&fx.reader, &["test", "--json"]).json();
    let seed = env["seed"].as_u64().expect("a decimal u64 seed");
    assert_eq!(env["results"][2]["seed"], seed);
    assert_eq!(env["receipt"]["configuration"]["seed"], seed);
    let run = cpl(&fx.reader, &["test"]);
    let last = run.stdout.lines().last().expect("summary");
    assert!(
        last.starts_with("test: 3 tested, 0 failed; seed "),
        "{last}"
    );
    assert!(last["test: 3 tested, 0 failed; seed ".len()..]
        .parse::<u64>()
        .is_ok());
    // The normalized outputs of two unseeded runs agree.
    let fx2 = Fixture::new("test-os-seed-2");
    let a = cpl(&fx.reader, &["test", "--json"]);
    let b = cpl(&fx2.reader, &["test", "--json"]);
    let na = normalize(&a.stdout, &Volatile::of(&a.json(), Some(&fx.root), true));
    let nb = normalize(&b.stdout, &Volatile::of(&b.json(), Some(&fx2.root), true));
    assert_eq!(na, nb);
}

/// The reader fixture with a `paginate` that drops the first element of a
/// short tail, which violates its first `ensures`.
fn broken_paginate(name: &str) -> Fixture {
    let fx = Fixture::new(name);
    let path = "reader/src/reader/pages.cpl";
    let text = fx.read(path).replace(
        "        pages = pages.push(rest)\n",
        "        pages = pages.push(rest.drop(1))\n",
    );
    fx.write(path, &text);
    fx
}

#[test]
fn a_failing_property_is_shrunk_and_reported() {
    let fx = broken_paginate("test-failing");
    let run = cpl(&fx.reader, &["test", "--seed", "1", "--json"]);
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["status"], "unsuccessful");
    assert_eq!(
        (
            env["selected"].clone(),
            env["tested"].clone(),
            env["failed"].clone()
        ),
        (json!(3), json!(1), json!(2))
    );
    let results = env["results"].as_array().expect("results");
    assert_eq!(results[0]["status"], "tested");
    for record in &results[1..] {
        assert_eq!(record["status"], "failed");
        assert_eq!(record["fault"]["code"], "contract_violation");
        assert_eq!(record["fault"]["clause_kind"], "ensures");
        let counterexample = &record["counterexamples"][0];
        assert_eq!(counterexample["code"], "contract_counterexample");
        assert_eq!(counterexample["symbol"], "pgn_7c2d");
        assert_eq!(counterexample["facet"], "body");
        assert_eq!(counterexample["clause"], "flatten(result) == items");
        assert_eq!(counterexample["status"], "failed");
        assert_eq!(counterexample["found_by"], record["symbol"]);
    }
    let property = &results[2];
    assert!(property["failing_case"]["index"].is_u64());
    assert!(property["shrunk"]["shrink_steps"].is_u64());
    assert_eq!(
        property["cases"],
        property["failing_case"]["index"].as_u64().unwrap() + 1
    );
    // The counterexamples are diagnostics of their own, beside the records.
    assert_eq!(
        kinds(&env),
        pairs(&[
            ("error", "contract_counterexample"),
            ("error", "contract_counterexample"),
            ("info", "test_result"),
            ("error", "test_result"),
            ("error", "test_result"),
        ])
    );
    assert_eq!(env["receipt"]["status"], "failed");
    // T46: the counterexamples, which span the violated clause, come
    // first; the span-less test_result diagnostics follow in completion
    // order.
    let codes: Vec<&str> = env["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "contract_counterexample",
            "contract_counterexample",
            "test_result",
            "test_result",
            "test_result"
        ]
    );
    let names: Vec<&Value> = env["diagnostics"].as_array().unwrap()[2..]
        .iter()
        .map(|d| &d["qualified_name"])
        .collect();
    assert_eq!(names, TESTS.map(|t| json!(t)).iter().collect::<Vec<_>>());

    let run = cpl(&fx.reader, &["test", "--seed", "1"]);
    run.expect_code(1);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines[0], format!("ok       {}", TESTS[0]));
    assert_eq!(lines[1], format!("FAILED   {}", TESTS[1]));
    assert!(
        lines[2].starts_with("         error[contract_violation]: "),
        "{}",
        lines[2]
    );
    assert!(
        lines[3].starts_with("           --> src/reader/pages.cpl:46:"),
        "{}",
        lines[3]
    );
    let index = property["failing_case"]["index"].as_u64().unwrap();
    let steps = property["shrunk"]["shrink_steps"].as_u64().unwrap();
    assert_eq!(
        lines[4],
        format!(
            "FAILED   {}  (case {index}; shrunk in {steps} step{})",
            TESTS[2],
            if steps == 1 { "" } else { "s" }
        )
    );
    assert!(lines[5].starts_with("         error[contract_violation]: "));
    assert!(
        lines[7].starts_with("         shrunk: items = ["),
        "{}",
        lines[7]
    );
    assert!(lines[7].contains(", size = "), "{}", lines[7]);
    assert_eq!(*lines.last().unwrap(), "test: 1 tested, 2 failed; seed 1");
    // Stderr has the counterexamples, not the test_result lines.
    assert!(
        run.stderr.starts_with("error[contract_counterexample]: "),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("[test_result]"));
}

#[test]
fn a_failing_run_golden() {
    let Some(fx) = Fixture::committed("test-failing-golden") else {
        return;
    };
    let path = "reader/src/reader/pages.cpl";
    let text = fx.read(path).replace(
        "        pages = pages.push(rest)\n",
        "        pages = pages.push(rest.drop(1))\n",
    );
    fx.write(path, &text);
    fx.commit_all("drop the first element of a short tail");
    let run = cpl(&fx.reader, &["test", "--seed", "1", "--json"]);
    run.expect_code(1);
    check_json_golden("test_reader_failing_seed1.json", &run, &fx, false);
}

#[test]
fn fail_fast_stops_after_the_first_failure() {
    let fx = broken_paginate("test-fail-fast");
    let run = cpl(
        &fx.reader,
        &["test", "--seed", "1", "--fail-fast", "--json"],
    );
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["selected"], 3);
    assert_eq!(env["tested"], 1);
    assert_eq!(env["failed"], 1);
    assert_eq!(env["results"].as_array().map(Vec::len), Some(2));
    assert_eq!(env["receipt"]["configuration"]["fail_fast"], true);
}

#[test]
fn test_on_holes_is_an_empty_selection() {
    let fx = Fixture::new("test-holes");
    let run = cpl(&fx.holes, &["test", "--seed", "3", "--json"]);
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["status"], "unsuccessful");
    assert_eq!(env["selected"], 0);
    assert_eq!(env["results"], json!([]));
    assert_eq!(kinds(&env), pairs(&[("error", "empty_test_selection")]));
    let d = with_code(&env, "empty_test_selection")[0];
    assert_eq!(d["category"], "test");
    assert_eq!(
        d["selector"],
        json!({ "namespace": null, "name": null, "id": null })
    );
    assert_eq!(env["receipt"]["status"], "failed");
    let run = cpl(&fx.holes, &["test", "--seed", "3"]);
    run.expect_code(1);
    assert_eq!(run.stdout, "test: 0 tested, 0 failed; seed 3\n");
    assert!(run.stderr.starts_with("error[empty_test_selection]: "));
}

#[test]
fn test_selectors_and_filters() {
    let fx = Fixture::new("test-selectors");
    let selected = |args: &[&str]| -> (i32, Value) {
        let mut all = vec!["test", "--seed", "5", "--json"];
        all.extend_from_slice(args);
        let run = cpl(&fx.reader, &all);
        (run.code, run.json())
    };
    for sel in ["tests.pagination", "tests.pagination.*", "tests", "tests.*"] {
        let (code, env) = selected(&[sel]);
        assert_eq!((code, env["selected"].clone()), (0, json!(3)), "{sel}");
        assert_eq!(
            env["receipt"]["configuration"]["selector"]["namespace"],
            json!(sel.trim_end_matches(".*").split('.').collect::<Vec<_>>())
        );
    }
    let (code, env) = selected(&["reader"]);
    assert_eq!((code, env["selected"].clone()), (1, json!(0)));
    let (code, env) = selected(&["--name", "pagination handles empty input"]);
    assert_eq!((code, env["selected"].clone()), (0, json!(1)));
    assert_eq!(env["results"][0]["qualified_name"], TESTS[0]);
    let (code, env) = selected(&["--id", TESTS[2]]);
    assert_eq!((code, env["selected"].clone()), (0, json!(1)));
    assert_eq!(env["results"][0]["kind"], "property");
    for bad in [
        vec!["tests.pagination.\"x\""],
        vec!["tests..pagination"],
        vec!["Tests"],
        vec!["--id", "Not An Id"],
    ] {
        let (code, env) = selected(&bad);
        assert_eq!(code, 2, "{bad:?}");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "usage_bad_argument")]),
            "{bad:?}"
        );
    }
}

#[test]
fn test_receipt_file_is_the_envelope_receipt() {
    let fx = Fixture::new("test-receipt");
    let path = fx.root.join("evidence.json");
    let path_text = path.to_string_lossy().to_string();
    let run = cpl(
        &fx.reader,
        &["test", "--seed", "1", "--json", "--receipt", &path_text],
    );
    run.expect_code(0);
    let env = run.json();
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the receipt file"))
            .expect("JSON");
    assert_eq!(written, env["receipt"]);
}

#[test]
fn test_jsonl_streams_records_then_the_envelope() {
    let fx = Fixture::new("test-jsonl");
    let run = cpl(&fx.reader, &["test", "--seed", "1", "--jsonl"]);
    run.expect_code(0);
    let lines = run.json_lines();
    assert_eq!(lines.len(), 4);
    for (line, name) in lines.iter().zip(TESTS) {
        assert_eq!(line["code"], "test_result");
        assert_eq!(line["qualified_name"], name);
    }
    let tail = &lines[3];
    assert_eq!(tail["command"], "test");
    assert!(tail.get("diagnostics").is_none());
    assert_eq!(tail["selected"], 3);
    assert!(tail["receipt"].is_object());
    assert_eq!(tail["results"].as_array().map(Vec::len), Some(3));
}

#[test]
fn test_does_not_run_when_the_check_fails() {
    let fx = Fixture::new("test-check-error");
    let path = "reader/src/tests/pagination.cpl";
    let text = fx
        .read(path)
        .replace("let items: List<Int> = []", "let items: Text = []");
    fx.write(path, &text);
    let run = cpl(&fx.reader, &["test", "--seed", "1", "--json"]);
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["status"], "unsuccessful");
    assert!(env.get("results").is_none(), "nothing runs");
    assert!(kinds(&env).iter().any(|(s, _)| s == "error"));
    assert!(with_code(&env, "test_result").is_empty());
}

// ===========================================================================
// cpl run (12.3 rows)
// ===========================================================================

/// `cpl run --json` in `dir`: the exit code and the envelope.
fn run_json(dir: &Path, args: &[&str]) -> (i32, Value) {
    let mut all = vec!["run"];
    all.extend_from_slice(args);
    all.push("--json");
    let run = cpl(dir, &all);
    (run.code, run.json())
}

#[track_caller]
fn assert_returns(dir: &Path, args: &[&str], text: &str) -> Value {
    let (code, env) = run_json(dir, args);
    assert_eq!(code, 0, "{args:?}: {env}");
    assert_eq!(env["status"], "ok");
    assert_eq!(env["result_text"], text, "{args:?}");
    assert_eq!(env["fault"], Value::Null);
    assert!(env.get("result").is_some_and(|r| !r.is_null()));
    env
}

#[test]
fn run_rows_of_the_interpreter_table() {
    let fx = Fixture::new("run-rows");
    let env = assert_returns(
        &fx.reader,
        &["reader.pages.paginate_ints", "[1, 2, 3]", "2"],
        "Ok([[1, 2], [3]])",
    );
    assert_eq!(
        env["result"],
        json!({ "$type": "Result", "$variant": "Ok", "$payload": [[[1, 2], [3]]] })
    );
    assert_eq!(env["function"]["path"], "reader.pages.paginate_ints");
    assert_eq!(env["function"]["provisional"], true);
    assert_returns(
        &fx.reader,
        &["reader.pages.paginate_ints", "[]", "2"],
        "Ok([])",
    );
    let env = assert_returns(
        &fx.reader,
        &["reader.pages.paginate_ints", "[1]", "0"],
        "Err(ConstraintError { type_name: \"reader.pages.Positive\", message: \"0 does not satisfy self > 0\" })",
    );
    assert_eq!(env["result"]["$variant"], "Err");
    assert_returns(&fx.reader, &["reader.pages.divide_exactly", "6", "3"], "2");
    assert_returns(
        &fx.reader,
        &["reader.pages.positive_size", "\"  12 \""],
        "Ok(12)",
    );
    let (code, env) = run_json(&fx.reader, &["reader.pages.positive_size", "\"-3\""]);
    assert_eq!(code, 0);
    assert!(env["result_text"]
        .as_str()
        .is_some_and(|t| t.starts_with("Err(")));

    for (args, clause) in [
        (["reader.pages.divide_exactly", "7", "0"], "b != 0"),
        (["reader.pages.divide_exactly", "7", "2"], "a % b == 0"),
    ] {
        let (code, env) = run_json(&fx.reader, &args);
        assert_eq!(code, 1, "{args:?}");
        assert_eq!(env["status"], "unsuccessful");
        assert_eq!(env["result"], Value::Null);
        assert_eq!(env["result_text"], Value::Null);
        let fault = &env["fault"];
        assert_eq!(fault["code"], "contract_violation");
        assert_eq!(fault["clause_kind"], "requires");
        assert_eq!(fault["attributed_to"], "caller");
        assert!(fault.get("caller").is_none(), "no calling declaration");
        assert_eq!(fault["clause"], clause);
        assert_eq!(fault["status"], "failed");
        assert_eq!(&env["diagnostics"][0], fault);
    }
    let (_, env) = run_json(&fx.reader, &["reader.pages.divide_exactly", "7", "0"]);
    assert_eq!(env["fault"]["input"], json!({ "a": 7, "b": 0 }));

    let (code, env) = run_json(
        &fx.holes,
        &[
            "holes.pages.summarize",
            "Page { id: PageId(\"p\"), text: \"t\" }",
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(env["fault"]["code"], "unresolved_hole");
    assert_eq!(env["fault"]["hole"], "summary");

    for target in ["holes.pages.paginate", "pgn_7c2d"] {
        let (code, env) = run_json(&fx.holes, &[target, "[1]", "1"]);
        assert_eq!(code, 2, "{target}");
        assert_eq!(env["status"], "error");
        assert_eq!(kinds(&env), pairs(&[("error", "cannot_run_generic")]));
        let d = &env["diagnostics"][0];
        assert_eq!(d["category"], "resolve");
        assert_eq!(d["symbol"], "pgn_7c2d");
        assert_eq!(d["qualified_name"], "holes.pages.paginate");
        assert_eq!(d["span"]["line"], 15);
        assert_eq!(d["notes"].as_array().map(Vec::len), Some(1));
    }
    // In reader the note names the monomorphic wrapper.
    let (code, env) = run_json(&fx.reader, &["reader.pages.paginate", "[1]", "1"]);
    assert_eq!(code, 2);
    let note = env["diagnostics"][0]["notes"][0]["message"]
        .as_str()
        .expect("note");
    assert!(note.contains("reader.pages.paginate_ints"), "{note}");
}

#[test]
fn run_arguments_as_value_json() {
    let fx = Fixture::new("run-json-args");
    let env = assert_returns(
        &fx.reader,
        &[
            "reader.pages.paginate_ints",
            "--arg",
            "[1, 2, 3]",
            "--arg",
            "2",
        ],
        "Ok([[1, 2], [3]])",
    );
    assert_eq!(env["arguments"], json!([[1, 2, 3], 2]));
    // Mixed forms bind in command-line order.
    assert_returns(
        &fx.reader,
        &["reader.pages.paginate_ints", "[1, 2, 3]", "--arg", "2"],
        "Ok([[1, 2], [3]])",
    );
    assert_returns(
        &fx.reader,
        &["reader.pages.divide_exactly", "--arg", "-6", "3"],
        "-2",
    );
    assert_returns(
        &fx.reader,
        &["reader.pages.divide_exactly", "-6", "3"],
        "-2",
    );
    let page = r#"{"$type": "holes.pages.Page", "id": {"$type": "holes.pages.PageId", "$payload": ["p"]}, "text": "t"}"#;
    let (code, env) = run_json(&fx.holes, &["holes.pages.summarize", "--arg", page]);
    assert_eq!(code, 1);
    assert_eq!(env["fault"]["hole"], "summary");
    assert_eq!(env["arguments"][0]["text"], "t");
}

#[test]
fn run_human_output() {
    let fx = Fixture::new("run-human");
    let run = cpl(
        &fx.reader,
        &["run", "reader.pages.paginate_ints", "[1, 2, 3]", "2"],
    );
    run.expect_code(0);
    assert_eq!(run.stdout, "Ok([[1, 2], [3]])\n");
    assert_eq!(run.stderr, "");
    let run = cpl(
        &fx.reader,
        &["run", "reader.pages.divide_exactly", "7", "0"],
    );
    run.expect_code(1);
    assert_eq!(run.stdout, "");
    let lines: Vec<&str> = run.stderr.lines().collect();
    assert_eq!(
        lines[0],
        "error[contract_violation]: requires clause failed: b != 0"
    );
    assert_eq!(lines[1], "  --> src/reader/pages.cpl:106:5");
}

#[test]
fn run_refusals_are_usage_errors() {
    let fx = Fixture::new("run-refusals");
    let refused = |dir: &Path, args: &[&str], code: &str| -> Value {
        let (exit, env) = run_json(dir, args);
        assert_eq!(exit, 2, "{args:?}: {env}");
        assert_eq!(env["status"], "error");
        let d = env["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .find(|d| d["code"] == code)
            .unwrap_or_else(|| panic!("{code} for {args:?}: {env}"))
            .clone();
        assert_eq!(d["severity"], "error");
        d
    };
    let d = refused(&fx.reader, &["reader.pages.nope"], "index_unknown_symbol");
    assert_eq!(d["category"], "index");
    refused(&fx.reader, &["not a path"], "index_unknown_symbol");
    let d = refused(&fx.reader, &["reader.pages.Page"], "usage_not_a_function");
    assert_eq!(d["category"], "usage");
    let d = refused(
        &fx.reader,
        &["reader.pages.load_page", "PageId(\"p\")"],
        "cannot_run_effectful",
    );
    assert_eq!(d["symbol"], "reader.pages.load_page");
    let d = refused(
        &fx.reader,
        &["reader.pages.divide_exactly", "1"],
        "usage_bad_argument",
    );
    assert_eq!(
        (d["parameters"].clone(), d["arguments"].clone()),
        (json!(2), json!(1))
    );
    // Not a constant expression: a binary operator (7.2).
    let d = refused(
        &fx.reader,
        &["reader.pages.divide_exactly", "1 + 2", "3"],
        "usage_bad_argument",
    );
    assert_eq!(d["argument"], 0);
    assert_eq!(d["span"]["file"], "<virtual:argument 0>");
    // Does not parse.
    let d = refused(
        &fx.reader,
        &["reader.pages.divide_exactly", "3", "(4"],
        "usage_bad_argument",
    );
    assert_eq!(d["argument"], 1);
    assert!(d["parse_code"].is_string());
    // Ill-typed, in both forms.
    let d = refused(
        &fx.reader,
        &["reader.pages.divide_exactly", "\"x\"", "3"],
        "usage_bad_argument",
    );
    assert_eq!(
        (d["argument"].clone(), d["pointer"].clone()),
        (json!(0), json!(""))
    );
    let d = refused(
        &fx.reader,
        &[
            "reader.pages.paginate_ints",
            "--arg",
            "[1, \"x\"]",
            "--arg",
            "2",
        ],
        "usage_bad_argument",
    );
    assert_eq!(
        (d["argument"].clone(), d["pointer"].clone()),
        (json!(0), json!("/1"))
    );
    // The strict JSON reader: duplicate keys, nesting beyond 128 levels, syntax.
    let d = refused(
        &fx.holes,
        &[
            "holes.pages.summarize",
            "--arg",
            r#"{"$type": "holes.pages.Page", "text": "a", "text": "b", "id": {"$type": "holes.pages.PageId", "$payload": ["p"]}}"#,
        ],
        "usage_bad_argument",
    );
    assert_eq!(
        (d["key"].clone(), d["pointer"].clone()),
        (json!("text"), json!(""))
    );
    let deep = format!("{}{}", "[".repeat(128), "]".repeat(128));
    let d = refused(
        &fx.reader,
        &["reader.pages.paginate_ints", "--arg", &deep, "--arg", "2"],
        "usage_bad_argument",
    );
    assert!(
        d["message"].as_str().is_some_and(|m| m.contains("128")),
        "{d}"
    );
    refused(
        &fx.reader,
        &["reader.pages.paginate_ints", "--arg", "[1,", "--arg", "2"],
        "usage_bad_argument",
    );
    // Within the limit: an ordinary decoding error.
    let ok_depth = format!("{}{}", "[".repeat(127), "]".repeat(127));
    let d = refused(
        &fx.reader,
        &[
            "reader.pages.paginate_ints",
            "--arg",
            &ok_depth,
            "--arg",
            "2",
        ],
        "usage_bad_argument",
    );
    assert!(d["pointer"].is_string());
}

/// T61 [D-257]: `cpl run` refuses a generator, a function whose declared
/// return type is a `Gen<T>`, with `usage_not_a_function` (exit 2), before
/// the generic check, the argument count, and any evaluation:
/// `std.gen.range 5 1` would otherwise fault on its `requires lo <= hi`
/// (exit 1), and `std.gen.int` would reach its builtin's `internal_error`.
#[test]
fn run_refuses_a_generator() {
    let fx = Fixture::new("run-generator");
    for args in [
        &["std.gen.int"][..],
        &["std.gen.range", "5", "1"],
        &["std.gen.range", "1", "5"],
        &["std.gen.list"],
        &["std.gen.option", "1"],
    ] {
        let (exit, env) = run_json(&fx.reader, args);
        assert_eq!(exit, 2, "{args:?}: {env}");
        assert_eq!(env["status"], "error");
        let codes: Vec<&str> = env["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .map(|d| d["code"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(codes, ["usage_not_a_function"], "{args:?}: {env}");
        let d = &env["diagnostics"][0];
        assert_eq!(d["category"], "usage");
        assert_eq!(d["qualified_name"], args[0]);
        assert!(
            d["message"]
                .as_str()
                .is_some_and(|m| m.contains("is a generator")),
            "{d}"
        );
        assert!(env.get("fault").is_none(), "{env}");
    }
    let run = cpl(&fx.reader, &["run", "std.gen.int"]);
    run.expect_code(2);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr
            .starts_with("error[usage_not_a_function]: std.gen.int is a generator, not a function"),
        "{}",
        run.stderr
    );
}

#[test]
fn run_does_not_run_when_the_check_fails() {
    let fx = Fixture::new("run-check-error");
    let path = "reader/src/reader/pages.cpl";
    let text = fx.read(path).replace("    a / b\n", "    a / \"b\"\n");
    fx.write(path, &text);
    let (code, env) = run_json(&fx.reader, &["reader.pages.divide_exactly", "6", "3"]);
    assert_eq!(code, 1);
    assert_eq!(env["status"], "unsuccessful");
    assert!(env.get("result").is_none(), "nothing runs");
    assert!(kinds(&env).iter().any(|(s, _)| s == "error"));
}

#[test]
fn run_goldens() {
    let Some(fx) = Fixture::committed("run-goldens") else {
        return;
    };
    let run = cpl(
        &fx.reader,
        &[
            "run",
            "reader.pages.paginate_ints",
            "--arg",
            "[1, 2, 3]",
            "--arg",
            "2",
            "--json",
        ],
    );
    run.expect_code(0);
    check_json_golden("run_paginate_ints.json", &run, &fx, false);
    let run = cpl(
        &fx.reader,
        &[
            "run",
            "reader.pages.divide_exactly",
            "--arg",
            "7",
            "--arg",
            "0",
            "--json",
        ],
    );
    run.expect_code(1);
    check_json_golden("run_divide_exactly_fault.json", &run, &fx, false);
    let run = cpl(&fx.holes, &["run", "holes.pages.paginate", "--json"]);
    run.expect_code(2);
    check_json_golden("run_cannot_run_generic.json", &run, &fx, false);
}

/// 12.4 item 2 for `cpl fmt` and `cpl id assign`: every result kind of
/// `FmtOutput` but `error` (which `fmt_leaves_a_file_that_does_not_parse`
/// covers), and the assignments of `id assign`, with and without
/// `--check`, whose generated ids are normalized as `<kind>_<generated>`.
#[test]
fn fmt_and_id_assign_goldens() {
    let Some(fx) = Fixture::committed("fmt-id-goldens") else {
        return;
    };
    let run = cpl(&fx.reader, &["fmt", "--check", "--json"]);
    run.expect_code(0);
    check_json_golden("fmt_check_reader.json", &run, &fx, false);

    let path = "reader/src/reader/pages.cpl";
    let original = fx.read(path);
    fx.write(
        path,
        &original.replace(
            "fn clean(page: Page) -> Page {",
            "fn clean( page:Page )->Page{",
        ),
    );
    let run = cpl(&fx.reader, &["fmt", "--check", "--json"]);
    run.expect_code(1);
    check_json_golden("fmt_check_deformatted.json", &run, &fx, false);
    let run = cpl(&fx.reader, &["fmt", "--json"]);
    run.expect_code(0);
    check_json_golden("fmt_deformatted.json", &run, &fx, false);
    assert_eq!(fx.read(path), original);

    let run = cpl(&fx.reader, &["id", "assign", "--check", "--json"]);
    run.expect_code(1);
    let planned = run.json();
    check_id_assign_golden("id_assign_check_reader.json", &run, &fx);
    let run = cpl(&fx.reader, &["id", "assign", "--json"]);
    run.expect_code(0);
    let assigned = run.json();
    check_id_assign_golden("id_assign_reader.json", &run, &fx);
    // The normalization hides only the random part: every id is of the
    // generated format of its declaration's kind, and no id repeats.
    let mut seen = std::collections::BTreeSet::new();
    for env in [&planned, &assigned] {
        for a in env["assignments"].as_array().expect("assignments") {
            let id = a["id"].as_str().expect("id");
            assert!(generated(id, a["kind"].as_str().expect("kind")), "{a}");
            assert!(seen.insert(id.to_string()), "{id} drawn twice");
        }
    }
    for a in assigned["assignments"].as_array().unwrap() {
        let id = a["id"].as_str().unwrap();
        let file = fx.read(&format!("reader/{}", a["path"].as_str().unwrap()));
        assert!(file.contains(&format!("@id(\"{id}\")\n")), "{a}");
    }
}

/// 12.4 item 2 for the reserved commands of Section 7.1.
#[test]
fn reserved_command_goldens() {
    let Some(fx) = Fixture::committed("reserved-goldens") else {
        return;
    };
    let run = cpl(
        &fx.reader,
        &["work", "run", "work/pagination.work", "--json"],
    );
    run.expect_code(2);
    check_json_golden("work_run_reserved.json", &run, &fx, false);
    let run = cpl(
        &fx.reader,
        &["work", "integrate", "pagination_update", "--json"],
    );
    run.expect_code(2);
    check_json_golden("work_integrate_reserved.json", &run, &fx, false);
}

// ===========================================================================
// cpl fmt
// ===========================================================================

const READER_FILES: [&str; 5] = [
    "src/reader/images.cpl",
    "src/reader/pages.cpl",
    "src/tests/pagination.cpl",
    "work/pagination.patch",
    "work/pagination.work",
];

#[test]
fn fmt_check_on_the_examples_is_clean() {
    let fx = Fixture::new("fmt-clean");
    let run = cpl(&fx.reader, &["fmt", "--check"]);
    run.expect_code(0);
    assert_eq!(run.stdout, "");
    let env = cpl(&fx.reader, &["fmt", "--check", "--json"]).json();
    assert_eq!(env["command"], "fmt");
    assert_eq!(env["check"], true);
    let files: Vec<(&str, &str)> = env["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| (f["path"].as_str().unwrap(), f["result"].as_str().unwrap()))
        .collect();
    let expected: Vec<(&str, &str)> = READER_FILES.iter().map(|p| (*p, "unchanged")).collect();
    assert_eq!(files, expected);
    assert!(env["revision"].is_object());
}

#[test]
fn fmt_check_names_a_deformatted_copy() {
    let fx = Fixture::new("fmt-deformatted");
    let path = "reader/src/reader/pages.cpl";
    let original = fx.read(path);
    let deformatted = original
        .replace(
            "fn clean(page: Page) -> Page {",
            "fn clean( page:Page )->Page{",
        )
        .replace("    a / b\n", "        a/b\n");
    assert_ne!(deformatted, original);
    fx.write(path, &deformatted);

    let run = cpl(&fx.reader, &["fmt", "--check"]);
    run.expect_code(1);
    assert_eq!(run.stdout, "src/reader/pages.cpl\n");
    assert_eq!(fx.read(path), deformatted, "--check writes nothing");
    let env = cpl(&fx.reader, &["fmt", "--check", "--json"]).json();
    assert_eq!(env["status"], "unsuccessful");
    let results: Vec<&str> = env["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["result"].as_str().unwrap())
        .collect();
    assert_eq!(
        results,
        [
            "unchanged",
            "would_format",
            "unchanged",
            "unchanged",
            "unchanged"
        ]
    );

    let env = cpl(&fx.reader, &["fmt", "--json"]).json();
    assert_eq!(env["status"], "ok");
    assert_eq!(env["check"], false);
    assert_eq!(
        env["files"][1],
        json!({ "path": "src/reader/pages.cpl", "result": "formatted" })
    );
    assert_eq!(
        fx.read(path),
        original,
        "formatting restores the formatted original"
    );
    cpl(&fx.reader, &["fmt", "--check"]).expect_code(0);
    let env = cpl(&fx.reader, &["fmt", "--json"]).json();
    assert!(env["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|f| f["result"] == "unchanged"));
}

#[test]
fn fmt_leaves_a_file_that_does_not_parse() {
    let fx = Fixture::new("fmt-broken");
    let broken = "module reader.broken\n\nfn f( {\n";
    fx.write("reader/src/reader/broken.cpl", broken);
    let run = cpl(&fx.reader, &["fmt", "--json"]);
    run.expect_code(1);
    let env = run.json();
    let entry = env["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "src/reader/broken.cpl")
        .expect("the broken file")
        .clone();
    assert_eq!(entry["result"], "error");
    let diagnostics = env["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["category"], "parse");
    assert_eq!(diagnostics[0]["span"]["file"], "src/reader/broken.cpl");
    assert_eq!(fx.read("reader/src/reader/broken.cpl"), broken);
    let run = cpl(&fx.reader, &["fmt"]);
    run.expect_code(1);
    assert!(
        run.stderr.contains("  --> src/reader/broken.cpl:3:"),
        "{}",
        run.stderr
    );
}

#[test]
fn fmt_named_paths_and_stdin() {
    let fx = Fixture::new("fmt-paths");
    let env = cpl(
        &fx.reader,
        &["fmt", "--check", "--json", "src/reader/pages.cpl", "work"],
    )
    .json();
    assert_eq!(
        env["files"],
        json!([{ "path": "src/reader/pages.cpl", "result": "unchanged" },
               { "path": "work/pagination.patch", "result": "unchanged" },
               { "path": "work/pagination.work", "result": "unchanged" }])
    );
    // Inside a project, paths are project-relative from any directory.
    let env = cpl(
        &fx.reader.join("src"),
        &["fmt", "--check", "--json", "tests"],
    )
    .json();
    assert_eq!(
        env["files"],
        json!([{ "path": "src/tests/pagination.cpl", "result": "unchanged" }])
    );

    let messy = "module m\n\nfn f( a:Int )->Int{\n  a\n}\n";
    let tidy = "module m\n\nfn f(a: Int) -> Int {\n    a\n}\n";
    let run = cpl_with_stdin(&fx.root, &["fmt", "--stdin", "cpl"], Some(messy));
    run.expect_code(0);
    assert_eq!(run.stdout, tidy);
    let run = cpl_with_stdin(&fx.root, &["fmt", "--stdin", "cpl", "--check"], Some(messy));
    run.expect_code(1);
    assert_eq!(run.stdout, "<virtual:stdin>\n");
    let run = cpl_with_stdin(
        &fx.root,
        &["fmt", "--stdin", "cpl", "--check", "--json"],
        Some(tidy),
    );
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["revision"], Value::Null);
    assert_eq!(
        env["files"],
        json!([{ "path": "<virtual:stdin>", "result": "unchanged" }])
    );
    let run = cpl_with_stdin(&fx.root, &["fmt", "--stdin", "cpl"], Some("module m\nfn ("));
    run.expect_code(1);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr.contains("  --> <virtual:stdin>:2:"),
        "{}",
        run.stderr
    );

    // Exit 2: --stdin with paths, --stdin --json without --check, bad paths.
    for args in [
        vec!["fmt", "--stdin", "cpl", "x.cpl", "--json"],
        vec!["fmt", "--stdin", "cpl", "--json"],
        vec!["fmt", "no/such/file.cpl", "--json"],
        vec!["fmt", "cpl.toml", "--json"],
    ] {
        let run = cpl_with_stdin(&fx.reader, &args, Some(tidy));
        run.expect_code(2);
        assert_eq!(
            kinds(&run.json()),
            pairs(&[("error", "usage_bad_argument")]),
            "{args:?}"
        );
    }
}

// ===========================================================================
// cpl id assign
// ===========================================================================

/// True for the generated format of Section 4.1 with this kind prefix.
fn generated(id: &str, kind: &str) -> bool {
    id.strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|rest| {
            rest.len() == 16
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        })
}

#[test]
fn id_assign_then_check_shows_no_provisional_ids() {
    let fx = Fixture::new("id-assign");
    let run = cpl(&fx.reader, &["id", "assign", "--json"]);
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["command"], "id assign");
    assert_eq!(env["status"], "ok");
    assert_eq!(env["check"], false);
    assert_eq!(env["skipped_files"], json!([]));
    let assignments = env["assignments"].as_array().expect("assignments");
    // Every declaration but paginate, which has @id("pgn_7c2d").
    assert_eq!(assignments.len(), 20);
    for a in assignments {
        let kind = a["kind"].as_str().unwrap();
        assert!(generated(a["id"].as_str().unwrap(), kind), "{a}");
    }
    let clean = assignments
        .iter()
        .find(|a| a["symbol"] == "reader.pages.clean")
        .expect("clean");
    assert_eq!(
        (
            clean["path"].clone(),
            clean["line"].clone(),
            clean["kind"].clone()
        ),
        (json!("src/reader/pages.cpl"), json!(21), json!("fn"))
    );
    let id = clean["id"].as_str().unwrap();
    assert!(fx
        .read("reader/src/reader/pages.cpl")
        .contains(&format!("@id(\"{id}\")\nfn clean(page: Page) -> Page {{")));

    // The check now names every project declaration by its @id.
    let env = cpl(&fx.reader, &["check", "--lint", "id_missing", "--json"]).json();
    assert_eq!(kinds(&env), pairs(&[("info", "verification_result")]));
    let env = cpl(&fx.reader, &["test", "--seed", "1", "--json"]).json();
    assert_eq!(env["status"], "ok");
    for d in env["diagnostics"].as_array().unwrap() {
        let symbol = d["symbol"].as_str().unwrap();
        assert!(!symbol.contains('.'), "provisional symbol {symbol}");
    }
    for record in env["results"].as_array().unwrap() {
        assert!(generated(
            record["symbol"].as_str().unwrap(),
            if record["kind"] == "test" {
                "test"
            } else {
                "prop"
            }
        ));
    }
    // Idempotent: a second run assigns nothing.
    let env = cpl(&fx.reader, &["id", "assign", "--check", "--json"]).json();
    assert_eq!(env["assignments"], json!([]));
    cpl(&fx.reader, &["id", "assign", "--check"]).expect_code(0);
    // `cpl fmt` lays the inserted lines out (Section 4.2 step 3): the two
    // adjacent `type` declarations of reader.pages now carry annotations,
    // so T51 no longer keeps them together. Formatting keeps every id.
    let run = cpl(&fx.reader, &["fmt", "--check"]);
    run.expect_code(1);
    assert_eq!(run.stdout, "src/reader/pages.cpl\n");
    cpl(&fx.reader, &["fmt"]).expect_code(0);
    cpl(&fx.reader, &["fmt", "--check"]).expect_code(0);
    assert!(fx
        .read("reader/src/reader/pages.cpl")
        .contains(&format!("@id(\"{id}\")\nfn clean(")));
    cpl(&fx.reader, &["id", "assign", "--check"]).expect_code(0);
}

#[test]
fn id_assign_check_writes_nothing() {
    let fx = Fixture::new("id-assign-check");
    let before: Vec<String> = READER_FILES[..3]
        .iter()
        .map(|p| fx.read(&format!("reader/{p}")))
        .collect();
    let run = cpl(&fx.reader, &["id", "assign", "--check"]);
    run.expect_code(1);
    let after: Vec<String> = READER_FILES[..3]
        .iter()
        .map(|p| fx.read(&format!("reader/{p}")))
        .collect();
    assert_eq!(before, after);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 20);
    let clean = lines
        .iter()
        .find(|l| l.ends_with("  reader.pages.clean"))
        .expect("clean");
    assert!(
        clean.starts_with("src/reader/pages.cpl:21  @id(\"fn_"),
        "{clean}"
    );
    assert_eq!(
        clean.len(),
        "src/reader/pages.cpl:21  @id(\"fn_k7w3qz2mb6ptx4ve\")  reader.pages.clean".len()
    );
}

#[test]
fn id_assign_skips_a_file_that_does_not_parse() {
    let fx = Fixture::new("id-assign-skip");
    fx.write(
        "reader/src/reader/broken.cpl",
        "module reader.broken\n\nfn f( {\n",
    );
    let run = cpl(&fx.reader, &["id", "assign", "--json"]);
    run.expect_code(1);
    let env = run.json();
    assert_eq!(
        env["skipped_files"],
        json!([{ "path": "src/reader/broken.cpl", "reason": "syntax errors" }])
    );
    assert_eq!(env["assignments"].as_array().map(Vec::len), Some(20));
    assert_eq!(env["diagnostics"].as_array().map(Vec::len), Some(1));
    assert_eq!(env["diagnostics"][0]["category"], "parse");
}

#[test]
fn id_assign_named_paths() {
    let fx = Fixture::new("id-assign-paths");
    let env = cpl(&fx.reader, &["id", "assign", "src/tests", "--json"]).json();
    let symbols: Vec<&str> = env["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["symbol"].as_str().unwrap())
        .collect();
    assert_eq!(symbols.len(), 3);
    assert!(symbols.iter().all(|s| s.starts_with("tests.pagination.")));
    assert!(
        fx.read("reader/src/reader/pages.cpl")
            == std::fs::read_to_string(workspace().join("examples/reader/src/reader/pages.cpl"))
                .unwrap()
    );
    // A named file that is no source file of the project.
    let run = cpl(&fx.reader, &["id", "assign", "cpl.toml", "--json"]);
    run.expect_code(2);
    assert_eq!(
        kinds(&run.json()),
        pairs(&[("error", "usage_bad_argument")])
    );
    // Outside any project, the named files are a project of their own.
    let loose = temp_dir("id-assign-loose");
    std::fs::write(
        loose.join("a.cpl"),
        "module a\n\nfn f() -> Int {\n    1\n}\n",
    )
    .unwrap();
    let run = cpl(&loose, &["id", "assign", "a.cpl", "--json"]);
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["revision"], Value::Null);
    assert_eq!(env["assignments"][0]["path"], "a.cpl");
    assert_eq!(env["assignments"][0]["symbol"], "a.f");
    let text = std::fs::read_to_string(loose.join("a.cpl")).unwrap();
    assert!(text.starts_with("module a\n\n@id(\"fn_"), "{text}");
}

// ===========================================================================
// Usage, envelopes, determinism
// ===========================================================================

#[test]
fn commands_without_a_project_are_usage_no_project() {
    let dir = temp_dir("no-project");
    for args in [
        vec!["check"],
        vec!["run", "a.f"],
        vec!["test"],
        vec!["fmt"],
        vec!["id", "assign"],
        vec!["inspect", "a.f"],
        vec!["prove", "a.f"],
        vec!["diff", "HEAD"],
        vec!["work", "check", "work/x.work"],
    ] {
        let run = cpl(&dir, &args);
        run.expect_code(2);
        assert!(
            run.stderr.starts_with("error[usage_no_project]: "),
            "{}",
            run.stderr
        );
        let mut json_args = args.clone();
        json_args.push("--json");
        let run = cpl(&dir, &json_args);
        run.expect_code(2);
        let env = run.json();
        assert_eq!(env["status"], "error");
        assert_eq!(env["revision"], Value::Null);
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "usage_no_project")]),
            "{args:?}"
        );
        assert_eq!(env["diagnostics"][0]["category"], "usage");
    }
    let run = cpl(
        &dir,
        &["check", "--project", dir.to_str().unwrap(), "--json"],
    );
    run.expect_code(2);
    assert_eq!(kinds(&run.json()), pairs(&[("error", "usage_no_project")]));
    // --project names the root from anywhere, before or after the command.
    let fx = Fixture::new("project-flag");
    let reader = fx.reader.to_str().unwrap();
    cpl(&dir, &["--project", reader, "check"]).expect_code(0);
    cpl(&dir, &["check", "--project", reader]).expect_code(0);
    // A nested directory finds the project above it.
    cpl(&fx.reader.join("src/reader"), &["check"]).expect_code(0);
}

#[test]
fn malformed_command_lines_are_usage_bad_argument() {
    let fx = Fixture::new("usage");
    for args in [
        vec!["check", "--bogus"],
        vec!["test", "--seed", "x"],
        vec!["test", "--seed", "-1"],
        vec!["test", "--cases", "1.5"],
        vec!["fmt", "--stdin", "toml"],
        vec!["run"],
        vec!["frobnicate"],
        vec!["run", "--max-call-depth", "many", "a.f"],
    ] {
        let human = args.clone();
        let run = cpl(&fx.reader, &human);
        run.expect_code(2);
        assert!(
            run.stderr.starts_with("error[usage_bad_argument]: "),
            "{human:?}: {}",
            run.stderr
        );
        let mut json = args.clone();
        json.push("--json");
        let run = cpl(&fx.reader, &json);
        run.expect_code(2);
        let env = run.json();
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "usage_bad_argument")]),
            "{json:?}"
        );
        assert_eq!(env["status"], "error");
    }
    let env = cpl(&fx.reader, &["check", "--bogus", "--json"]).json();
    assert_eq!(env["command"], "check");
    assert_eq!(
        cpl(&fx.reader, &["frobnicate", "--json"]).json()["command"],
        "cpl"
    );
    // --json and --jsonl together.
    let run = cpl(&fx.reader, &["check", "--json", "--jsonl"]);
    run.expect_code(2);
    assert_eq!(
        kinds(&run.json()),
        pairs(&[("error", "usage_bad_argument")])
    );
    // Under --jsonl a refused command line is one diagnostic line, then the envelope.
    let run = cpl(&fx.reader, &["test", "--jsonl", "--seed", "x"]);
    run.expect_code(2);
    let lines = run.json_lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["code"], "usage_bad_argument");
    assert_eq!(lines[1]["command"], "test");
    assert_eq!(lines[1]["status"], "error");
}

#[test]
fn a_refused_flag_is_reported_as_its_command_reports() {
    // `--jsonl` is check's and test's only (7.1): after `run` it is the
    // refused flag, so the refusal prints in the human form (or as one
    // envelope under --json), never as JSON lines; the usage note is the
    // command's own usage line, not one listing the flags the line used.
    let fx = Fixture::new("usage-jsonl");
    let args = ["run", "reader.pages.paginate_ints", "[1]", "1", "--jsonl"];
    let run = cpl(&fx.reader, &args);
    run.expect_code(2);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr
            .starts_with("error[usage_bad_argument]: unexpected argument '--jsonl' found\n"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr
            .contains("  = note: Usage: cpl run [OPTIONS] <FUNCTION> [ARG]...\n"),
        "{}",
        run.stderr
    );
    let mut with_json = args.to_vec();
    with_json.push("--json");
    let run = cpl(&fx.reader, &with_json);
    run.expect_code(2);
    let env = run.json();
    assert_eq!(env["command"], "run");
    assert_eq!(kinds(&env), pairs(&[("error", "usage_bad_argument")]));
    let notes: Vec<&str> = env["diagnostics"][0]["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .map(|n| n["message"].as_str().expect("message"))
        .collect();
    assert_eq!(
        notes.last(),
        Some(&"Usage: cpl run [OPTIONS] <FUNCTION> [ARG]...")
    );
    assert!(
        notes
            .iter()
            .all(|n| !n.starts_with("Usage:") || !n.contains("--json")),
        "{notes:?}"
    );
    for (args, usage) in [
        (
            vec!["fmt", "--check", "--jsonl"],
            "Usage: cpl fmt [OPTIONS] [PATH]...",
        ),
        (
            vec!["id", "assign", "--check", "--jsonl"],
            "Usage: cpl id assign [OPTIONS] [PATH]...",
        ),
        (
            vec!["check", "--bogus", "--json"],
            "Usage: cpl check [OPTIONS]",
        ),
    ] {
        let run = cpl(&fx.reader, &args);
        run.expect_code(2);
        if args.contains(&"--json") {
            let env = run.json();
            let notes = env["diagnostics"][0]["notes"].as_array().expect("notes");
            assert_eq!(notes.last().unwrap()["message"], usage, "{args:?}");
        } else {
            assert_eq!(run.stdout, "", "{args:?}");
            assert!(
                run.stderr.contains(&format!("  = note: {usage}\n")),
                "{args:?}: {}",
                run.stderr
            );
        }
    }
}

#[test]
fn refused_arguments_name_the_revision_of_their_project() {
    // 1.4.2: `revision` is null only when no project is involved. A command
    // that refuses one of its arguments runs in the located project,
    // whether it refuses before loading it or after, so its envelope names
    // that project's revision; the same refusal outside any project names
    // none, and so does a command line clap refuses (no command ran).
    let fx = Fixture::new("refusal-revisions");
    let loose = temp_dir("refusal-revisions-loose");
    let refusals: [&[&str]; 7] = [
        &["run", "reader.pages.divide_exactly", "--arg", "[", "1"],
        &["test", "Not.A.Module"],
        &["check", "--lint", "bogus"],
        &["inspect", "paginate", "--include", "bogus"],
        &["fmt", "--check", "missing.cpl"],
        &["id", "assign", "missing.cpl"],
        &["check", "--jsonl"],
    ];
    for args in refusals {
        let (code, env) = json_of(&fx.reader, args);
        assert_eq!(code, 2, "{args:?}: {env}");
        assert_eq!(kinds(&env), pairs(&[("error", "usage_bad_argument")]));
        assert_eq!(env["revision"]["state"], "worktree", "{args:?}: {env}");
        let (code, env) = json_of(&loose, args);
        assert_eq!(code, 2, "{args:?}: {env}");
        assert_eq!(kinds(&env), pairs(&[("error", "usage_bad_argument")]));
        assert_eq!(env["revision"], Value::Null, "{args:?}: no project");
    }
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/missing.work"]);
    assert_eq!(code, 2, "{env}");
    assert_eq!(kinds(&env), pairs(&[("error", "usage_bad_argument")]));
    assert_eq!(env["revision"]["state"], "worktree");
    let (code, env) = json_of(&fx.reader, &["check", "--frobnicate"]);
    assert_eq!(code, 2);
    assert_eq!(env["revision"], Value::Null, "a refused command line");
}

#[test]
fn reserved_commands_are_not_implemented() {
    let fx = Fixture::new("reserved");
    for (args, command) in [
        (vec!["work", "run", "work/pagination.work"], "work run"),
        (
            vec!["work", "integrate", "pagination_update"],
            "work integrate",
        ),
    ] {
        let run = cpl(&fx.reader, &args);
        run.expect_code(2);
        assert!(run.stderr.starts_with("error[usage_not_implemented]: "));
        let mut json = args.clone();
        json.push("--json");
        let env = cpl(&fx.reader, &json).json();
        assert_eq!(env["command"], command);
        assert_eq!(kinds(&env), pairs(&[("error", "usage_not_implemented")]));
        let note = env["diagnostics"][0]["notes"][0]["message"]
            .as_str()
            .unwrap();
        assert!(note.contains("Milestone 6"), "{note}");
    }
}

#[test]
fn help_and_version() {
    let dir = temp_dir("help");
    let run = cpl(&dir, &["--help"]);
    run.expect_code(0);
    for command in [
        "check", "fmt", "run", "test", "inspect", "prove", "diff", "id", "work",
    ] {
        assert!(run.stdout.contains(command), "{command} in\n{}", run.stdout);
    }
    let run = cpl(&dir, &["--version"]);
    run.expect_code(0);
    assert_eq!(run.stdout, "cpl 0.1.0\n");
    cpl(&dir, &["id", "assign", "--help"]).expect_code(0);
}

#[test]
fn every_json_output_is_exactly_one_object() {
    let fx = Fixture::new("one-object");
    let cases: Vec<(&Path, Vec<&str>)> = vec![
        (&fx.reader, vec!["check"]),
        (&fx.holes, vec!["check", "--release"]),
        (&fx.reader, vec!["fmt", "--check"]),
        (
            &fx.reader,
            vec!["run", "reader.pages.paginate_ints", "[1, 2, 3]", "2"],
        ),
        (
            &fx.reader,
            vec!["run", "reader.pages.divide_exactly", "7", "0"],
        ),
        (&fx.holes, vec!["run", "holes.pages.paginate"]),
        (&fx.reader, vec!["test", "--seed", "1"]),
        (&fx.holes, vec!["test"]),
        (&fx.reader, vec!["id", "assign", "--check"]),
        (&fx.reader, vec!["inspect", "reader.pages.paginate"]),
        (
            &fx.reader,
            vec![
                "inspect",
                "paginate",
                "--include",
                "implementation,callers,dependencies,obligations,tests",
            ],
        ),
        (&fx.reader, vec!["inspect", "nope"]),
        (
            &fx.reader,
            vec!["inspect", "paginate", "--include", "bogus"],
        ),
        (&fx.reader, vec!["prove", "reader.pages.paginate"]),
        (&fx.reader, vec!["prove", "reader.pages.Positive"]),
        (&fx.reader, vec!["diff", "HEAD"]),
        (&fx.reader, vec!["work", "check", "work/pagination.work"]),
        (&fx.reader, vec!["work", "check", "work/nope.work"]),
        (&fx.reader, vec!["work", "run", "work/pagination.work"]),
        (&fx.reader, vec!["check", "--frobnicate"]),
    ];
    for (dir, args) in cases {
        let mut all = args.clone();
        all.push("--json");
        let run = cpl(dir, &all);
        let env = run.json();
        let status = env["status"].as_str().expect("status");
        let expected = match run.code {
            0 => "ok",
            1 => "unsuccessful",
            _ => "error",
        };
        assert_eq!(status, expected, "{args:?}");
        for key in ["command", "toolchain", "revision", "status", "diagnostics"] {
            assert!(env.get(key).is_some(), "{key} in {args:?}");
        }
        assert_eq!(
            env["toolchain"],
            json!({ "version": "0.1.0", "edition": "0.1", "json_schema": 1 })
        );
        // The envelope keys come first, in declaration order.
        let order: Vec<&str> = env
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let _ = order;
        let raw = &run.stdout;
        let at = |key: &str| raw.find(&format!("\n  \"{key}\":")).unwrap_or(usize::MAX);
        assert!(at("command") < at("toolchain") && at("toolchain") < at("revision"));
        assert!(at("revision") < at("status") && at("status") < at("diagnostics"));
    }
}

#[test]
fn outputs_are_deterministic() {
    let Some(fx) = Fixture::committed("deterministic") else {
        return;
    };
    for (dir, args) in [
        (&fx.reader, vec!["check", "--json"]),
        (&fx.holes, vec!["check", "--json"]),
        (&fx.holes, vec!["check", "--release", "--json"]),
        (&fx.reader, vec!["test", "--seed", "7", "--json"]),
        (&fx.reader, vec!["test", "--seed", "7", "--jsonl"]),
        (
            &fx.reader,
            vec![
                "run",
                "reader.pages.paginate_ints",
                "[5, 6, 7]",
                "2",
                "--json",
            ],
        ),
        (&fx.reader, vec!["fmt", "--check", "--json"]),
        (
            &fx.reader,
            vec![
                "inspect",
                "paginate",
                "--include",
                "signature,contract,effects,implementation,docs,callers,dependencies,obligations,tests",
                "--json",
            ],
        ),
        (&fx.reader, vec!["inspect", "paginate"]),
        (&fx.reader, vec!["prove", "paginate", "--json"]),
        (&fx.reader, vec!["prove", "paginate"]),
        (&fx.reader, vec!["diff", "HEAD", "--json"]),
        (
            &fx.reader,
            vec!["work", "check", "work/pagination.work", "--json"],
        ),
    ] {
        let a = cpl(dir, &args);
        let b = cpl(dir, &args);
        let volatile = Volatile::default();
        assert_eq!(
            normalize(&a.stdout, &volatile),
            normalize(&b.stdout, &volatile),
            "{args:?}"
        );
        assert_eq!(a.code, b.code);
    }
}

// ===========================================================================
// Flags, configuration, and the evaluation stack
// ===========================================================================

/// A project with a value nested as deep as a loop makes it (no call
/// nesting), and a fault whose `input` holds such a value.
const DEEP: &str = r#"module deep.tree

pub struct Tree {
    children: List<Tree>
}

pub fn nest(n: Int) -> Tree {
    var tree = Tree { children: [] }
    var i = 0
    while i < n
        decreases n - i
    {
        tree = Tree { children: [tree] }
        i = i + 1
    }
    tree
}

pub fn inner(t: Tree) -> Int
    requires t.children.len == 7
{
    0
}

pub fn outer(n: Int) -> Int {
    inner(nest(n))
}

pub fn count(n: Int) -> Int
    requires n >= 0
    decreases n
{
    if n == 0 {
        0
    } else {
        1 + count(n - 1)
    }
}
"#;

fn deep_project(name: &str) -> std::path::PathBuf {
    let dir = temp_dir(name);
    std::fs::write(
        dir.join("cpl.toml"),
        "[project]\nname = \"deep\"\nedition = \"0.1\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src/deep")).unwrap();
    std::fs::write(dir.join("src/deep/tree.cpl"), DEEP).unwrap();
    dir
}

#[test]
fn deep_values_are_serialized_and_dropped_on_the_evaluation_stack() {
    // The value JSON of a result or of a fault's `input` drops and
    // serializes recursively (toolchain 1.7 `stack`); on the main thread's
    // stack these runs overflow it.
    let dir = deep_project("deep-values");
    let n = 100_000;
    let run = cpl(&dir, &["run", "deep.tree.nest", &n.to_string()]);
    run.expect_code(0);
    assert_eq!(run.stdout.len(), 21 * n + 22);
    assert!(run
        .stdout
        .starts_with("Tree { children: [Tree { children: ["));
    let run = cpl(&dir, &["run", "deep.tree.outer", &n.to_string()]);
    run.expect_code(1);
    assert!(
        run.stderr.starts_with(
            "error[contract_violation]: requires clause failed: t.children.len == 7\n"
        ),
        "{}",
        &run.stderr[..run.stderr.len().min(300)]
    );
    // With --json (its pretty form grows with the square of the depth, so
    // the depth is moderate here; the reader of the test cannot parse it,
    // its nesting being beyond serde_json's limit).
    let run = cpl(&dir, &["run", "deep.tree.nest", "300", "--json"]);
    run.expect_code(0);
    assert!(run.stderr.is_empty());
    assert!(run.stdout.starts_with("{\n  \"command\": \"run\","));
    assert!(run.stdout.ends_with("  \"fault\": null\n}\n"));
    assert!(run
        .stdout
        .contains("\"result_text\": \"Tree { children: [Tree { children: ["));
}

#[test]
fn max_call_depth_limits_the_run() {
    let dir = deep_project("max-call-depth");
    let (code, env) = run_json(&dir, &["deep.tree.count", "50"]);
    assert_eq!(code, 0);
    assert_eq!(env["result_text"], "50");
    let (code, env) = run_json(&dir, &["deep.tree.count", "50", "--max-call-depth", "10"]);
    assert_eq!(code, 1);
    let fault = &env["fault"];
    assert_eq!(fault["code"], "resource_exhaustion");
    assert_eq!(fault["resource"], "stack");
    assert_eq!(fault["limit"], 10);
    // `[run] max_call_depth` is the default the flag overrides.
    let manifest = std::fs::read_to_string(dir.join("cpl.toml")).unwrap();
    std::fs::write(
        dir.join("cpl.toml"),
        format!("{manifest}\n[run]\nmax_call_depth = 20\n"),
    )
    .unwrap();
    let (code, env) = run_json(&dir, &["deep.tree.count", "50"]);
    assert_eq!(code, 1);
    assert_eq!(env["fault"]["limit"], 20);
    let (code, _) = run_json(&dir, &["deep.tree.count", "50", "--max-call-depth", "100"]);
    assert_eq!(code, 0);
}

#[test]
fn test_flags_and_the_test_section() {
    let fx = Fixture::new("test-flags");
    let env = cpl(
        &fx.reader,
        &["test", "--seed", "1", "--cases", "5", "--json"],
    )
    .json();
    assert_eq!(env["cases"], 5);
    assert_eq!(env["results"][2]["cases"], 5);
    assert_eq!(
        env["results"][2]["receipt"]["check_configuration"]["cases"],
        5
    );
    assert_eq!(env["receipt"]["configuration"]["cases"], 5);
    let env = cpl(
        &fx.reader,
        &[
            "test",
            "--seed",
            "1",
            "--timeout-ms",
            "60000",
            "--max-shrinks",
            "3",
            "--json",
        ],
    )
    .json();
    assert_eq!(env["status"], "ok");
    assert_eq!(env["receipt"]["configuration"]["timeout_ms"], 60000);
    assert_eq!(env["receipt"]["configuration"]["max_shrinks"], 3);
    assert_eq!(
        env["results"][0]["receipt"]["check_configuration"]["max_shrinks"],
        3
    );
    // The [test] section supplies the defaults the flags override.
    let manifest = fx.read("reader/cpl.toml");
    fx.write(
        "reader/cpl.toml",
        &format!("{manifest}\n[test]\ncases = 7\nmax_shrinks = 2\n"),
    );
    let env = cpl(&fx.reader, &["test", "--seed", "1", "--json"]).json();
    assert_eq!(env["cases"], 7);
    assert_eq!(env["receipt"]["configuration"]["max_shrinks"], 2);
    let env = cpl(
        &fx.reader,
        &["test", "--seed", "1", "--cases", "3", "--json"],
    )
    .json();
    assert_eq!(env["cases"], 3);

    let fx = broken_paginate("test-max-shrinks");
    let env = cpl(
        &fx.reader,
        &["test", "--seed", "1", "--max-shrinks", "0", "--json"],
    )
    .json();
    let shrunk = &env["results"][2]["shrunk"];
    assert_eq!(shrunk["shrink_steps"], 0);
    assert_eq!(shrunk["shrink_exhausted"], true);
    assert_eq!(shrunk["input"], env["results"][2]["failing_case"]["input"]);
}

#[test]
fn zero_cases_are_refused_rather_than_tested() {
    // A property run with zero cases would never evaluate its body and yet
    // be `tested` in its record and receipt: `--cases 0` is a bad flag
    // value (exit 2) and `[test] cases = 0` the fatal
    // project_manifest_invalid (exit 2), so no such evidence is produced.
    let fx = Fixture::new("test-zero-cases");
    let run = cpl(
        &fx.reader,
        &["test", "--seed", "1", "--cases", "0", "--json"],
    );
    run.expect_code(2);
    let env = run.json();
    assert_eq!(env["command"], "test");
    assert_eq!(kinds(&env), pairs(&[("error", "usage_bad_argument")]));
    assert!(
        env["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("'0'") && m.contains("--cases")),
        "{env}"
    );
    assert!(env.get("results").is_none(), "nothing runs");
    let run = cpl(&fx.reader, &["test", "--seed", "1", "--cases", "0"]);
    run.expect_code(2);
    assert!(
        run.stderr.starts_with("error[usage_bad_argument]: "),
        "{}",
        run.stderr
    );
    assert_eq!(run.stdout, "");
    // One case is the smallest count.
    let env = cpl(
        &fx.reader,
        &["test", "--seed", "1", "--cases", "1", "--json"],
    )
    .json();
    assert_eq!(
        (env["status"].clone(), env["cases"].clone()),
        (json!("ok"), json!(1))
    );
    assert_eq!(env["results"][2]["cases"], 1);

    let manifest = fx.read("reader/cpl.toml");
    fx.write(
        "reader/cpl.toml",
        &format!("{manifest}\n[test]\ncases = 0\n"),
    );
    for args in [vec!["test", "--seed", "1"], vec!["check"]] {
        let mut all = args.clone();
        all.push("--json");
        let run = cpl(&fx.reader, &all);
        run.expect_code(2);
        let env = run.json();
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "project_manifest_invalid")]),
            "{args:?}"
        );
        assert!(
            env["diagnostics"][0]["message"]
                .as_str()
                .is_some_and(|m| m.contains("`test.cases` must be an integer from 1 to")),
            "{env}"
        );
    }
}

#[test]
fn load_failures_and_manifest_diagnostics() {
    let fx = Fixture::new("load-failures");
    let manifest = fx.read("holes/cpl.toml");
    // Not TOML: the fatal project_manifest_invalid (exit 2).
    fx.write("holes/cpl.toml", "[project\n");
    let run = cpl(&fx.holes, &["check", "--json"]);
    run.expect_code(2);
    let env = run.json();
    assert_eq!(kinds(&env), pairs(&[("error", "project_manifest_invalid")]));
    assert_eq!(env["diagnostics"][0]["path"], "cpl.toml");
    assert_eq!(env["revision"], Value::Null);
    for args in [
        vec!["run", "holes.pages.summarize"],
        vec!["test"],
        vec!["fmt", "--check"],
        vec!["id", "assign", "--check"],
    ] {
        let mut all = args.clone();
        all.push("--json");
        let run = cpl(&fx.holes, &all);
        run.expect_code(2);
        assert_eq!(
            kinds(&run.json()),
            pairs(&[("error", "project_manifest_invalid")]),
            "{args:?}"
        );
    }
    // An unknown key is a diagnostic of the check (exit 1), not a load failure.
    fx.write("holes/cpl.toml", &format!("{manifest}colour = \"blue\"\n"));
    let run = cpl(&fx.holes, &["check", "--json"]);
    run.expect_code(1);
    let env = run.json();
    let unknown = with_code(&env, "project_manifest_unknown_key");
    assert_eq!(unknown.len(), 1);
    assert!(unknown[0].get("span").is_none());
    // A lock that does not parse is the fatal project_lock_invalid.
    fx.write("holes/cpl.toml", &manifest);
    fx.write("holes/cpl.lock", "version = 1\nbogus = true\n");
    let run = cpl(&fx.holes, &["check", "--json"]);
    run.expect_code(2);
    assert_eq!(
        kinds(&run.json()),
        pairs(&[("error", "project_lock_invalid")])
    );
}

#[test]
fn the_std_dir_override() {
    let fx = Fixture::new("std-dir");
    let std_dir = workspace().join("std");
    let std_dir = std_dir.to_str().unwrap();
    let a = cpl(&fx.holes, &["check", "--json"]);
    let b = cpl_env(&fx.holes, &["check", "--json"], &[("CPL_STD_DIR", std_dir)]);
    b.expect_code(0);
    assert_eq!(a.stdout, b.stdout, "the workspace std is the embedded std");
    let empty = temp_dir("std-dir-empty");
    let run = cpl_env(
        &fx.holes,
        &["check", "--json"],
        &[("CPL_STD_DIR", empty.to_str().unwrap())],
    );
    run.expect_code(2);
    let env = run.json();
    assert_eq!(kinds(&env), pairs(&[("error", "internal_error")]));
    assert_eq!(env["diagnostics"][0]["kind"], "std_unavailable");
}

#[test]
fn run_targets_resolve_as_index_targets() {
    let fx = Fixture::new("run-targets");
    // A provisional id is the qualified name; a bare name that names one
    // definition resolves too (Section 6.1).
    for target in ["reader.pages.paginate_ints", "paginate_ints"] {
        assert_returns(&fx.reader, &[target, "[1, 2, 3]", "2"], "Ok([[1, 2], [3]])");
    }
    // `Positive` names reader.pages.Positive and std.gen.Positive.
    let (code, env) = run_json(&fx.reader, &["Positive"]);
    assert_eq!(code, 2);
    assert_eq!(kinds(&env), pairs(&[("error", "index_ambiguous_symbol")]));
    // A std function is a function of the checked program (7.2).
    assert_returns(
        &fx.reader,
        &["std.text.split", "\"a,b\"", "\",\""],
        "[\"a\", \"b\"]",
    );
    assert_returns(&fx.reader, &["std.prelude.Int.abs", "-4"], "4");
}

#[test]
fn check_of_a_project_that_does_not_parse() {
    let fx = Fixture::new("check-parse-error");
    fx.write(
        "reader/src/reader/broken.cpl",
        "module reader.broken\n\nfn f( {\n",
    );
    let run = cpl(&fx.reader, &["check", "--json"]);
    run.expect_code(1);
    let env = run.json();
    let diagnostics = env["diagnostics"].as_array().unwrap();
    assert_eq!(
        diagnostics.len(),
        1,
        "one parse diagnostic, and nothing is checked (S52)"
    );
    assert_eq!(diagnostics[0]["category"], "parse");
    assert_eq!(diagnostics[0]["span"]["file"], "src/reader/broken.cpl");
    assert_eq!(env["summary"]["errors"], 1);
    assert_eq!(env["summary"]["definitions"], 0);
    assert_eq!(env["summary"]["files"], 4);
    // A work file that does not parse is reported too (semantics S53).
    let fx = Fixture::new("check-work-parse-error");
    fx.write("reader/work/broken.work", "change {\n");
    let env = cpl(&fx.reader, &["check", "--json"]).json();
    let parse: Vec<&Value> = env["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["category"] == "parse")
        .collect();
    assert_eq!(parse.len(), 1);
    assert_eq!(parse[0]["span"]["file"], "work/broken.work");
    assert_eq!(env["status"], "unsuccessful");
}

#[test]
fn timeout_ms_budgets_each_body() {
    let dir = temp_dir("timeout");
    std::fs::write(
        dir.join("cpl.toml"),
        "[project]\nname = \"spin\"\nedition = \"0.1\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src/spin")).unwrap();
    std::fs::write(
        dir.join("src/spin/busy.cpl"),
        r#"module spin.busy

test "spins for a long time" {
    var i = 0
    while i < 1000000000
        decreases 1000000000 - i
    {
        i = i + 1
    }
}

test "returns at once" {
    assert 1 + 1 == 2
}
"#,
    )
    .unwrap();
    // Run the module only with a budget: without one the first test spins.
    let run = cpl(
        &dir,
        &["test", "--seed", "1", "--timeout-ms", "50", "--json"],
    );
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["failed"], 1);
    assert_eq!(env["tested"], 1);
    let spins = env["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["qualified_name"] == "spin.busy.\"spins for a long time\"")
        .expect("the spinning test")
        .clone();
    assert_eq!(spins["status"], "failed");
    assert_eq!(spins["fault"]["code"], "resource_exhaustion");
    assert_eq!(spins["fault"]["resource"], "time");
    assert_eq!(env["receipt"]["configuration"]["timeout_ms"], 50);
}

// ===========================================================================
// Helpers of the inspect, prove, diff, and work check tests
// ===========================================================================

/// `cpl <args> --json` in `dir`: the exit code and the envelope.
fn json_of(dir: &Path, args: &[&str]) -> (i32, Value) {
    let mut all = args.to_vec();
    all.push("--json");
    let run = cpl(dir, &all);
    (run.code, run.json())
}

/// The `path` of every entry of a list of symbol blocks or versions.
fn paths_of(list: &Value) -> Vec<String> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|b| b["path"].as_str().expect("path").to_string())
        .collect()
}

/// The entry with this path in a list of symbol blocks or versions.
#[track_caller]
fn block<'a>(list: &'a Value, path: &str) -> &'a Value {
    list.as_array()
        .expect("a list")
        .iter()
        .find(|b| b["path"] == path)
        .unwrap_or_else(|| panic!("no {path} in {list}"))
}

/// The keys of a JSON object, sorted.
fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The contract clauses of `paginate` as a symbol block shows them.
const PAGINATE_CONTRACT: [&str; 3] = [
    "ensures flatten(result) == items",
    "ensures forall page in result: page.len > 0 and page.len <= size",
    "ensures forall page in result.drop_last(): page.len == size",
];

/// Edit `paginate`'s second `ensures` clause: its contract facet changes,
/// no other facet of any declaration does.
fn edit_contract(fx: &Fixture) {
    let path = "reader/src/reader/pages.cpl";
    let before = fx.read(path);
    let after = before.replace(
        "page.len > 0 and page.len <= size",
        "page.len >= 1 and page.len <= size",
    );
    assert_ne!(before, after);
    fx.write(path, &after);
}

/// Make `divide_exactly` ill-typed: a check error in reader.pages.
fn break_reader(fx: &Fixture) {
    let path = "reader/src/reader/pages.cpl";
    let before = fx.read(path);
    let after = before.replace("    a / b\n", "    a / \"b\"\n");
    assert_ne!(before, after);
    fx.write(path, &after);
}

// ===========================================================================
// cpl inspect (Sections 6, 7.2)
// ===========================================================================

#[test]
fn inspect_default_response_of_paginate() {
    let Some(fx) = Fixture::committed("inspect-default") else {
        return;
    };
    let run = cpl(&fx.reader, &["inspect", "reader.pages.paginate", "--json"]);
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["command"], "inspect");
    assert_eq!(env["status"], "ok");
    assert_eq!(env["diagnostics"], json!([]));
    assert_eq!(env["revision"]["commit"], head(&fx));
    assert_eq!(env["revision"]["state"], "clean");
    assert_eq!(
        env["include"],
        json!([
            "signature",
            "contract",
            "effects",
            "docs",
            "interface_dependencies"
        ])
    );
    let symbol = &env["symbol"];
    assert_eq!(symbol["id"], "pgn_7c2d");
    assert_eq!(symbol["path"], "reader.pages.paginate");
    assert_eq!(symbol["provisional"], false);
    assert_eq!(
        keys(&symbol["facets"]),
        [
            "body",
            "contract",
            "docs",
            "effects",
            "identity",
            "name",
            "signature",
            "verification_policy"
        ]
    );
    assert!(symbol["definition"].is_string());
    assert_eq!(symbol["kind"], "fn");
    assert_eq!(symbol["module"], "reader.pages");
    assert_eq!(symbol["public"], true);
    assert_eq!(
        symbol["location"],
        json!({ "file": "src/reader/pages.cpl", "line": 43, "col": 1, "end_line": 67, "end_col": 2 })
    );
    assert_eq!(symbol["verification_policy"], json!({ "prove": true }));
    assert_eq!(
        symbol["signature"],
        "pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>"
    );
    assert_eq!(symbol["contract"], json!(PAGINATE_CONTRACT));
    assert_eq!(symbol["effects"], json!([]));
    assert_eq!(symbol["docs"], Value::Null);
    for absent in [
        "implementation",
        "holes",
        "relation",
        "interface",
        "call_sites",
    ] {
        assert!(symbol.get(absent).is_none(), "{absent}");
    }
    // The interface dependencies, each with the same four and its relation.
    let deps = &env["interface_dependencies"];
    let positive = block(deps, "reader.pages.Positive");
    assert_eq!(positive["relation"], "type_use");
    assert_eq!(positive["provisional"], true);
    assert_eq!(positive["kind"], "ty");
    assert_eq!(positive["signature"], "pub type Positive = Int");
    assert_eq!(positive["contract"], json!(["where self > 0"]));
    assert!(positive.get("verification_policy").is_none());
    let flatten = block(deps, "std.prelude.flatten");
    assert_eq!(flatten["relation"], "call");
    assert_eq!(flatten["location"]["file"], "std/prelude.cpl");
    assert_eq!(
        flatten["signature"],
        "pub fn flatten<T>(lists: List<List<T>>) -> List<T>"
    );
    for dep in deps.as_array().unwrap() {
        assert!(dep["signature"].is_string(), "{dep}");
        assert!(dep.get("docs").is_some(), "docs in {dep}");
        assert!(dep.get("interface").is_none(), "{dep}");
        assert!(dep["facets"].is_object() && dep["definition"].is_string());
    }
    // Lists are sorted by display path; body-only dependencies are not in it.
    let paths = paths_of(deps);
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    assert!(!paths.contains(&"std.prelude.List.take".to_string()));
    for absent in ["dependencies", "callers", "obligations", "tests", "changes"] {
        assert!(env.get(absent).is_none(), "{absent}");
    }
    check_json_golden("inspect_reader_paginate.json", &run, &fx, false);
}

#[test]
fn inspect_callers_is_the_example_of_section_6_4() {
    let Some(fx) = Fixture::committed("inspect-callers") else {
        return;
    };
    let run = cpl(
        &fx.reader,
        &[
            "inspect",
            "reader.pages.paginate",
            "--include",
            "signature,contract,effects,callers",
            "--json",
        ],
    );
    run.expect_code(0);
    let env = run.json();
    assert_eq!(
        env["include"],
        json!(["signature", "contract", "effects", "callers"])
    );
    // An explicit list without dependencies has no dependency list.
    assert!(env.get("interface_dependencies").is_none());
    assert!(env["symbol"].get("docs").is_none());
    assert_eq!(env["symbol"]["contract"], json!(PAGINATE_CONTRACT));
    let callers = &env["callers"];
    let mut expected = vec!["reader.pages.paginate_ints".to_string()];
    expected.extend(TESTS.iter().map(|t| t.to_string()));
    assert_eq!(paths_of(callers), expected);
    let ints = block(callers, "reader.pages.paginate_ints");
    assert_eq!(
        ints["call_sites"],
        json!([{ "file": "src/reader/pages.cpl", "line": 75, "col": 8, "end_line": 75, "end_col": 37 }])
    );
    for caller in callers.as_array().unwrap() {
        assert!(!caller["call_sites"].as_array().unwrap().is_empty());
        assert!(
            caller.get("signature").is_none(),
            "caller blocks carry no facet texts"
        );
    }
    let tail = block(callers, TESTS[1]);
    assert_eq!(tail["kind"], "test");
    assert_eq!(tail["call_sites"][0]["file"], "src/tests/pagination.cpl");
    check_json_golden("inspect_reader_paginate_callers.json", &run, &fx, false);
}

#[test]
fn inspect_targets_resolve_by_id_path_and_name() {
    let fx = Fixture::new("inspect-targets");
    for target in ["pgn_7c2d", "reader.pages.paginate", "paginate"] {
        let (code, env) = json_of(&fx.reader, &["inspect", target]);
        assert_eq!(code, 0, "{target}: {env}");
        assert_eq!(env["symbol"]["id"], "pgn_7c2d", "{target}");
    }
    // A provisional id is the qualified name.
    let (code, env) = json_of(&fx.reader, &["inspect", "reader.pages.Positive"]);
    assert_eq!(code, 0);
    let symbol = &env["symbol"];
    assert_eq!(
        (symbol["provisional"].clone(), symbol["kind"].clone()),
        (json!(true), json!("ty"))
    );
    assert_eq!(symbol["contract"], json!(["where self > 0"]));
    assert!(symbol.get("verification_policy").is_none(), "fn only");
    // 6.4 writes `effects` "when `effects` included" for every kind (only
    // `verification_policy` and `implementation` depend on the kind; 6.2:
    // the response shape is a function of the include set), so a type in
    // the default set has an empty list, and its `facets` show that it has
    // no `effects` facet (5.1).
    assert_eq!(symbol["effects"], json!([]), "included for every kind");
    assert!(
        symbol["facets"].get("effects").is_none(),
        "a type has no effects facet"
    );
    // A property, by its path, its quoted name, and its bare name text.
    for target in [
        TESTS[2],
        "\"pagination preserves input\"",
        "pagination preserves input",
    ] {
        let (code, env) = json_of(&fx.reader, &["inspect", target]);
        assert_eq!(code, 0, "{target}: {env}");
        assert_eq!(env["symbol"]["path"], TESTS[2]);
        assert_eq!(env["symbol"]["kind"], "prop");
    }
    // Std declarations are targets too.
    let (code, env) = json_of(&fx.reader, &["inspect", "std.text.trim"]);
    assert_eq!(code, 0);
    assert_eq!(env["symbol"]["module"], "std.text");
    assert!(env["symbol"]["location"]["file"]
        .as_str()
        .unwrap()
        .starts_with("std/"));
    // Unknown and ambiguous targets are answers: exit 1.
    let (code, env) = json_of(&fx.reader, &["inspect", "reader.pages.nope"]);
    assert_eq!(code, 1);
    assert_eq!(env["status"], "unsuccessful");
    assert_eq!(kinds(&env), pairs(&[("error", "index_unknown_symbol")]));
    assert!(env.get("symbol").is_none());
    let (code, env) = json_of(&fx.reader, &["inspect", "Positive"]);
    assert_eq!(code, 1);
    assert_eq!(kinds(&env), pairs(&[("error", "index_ambiguous_symbol")]));
    assert_eq!(
        env["diagnostics"][0]["candidates"],
        json!(["reader.pages.Positive", "std.gen.Positive"])
    );
}

#[test]
fn inspect_include_items() {
    let fx = Fixture::new("inspect-includes");
    // `dependencies`: interface and body, each with its interface flag;
    // dependency blocks carry their signature and contract.
    let (code, env) = json_of(
        &fx.reader,
        &[
            "inspect",
            "reader.pages.paginate",
            "--include",
            "dependencies",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(env["include"], json!(["dependencies"]));
    assert!(env["symbol"].get("signature").is_none());
    let deps = &env["dependencies"];
    let take = block(deps, "std.prelude.List.take");
    assert_eq!(
        (take["relation"].clone(), take["interface"].clone()),
        (json!("call"), json!(false))
    );
    assert!(take["signature"].is_string() && take["contract"].is_array());
    assert!(take.get("docs").is_none() && take.get("effects").is_none());
    let positive = block(deps, "reader.pages.Positive");
    assert_eq!(
        (positive["relation"].clone(), positive["interface"].clone()),
        (json!("type_use"), json!(true))
    );
    assert!(env.get("interface_dependencies").is_none());

    // `obligations` (Section 8.1).
    let (code, env) = json_of(
        &fx.reader,
        &["inspect", "paginate", "--include", "obligations"],
    );
    assert_eq!(code, 0);
    let first = &env["obligations"][0];
    assert_eq!(first["id"], "pgn_7c2d#ensures/1");
    assert_eq!(first["kind"], "ensures");
    assert_eq!(first["clause"], "flatten(result) == items");
    assert_eq!(
        first["location"],
        json!({ "file": "src/reader/pages.cpl", "line": 46, "col": 5, "end_line": 46, "end_col": 37 })
    );
    assert_eq!(
        (first["status"].clone(), first["reason"].clone()),
        (json!("unknown"), json!("verifier_unavailable"))
    );

    // `tests`: direct for paginate, transitive for what it calls.
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate", "--include", "tests"]);
    assert_eq!(code, 0);
    assert_eq!(paths_of(&env["tests"]), TESTS);
    assert!(env["tests"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["relation"] == "direct"));
    let (_, env) = json_of(
        &fx.reader,
        &["inspect", "std.prelude.List.take", "--include", "tests"],
    );
    assert_eq!(paths_of(&env["tests"]), TESTS);
    assert!(env["tests"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["relation"] == "transitive"));
    let (_, env) = json_of(
        &fx.reader,
        &["inspect", "paginate_ints", "--include", "tests"],
    );
    assert_eq!(env["tests"], json!([]));

    // `implementation`: the body's canonical text and the holes in it.
    let (_, env) = json_of(
        &fx.reader,
        &["inspect", "paginate", "--include", "implementation"],
    );
    let body = env["symbol"]["implementation"].as_str().expect("a body");
    assert!(body.starts_with('{') && body.contains("pages = pages.push(rest)"));
    assert_eq!(env["symbol"]["holes"], json!([]));

    // A repeated item counts once; the effective set is sorted.
    let (code, env) = json_of(
        &fx.reader,
        &["inspect", "paginate", "--include", "tests,signature,tests"],
    );
    assert_eq!(code, 0);
    assert_eq!(env["include"], json!(["signature", "tests"]));
}

#[test]
fn inspect_implementation_with_holes() {
    let Some(fx) = Fixture::committed("inspect-holes") else {
        return;
    };
    let run = cpl(
        &fx.holes,
        &[
            "inspect",
            "holes.pages.paginate",
            "--include",
            "signature,implementation",
            "--json",
        ],
    );
    run.expect_code(0);
    let env = run.json();
    let symbol = &env["symbol"];
    assert!(symbol["implementation"]
        .as_str()
        .unwrap()
        .contains("?build_pages"));
    let holes = symbol["holes"].as_array().expect("holes");
    assert_eq!(holes.len(), 1);
    assert_eq!(holes[0]["hole"], "build_pages");
    assert_eq!(holes[0]["expected_type"], "List<List<T>>");
    // The hole report carries its dependencies, as `cpl check` reports it.
    let deps = &holes[0]["dependencies"];
    assert_eq!(deps["source_revision"], head(&fx));
    assert_eq!(module_names(&deps["modules"]), ["holes.pages"]);
    let check = cpl(&fx.holes, &["check", "--json"]).json();
    let reported = with_code(&check, "hole")[0];
    assert_eq!(reported["hole"], "build_pages");
    assert_eq!(&reported["dependencies"], deps);
    assert_eq!(&check["holes"][0], &holes[0]);
    check_json_golden(
        "inspect_holes_paginate_implementation.json",
        &run,
        &fx,
        false,
    );
}

#[test]
fn inspect_changes_since_an_earlier_revision() {
    let Some(fx) = Fixture::committed("inspect-changes") else {
        return;
    };
    let first = head(&fx).to_string();
    // One commit: the default base HEAD~1 does not exist.
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate", "--include", "changes"]);
    assert_eq!(code, 2);
    assert_eq!(kinds(&env), pairs(&[("error", "git_unknown_revision")]));
    assert_eq!(env["diagnostics"][0]["revision"], "HEAD~1");
    edit_contract(&fx);
    let second = fx.commit_all("a contract edit");

    let (code, env) = json_of(&fx.reader, &["inspect", "paginate", "--include", "changes"]);
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["include"], json!(["changes"]));
    let changes = &env["changes"];
    assert_eq!(changes["since"]["commit"], first);
    assert_eq!(changes["since"]["state"], "clean");
    let statements = changes["statements"].as_array().expect("statements");
    assert_eq!(statements.len(), 1, "{statements:?}");
    assert_eq!(statements[0]["statement"], "contract_changed");
    assert_eq!(statements[0]["id"], "pgn_7c2d");
    assert_eq!(statements[0]["facet"], "contract");
    // `--since` names the base; only the target's statements are listed.
    let (_, env) = json_of(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "changes",
            "--since",
            &second,
        ],
    );
    assert_eq!(env["changes"]["since"]["commit"], second);
    assert_eq!(env["changes"]["statements"], json!([]));
    let (_, env) = json_of(
        &fx.reader,
        &[
            "inspect",
            "paginate_ints",
            "--include",
            "changes",
            "--since",
            &first,
        ],
    );
    assert_eq!(env["changes"]["statements"], json!([]));
    // A base that does not resolve is a git error (exit 2), and the
    // target is resolved first.
    let (code, env) = json_of(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "changes",
            "--since",
            "nope",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(kinds(&env), pairs(&[("error", "git_unknown_revision")]));
    let (code, env) = json_of(
        &fx.reader,
        &["inspect", "nope", "--include", "changes", "--since", "nope"],
    );
    assert_eq!(code, 1);
    assert_eq!(kinds(&env), pairs(&[("error", "index_unknown_symbol")]));
    // Without `changes`, `--since` has nothing to set.
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate", "--since", "nope"]);
    assert_eq!(code, 0);
    assert!(env.get("changes").is_none());

    // Every item at once.
    let run = cpl(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "signature,contract,effects,implementation,docs,callers,dependencies,obligations,tests,changes",
            "--json",
        ],
    );
    run.expect_code(0);
    let env = run.json();
    for key in ["dependencies", "callers", "obligations", "tests", "changes"] {
        assert!(env.get(key).is_some(), "{key}");
    }
    assert!(env.get("interface_dependencies").is_none());
    check_json_golden("inspect_reader_paginate_everything.json", &run, &fx, false);
}

#[test]
fn inspect_changes_needs_a_repository() {
    let fx = Fixture::new("inspect-changes-no-repo");
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate", "--include", "changes"]);
    assert_eq!(code, 2);
    assert_eq!(env["status"], "error");
    // Without a repository (with git installed), git_not_a_repository.
    if git_available() {
        assert_eq!(kinds(&env), pairs(&[("error", "git_not_a_repository")]));
    }
    // Without git on PATH, git cannot be run (git_command_failed).
    let empty = temp_dir("inspect-no-git-path");
    let run = cpl_env(
        &fx.reader,
        &["inspect", "paginate", "--include", "changes", "--json"],
        &[("PATH", empty.to_str().unwrap())],
    );
    run.expect_code(2);
    assert_eq!(
        kinds(&run.json()),
        pairs(&[("error", "git_command_failed")])
    );
    // Every other include works without one.
    let (code, _) = json_of(&fx.reader, &["inspect", "paginate", "--include", "callers"]);
    assert_eq!(code, 0);
}

#[test]
fn inspect_refuses_a_bad_include_list() {
    let fx = Fixture::new("inspect-bad-include");
    for list in [
        "foo",
        "signature,,contract",
        "signature, contract",
        "interface_dependencies",
        "",
        "signature,",
    ] {
        let (code, env) = json_of(&fx.reader, &["inspect", "paginate", "--include", list]);
        assert_eq!(code, 2, "{list:?}");
        assert_eq!(env["status"], "error");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "usage_bad_argument")]),
            "{list:?}"
        );
        assert_eq!(env["diagnostics"][0]["include"], list);
    }
}

#[test]
fn inspect_answers_with_index_partial_when_the_project_has_errors() {
    let fx = Fixture::new("inspect-partial");
    break_reader(&fx);
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate", "--include", "callers"]);
    assert_eq!(code, 0);
    assert_eq!(env["status"], "ok");
    assert_eq!(kinds(&env), pairs(&[("warning", "index_partial")]));
    assert_eq!(env["diagnostics"][0]["category"], "index");
    assert!(env["callers"].is_array());
    // A file that does not parse leaves no definitions to inspect.
    fx.write(
        "reader/src/reader/broken.cpl",
        "module reader.broken\n\nfn f( {\n",
    );
    let (code, env) = json_of(&fx.reader, &["inspect", "paginate"]);
    assert_eq!(code, 1);
    assert_eq!(
        kinds(&env),
        pairs(&[
            ("error", "index_unknown_symbol"),
            ("warning", "index_partial")
        ])
    );
}

#[test]
fn inspect_human_output_has_labelled_sections() {
    let fx = Fixture::new("inspect-human");
    let (_, env) = json_of(&fx.reader, &["inspect", "reader.pages.paginate"]);
    let run = cpl(&fx.reader, &["inspect", "reader.pages.paginate"]);
    run.expect_code(0);
    assert_eq!(run.stderr, "");
    let lines: Vec<&str> = run.stdout.lines().collect();
    let sections: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.starts_with(' '))
        .collect();
    assert_eq!(
        sections,
        [
            "revision",
            "symbol reader.pages.paginate",
            "signature",
            "contract",
            "effects",
            "docs",
            "interface dependencies"
        ]
    );
    // The revision the versions belong to.
    assert_eq!(lines[1], "  commit      none");
    assert_eq!(lines[2], "  state       worktree");
    assert_eq!(
        lines[3],
        format!(
            "  digest      {}",
            env["revision"]["digest"].as_str().unwrap()
        )
    );
    for line in [
        "  id          pgn_7c2d",
        "  kind        fn",
        "  module      reader.pages",
        "  location    src/reader/pages.cpl:43:1-67:2",
        "  prove       true",
        "  pub fn paginate<T>(items: List<T>, size: Positive) -> List<List<T>>",
        "  ensures flatten(result) == items",
        "  reader.pages.Positive  (type_use)",
        "    id          reader.pages.Positive  (provisional)",
        "      where self > 0",
        "  std.prelude.flatten  (call)",
    ] {
        assert!(lines.contains(&line), "{line:?} in\n{}", run.stdout);
    }
    // One facet hash table per symbol, with the JSON's hashes.
    let tables = lines.iter().filter(|l| l.trim() == "facets").count();
    let deps = env["interface_dependencies"].as_array().unwrap().len();
    assert_eq!(tables, 1 + deps);
    let identity = env["symbol"]["facets"]["identity"].as_str().unwrap();
    assert!(lines.contains(&format!("    identity             {identity}").as_str()));
    let definition = env["symbol"]["definition"].as_str().unwrap();
    assert!(lines.contains(&format!("    definition           {definition}").as_str()));

    let run = cpl(
        &fx.reader,
        &[
            "inspect",
            "paginate",
            "--include",
            "callers,obligations,tests",
        ],
    );
    run.expect_code(0);
    let lines: Vec<&str> = run.stdout.lines().collect();
    let sections: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.starts_with(' '))
        .collect();
    assert_eq!(
        sections,
        [
            "revision",
            "symbol reader.pages.paginate",
            "callers",
            "obligations",
            "tests"
        ]
    );
    for line in [
        "  reader.pages.paginate_ints",
        "    call sites",
        "      src/reader/pages.cpl:75:8-37",
        "  pgn_7c2d#ensures/1  ensures  unknown (verifier_unavailable)",
        "    clause    flatten(result) == items",
        "    location  src/reader/pages.cpl:46:5-37",
        "  tests.pagination.\"pagination preserves input\"  (direct)",
    ] {
        assert!(lines.contains(&line), "{line:?} in\n{}", run.stdout);
    }

    // Diagnostics go to stderr with their stable lines.
    let run = cpl(&fx.reader, &["inspect", "reader.pages.nope"]);
    run.expect_code(1);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr
            .starts_with("error[index_unknown_symbol]: no declaration `reader.pages.nope`"),
        "{}",
        run.stderr
    );
    let fx = Fixture::new("inspect-human-holes");
    let run = cpl(
        &fx.holes,
        &[
            "inspect",
            "holes.pages.summarize",
            "--include",
            "implementation",
        ],
    );
    run.expect_code(0);
    assert!(
        run.stdout.contains(
            "implementation\n  {\n      ?summary\n  }\nholes\n  ?summary: Text\n    bindings\n      page: Page  (parameter)\n"
        ),
        "{}",
        run.stdout
    );
    // The hole report's dependencies, as `cpl check` reports them.
    let lines: Vec<&str> = run.stdout.lines().collect();
    for line in [
        "    dependencies",
        "      source      unversioned",
        "      modules",
        "      symbols",
    ] {
        assert!(lines.contains(&line), "{line:?} in\n{}", run.stdout);
    }
    assert!(lines
        .iter()
        .any(|l| l.starts_with("        holes.pages.Page  ")));
}

// ===========================================================================
// cpl prove (Sections 7.2, 8.1)
// ===========================================================================

#[test]
fn prove_paginate_is_unknown_in_milestone_1() {
    let Some(fx) = Fixture::committed("prove-paginate") else {
        return;
    };
    let run = cpl(&fx.reader, &["prove", "reader.pages.paginate", "--json"]);
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["command"], "prove");
    // T44: the envelope says unsuccessful, the result says unknown.
    assert_eq!(env["status"], "unsuccessful");
    assert_eq!(env["result"], "unknown");
    let symbol = &env["symbol"];
    assert_eq!(
        keys(symbol),
        ["definition", "facets", "id", "path", "provisional"],
        "a SymbolVersion, not a symbol block"
    );
    assert_eq!(symbol["id"], "pgn_7c2d");
    let obligations = env["obligations"].as_array().expect("obligations");
    let ensures: Vec<String> = obligations
        .iter()
        .filter(|o| o["kind"] == "ensures")
        .map(|o| format!("ensures {}", o["clause"].as_str().expect("a clause")))
        .collect();
    assert_eq!(ensures, PAGINATE_CONTRACT);
    for o in obligations {
        assert!(o["id"].as_str().unwrap().starts_with("pgn_7c2d#"), "{o}");
        assert_eq!(o["status"], "unknown", "{o}");
        assert_eq!(o["reason"], "verifier_unavailable", "{o}");
        assert_eq!(o["evidence"], Value::Null);
        assert_eq!(o["assumptions"], json!([]));
    }
    // The receipt of Section 8.1.
    let receipt = &env["receipt"];
    assert_eq!(receipt["receipt"], "verification");
    assert_eq!(receipt["schema"], 1);
    assert_eq!(
        receipt["verifier"],
        json!({ "name": "none", "version": null, "configuration": {} })
    );
    assert_eq!(receipt["revision"], env["revision"]);
    assert_eq!(&receipt["target"], symbol);
    assert_eq!(receipt["status"], "unknown");
    assert_eq!(&receipt["obligations"], &env["obligations"]);
    assert_eq!(receipt["assumptions"], json!([]));
    assert_eq!(
        receipt["configuration"],
        json!({ "release": false, "policy": { "prove": true }, "proof_timeout_ms": 10000, "admitted": [] })
    );
    assert_eq!(receipt["inputs"]["lock"], Value::Null);
    let sources: Vec<String> = keys(&receipt["inputs"]["sources"]);
    let mut expected: Vec<String> = READER_FILES.iter().map(|f| f.to_string()).collect();
    expected.push("cpl.toml".to_string());
    expected.sort();
    assert_eq!(sources, expected);
    let dependencies = paths_of(&receipt["dependencies"]);
    for dep in [
        "reader.pages.Positive",
        "std.prelude.flatten",
        "std.prelude.List.take",
    ] {
        assert!(dependencies.contains(&dep.to_string()), "{dependencies:?}");
    }
    let core = &receipt["core"];
    assert_eq!(core["source_revision"], head(&fx));
    assert_eq!(core["check_configuration"], json!({ "release": false }));
    assert_eq!(module_names(&core["modules"]), ["reader.pages"]);
    // One verification_result, whose receipt is the receipt's core (S59).
    assert_eq!(kinds(&env), pairs(&[("info", "verification_result")]));
    let d = &env["diagnostics"][0];
    assert_eq!(d["symbol"], "pgn_7c2d");
    assert_eq!(d["qualified_name"], "reader.pages.paginate");
    assert_eq!(d["facet"], "verification_policy");
    assert_eq!(d["status"], "unknown");
    assert_eq!(d["reason"], "verifier_unavailable");
    assert_eq!(&d["receipt"], core);
    assert_eq!(
        (d["span"]["line"].clone(), d["span"]["col"].clone()),
        (json!(45), json!(8))
    );
    check_json_golden("prove_reader_paginate.json", &run, &fx, false);
}

#[test]
fn prove_targets_and_refusals() {
    let fx = Fixture::new("prove-targets");
    for target in ["pgn_7c2d", "paginate"] {
        let (code, env) = json_of(&fx.reader, &["prove", target]);
        assert_eq!(code, 1, "{target}");
        assert_eq!(env["symbol"]["id"], "pgn_7c2d");
    }
    // A function without @prove has its verification status too: unknown,
    // with the policy it has.
    let (code, env) = json_of(&fx.reader, &["prove", "reader.pages.clean"]);
    assert_eq!(code, 1);
    assert_eq!(env["status"], "unsuccessful");
    assert_eq!(env["result"], "unknown");
    assert_eq!(env["symbol"]["provisional"], true);
    assert_eq!(
        env["receipt"]["configuration"]["policy"],
        json!({ "prove": false })
    );
    assert_eq!(kinds(&env), pairs(&[("info", "verification_result")]));
    // A std function is a function too.
    let (code, env) = json_of(&fx.reader, &["prove", "std.text.trim"]);
    assert_eq!(code, 1);
    assert_eq!(env["result"], "unknown");
    // Declarations that are not functions: usage_not_provable, exit 2.
    for target in [
        "reader.pages.Positive",
        "reader.pages.Page",
        "reader.pages.StoreError",
        "reader.pages.PageRead",
        TESTS[2],
    ] {
        let (code, env) = json_of(&fx.reader, &["prove", target]);
        assert_eq!(code, 2, "{target}");
        assert_eq!(env["status"], "error");
        assert_eq!(kinds(&env), pairs(&[("error", "usage_not_provable")]));
        assert_eq!(env["diagnostics"][0]["category"], "usage");
        assert!(env.get("result").is_none());
    }
    // Targets that name no function: exit 2, as for `cpl run`.
    let (code, env) = json_of(&fx.reader, &["prove", "reader.pages.nope"]);
    assert_eq!(code, 2);
    assert_eq!(kinds(&env), pairs(&[("error", "index_unknown_symbol")]));
    let (code, env) = json_of(&fx.reader, &["prove", "Positive"]);
    assert_eq!(code, 2);
    assert_eq!(kinds(&env), pairs(&[("error", "index_ambiguous_symbol")]));
}

#[test]
fn prove_needs_a_project_that_checks() {
    let fx = Fixture::new("prove-check-error");
    break_reader(&fx);
    let (code, env) = json_of(&fx.reader, &["prove", "paginate"]);
    assert_eq!(code, 1);
    assert_eq!(env["status"], "unsuccessful");
    assert!(env.get("result").is_none(), "nothing is proved");
    assert!(kinds(&env).contains(&("error".to_string(), "type_mismatch".to_string())));
    assert!(with_code(&env, "verification_result").is_empty());
}

#[test]
fn prove_human_output() {
    let fx = Fixture::new("prove-human");
    let (_, env) = json_of(&fx.reader, &["prove", "reader.pages.paginate"]);
    let run = cpl(&fx.reader, &["prove", "reader.pages.paginate"]);
    run.expect_code(1);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", run.stdout);
    assert_eq!(lines[0], "prove reader.pages.paginate (pgn_7c2d): unknown");
    let n = env["obligations"].as_array().unwrap().len();
    assert_eq!(
        lines[1],
        format!("  {n} obligations: 0 proved, 0 failed, {n} unknown (verifier unavailable in cpl 0.1.0)")
    );
    // The receipt is named by its digest, which leaves out `produced_at`
    // and `duration_ms`: two runs on one revision give the same name.
    let digest = cpl_index::receipt_digest(&env["receipt"]);
    assert_eq!(
        lines[2],
        format!("  receipt: {digest}  (blake3 of the receipt)")
    );
    assert!(
        run.stderr.starts_with(
            "info[verification_result]: paginate: unknown (verifier unavailable)\n  --> src/reader/pages.cpl:45:8\n"
        ),
        "{}",
        run.stderr
    );
    let run = cpl(&fx.reader, &["prove", "reader.pages.clean"]);
    run.expect_code(1);
    assert!(run
        .stdout
        .starts_with("prove reader.pages.clean (reader.pages.clean): unknown\n"));
    let run = cpl(&fx.reader, &["prove", "reader.pages.Positive"]);
    run.expect_code(2);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr.starts_with("error[usage_not_provable]: "),
        "{}",
        run.stderr
    );
}

#[test]
fn evidence_agrees_across_commands() {
    let Some(fx) = Fixture::committed("evidence") else {
        return;
    };
    // `cpl check` and `cpl prove` attach the same verification receipt core.
    let check = cpl(&fx.reader, &["check", "--json"]).json();
    let prove = cpl(&fx.reader, &["prove", "paginate", "--json"]).json();
    let verification = with_code(&check, "verification_result")[0];
    assert_eq!(verification["receipt"], prove["receipt"]["core"]);
    assert_eq!(
        &verification["receipt"],
        &prove["diagnostics"][0]["receipt"]
    );
    // `cpl inspect` and `cpl prove` see the same versions.
    let inspect = cpl(&fx.reader, &["inspect", "paginate", "--json"]).json();
    for key in ["id", "path", "provisional", "facets", "definition"] {
        assert_eq!(inspect["symbol"][key], prove["symbol"][key], "{key}");
    }
    let flatten = block(&inspect["interface_dependencies"], "std.prelude.flatten");
    let in_receipt = block(&prove["receipt"]["dependencies"], "std.prelude.flatten");
    assert_eq!(flatten["facets"], in_receipt["facets"]);
    assert_eq!(flatten["definition"], in_receipt["definition"]);
}

// ===========================================================================
// cpl diff (Sections 7.2, 9)
// ===========================================================================

#[test]
fn diff_of_a_contract_edit_is_contract_changed() {
    let Some(fx) = Fixture::committed("diff-contract") else {
        return;
    };
    let first = head(&fx).to_string();
    edit_contract(&fx);
    let second = fx.commit_all("a contract edit");
    let ranges = [
        format!("{first}..{second}"),
        "HEAD~1..HEAD".to_string(),
        "main~1..main".to_string(),
        "HEAD~1...HEAD".to_string(),
        "HEAD~1".to_string(),
    ];
    for range in &ranges {
        let run = cpl(&fx.reader, &["diff", range, "--json"]);
        run.expect_code(0);
        let env = run.json();
        assert_eq!(env["command"], "diff");
        assert_eq!(env["status"], "ok", "{range}");
        assert_eq!(env["diagnostics"], json!([]));
        assert_eq!(env["old"]["commit"], first, "{range}");
        assert_eq!(env["old"]["state"], "clean");
        assert_eq!(env["new"]["commit"], second, "{range}");
        let statements = env["statements"].as_array().expect("statements");
        assert_eq!(statements.len(), 1, "{range}: {statements:?}");
        let s = &statements[0];
        assert_eq!(s["statement"], "contract_changed");
        assert_eq!(s["id"], "pgn_7c2d");
        assert_eq!(s["path"], "reader.pages.paginate");
        assert_eq!(s["kind"], "fn");
        assert_eq!(s["facet"], "contract");
        assert_eq!(keys(&s["detail"]), ["new", "old"]);
        assert_ne!(s["detail"]["old"], s["detail"]["new"]);
        assert_eq!(env["duplicates"], json!({ "old": [], "new": [] }));
    }
    // The old side's contract hash is the first commit's.
    let detail = cpl(&fx.reader, &["diff", "HEAD~1..HEAD", "--json"]).json()["statements"][0]
        ["detail"]
        .clone();
    let now = cpl(&fx.reader, &["inspect", "paginate", "--json"]).json();
    assert_eq!(detail["new"], now["symbol"]["facets"]["contract"]);
    // Against the working tree (clean here), the new side is its revision.
    let env = cpl(&fx.reader, &["diff", "HEAD~1", "--json"]).json();
    assert_eq!(env["new"], env["revision"]);
    // --semantic is accepted; --exit-code makes a non-empty diff exit 1.
    cpl(&fx.reader, &["diff", "HEAD~1..HEAD", "--semantic"]).expect_code(0);
    let run = cpl(
        &fx.reader,
        &["diff", "HEAD~1..HEAD", "--exit-code", "--json"],
    );
    run.expect_code(1);
    assert_eq!(run.json()["status"], "unsuccessful");
    let run = cpl(&fx.reader, &["diff", "HEAD..HEAD", "--exit-code"]);
    run.expect_code(0);
    assert_eq!(run.stdout, "");
    let run = cpl(&fx.reader, &["diff", "HEAD~1..HEAD", "--json"]);
    check_json_golden("diff_contract_changed.json", &run, &fx, false);
}

#[test]
fn diff_against_the_working_tree_and_its_human_lines() {
    let Some(fx) = Fixture::committed("diff-worktree") else {
        return;
    };
    let pages = "reader/src/reader/pages.cpl";
    let text = fx
        .read(pages)
        .replace("pub fn paginate<T>(", "pub fn pages_of<T>(")
        .replace("    a / b\n", "    a / b + 0\n");
    fx.write(pages, &text);
    let tests = "reader/src/tests/pagination.cpl";
    let text = format!(
        "{}\ntest \"pagination keeps one page\" {{\n    assert paginate([1], 1) == [[1]]\n}}\n",
        fx.read(tests)
    );
    fx.write(tests, &text);
    const KEEPS: &str = "tests.pagination.\"pagination keeps one page\"";

    let run = cpl(&fx.reader, &["diff", "HEAD", "--json"]);
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["old"]["commit"], head(&fx));
    assert_eq!(env["new"]["commit"], head(&fx));
    assert_eq!(env["new"]["state"], "worktree");
    assert_eq!(env["new"], env["revision"]);
    let statements: Vec<(&str, &str)> = env["statements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["statement"].as_str().unwrap(),
                s["path"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        statements,
        [
            ("body_changed", "reader.pages.divide_exactly"),
            ("identity_renamed", "reader.pages.pages_of"),
            ("identity_added", KEEPS),
        ]
    );
    let renamed = &env["statements"][1];
    assert_eq!(renamed["id"], "pgn_7c2d");
    assert_eq!(renamed["facet"], "name");
    assert_eq!(
        renamed["detail"],
        json!({ "from": "reader.pages.paginate", "to": "reader.pages.pages_of" })
    );
    let added = &env["statements"][2];
    assert_eq!(added["facet"], Value::Null);
    assert_eq!(added["kind"], "test");
    assert_eq!(added["detail"]["version"]["id"], KEEPS);
    assert_eq!(added["detail"]["version"]["provisional"], true);

    let run = cpl(&fx.reader, &["diff", "HEAD"]);
    run.expect_code(0);
    assert_eq!(run.stderr, "");
    assert_eq!(
        run.stdout,
        format!(
            "body_changed  reader.pages.divide_exactly  (reader.pages.divide_exactly)\n\
             identity_renamed  reader.pages.pages_of  (pgn_7c2d)  was reader.pages.paginate\n\
             identity_added  {KEEPS}  ({KEEPS})\n"
        )
    );
}

#[test]
fn diff_from_the_merge_base() {
    let Some(fx) = Fixture::committed("diff-merge-base") else {
        return;
    };
    let base = head(&fx).to_string();
    fx.git(&["branch", "feature"]);
    let pages = "reader/src/reader/pages.cpl";
    let text = fx.read(pages).replace("    a / b\n", "    a / b + 0\n");
    fx.write(pages, &text);
    let main = fx.commit_all("main: a body edit");
    fx.git(&["checkout", "-q", "feature"]);
    edit_contract(&fx);
    let feature = fx.commit_all("feature: a contract edit");
    // A...B: from the merge base to B; main's own edit is not in it.
    let env = cpl(&fx.reader, &["diff", "main...feature", "--json"]).json();
    assert_eq!(env["old"]["commit"], base);
    assert_eq!(env["new"]["commit"], feature);
    let names: Vec<&Value> = env["statements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["statement"])
        .collect();
    assert_eq!(names, [&json!("contract_changed")]);
    // A..B: from A to B, so main's edit appears too.
    let env = cpl(&fx.reader, &["diff", "main..feature", "--json"]).json();
    assert_eq!(env["old"]["commit"], main);
    let found: Vec<(&Value, &Value)> = env["statements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (&s["statement"], &s["path"]))
        .collect();
    assert_eq!(
        found,
        [
            (
                &json!("body_changed"),
                &json!("reader.pages.divide_exactly")
            ),
            (&json!("contract_changed"), &json!("reader.pages.paginate")),
        ]
    );
}

#[test]
fn diff_refuses_unknown_revisions() {
    let Some(fx) = Fixture::committed("diff-unknown") else {
        return;
    };
    for range in [
        "nope",
        "HEAD~1",
        "HEAD@{5}",
        "nope..HEAD",
        "HEAD..nope",
        "nope...HEAD",
        "HEAD...nope",
        "HEAD..",
        "",
        "-x",
    ] {
        let run = cpl(&fx.reader, &["diff", "--json", "--", range]);
        run.expect_code(2);
        let env = run.json();
        assert_eq!(env["status"], "error", "{range:?}");
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "git_unknown_revision")]),
            "{range:?}"
        );
        assert_eq!(env["diagnostics"][0]["category"], "git");
        assert!(env.get("statements").is_none());
    }
    // The names are resolved in the order written: the first unknown one is reported.
    let env = cpl(&fx.reader, &["diff", "nope...nada", "--json"]).json();
    assert_eq!(env["diagnostics"][0]["revision"], "nope");
    let env = cpl(&fx.reader, &["diff", "HEAD..nada", "--json"]).json();
    assert_eq!(env["diagnostics"][0]["revision"], "nada");
    // Two commits without a common ancestor: git_command_failed.
    fx.git(&["checkout", "-q", "--orphan", "lonely"]);
    fx.commit_all("an unrelated history");
    let run = cpl(&fx.reader, &["diff", "main...lonely", "--json"]);
    run.expect_code(2);
    assert_eq!(
        kinds(&run.json()),
        pairs(&[("error", "git_command_failed")])
    );
    // Between them, A..B works (both are commits of the project).
    cpl(&fx.reader, &["diff", "main..lonely"]).expect_code(0);
    let run = cpl(&fx.reader, &["diff", "nope"]);
    run.expect_code(2);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr
            .starts_with("error[git_unknown_revision]: unknown revision `nope`"),
        "{}",
        run.stderr
    );
}

#[test]
fn diff_of_a_commit_without_the_project_is_no_project_at_revision() {
    if !git_available() {
        return;
    }
    // A repository whose first commit predates the project.
    let fx = Fixture::new("diff-no-project");
    fx.git(&["init", "-q", "-b", "main"]);
    fx.write("README", "the project arrives in the next commit\n");
    fx.git(&["add", "README"]);
    fx.git(&["commit", "-q", "--no-verify", "-m", "readme"]);
    let readme = fx.git(&["rev-parse", "HEAD"]);
    fx.commit_all("examples");
    for range in ["HEAD~1", "HEAD~1..HEAD", "HEAD...HEAD~1"] {
        let run = cpl(&fx.reader, &["diff", range, "--json"]);
        run.expect_code(2);
        let env = run.json();
        assert_eq!(
            kinds(&env),
            pairs(&[("error", "git_no_project_at_revision")]),
            "{range}"
        );
        let d = &env["diagnostics"][0];
        assert_eq!(d["category"], "git");
        assert_eq!(d["commit"], readme, "{range}");
    }
    let env = cpl(&fx.reader, &["diff", "HEAD~1", "--json"]).json();
    assert_eq!(env["diagnostics"][0]["revision"], "HEAD~1");
    let run = cpl(&fx.reader, &["diff", "HEAD~1"]);
    assert!(
        run.stderr.starts_with(&format!(
            "error[git_no_project_at_revision]: revision `HEAD~1` ({readme}) has no cpl.toml at the project's path\n"
        )),
        "{}",
        run.stderr
    );
    // A commit whose manifest does not load: the manifest's error, naming
    // the revision.
    let manifest = fx.read("reader/cpl.toml");
    fx.write("reader/cpl.toml", "[project\n");
    let broken = fx.commit_all("a broken manifest");
    fx.write("reader/cpl.toml", &manifest);
    let run = cpl(&fx.reader, &["diff", "HEAD", "--json"]);
    run.expect_code(2);
    let env = run.json();
    assert_eq!(kinds(&env), pairs(&[("error", "project_manifest_invalid")]));
    assert_eq!(env["diagnostics"][0]["revision"], "HEAD");
    assert_eq!(env["diagnostics"][0]["commit"], broken);
}

#[test]
fn diff_needs_a_repository() {
    let fx = Fixture::new("diff-no-repo");
    if git_available() {
        let run = cpl(&fx.reader, &["diff", "HEAD", "--json"]);
        run.expect_code(2);
        let env = run.json();
        assert_eq!(kinds(&env), pairs(&[("error", "git_not_a_repository")]));
        assert_eq!(env["revision"]["commit"], Value::Null);
        let run = cpl(&fx.reader, &["diff", "HEAD"]);
        run.expect_code(2);
        assert!(run.stderr.starts_with("error[git_not_a_repository]: "));
    }
    let empty = temp_dir("diff-no-git-path");
    let run = cpl_env(
        &fx.reader,
        &["diff", "HEAD", "--json"],
        &[("PATH", empty.to_str().unwrap())],
    );
    run.expect_code(2);
    assert_eq!(
        kinds(&run.json()),
        pairs(&[("error", "git_command_failed")])
    );
}

#[test]
fn diff_lists_ids_declared_twice() {
    let Some(fx) = Fixture::committed("diff-duplicates") else {
        return;
    };
    let pages = "reader/src/reader/pages.cpl";
    let text = format!(
        "{}\n@id(\"pgn_7c2d\")\nfn twin() -> Int {{\n    1\n}}\n",
        fx.read(pages)
    );
    fx.write(pages, &text);
    let env = cpl(&fx.reader, &["diff", "HEAD", "--json"]).json();
    assert_eq!(env["status"], "ok");
    assert_eq!(env["duplicates"]["old"], json!([]));
    assert_eq!(
        env["duplicates"]["new"],
        json!([{ "id": "pgn_7c2d", "files": ["src/reader/pages.cpl", "src/reader/pages.cpl"] }])
    );
    let run = cpl(&fx.reader, &["diff", "HEAD"]);
    run.expect_code(0);
    assert!(
        run.stdout.contains(
            "note: pgn_7c2d is declared more than once in the new revision (src/reader/pages.cpl, src/reader/pages.cpl); its statements describe the first\n"
        ),
        "{}",
        run.stdout
    );
}

// ===========================================================================
// cpl work check (Section 7.2)
// ===========================================================================

#[test]
fn work_check_pagination_work() {
    let Some(fx) = Fixture::committed("work-check") else {
        return;
    };
    let run = cpl(
        &fx.reader,
        &["work", "check", "work/pagination.work", "--json"],
    );
    run.expect_code(0);
    let env = run.json();
    assert_eq!(env["command"], "work check");
    assert_eq!(env["status"], "ok");
    assert_eq!(env["file"], "work/pagination.work");
    // In a repository base and target resolve, so the one warning is
    // Positive's provisional identity.
    assert_eq!(
        kinds(&env),
        pairs(&[("warning", "work_provisional_identity")])
    );
    let d = &env["diagnostics"][0];
    assert_eq!(d["symbol"], "reader.pages.Positive");
    assert_eq!(d["span"]["file"], "work/pagination.work");
    let change = &env["changes"][0];
    assert_eq!(env["changes"].as_array().map(Vec::len), Some(1));
    assert_eq!(change["name"], "pagination_update");
    assert_eq!(
        (
            change["base"].clone(),
            change["target"].clone(),
            change["workers"].clone()
        ),
        (json!("main"), json!("main"), json!(3))
    );
    assert_eq!(change["base_commit"], head(&fx));
    let tasks: Vec<&Value> = change["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| &t["name"])
        .collect();
    assert_eq!(
        tasks,
        [
            &json!("implementation"),
            &json!("examples"),
            &json!("audit")
        ]
    );
    let implementation = &change["tasks"][0];
    assert_eq!(
        implementation["write"],
        json!([{ "selector": "paginate.body", "targets": [{ "id": "pgn_7c2d", "facet": "body" }] }])
    );
    assert_eq!(
        implementation["depend"][1],
        json!({ "selector": "Positive.definition", "targets": [{ "id": "reader.pages.Positive", "facet": "definition" }] })
    );
    assert_eq!(
        implementation["goal"],
        "Implement pagination and discharge its proof obligations."
    );
    assert_eq!(
        change["tasks"][1]["add"],
        json!([{ "selector": "tests.pagination.*", "namespace": "tests.pagination" }])
    );
    assert_eq!(
        change["tasks"][2]["after"],
        json!(["implementation", "examples"])
    );
    assert_eq!(
        change["integrate"],
        json!([
            { "require": "typecheck" },
            { "require": "prove", "targets": [{ "id": "pgn_7c2d", "facet": "definition" }] },
            { "require": "test", "namespace": "tests.pagination", "matched": 3 },
            { "require": "review", "task": "audit" },
            { "require": "no_holes" }
        ])
    );
    check_json_golden("work_check_pagination.json", &run, &fx, false);
}

#[test]
fn work_check_without_a_repository_or_git_warns_git_unavailable() {
    let fx = Fixture::new("work-check-unversioned");
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_eq!(code, 0);
    assert_eq!(
        kinds(&env),
        pairs(&[
            ("warning", "git_unavailable"),
            ("warning", "work_provisional_identity")
        ])
    );
    let git = with_code(&env, "git_unavailable")[0];
    let expected = if git_available() {
        "no_repository"
    } else {
        "not_installed"
    };
    assert_eq!(git["reason"], expected);
    assert_eq!(git["category"], "git");
    assert!(git.get("span").is_none());
    assert!(env["changes"][0].get("base_commit").is_none());
    let empty = temp_dir("work-check-no-git-path");
    let run = cpl_env(
        &fx.reader,
        &["work", "check", "work/pagination.work", "--json"],
        &[("PATH", empty.to_str().unwrap())],
    );
    run.expect_code(0);
    let env = run.json();
    assert_eq!(
        with_code(&env, "git_unavailable")[0]["reason"],
        "not_installed"
    );
}

#[test]
fn work_check_overlapping_writes() {
    let fx = Fixture::new("work-check-overlap");
    let path = "reader/work/pagination.work";
    let before = fx.read(path);
    let after = before.replace(
        "        add { tests.pagination.* }\n",
        "        add { tests.pagination.* }\n        write { paginate.body }\n",
    );
    assert_ne!(before, after);
    fx.write(path, &after);
    let run = cpl(
        &fx.reader,
        &["work", "check", "work/pagination.work", "--json"],
    );
    run.expect_code(1);
    let env = run.json();
    assert_eq!(env["status"], "unsuccessful");
    let overlap = with_code(&env, "work_overlapping_writes");
    assert_eq!(overlap.len(), 1);
    assert_eq!(overlap[0]["severity"], "error");
    assert_eq!(overlap[0]["category"], "work");
    assert_eq!(overlap[0]["span"]["file"], "work/pagination.work");
    assert_eq!(
        kinds(&env)
            .iter()
            .filter(|(severity, _)| severity == "error")
            .count(),
        1
    );
    // The output still describes the file.
    assert_eq!(
        env["changes"][0]["tasks"][1]["write"][0]["selector"],
        "paginate.body"
    );
    let run = cpl(&fx.reader, &["work", "check", "work/pagination.work"]);
    run.expect_code(1);
    assert_eq!(
        run.stdout,
        "work check: work/pagination.work: 1 change, 3 tasks, 1 error\n"
    );
    assert!(
        run.stderr.contains(
            "error[work_overlapping_writes]: tasks `implementation` and `examples` are unordered"
        ),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("\n  --> work/pagination.work:22:17\n"),
        "{}",
        run.stderr
    );
}

#[test]
fn work_check_of_files_outside_the_source_set() {
    let fx = Fixture::new("work-check-outside");
    let text = fx.read("reader/work/pagination.work");
    let reader = fx.reader.to_str().unwrap();
    // Outside the project: shown relative to the current directory.
    fx.write("plans/pagination.work", &text);
    let (code, env) = json_of(
        &fx.root,
        &[
            "work",
            "check",
            "plans/pagination.work",
            "--project",
            reader,
        ],
    );
    assert_eq!(code, 0, "{env}");
    assert_eq!(env["file"], "plans/pagination.work");
    let d = with_code(&env, "work_provisional_identity")[0];
    assert_eq!(d["span"]["file"], "plans/pagination.work");
    assert_eq!(d["span"]["line"], 10);
    // Inside the project but not in the source set (work/ is not recursive).
    fx.write("reader/work/drafts/next.work", &text);
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/drafts/next.work"]);
    assert_eq!(code, 0);
    assert_eq!(env["file"], "work/drafts/next.work");
    // A relative path from a subdirectory names the source-set file.
    let (code, env) = json_of(
        &fx.reader.join("src"),
        &["work", "check", "../work/pagination.work"],
    );
    assert_eq!(code, 0);
    assert_eq!(env["file"], "work/pagination.work");
    // An outside file that does not parse.
    fx.write("plans/broken.work", "change {\n");
    let (code, env) = json_of(
        &fx.root,
        &["work", "check", "plans/broken.work", "--project", reader],
    );
    assert_eq!(code, 1);
    assert_eq!(env["diagnostics"][0]["category"], "parse");
    assert_eq!(env["diagnostics"][0]["span"]["file"], "plans/broken.work");
}

#[test]
fn work_check_refusals() {
    let fx = Fixture::new("work-check-refusals");
    for path in ["work/nope.work", "work"] {
        let (code, env) = json_of(&fx.reader, &["work", "check", path]);
        assert_eq!(code, 2, "{path}");
        assert_eq!(kinds(&env), pairs(&[("error", "usage_bad_argument")]));
        assert_eq!(env["diagnostics"][0]["path"], path);
    }
    // A work file that does not parse: its parse diagnostic, exit 1...
    fx.write("reader/work/broken.work", "change {\n");
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/broken.work"]);
    assert_eq!(code, 1);
    let diagnostics = env["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["category"], "parse");
    assert_eq!(diagnostics[0]["span"]["file"], "work/broken.work");
    assert!(env.get("changes").is_none());
    // ...which does not stop another work file's validation (semantics S53).
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_eq!(code, 0);
    assert!(kinds(&env)
        .iter()
        .all(|(severity, _)| severity == "warning"));
    // A project that does not check: its errors, and nothing is validated.
    break_reader(&fx);
    let (code, env) = json_of(&fx.reader, &["work", "check", "work/pagination.work"]);
    assert_eq!(code, 1);
    assert_eq!(env["status"], "unsuccessful");
    assert!(kinds(&env).contains(&("error".to_string(), "type_mismatch".to_string())));
    assert!(env.get("changes").is_none());
    assert!(with_code(&env, "work_provisional_identity").is_empty());
}

#[test]
fn work_check_human_output() {
    let fx = Fixture::new("work-check-human");
    let run = cpl(&fx.reader, &["work", "check", "work/pagination.work"]);
    run.expect_code(0);
    assert_eq!(
        run.stdout,
        "work check: work/pagination.work: 1 change, 3 tasks, ok\n"
    );
    let lines: Vec<&str> = run.stderr.lines().collect();
    assert!(
        lines[0].starts_with(
            "warning[work_provisional_identity]: `reader.pages.Positive` has no `@id`"
        ),
        "{}",
        run.stderr
    );
    assert_eq!(lines[1], "  --> work/pagination.work:10:37");
    assert!(lines
        .iter()
        .any(|l| l.starts_with("warning[git_unavailable]: ")));
}

#[test]
fn reserved_commands_need_no_project() {
    let dir = temp_dir("reserved-no-project");
    for args in [
        vec!["work", "run", "work/pagination.work"],
        vec!["work", "integrate", "pagination_update"],
    ] {
        let run = cpl(&dir, &args);
        run.expect_code(2);
        assert_eq!(run.stdout, "");
        let lines: Vec<&str> = run.stderr.lines().collect();
        assert!(
            lines[0].starts_with("error[usage_not_implemented]: "),
            "{}",
            run.stderr
        );
        assert!(
            run.stderr.contains("= note: reserved for Milestone 6"),
            "{}",
            run.stderr
        );
        let (code, env) = json_of(&dir, &args);
        assert_eq!(code, 2);
        assert_eq!(env["status"], "error");
        assert_eq!(env["revision"], Value::Null);
        assert_eq!(kinds(&env), pairs(&[("error", "usage_not_implemented")]));
    }
}
