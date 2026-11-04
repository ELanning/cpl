use cpl_diag::Facet;
use cpl_evolve::{change::*, storage, verify::VerifyConfig, Snapshot};
use cpl_project::LoadOptions;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "cpl-evolution-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("src")).unwrap();
        fs::write(
            path.join("cpl.toml"),
            "[project]\nname=\"storage\"\nedition=\"0.1\"\n",
        )
        .unwrap();
        fs::write(path.join("src/m.cpl"),"module m\n@id(\"fn_calc\")\npure fn calc(n: Int) -> Int { n + 1 }\ntest \"calc\" { assert calc(1) == 2 }\n").unwrap();
        Self(path)
    }
    fn read(&self) -> Snapshot {
        Snapshot::read(&self.0, &LoadOptions::default()).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn candidate(base: &Snapshot, body: &str) -> Candidate {
    let id = base.resolve("fn_calc").unwrap();
    let c = Change {
        name: "optimize".into(),
        actor: "optimizer".into(),
        edits: vec![Edit {
            symbol: "fn_calc".into(),
            expect: BTreeMap::from([(
                Facet::Body,
                base.index().version(id).facets[&Facet::Body].to_string(),
            )]),
            operation: Operation::ReplaceBody { body: body.into() },
        }],
        reads: vec![],
        contract: ChangeContract::default(),
        verification: VerifyConfig::default(),
    };
    prepare(base, &c, None, &LoadOptions::default()).unwrap()
}
fn install(
    project: &Project,
    before: &Snapshot,
    candidate: &Candidate,
) -> Result<Vec<String>, cpl_diag::Diagnostic> {
    let install = candidate.install(before)?;
    storage::apply(&project.0, before, &install, &LoadOptions::default())
}
#[test]
fn installation_rejects_stale_and_failed_candidates_without_changing_files() {
    let project = Project::new();
    let before = project.read();
    let bad = candidate(&before, "{ n + 2 }");
    assert!(install(&project, &before, &bad).is_err());
    assert_eq!(project.read().digest(), before.digest());
    let good = candidate(&before, "{ 1 + n }");
    fs::write(
        project.0.join("src/extra.cpl"),
        "module extra\npure fn extra() -> Int { 1 }\n",
    )
    .unwrap();
    assert_eq!(
        install(&project, &before, &good).unwrap_err().code.as_str(),
        "change_stale"
    );
    fs::remove_file(project.0.join("src/extra.cpl")).unwrap();
    let files = install(&project, &before, &good).unwrap();
    assert_eq!(files, vec!["src/m.cpl"]);
    assert_eq!(project.read().digest(), good.snapshot.digest());
    assert!(project.0.join(".cpl/last-change.json").is_file());
    assert!(!project.0.join(".cpl/evolution-journal.json").exists());
}
#[test]
fn crash_recovery_restores_all_files_and_never_overwrites_independent_edits() {
    let project = Project::new();
    let before = project.read();
    let good = candidate(&before, "{ 1 + n }");
    let a = &before.tree.files["src/m.cpl"];
    let b = &good.snapshot.tree.files["src/m.cpl"];
    fs::create_dir(project.0.join(".cpl")).unwrap();
    let journal = json!({"format":1,"files":{"src/m.cpl":{"before":a,"after":b},"src/new.cpl":{"before":null,"after":b"module new\n".to_vec()}}});
    fs::write(
        project.0.join(".cpl/evolution-journal.json"),
        journal.to_string(),
    )
    .unwrap();
    fs::write(project.0.join("src/m.cpl"), b).unwrap();
    fs::write(project.0.join("src/new.cpl"), b"module new\n").unwrap();
    assert_eq!(storage::recover(&project.0).unwrap(), 2);
    assert_eq!(project.read().digest(), before.digest());
    assert!(!project.0.join("src/new.cpl").exists());
    fs::write(
        project.0.join(".cpl/evolution-journal.json"),
        journal.to_string(),
    )
    .unwrap();
    fs::write(project.0.join("src/m.cpl"), "independent editor").unwrap();
    assert_eq!(
        storage::recover(&project.0).unwrap_err().code.as_str(),
        "change_recovery_conflict"
    );
    assert_eq!(
        fs::read_to_string(project.0.join("src/m.cpl")).unwrap(),
        "independent editor"
    );
}
#[test]
fn forged_acceptance_is_not_a_validated_candidate() {
    let project = Project::new();
    let before = project.read();
    let mut bad = candidate(&before, "{ n + 2 }");
    bad.report.accepted = true;
    bad.report.evidence.accepted = true;
    assert!(install(&project, &before, &bad).is_err());
    assert_eq!(project.read().digest(), before.digest());
}
#[cfg(unix)]
#[test]
fn paths_and_symlinks_cannot_escape_a_transaction() {
    for path in [
        "../outside",
        "/tmp/outside",
        "src/../outside",
        "src//file.cpl",
        ".git/config",
        "src\\outside",
    ] {
        assert!(storage::safe_relative(path).is_err(), "{path}");
    }
    let project = Project::new();
    let before = project.read();
    let good = candidate(&before, "{ 1 + n }");
    let outside = project.0.join("outside.cpl");
    fs::rename(project.0.join("src/m.cpl"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, project.0.join("src/m.cpl")).unwrap();
    assert!(install(&project, &before, &good).is_err());
    assert_eq!(fs::read(outside).unwrap(), before.tree.files["src/m.cpl"]);
}
#[test]
fn an_accepted_merge_installs_over_its_left_side_only() {
    let project = Project::new();
    let base = project.read();
    let source = String::from_utf8(base.tree.files["src/m.cpl"].clone()).unwrap();
    let mut right_tree = base.tree.clone();
    right_tree.files.insert(
        "src/m.cpl".into(),
        format!("{source}@id(\"fn_more\")\npure fn more() -> Int {{ calc(2) }}\n").into_bytes(),
    );
    let right = Snapshot::new(right_tree, &LoadOptions::default()).unwrap();
    fs::write(
        project.0.join("src/m.cpl"),
        source.replace("n + 1", "1 + n"),
    )
    .unwrap();
    let left = project.read();
    let merge = cpl_evolve::merge::merge(
        &base,
        &left,
        &right,
        &VerifyConfig::default(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert!(merge.report.accepted, "{:#?}", merge.report);
    assert!(merge.install(&base).is_err());
    let install = merge.install(&left).unwrap();
    assert!(storage::apply(&project.0, &base, &install, &LoadOptions::default()).is_err());
    storage::apply(&project.0, &left, &install, &LoadOptions::default()).unwrap();
    let text = fs::read_to_string(project.0.join("src/m.cpl")).unwrap();
    assert!(text.contains("1 + n") && text.contains("fn more"), "{text}");
    assert_eq!(
        Some(project.read().digest()),
        merge.report.merged,
        "the checkout is the verified composition"
    );
}
