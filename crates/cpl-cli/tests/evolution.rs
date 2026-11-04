mod common;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The `@preserve` statement of `demo.price`, which body replacements must acknowledge.
const PRESERVED: &str = "Nonnegative quantities retain their price";

fn project(name: &str) -> PathBuf {
    let root = common::temp_dir(name);
    common::copy_dir(&common::workspace().join("examples/evolution"), &root);
    root
}
fn run(root: &Path, args: &[&str], code: i32) -> Value {
    let mut argv = vec!["--json"];
    argv.extend_from_slice(args);
    common::cpl(root, &argv).expect_code(code).json()
}

#[test]
fn evolution_example_checks_formats_and_runs_on_both_backends() {
    let root = project("evolution-engines");
    run(&root, &["check"], 0);
    run(&root, &["fmt", "--check"], 0);
    let out = run(
        &root,
        &["test", "--engine", "both", "--seed", "1", "--cases", "30"],
        0,
    );
    assert_eq!(out["failed"], 0);
    let metadata = run(&root, &["query", "symbol fn_price"], 0);
    assert_eq!(
        metadata["symbol"]["metadata"]["intent"][0],
        "Compute the price of a quantity of items"
    );
    let effects = run(
        &root,
        &["query", "functions with demo.Store.read effects"],
        0,
    );
    assert_eq!(effects["symbols"].as_array().unwrap().len(), 1);
    let protocol = run(&root, &["query", "protocols"], 0);
    assert_eq!(protocol["symbols"].as_array().unwrap().len(), 2);
}

#[test]
fn cli_previews_changes_applies_with_authority_and_rejects_stale_or_failing_edits() {
    let root = project("evolution-edits");
    let original = fs::read(root.join("src/demo.cpl")).unwrap();
    let proposal = run(
        &root,
        &[
            "edit",
            "replace-body",
            "--symbol",
            "fn_price",
            "--input",
            "price.body",
            "--actor",
            "optimizer",
            "--policy",
            "authority.json",
            "--patch",
            "change.json",
            "--acknowledge",
            PRESERVED,
        ],
        0,
    );
    assert_eq!(proposal["change"]["accepted"], true);
    assert_eq!(proposal["change"]["attestations"][0]["status"], "attested");
    assert_eq!(fs::read(root.join("src/demo.cpl")).unwrap(), original);
    // The verifier proves `price`'s postcondition for the new body as well.
    assert!(proposal["change"]["evidence"]["obligations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|o| o["status"] == "proved"));
    let applied = run(
        &root,
        &[
            "transaction",
            "change.json",
            "--actor",
            "optimizer",
            "--policy",
            "authority.json",
            "--apply",
        ],
        0,
    );
    assert_eq!(applied["applied"], json!(["src/demo.cpl"]));
    run(
        &root,
        &[
            "transaction",
            "change.json",
            "--actor",
            "optimizer",
            "--policy",
            "authority.json",
            "--apply",
        ],
        1,
    );
    let before = fs::read(root.join("src/demo.cpl")).unwrap();
    fs::write(root.join("bad.body"), "{ quantity * 11 }").unwrap();
    let unacknowledged = run(
        &root,
        &[
            "edit",
            "replace-body",
            "--symbol",
            "fn_price",
            "--input",
            "bad.body",
        ],
        1,
    );
    assert_eq!(
        unacknowledged["diagnostics"][0]["code"],
        "change_preserve_unacknowledged"
    );
    let failing = run(
        &root,
        &[
            "edit",
            "replace-body",
            "--symbol",
            "fn_price",
            "--input",
            "bad.body",
            "--acknowledge",
            PRESERVED,
            "--apply",
        ],
        1,
    );
    assert_eq!(failing["change"]["accepted"], false);
    assert_eq!(fs::read(root.join("src/demo.cpl")).unwrap(), before);
    let denied = run(
        &root,
        &[
            "edit",
            "rename",
            "--symbol",
            "fn_price",
            "--name",
            "calculate_price",
            "--actor",
            "optimizer",
            "--policy",
            "authority.json",
            "--apply",
        ],
        1,
    );
    assert_eq!(denied["diagnostics"][0]["code"], "change_write_denied");
    run(
        &root,
        &[
            "edit",
            "rename",
            "--symbol",
            "fn_price",
            "--name",
            "calculate_price",
            "--apply",
        ],
        0,
    );
    assert!(fs::read_to_string(root.join("src/demo.cpl"))
        .unwrap()
        .contains("fn calculate_price"));
    run(&root, &["transaction", "--recover"], 0);
}

#[test]
fn cli_explores_migrates_and_replays_effects_with_tamper_detection() {
    let root = project("evolution-runtime");
    let exploration = run(
        &root,
        &[
            "explore",
            "demo.update_counter",
            "--state",
            r#"{"$type":"demo.Counter","value":0}"#,
            "--events",
            r#"[{"$type":"demo.CounterEvent","$variant":"Increment"},{"$type":"demo.CounterEvent","$variant":"Reset"}]"#,
            "--depth",
            "100",
        ],
        0,
    );
    assert_eq!(exploration["exploration"]["result"], "exhaustive");
    assert_eq!(exploration["exploration"]["states"], 4);
    // The initializer and the finite event type supply the defaults; the goal has a witness.
    let goal = run(
        &root,
        &[
            "explore",
            "demo.update_counter",
            "--goal",
            "demo.counter_full",
        ],
        0,
    );
    assert_eq!(goal["exploration"]["result"], "witness");
    assert_eq!(goal["exploration"]["initial_source"], "initializer");
    assert_eq!(
        goal["exploration"]["witness"]["events"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let writable = run(
        &root,
        &[
            "query",
            "writable by optimizer",
            "--policy",
            "authority.json",
        ],
        0,
    );
    assert_eq!(
        writable["permissions"]["symbols"],
        json!([{"symbol": "fn_price", "path": "demo.price", "writable": ["body"], "removable": false}])
    );
    run(&root, &["query", "writable by optimizer"], 1);
    let packed = run(
        &root,
        &[
            "schema",
            "demo.UserV1",
            "--value",
            r#"{"$type":"demo.UserV1","name":"Ada"}"#,
        ],
        0,
    );
    fs::write(root.join("user.json"), packed["stored"].to_string()).unwrap();
    let migrated = run(
        &root,
        &["migrate", "demo.upgrade_user", "--input", "user.json"],
        0,
    );
    assert_eq!(migrated["stored"]["version"], "2");
    assert_eq!(migrated["stored"]["value"]["active"], true);
    let path = run(&root, &["migrate", "--input", "user.json", "--to", "2"], 0);
    assert_eq!(path["stored"], migrated["stored"]);
    assert_eq!(path["steps"], json!(["demo.upgrade_user"]));
    run(&root, &["migrate", "--input", "user.json", "--to", "9"], 1);
    let plan = run(&root, &["concurrent", "scope.json"], 0);
    assert_eq!(plan["plan"]["waves"].as_array().unwrap().len(), 1);
    let executed = run(
        &root,
        &[
            "concurrent",
            "scope.json",
            "--responses",
            "responses.json",
            "--receipt",
            "effects.json",
        ],
        0,
    );
    let replayed = run(
        &root,
        &[
            "concurrent",
            "scope.json",
            "--replay",
            "effects.json",
            "--workers",
            "1",
        ],
        0,
    );
    assert_eq!(executed["execution"], replayed["execution"]);
    let mut forged = executed["execution"].clone();
    forged["events"][0]["response"] = json!({"Ok":999});
    fs::write(root.join("effects.json"), forged.to_string()).unwrap();
    run(
        &root,
        &["concurrent", "scope.json", "--replay", "effects.json"],
        1,
    );
}

#[test]
fn cli_evidence_is_recomputed_and_unknown_proofs_remain_unknown() {
    let root = project("evolution-evidence");
    let verified = run(
        &root,
        &[
            "verify",
            "--receipt",
            "evidence.json",
            "--seed",
            "42",
            "--cases",
            "20",
        ],
        0,
    );
    assert_eq!(verified["verification"]["configuration"]["seed"], 42);
    run(&root, &["verify", "--evidence", "evidence.json"], 0);
    // Every obligation of the example is proved.
    run(&root, &["verify", "--require-proofs"], 0);
    let mut evidence = verified["verification"].clone();
    evidence["tests"] = json!([]);
    fs::write(root.join("evidence.json"), evidence.to_string()).unwrap();
    let rejected = run(&root, &["verify", "--evidence", "evidence.json"], 1);
    assert_eq!(rejected["diagnostics"][0]["code"], "evidence_stale");
    run(&root, &["verify", "--receipt", "src/demo.cpl"], 2);
    // A postcondition about text is outside the verifier's fragment: it stays unknown, which
    // only `--require-proofs` rejects.
    let mut source = fs::read_to_string(root.join("src/demo.cpl")).unwrap();
    source.push_str(
        "\npub fn shout(t: Text) -> Text\n    ensures result.len > t.len\n{\n    t ++ \"!\"\n}\n",
    );
    fs::write(root.join("src/demo.cpl"), source).unwrap();
    let unknown = run(&root, &["verify"], 0);
    assert!(unknown["verification"]["obligations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["status"] == "unknown"));
    run(&root, &["verify", "--require-proofs"], 1);
}

#[test]
fn publication_carries_evidence_and_preserves_the_checkout_and_user_index() {
    let Some(fixture) = common::Fixture::committed("evolution-publish") else {
        return;
    };
    let root = fixture.root.join("evolution");
    common::copy_dir(&common::workspace().join("examples/evolution"), &root);
    let head = fixture.commit_all("add evolution project");
    fs::write(
        fixture.root.join("unrelated.txt"),
        "staged outside the project",
    )
    .unwrap();
    fixture.git(&["add", "unrelated.txt"]);
    let index = fixture.git(&["write-tree"]);
    let source = fs::read(root.join("src/demo.cpl")).unwrap();
    run(
        &root,
        &[
            "edit",
            "replace-body",
            "--symbol",
            "fn_price",
            "--input",
            "price.body",
            "--patch",
            "change.json",
            "--acknowledge",
            PRESERVED,
        ],
        0,
    );
    let published = run(
        &root,
        &[
            "transaction",
            "change.json",
            "--publish",
            "refs/cpl/changes/price",
        ],
        0,
    );
    let commit = published["commit"].as_str().unwrap();
    assert_eq!(
        fixture.git(&["rev-parse", "refs/cpl/changes/price"]),
        commit
    );
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["write-tree"]), index);
    assert_eq!(fs::read(root.join("src/demo.cpl")).unwrap(), source);
    let evidence = published["change"]["evidence"]["digest"].as_str().unwrap();
    let object = fixture.git(&[
        "show",
        &format!("{commit}:evolution/.cpl/evidence/{evidence}.json"),
    ]);
    let report: Value = serde_json::from_str(&object).unwrap();
    assert_eq!(report["accepted"], true);
    run(&root, &["transaction", "change.json", "--apply"], 0);
    let verified = run(&root, &["verify", "--changed", "--since", "HEAD"], 0);
    assert_eq!(
        verified["verification"]["impact"]["tests"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(verified["verification"]["impact"]["full_recheck"], false);
    run(&root, &["query", "changed", "--since", "HEAD"], 0);
    // The published commit and the applied worktree made the same change: nothing conflicts.
    let same = run(
        &root,
        &[
            "conflicts",
            "refs/cpl/changes/price",
            "worktree",
            "--base",
            "HEAD",
        ],
        0,
    );
    assert_eq!(same["merge"]["status"], "tested");
}

#[test]
fn published_changes_merge_by_identity_into_a_two_parent_commit() {
    let Some(fixture) = common::Fixture::committed("evolution-merge") else {
        return;
    };
    let root = fixture.root.join("evolution");
    common::copy_dir(&common::workspace().join("examples/evolution"), &root);
    let head = fixture.commit_all("add evolution project");
    let source = fs::read(root.join("src/demo.cpl")).unwrap();
    // Two agents start from HEAD: one renames price, the other optimizes its body.
    for (name, args) in [
        (
            "rename",
            vec![
                "edit",
                "rename",
                "--symbol",
                "fn_price",
                "--name",
                "calculate_price",
            ],
        ),
        (
            "body",
            vec![
                "edit",
                "replace-body",
                "--symbol",
                "fn_price",
                "--input",
                "price.body",
                "--acknowledge",
                PRESERVED,
            ],
        ),
    ] {
        let patch = format!("{name}.json");
        let mut argv = args.clone();
        argv.extend(["--patch", patch.as_str()]);
        run(&root, &argv, 0);
        let reference = format!("refs/cpl/changes/{name}");
        run(&root, &["transaction", &patch, "--publish", &reference], 0);
    }
    assert_eq!(fs::read(root.join("src/demo.cpl")).unwrap(), source);
    let preview = run(
        &root,
        &[
            "conflicts",
            "refs/cpl/changes/rename",
            "refs/cpl/changes/body",
        ],
        0,
    );
    assert_eq!(preview["base_revision"], head);
    assert_eq!(preview["merge"]["status"], "tested");
    let merged = run(
        &root,
        &[
            "merge",
            "refs/cpl/changes/rename",
            "refs/cpl/changes/body",
            "--publish",
            "refs/cpl/changes/merged",
        ],
        0,
    );
    let commit = merged["commit"].as_str().unwrap();
    let parents = fixture.git(&["rev-list", "--parents", "-n", "1", commit]);
    assert_eq!(parents.split_whitespace().count(), 3, "{parents}");
    let text = fixture.git(&["show", &format!("{commit}:evolution/src/demo.cpl")]);
    assert!(text.contains("fn calculate_price(quantity: Int)"), "{text}");
    assert!(text.contains("10 * quantity"), "{text}");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    // Over the worktree: the left side must be the worktree itself.
    run(
        &root,
        &[
            "merge",
            "refs/cpl/changes/body",
            "worktree",
            "--base",
            "HEAD",
            "--apply",
        ],
        1,
    );
    let applied = run(
        &root,
        &[
            "merge",
            "worktree",
            "refs/cpl/changes/merged",
            "--base",
            "HEAD",
            "--apply",
        ],
        0,
    );
    assert_eq!(applied["applied"], json!(["src/demo.cpl"]));
    let worktree = fs::read_to_string(root.join("src/demo.cpl")).unwrap();
    assert_eq!(worktree.trim_end(), text.trim_end());
}

#[test]
fn staged_edits_accumulate_into_one_transaction_and_holes_fill_by_name() {
    let root = project("evolution-staging");
    fs::write(
        root.join("src/staged.cpl"),
        "module staged\n\n@id(\"fn_half\")\npure fn half(n: Int) -> Int {\n    ?h\n}\n\n@id(\"fn_double\")\npure fn double(n: Int) -> Int {\n    n + n\n}\n\n@id(\"test_half\")\ntest \"half\" {\n    assert half(double(3)) == 3\n}\n",
    )
    .unwrap();
    let holes = run(&root, &["query", "holes"], 0);
    assert_eq!(holes["holes"][0]["symbol"], "fn_half");
    fs::write(root.join("double.body"), "{\n    2 * n\n}").unwrap();
    // A first staged edit alone still leaves the hole unfilled: previewed, not accepted.
    let first = run(
        &root,
        &[
            "edit",
            "replace-body",
            "--symbol",
            "fn_double",
            "--input",
            "double.body",
            "--patch",
            "staged.json",
            "--append",
        ],
        1,
    );
    assert_eq!(first["change"]["evidence"]["holes"], 1);
    let both = run(
        &root,
        &[
            "edit",
            "fill-hole",
            "--symbol",
            "fn_half",
            "--hole",
            "h",
            "--expression",
            "n / 2",
            "--patch",
            "staged.json",
            "--append",
        ],
        0,
    );
    assert_eq!(both["patch"]["edits"].as_array().unwrap().len(), 2);
    let applied = run(&root, &["transaction", "staged.json", "--apply"], 0);
    assert_eq!(applied["applied"], json!(["src/staged.cpl"]));
    let text = fs::read_to_string(root.join("src/staged.cpl")).unwrap();
    assert!(text.contains("(n / 2)") && text.contains("2 * n"), "{text}");
    run(
        &root,
        &[
            "edit",
            "rename",
            "--symbol",
            "fn_double",
            "--name",
            "twice",
            "--patch",
            "staged.json",
            "--append",
            "--actor",
            "someone",
        ],
        1,
    );
}
