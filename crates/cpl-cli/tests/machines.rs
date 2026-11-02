use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

fn project() -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-machines");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("cpl.toml"),
        include_str!("../../cpl-cabi/tests/projects/machines/cpl.toml"),
    )
    .unwrap();
    std::fs::write(
        root.join("src/m.cpl"),
        include_str!("../../cpl-cabi/tests/projects/machines/src/m.cpl"),
    )
    .unwrap();
    root
}

fn run(root: &Path, args: &[&str], code: i32) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_cpl"))
        .arg("--project")
        .arg(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    let json: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", output));
    assert_eq!(output.status.code(), Some(code), "{json}");
    json
}

#[test]
fn replay_cli_runs_both_engines_verifies_receipts_and_reports_rollback() {
    let root = project();
    let state = json!({"$type":"m.App","count":{"$type":"m.Count","n":50}}).to_string();
    let event = json!({"$type":"m.Event","$variant":"Add","$payload":[2]});
    let events = json!([event, event]).to_string();
    let output = run(
        &root,
        &[
            "replay", "m.reduce", "--state", &state, "--events", &events, "--engine", "both",
        ],
        0,
    );
    assert_eq!(output["sequence"], 2);
    assert_eq!(output["state"]["count"]["n"], 54);
    let records = output["records"].to_string();
    let replayed = run(
        &root,
        &[
            "replay",
            "m.reduce",
            "--state",
            &state,
            "--records",
            &records,
        ],
        0,
    );
    assert_eq!(replayed["state"], output["state"]);
    let mut bad = output["records"].clone();
    bad[1]["after"]["count"]["n"] = json!(99);
    let output = run(
        &root,
        &[
            "replay",
            "m.reduce",
            "--state",
            &state,
            "--records",
            &bad.to_string(),
        ],
        1,
    );
    assert_eq!(output["sequence"], 1);
    assert_eq!(output["state"]["count"]["n"], 52);
    assert_eq!(output["diagnostics"][0]["code"], "machine_replay_mismatch");
    let duplicate = "{\"$type\":\"m.App\",\"count\":0,\"count\":1}";
    let refused = run(&root, &["replay", "m.reduce", "--state", duplicate], 2);
    assert_eq!(refused["diagnostics"][0]["code"], "usage_bad_argument");
    let events = json!([event,{"$type":"m.Event","$variant":"Divide","$payload":[0]}]).to_string();
    let fault = run(
        &root,
        &[
            "replay", "m.reduce", "--state", &state, "--events", &events, "--engine", "both",
        ],
        1,
    );
    assert_eq!(fault["sequence"], 1);
    assert_eq!(fault["state"]["count"]["n"], 52);
    assert_eq!(fault["diagnostics"][0]["code"], "division_by_zero");
    assert_eq!(fault["diagnostics"][0]["state_unchanged"], true);
    let path = root.join("src/m.cpl");
    let mut source = std::fs::read_to_string(&path).unwrap();
    source.push_str(
        "\nmachine spin(state: Count, event: Event) -> Count {\n    while true { }\n    state\n}\n",
    );
    std::fs::write(path, source).unwrap();
    let state = json!({"$type":"m.Count","n":1}).to_string();
    let timed = run(
        &root,
        &[
            "replay",
            "m.spin",
            "--state",
            &state,
            "--events",
            &json!([event]).to_string(),
            "--timeout-ms",
            "10",
            "--engine",
            "both",
        ],
        1,
    );
    assert_eq!(timed["sequence"], 0);
    assert!(
        timed["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "resource_exhaustion" && d["state_unchanged"] == true),
        "{timed}"
    );
}
