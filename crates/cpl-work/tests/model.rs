//! The work model and the records of `spec/toolchain.md` 1.9 and 10.1:
//! identifiers, file selectors (T42), namespaces (T43), timestamps and
//! lease expiry, grants, the frozen policy, and the JSON forms the
//! coordinator persists, the baseline object of Section 5.5 in particular.

mod common;

use common::*;
use cpl_diag::json::{Digest, Revision, RevisionState};
use cpl_diag::{Facet, FacetSelector, FileId, Span, StableId};
use cpl_project::load_project;
use cpl_work::{
    boundary_text, namespace_covers, Baseline, CandidateId, ChangeRunId, ChangeState,
    ExecutionBoundary, FencingToken, FileSelector, FrozenInputs, FsAccess, Grant, Lease,
    NetworkAccess, ProcessAccess, RequirementSpec, ResolvedNamespace, ResolvedNamespaceOrSelector,
    ResolvedSelector, RunPolicy, TaskId, TaskState, Timestamp, Verdict,
};
use serde_json::json;
use std::cmp::Ordering;
use std::path::PathBuf;

fn task_id() -> TaskId {
    TaskId {
        change: ChangeRunId::from_random_bytes(*b"foobafooba"),
        name: "implementation".to_string(),
    }
}

#[test]
fn change_run_ids() {
    // The bytes `foobafooba` encode as in cpl-index's generated-id test vector.
    let id = ChangeRunId::from_random_bytes(*b"foobafooba");
    assert_eq!(id.as_str(), "chg_mzxw6ytbmzxw6ytb");
    assert_eq!(ChangeRunId::parse("chg_mzxw6ytbmzxw6ytb"), Ok(id.clone()));
    assert_eq!(
        ChangeRunId::from_random_bytes([0; 10]).as_str(),
        "chg_aaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        ChangeRunId::from_random_bytes([0xff; 10]).as_str(),
        "chg_7777777777777777"
    );
    for bad in [
        "chg_mzxw6ytbmzxw6yt",
        "chg_mzxw6ytbmzxw6ytb1",
        "chg_MZXW6YTBMZXW6YTB",
        "chg_mzxw6ytbmzxw6yt1",
        "fn_mzxw6ytbmzxw6ytb",
        "",
    ] {
        assert!(ChangeRunId::parse(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        serde_json::to_value(&id).expect("json"),
        json!("chg_mzxw6ytbmzxw6ytb")
    );
    assert!(serde_json::from_value::<ChangeRunId>(json!("chg_0")).is_err());
}

#[test]
fn task_and_candidate_ids() {
    let task = task_id();
    assert_eq!(task.to_string(), "chg_mzxw6ytbmzxw6ytb/implementation");
    let id = CandidateId::derive(
        &task,
        2,
        FencingToken(7),
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
    );
    assert!(id.as_str().starts_with("cand_"));
    assert_eq!(id.as_str().len(), "cand_".len() + 32);
    assert_eq!(CandidateId::parse(id.as_str()), Ok(id.clone()));
    // The hashed text (Section 10.1; this crate's encoding of the fields).
    let hash = cpl_index::facet_hash(
        "chg_mzxw6ytbmzxw6ytb/implementation\n2\n7\n4b825dc642cb6eb9a060e54bf8d69288fbee4904",
    );
    assert_eq!(id.as_str(), format!("cand_{hash}"));
    // Every field matters.
    let tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    assert_ne!(id, CandidateId::derive(&task, 3, FencingToken(7), tree));
    assert_ne!(id, CandidateId::derive(&task, 2, FencingToken(8), tree));
    assert_ne!(id, CandidateId::derive(&task, 2, FencingToken(7), "other"));
    let other_task = TaskId {
        name: "examples".to_string(),
        ..task.clone()
    };
    assert_ne!(
        id,
        CandidateId::derive(&other_task, 2, FencingToken(7), tree)
    );
    assert!(CandidateId::parse("cand_XYZ").is_err());
    assert!(serde_json::from_value::<CandidateId>(json!("cand_00")).is_err());
    assert_eq!(
        serde_json::to_value(&task).expect("json"),
        json!({"change": "chg_mzxw6ytbmzxw6ytb", "name": "implementation"})
    );
}

#[test]
fn file_selectors() {
    let cases = [
        ("project.manifest", FileSelector::Manifest, "cpl.toml"),
        ("project.lock", FileSelector::Lock, "cpl.lock"),
        (
            "work.pagination",
            FileSelector::Work("pagination".into()),
            "work/pagination.work",
        ),
    ];
    for (spelling, selector, path) in cases {
        assert_eq!(FileSelector::parse(spelling), Some(selector.clone()));
        assert_eq!(selector.spelling(), spelling);
        assert_eq!(selector.to_string(), spelling);
        assert_eq!(selector.path(), path);
        assert_eq!(FileSelector::for_path(path), Some(selector.clone()));
        assert_eq!(
            serde_json::to_value(&selector).expect("json"),
            json!(spelling)
        );
        assert_eq!(
            serde_json::from_value::<FileSelector>(json!(spelling)).expect("parses"),
            selector
        );
    }
    for bad in ["project", "project.other", "work.", "work.a/b", "manifest"] {
        assert_eq!(FileSelector::parse(bad), None, "{bad}");
    }
    for path in [
        "work/x.patch",
        "work/a/b.work",
        "work/.work",
        "src/cpl.toml",
        "x.work",
    ] {
        assert_eq!(FileSelector::for_path(path), None, "{path}");
    }
    assert!(serde_json::from_value::<FileSelector>(json!("project.x")).is_err());
    // Ordered, so a grant's file set is deterministic.
    assert!(FileSelector::Manifest < FileSelector::Lock);
    assert!(FileSelector::Lock < FileSelector::Work("a".into()));
}

#[test]
fn namespaces_cover_their_module_and_every_module_below() {
    let path = |s: &str| -> Vec<String> { s.split('.').map(str::to_string).collect() };
    let tests = ResolvedNamespace {
        text: "tests.*".into(),
        span: Span::new(FileId(0), 0, 7),
        module_prefix: path("tests"),
    };
    assert!(tests.covers(&path("tests")));
    assert!(tests.covers(&path("tests.pagination")));
    assert!(tests.covers(&path("tests.pagination.edge")));
    assert!(!tests.covers(&path("testsuite")));
    assert!(!tests.covers(&path("reader.tests")));
    assert!(namespace_covers(&[], &path("any.module")));
    assert!(!namespace_covers(&path("tests.pagination"), &path("tests")));
}

#[test]
fn timestamps_compare_by_instant() {
    let noon = Timestamp("2026-10-28T12:00:00Z".to_string());
    let same = Timestamp("2026-10-28T14:00:00+02:00".to_string());
    let later = Timestamp("2026-10-28T12:00:00.5Z".to_string());
    assert_ne!(noon, same, "different spellings");
    assert_eq!(noon.cmp_instant(&same), Some(Ordering::Equal));
    assert_eq!(noon.cmp_instant(&later), Some(Ordering::Less));
    assert_eq!(noon.unix_seconds(), Some(1_793_188_800));
    assert_eq!(
        Timestamp::from_unix_seconds(1_793_188_800),
        Some(noon.clone())
    );
    assert_eq!(
        noon.add_seconds(900).map(|t| t.0),
        Some("2026-10-28T12:15:00Z".to_string())
    );
    assert_eq!(
        later.add_seconds(60).map(|t| t.0),
        Some("2026-10-28T12:01:00.5Z".to_string())
    );
    assert_eq!(same.add_seconds(0), Some(noon.clone()), "normalized to UTC");
    let bad = Timestamp("noon".to_string());
    assert!(!bad.is_valid());
    assert_eq!(bad.cmp_instant(&noon), None);
    assert_eq!(bad.add_seconds(1), None);
    assert!(Timestamp::now().is_valid());
    assert_eq!(
        Timestamp::from_unix_seconds(253_402_300_800),
        None,
        "year 10000"
    );
}

#[test]
fn leases_expire_at_their_expiry_and_fail_closed() {
    let lease = Lease {
        task: task_id(),
        attempt: 1,
        token: FencingToken(1),
        holder: "agent".into(),
        worktree: PathBuf::from("/w"),
        boundary: ExecutionBoundary::default(),
        issued_at: Timestamp("2026-10-28T12:00:00Z".into()),
        expires_at: Timestamp("2026-10-28T12:15:00Z".into()),
    };
    assert!(!lease.is_expired(&Timestamp("2026-10-28T12:14:59Z".into())));
    assert!(lease.is_expired(&Timestamp("2026-10-28T12:15:00Z".into())));
    assert!(lease.is_expired(&Timestamp("2026-10-28T13:00:00Z".into())));
    assert!(lease.is_expired(&Timestamp("garbage".into())));
    let corrupt = Lease {
        expires_at: Timestamp("never".into()),
        ..lease
    };
    assert!(corrupt.is_expired(&Timestamp("2026-10-28T12:00:00Z".into())));
}

#[test]
fn grants_never_write_identity() {
    let id = StableId::parse("pgn_7c2d").expect("id");
    let grant = Grant {
        write: [(id.clone(), Facet::Identity), (id.clone(), Facet::Body)].into(),
        add: vec![vec!["tests".to_string()]],
        files: [FileSelector::Lock].into(),
        ..Grant::default()
    };
    assert!(!grant.may_write(&id, Facet::Identity), "a fixed exclusion");
    assert!(grant.may_write(&id, Facet::Body));
    assert!(!grant.may_write(&id, Facet::Contract));
    assert!(grant.may_add(&["tests".to_string(), "pagination".to_string()]));
    assert!(!grant.may_add(&["reader".to_string()]));
    assert!(grant.may_change_file(&FileSelector::Lock));
    assert!(!grant.may_change_file(&FileSelector::Manifest));
    let text = serde_json::to_string(&grant).expect("json");
    assert_eq!(serde_json::from_str::<Grant>(&text).expect("parses"), grant);
}

#[test]
fn boundaries_and_states_spell_like_the_spec() {
    assert_eq!(
        boundary_text(&ExecutionBoundary::default()),
        "filesystem=worktree process=none network=none"
    );
    assert_eq!(
        boundary_text(&ExecutionBoundary {
            filesystem: FsAccess::WorktreeAndRepository,
            process: ProcessAccess::Toolchain,
            network: NetworkAccess::Adapters,
        }),
        "filesystem=worktree_and_repository process=toolchain network=adapters"
    );
    let states = [
        TaskState::Ready,
        TaskState::Running,
        TaskState::Submitted,
        TaskState::Blocked,
        TaskState::Accepted,
        TaskState::Integrated,
        TaskState::Failed,
        TaskState::Cancelled,
    ];
    for state in states {
        assert_eq!(
            serde_json::to_value(state).expect("json"),
            json!(state.as_str())
        );
        assert_eq!(
            state.is_terminal(),
            matches!(
                state,
                TaskState::Integrated | TaskState::Failed | TaskState::Cancelled
            )
        );
    }
    assert_eq!(
        serde_json::to_value(ChangeState::Open).expect("json"),
        json!("open")
    );
    assert_eq!(
        serde_json::to_value(ChangeState::Integrated {
            commit: "abc".into()
        })
        .expect("json"),
        json!({"integrated": {"commit": "abc"}})
    );
    assert!(ChangeState::Cancelled.is_terminal() && !ChangeState::Integrating.is_terminal());
    assert_eq!(
        serde_json::to_value(Verdict::NeedsChanges).expect("json"),
        json!("needs_changes")
    );
}

#[test]
fn frozen_inputs_capture_the_run_policy() {
    let project = load_project(&workspace_path("examples/reader"), &Default::default())
        .expect("reader loads");
    let frozen = FrozenInputs::freeze(&project, Digest("a".repeat(64)));
    assert_eq!(frozen.manifest, project.manifest_digest);
    assert_eq!(frozen.lock, None, "the corpus has no cpl.lock");
    assert_eq!(frozen.work_file, Digest("a".repeat(64)));
    assert_eq!(frozen.toolchain, cpl_diag::json::ToolchainInfo::current());
    assert_eq!(frozen.policy, RunPolicy::from_manifest(&project.manifest));
    assert_eq!(frozen.policy.work.lease_seconds, 900);
    assert_eq!(frozen.policy.test.cases, 100);
    let text = serde_json::to_string(&frozen).expect("json");
    assert_eq!(
        serde_json::from_str::<FrozenInputs>(&text).expect("parses"),
        frozen
    );
}

#[test]
fn baseline_json_is_the_object_of_section_5_5() {
    let facets = map_of(&reader_tree());
    let revision = Revision {
        commit: Some("3f2e0c41a5f8c2e2b0f5e7a3d9c1b4a6e8f0a2c4".into()),
        state: RevisionState::Clean,
        digest: facets.revision.digest.clone(),
    };
    let baseline = Baseline {
        revision: revision.clone(),
        snapshot_commit: "3f2e0c41a5f8c2e2b0f5e7a3d9c1b4a6e8f0a2c4".into(),
        predecessors: vec![CandidateId::derive(&task_id(), 1, FencingToken(1), "tree")],
        facets: cpl_index::FacetMap {
            revision: revision.clone(),
            ..facets
        },
    };
    let value = serde_json::to_value(&baseline).expect("json");
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["facets", "predecessors", "revision", "snapshot_commit"]
    );
    assert_eq!(value["revision"]["state"], "clean");
    let paginate = &value["facets"]["pgn_7c2d"];
    assert_eq!(paginate["path"], "reader.pages.paginate");
    assert_eq!(paginate["provisional"], false);
    let facet_names: Vec<&str> = paginate["facets"]
        .as_object()
        .expect("facets")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(facet_names.len(), 8);
    assert_eq!(
        paginate["facets"]["effects"],
        "af1349b9f5f9a1a6a0404dea36dcc949"
    );
    assert_eq!(paginate["kind"], "fn");
    assert_eq!(paginate["module"], "reader.pages");
    assert_eq!(paginate["file"], "src/reader/pages.cpl");
    assert!(paginate.get("effects").is_none(), "no effect set");
    assert_eq!(
        value["facets"]["reader.pages.load_page"]["effects"],
        json!(["db.get"])
    );
    assert_eq!(
        value["facets"]["reader.pages.Positive"]["provisional"],
        true
    );
    // Lossless: the facet map comes back, effect sets and modules included.
    let text = serde_json::to_string(&baseline).expect("json");
    assert_eq!(
        serde_json::from_str::<Baseline>(&text).expect("parses"),
        baseline
    );
    // Inconsistent documents are refused.
    let mut bad_kind = value.clone();
    bad_kind["facets"]["pgn_7c2d"]["kind"] = json!("function");
    assert!(serde_json::from_value::<Baseline>(bad_kind).is_err());
    let mut bad_flag = value;
    bad_flag["facets"]["pgn_7c2d"]["provisional"] = json!(true);
    assert!(serde_json::from_value::<Baseline>(bad_flag).is_err());
}

#[test]
fn requirement_and_selector_models_serialize() {
    let span = Span::new(FileId(2), 10, 23);
    let selector = ResolvedSelector {
        text: "paginate".into(),
        span,
        targets: vec![(
            StableId::parse("pgn_7c2d").expect("id"),
            FacetSelector::Definition,
        )],
    };
    let namespace = ResolvedNamespace {
        text: "tests.pagination.*".into(),
        span,
        module_prefix: vec!["tests".into(), "pagination".into()],
    };
    let integrate = vec![
        RequirementSpec::Typecheck,
        RequirementSpec::Prove(selector.clone()),
        RequirementSpec::Test(ResolvedNamespaceOrSelector::Namespace(namespace)),
        RequirementSpec::Test(ResolvedNamespaceOrSelector::Selector(selector)),
        RequirementSpec::Review("audit".into()),
        RequirementSpec::NoHoles,
        RequirementSpec::NoUnresolvedAssumptions,
    ];
    let value = serde_json::to_value(&integrate).expect("json");
    assert_eq!(value[0], json!("typecheck"));
    assert_eq!(
        value[1]["prove"]["targets"],
        json!([["pgn_7c2d", "definition"]])
    );
    assert_eq!(value[4], json!({"review": "audit"}));
    assert_eq!(value[6], json!("no_unresolved_assumptions"));
    let back: Vec<RequirementSpec> = serde_json::from_value(value).expect("parses");
    assert_eq!(back, integrate);
}

#[test]
fn candidates_and_authorities_round_trip() {
    use cpl_work::{Authority, Candidate, PatchSummary, ReviewRecord};
    let before = map_of(&reader_tree());
    let changed = with_file(
        &reader_tree(),
        "src/reader/pages.cpl",
        &text(&reader_tree(), "src/reader/pages.cpl").replace("    a / b\n", "    (a / b)\n"),
    );
    let after = map_of(&changed);
    let diff = cpl_index::semantic_diff(&before, &after);
    assert_eq!(diff.statements.len(), 1, "divide_exactly's body");
    let task = task_id();
    let tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let id = CandidateId::derive(&task, 1, FencingToken(3), tree);
    let pgn = StableId::parse("pgn_7c2d").expect("id");
    let hash = before
        .get(&pgn)
        .and_then(|e| e.facet(Facet::Body))
        .expect("body")
        .clone();
    let candidate = Candidate {
        id: id.clone(),
        task: task.clone(),
        attempt: 1,
        token: FencingToken(3),
        baseline: Baseline {
            revision: before.revision.clone(),
            snapshot_commit: "a".repeat(40),
            predecessors: Vec::new(),
            facets: before,
        },
        tree: tree.to_string(),
        snapshot_commit: "b".repeat(40),
        patches: vec![PatchSummary {
            target: (pgn.clone(), Facet::Body),
            expected: hash.clone(),
            applied: hash,
        }],
        boundary: ExecutionBoundary::default(),
        facet_diff: diff.statements,
        dependency_versions: Vec::new(),
        evidence: vec![Digest("c".repeat(64))],
        review: Some(ReviewRecord {
            reviewer_task: TaskId {
                name: "audit".into(),
                ..task
            },
            reviewed: vec![id],
            verdict: Verdict::Approve,
            notes: "matches the frozen contract".into(),
            token: FencingToken(4),
        }),
        submitted_at: Timestamp("2026-10-28T12:00:00Z".into()),
    };
    let text = serde_json::to_string(&candidate).expect("json");
    assert_eq!(
        serde_json::from_str::<Candidate>(&text).expect("parses"),
        candidate
    );
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["facet_diff"][0]["statement"], "body_changed");
    assert_eq!(value["patches"][0]["target"], json!(["pgn_7c2d", "body"]));
    assert_eq!(value["review"]["verdict"], "approve");

    let authority = Authority {
        write: Some([(pgn, Facet::Body)].into()),
        add: Some(vec![vec!["tests".into()]]),
        files: [FileSelector::Lock].into(),
        boundary: ExecutionBoundary {
            filesystem: FsAccess::Host,
            process: ProcessAccess::Any,
            network: NetworkAccess::None,
        },
        may_integrate: true,
    };
    let value = serde_json::to_value(&authority).expect("json");
    assert_eq!(value["files"], json!(["project.lock"]));
    assert_eq!(
        value["boundary"],
        json!({"filesystem": "host", "process": "any", "network": "none"})
    );
    assert_eq!(
        serde_json::from_value::<Authority>(value).expect("parses"),
        authority
    );
}
