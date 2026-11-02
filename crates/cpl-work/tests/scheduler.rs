//! The task state machine, leases, and fencing tokens (`spec/toolchain.md`
//! 10.2, 10.8; the integration transitions of 10.6), as pure functions over
//! a change run, with a fake `Sandbox`; and the coordinator state under
//! `.cpl/work/` (10.1, T33): JSON round trip and restart.

mod common;

use common::*;
use cpl_diag::json::{Digest, Revision, RevisionState, ToolchainInfo};
use cpl_diag::{FileId, Span};
use cpl_index::FacetMap;
use cpl_work::{
    Actor, Authority, Baseline, CandidateId, ChangeRecord, ChangeRun, ChangeRunId, ChangeState,
    Coordinator, CoordinatorError, ExecutionBoundary, FencingToken, FrozenInputs, FsAccess, Grant,
    Lease, NetworkAccess, ProcessAccess, RunPolicy, Sandbox, SandboxError, ScheduleError,
    Submission, SubmitOutcome, TaskSpec, TaskState, Timestamp,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

const BASE: &str = "3f2e0c41a5f8c2e2b0f5e7a3d9c1b4a6e8f0a2c4";
const T0: i64 = 1_793_188_800; // 2026-10-28T12:00:00Z

/// `T0 + seconds`.
fn at(seconds: i64) -> Timestamp {
    Timestamp::from_unix_seconds(T0 + seconds).expect("in range")
}

fn change_record(workers: u32, lease_seconds: u64) -> ChangeRecord {
    let mut policy = RunPolicy::default();
    policy.work.lease_seconds = lease_seconds;
    ChangeRecord {
        id: ChangeRunId::from_random_bytes(*b"foobafooba"),
        name: "pagination_update".to_string(),
        work_file: "work/pagination.work".to_string(),
        work_digest: Digest("1".repeat(64)),
        base_commit: BASE.to_string(),
        target_ref: "refs/heads/main".to_string(),
        workers,
        opened_by: Actor {
            id: "operator".to_string(),
            authority: Authority::default(),
        },
        frozen_inputs: FrozenInputs {
            manifest: Digest("2".repeat(64)),
            lock: None,
            work_file: Digest("1".repeat(64)),
            toolchain: ToolchainInfo::current(),
            policy,
        },
        state: ChangeState::Open,
        tasks: Vec::new(),
        next_token: 1,
        opened_at: at(0),
    }
}

fn spec(name: &str, after: &[&str]) -> TaskSpec {
    TaskSpec {
        name: name.to_string(),
        span: Span::new(FileId(0), 0, 0),
        after: after.iter().map(|s| s.to_string()).collect(),
        write: Vec::new(),
        add: Vec::new(),
        write_files: Vec::new(),
        depend: Vec::new(),
        preserve: Vec::new(),
        goal: None,
    }
}

fn baseline(predecessors: Vec<CandidateId>, facets: FacetMap) -> Baseline {
    Baseline {
        revision: facets.revision.clone(),
        snapshot_commit: BASE.to_string(),
        predecessors,
        facets,
    }
}

fn empty_facets() -> FacetMap {
    FacetMap {
        revision: Revision {
            commit: Some(BASE.to_string()),
            state: RevisionState::Clean,
            digest: Digest("3".repeat(64)),
        },
        entries: BTreeMap::new(),
        duplicates: Vec::new(),
    }
}

/// The three tasks of `pagination.work`: `implementation` and `examples`
/// independent, `audit` after both.
fn pagination_run(workers: u32) -> ChangeRun {
    ChangeRun::open(
        change_record(workers, 900),
        vec![
            (spec("implementation", &[]), Grant::default()),
            (spec("examples", &[]), Grant::default()),
            (
                spec("audit", &["implementation", "examples"]),
                Grant::default(),
            ),
        ],
        baseline(Vec::new(), empty_facets()),
    )
    .expect("the run opens")
}

/// A host sandbox that records launches.
struct FakeSandbox {
    enforceable: ExecutionBoundary,
    /// The boundary to report; the requested one when `None`.
    report: Option<ExecutionBoundary>,
    fail: Option<String>,
    launched: Vec<(String, u64)>,
}

impl FakeSandbox {
    fn new() -> FakeSandbox {
        FakeSandbox {
            enforceable: ExecutionBoundary::default(),
            report: None,
            fail: None,
            launched: Vec::new(),
        }
    }
}

impl Sandbox for FakeSandbox {
    fn enforceable(&self) -> ExecutionBoundary {
        self.enforceable
    }

    fn launch(
        &mut self,
        lease: &Lease,
        boundary: &ExecutionBoundary,
    ) -> Result<ExecutionBoundary, SandboxError> {
        self.launched.push((lease.task.name.clone(), lease.token.0));
        match &self.fail {
            Some(message) => Err(SandboxError {
                message: message.clone(),
            }),
            None => Ok(self.report.unwrap_or(*boundary)),
        }
    }
}

fn worktree(name: &str) -> PathBuf {
    PathBuf::from(format!("/work/{name}"))
}

fn assign(run: &mut ChangeRun, task: &str, now: &Timestamp) -> Lease {
    run.assign(
        task,
        &format!("{task}-worker"),
        worktree(task),
        &mut FakeSandbox::new(),
        now,
    )
    .expect("assigned")
}

fn submission(lease: &Lease, tree: &str) -> Submission {
    Submission {
        attempt: lease.attempt,
        token: lease.token,
        tree: tree.to_string(),
    }
}

fn state(run: &ChangeRun, task: &str) -> TaskState {
    run.task(task).expect("a task").state
}

/// Run `task` from `Ready` to `Accepted`; returns its candidate.
fn accept(run: &mut ChangeRun, task: &str, tree: &str, now: &Timestamp) -> CandidateId {
    let lease = assign(run, task, now);
    let outcome = run
        .submit(task, &submission(&lease, tree), now)
        .expect("submitted");
    run.validation_passed(task, now).expect("accepted");
    outcome.id().clone()
}

fn stale_reason(error: ScheduleError) -> String {
    match error {
        ScheduleError::StaleSubmission { reason, .. } => reason,
        other => panic!("expected a stale submission, found {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------

#[test]
fn opening_makes_initial_tasks_ready_and_dependents_blocked() {
    let run = pagination_run(3);
    assert_eq!(state(&run, "implementation"), TaskState::Ready);
    assert_eq!(state(&run, "examples"), TaskState::Ready);
    assert_eq!(state(&run, "audit"), TaskState::Blocked);
    let audit = run.task("audit").expect("audit");
    assert_eq!(
        audit.blocked.as_deref(),
        Some("waiting_for: [implementation, examples]")
    );
    assert!(audit.baseline.is_none());
    let implementation = run.task("implementation").expect("implementation");
    assert_eq!(implementation.attempt, 0);
    assert!(implementation
        .baseline
        .as_ref()
        .is_some_and(|b| b.predecessors.is_empty()));
    assert!(implementation.history.is_empty());
    let names: Vec<String> = run.change.tasks.iter().map(|t| t.to_string()).collect();
    assert_eq!(
        names,
        [
            "chg_mzxw6ytbmzxw6ytb/implementation",
            "chg_mzxw6ytbmzxw6ytb/examples",
            "chg_mzxw6ytbmzxw6ytb/audit",
        ]
    );
    assert_eq!(run.running(), 0);
    assert!(run.ready_to_unblock().is_empty());
}

#[test]
fn opening_rejects_unknown_predecessors_cycles_and_duplicates() {
    let open = |tasks: Vec<TaskSpec>| {
        ChangeRun::open(
            change_record(1, 900),
            tasks.into_iter().map(|t| (t, Grant::default())).collect(),
            baseline(Vec::new(), empty_facets()),
        )
    };
    let unknown = open(vec![spec("a", &["ghost"])]).expect_err("unknown");
    assert_eq!(unknown.code().map(|c| c.0), Some("work_unknown_task"));
    let cycle = open(vec![
        spec("a", &["c"]),
        spec("b", &["a"]),
        spec("c", &["b"]),
    ])
    .expect_err("cycle");
    assert_eq!(
        cycle,
        ScheduleError::Cycle {
            tasks: vec!["a".into(), "c".into(), "b".into(), "a".into()]
        }
    );
    assert_eq!(cycle.to_diagnostic().code, "work_cycle");
    let duplicate = open(vec![spec("a", &[]), spec("a", &[])]).expect_err("duplicate");
    assert!(matches!(duplicate, ScheduleError::DuplicateTask { .. }));
    assert_eq!(duplicate.to_diagnostic().code, "internal_error");
}

#[test]
fn opening_refuses_a_change_without_workers() {
    // 10.2 refuses at opening what `cpl work check` refuses (7.2,
    // `work_bad_workers`): a run whose ceiling is 0 could assign no task.
    let open = |workers: u32| {
        ChangeRun::open(
            change_record(workers, 900),
            vec![(spec("implementation", &[]), Grant::default())],
            baseline(Vec::new(), empty_facets()),
        )
    };
    let error = open(0).expect_err("no workers");
    assert_eq!(
        error,
        ScheduleError::BadWorkers {
            change: "pagination_update".into(),
            workers: 0
        }
    );
    assert_eq!(error.code().map(|c| c.0), Some("work_bad_workers"));
    let d = error.to_diagnostic();
    assert_eq!(d.code, "work_bad_workers");
    assert_eq!(d.category, cpl_diag::Category::Work);
    assert_eq!(d.data["change"], serde_json::json!("pagination_update"));
    assert_eq!(d.data["found"], serde_json::json!(0));
    assert_eq!(
        d.message,
        "change `pagination_update` has `workers 0`; the concurrency ceiling must be at least 1"
    );
    // The refusal comes before every task rule.
    let refused = ChangeRun::open(
        change_record(0, 900),
        vec![(spec("a", &["phantom"]), Grant::default())],
        baseline(Vec::new(), empty_facets()),
    )
    .expect_err("no workers");
    assert!(matches!(refused, ScheduleError::BadWorkers { .. }));
    // One worker is enough.
    let run = open(1).expect("the run opens");
    assert_eq!(run.change.workers, 1);
    assert_eq!(state(&run, "implementation"), TaskState::Ready);
}

// ---------------------------------------------------------------------------
// Assignment and leases
// ---------------------------------------------------------------------------

#[test]
fn assignment_issues_a_lease_with_a_fresh_token() {
    let mut run = pagination_run(3);
    let mut sandbox = FakeSandbox::new();
    let lease = run
        .assign(
            "implementation",
            "agent-1",
            worktree("impl"),
            &mut sandbox,
            &at(0),
        )
        .expect("assigned");
    assert_eq!(lease.token, FencingToken(1));
    assert_eq!(lease.attempt, 1);
    assert_eq!(lease.holder, "agent-1");
    assert_eq!(lease.issued_at, at(0));
    assert_eq!(lease.expires_at, at(900));
    assert_eq!(lease.expires_at.0, "2026-10-28T12:15:00Z");
    assert_eq!(lease.boundary, ExecutionBoundary::default());
    assert_eq!(sandbox.launched, [("implementation".to_string(), 1)]);
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Running);
    assert_eq!(record.lease.as_ref(), Some(&lease));
    assert_eq!(record.history.len(), 1);
    assert_eq!(record.history[0].from, TaskState::Ready);
    assert_eq!(record.history[0].to, TaskState::Running);
    assert_eq!(record.history[0].token, Some(FencingToken(1)));
    assert_eq!(run.change.next_token, 2);
    // Tokens strictly increase across the tasks of the run.
    let second = assign(&mut run, "examples", &at(5));
    assert_eq!(second.token, FencingToken(2));
    assert_eq!(run.running(), 2);
    // Only a Ready task can be assigned.
    let again = run
        .assign(
            "implementation",
            "agent-2",
            worktree("x"),
            &mut FakeSandbox::new(),
            &at(6),
        )
        .expect_err("running");
    assert!(matches!(
        again,
        ScheduleError::InvalidTaskState {
            state: TaskState::Running,
            ..
        }
    ));
    let blocked = run
        .assign(
            "audit",
            "agent-2",
            worktree("x"),
            &mut FakeSandbox::new(),
            &at(6),
        )
        .expect_err("blocked");
    assert!(matches!(
        blocked,
        ScheduleError::InvalidTaskState {
            state: TaskState::Blocked,
            ..
        }
    ));
}

#[test]
fn assignment_respects_the_workers_ceiling() {
    let mut run = pagination_run(1);
    let lease = assign(&mut run, "implementation", &at(0));
    let busy = run
        .assign(
            "examples",
            "agent-2",
            worktree("ex"),
            &mut FakeSandbox::new(),
            &at(1),
        )
        .expect_err("busy");
    assert!(matches!(
        busy,
        ScheduleError::WorkersBusy { workers: 1, .. }
    ));
    assert_eq!(state(&run, "examples"), TaskState::Ready);
    assert_eq!(run.change.next_token, 2, "no token spent");
    run.submit("implementation", &submission(&lease, "tree1"), &at(2))
        .expect("submitted");
    assert_eq!(assign(&mut run, "examples", &at(3)).token, FencingToken(2));
}

// `Sandbox::enforceable` is the strictest boundary the host can enforce, so
// a grant is enforceable iff `enforceable().within(&grant.boundary)`: 10.8's
// "host" row (a host with no mechanism reports host/any/any "and can
// therefore run only tasks whose policy grants exactly that"). Its "refusal"
// row writes the comparison the other way, which this test followed before.
#[test]
fn unenforceable_boundary_issues_no_lease() {
    let mut run = pagination_run(3);
    let wide = ExecutionBoundary {
        filesystem: FsAccess::WorktreeAndRepository,
        process: ProcessAccess::Toolchain,
        network: NetworkAccess::None,
    };
    run.tasks[0].grant.boundary = wide;
    // A host that cannot confine a worker to the repository and the cpl
    // binary: one with no mechanism at all.
    let mut sandbox = FakeSandbox::new();
    sandbox.enforceable = ExecutionBoundary {
        filesystem: FsAccess::Host,
        process: ProcessAccess::Any,
        network: NetworkAccess::Any,
    };
    let error = run
        .assign(
            "implementation",
            "agent",
            worktree("impl"),
            &mut sandbox,
            &at(0),
        )
        .expect_err("unenforceable");
    assert_eq!(
        error.code().map(|c| c.0),
        Some("work_boundary_unenforceable")
    );
    assert!(error
        .to_string()
        .contains("filesystem=worktree_and_repository process=toolchain network=none"));
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Ready);
    assert!(record.lease.is_none());
    assert_eq!(record.attempt, 0);
    assert!(sandbox.launched.is_empty(), "no worker is started");
    assert_eq!(run.change.next_token, 1, "no token is issued");
    // A host that can confine a worker that far (or further) runs the task
    // under it.
    sandbox.enforceable = ExecutionBoundary::default();
    let lease = run
        .assign(
            "implementation",
            "agent",
            worktree("impl"),
            &mut sandbox,
            &at(1),
        )
        .expect("assigned");
    assert_eq!(lease.boundary, wide);
}

#[test]
fn launch_reports_are_recorded_and_wider_ones_refused() {
    let mut run = pagination_run(3);
    run.tasks[0].grant.boundary = ExecutionBoundary {
        filesystem: FsAccess::WorktreeAndRepository,
        process: ProcessAccess::Toolchain,
        network: NetworkAccess::Adapters,
    };
    // A host that can confine a worker completely, so it can enforce every
    // boundary (10.8 "host"; see unenforceable_boundary_issues_no_lease).
    let mut sandbox = FakeSandbox::new();
    sandbox.enforceable = ExecutionBoundary::default();
    // Stricter than granted: recorded, never more than enforced (10.8).
    let strict = ExecutionBoundary {
        filesystem: FsAccess::Worktree,
        process: ProcessAccess::Toolchain,
        network: NetworkAccess::None,
    };
    sandbox.report = Some(strict);
    let lease = run
        .assign(
            "implementation",
            "agent",
            worktree("impl"),
            &mut sandbox,
            &at(0),
        )
        .expect("assigned");
    assert_eq!(lease.boundary, strict);
    // Wider than granted: refused, the task stays Ready, the token is spent.
    sandbox.report = Some(ExecutionBoundary {
        filesystem: FsAccess::Host,
        process: ProcessAccess::None,
        network: NetworkAccess::None,
    });
    let error = run
        .assign("examples", "agent", worktree("ex"), &mut sandbox, &at(1))
        .expect_err("wider");
    assert!(matches!(error, ScheduleError::LaunchExceededGrant { .. }));
    assert_eq!(
        error.code().map(|c| c.0),
        Some("work_boundary_unenforceable")
    );
    assert_eq!(state(&run, "examples"), TaskState::Ready);
    assert_eq!(run.change.next_token, 3);
}

#[test]
fn launch_failure_spends_the_token_but_keeps_the_task_ready() {
    let mut run = pagination_run(3);
    let mut sandbox = FakeSandbox::new();
    sandbox.fail = Some("no sandbox slots".to_string());
    let error = run
        .assign(
            "implementation",
            "agent",
            worktree("impl"),
            &mut sandbox,
            &at(0),
        )
        .expect_err("launch failure");
    assert!(error.to_string().contains("no sandbox slots"));
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Ready);
    assert_eq!(record.attempt, 0);
    assert!(record.lease.is_none());
    assert_eq!(
        run.change.next_token, 2,
        "the token handed to the sandbox is spent"
    );
    assert_eq!(
        assign(&mut run, "implementation", &at(1)).token,
        FencingToken(2)
    );
}

#[test]
fn renewal_extends_the_current_lease_only() {
    let mut run = pagination_run(3);
    let lease = assign(&mut run, "implementation", &at(0));
    let renewed = run
        .renew("implementation", lease.token, &at(600))
        .expect("renewed");
    assert_eq!(renewed, at(1500));
    let record = run.task("implementation").expect("implementation");
    assert_eq!(
        record.lease.as_ref().map(|l| &l.expires_at),
        Some(&at(1500))
    );
    assert_eq!(record.state, TaskState::Running);
    let last = record.history.last().expect("a transition");
    assert_eq!(
        (last.from, last.to),
        (TaskState::Running, TaskState::Running)
    );
    assert_eq!(last.reason, "lease_renewed");
    // A token that was never issued, and a renewal after expiry.
    let wrong = run
        .renew("implementation", FencingToken(7), &at(700))
        .expect_err("wrong token");
    assert_eq!(wrong.code().map(|c| c.0), Some("work_stale_submission"));
    let late = run
        .renew("implementation", lease.token, &at(1500))
        .expect_err("expired");
    assert!(stale_reason(late).contains("expired"));
}

#[test]
fn expired_lease_returns_the_task_to_ready() {
    let mut run = pagination_run(3);
    let lease = assign(&mut run, "implementation", &at(0));
    assert_eq!(
        run.expire_leases(&at(899)).expect("valid time"),
        Vec::<String>::new()
    );
    assert_eq!(
        run.expire_leases(&at(900)).expect("valid time"),
        ["implementation"]
    );
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Ready);
    assert_eq!(
        record.history.last().map(|t| t.reason.as_str()),
        Some("lease_expired")
    );
    // The lease stays recorded (its worktree is kept until reassignment).
    assert_eq!(record.lease.as_ref(), Some(&lease));
    // The expired worker cannot submit: its assignment has ended.
    let late = run
        .submit("implementation", &submission(&lease, "tree1"), &at(901))
        .expect_err("stale");
    assert!(stale_reason(late).contains("has ended"));
    // Reassignment: the next attempt with a fresh token.
    let second = assign(&mut run, "implementation", &at(902));
    assert_eq!((second.attempt, second.token), (2, FencingToken(2)));
}

#[test]
fn superseded_assignment_cannot_overwrite_current_work() {
    let mut run = pagination_run(3);
    let first = assign(&mut run, "implementation", &at(0));
    run.expire_leases(&at(1000)).expect("valid time");
    let second = assign(&mut run, "implementation", &at(1001));
    // The old worker comes back with its result: fenced.
    let error = run
        .submit("implementation", &submission(&first, "old-tree"), &at(1002))
        .expect_err("superseded");
    assert_eq!(error.code().map(|c| c.0), Some("work_stale_submission"));
    assert!(stale_reason(error).contains("lower than the current token 2"));
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Running);
    assert!(record.candidate.is_none(), "nothing changed");
    // Its failure report cannot reset the new assignment either.
    let report = run
        .report_failure("implementation", first.token, "crashed", &at(1003))
        .expect_err("superseded");
    assert_eq!(report.code().map(|c| c.0), Some("work_stale_submission"));
    // The current worker submits.
    let outcome = run
        .submit(
            "implementation",
            &submission(&second, "new-tree"),
            &at(1004),
        )
        .expect("submitted");
    let expected = CandidateId::derive(
        &record_id(&run, "implementation"),
        2,
        FencingToken(2),
        "new-tree",
    );
    assert_eq!(outcome, SubmitOutcome::Recorded(expected));
    assert_eq!(state(&run, "implementation"), TaskState::Submitted);
    // A token the run never issued for the task is refused too.
    let mut forged = submission(&second, "forged");
    forged.token = FencingToken(99);
    let mut fresh = pagination_run(3);
    let lease = assign(&mut fresh, "examples", &at(0));
    forged.attempt = lease.attempt;
    let error = fresh
        .submit("examples", &forged, &at(1))
        .expect_err("never issued");
    assert!(stale_reason(error).contains("never issued"));
}

fn record_id(run: &ChangeRun, task: &str) -> cpl_work::TaskId {
    run.task(task).expect("a task").id.clone()
}

#[test]
fn resubmission_is_idempotent() {
    let mut run = pagination_run(3);
    let lease = assign(&mut run, "implementation", &at(0));
    let first = run
        .submit("implementation", &submission(&lease, "tree1"), &at(10))
        .expect("submitted");
    let SubmitOutcome::Recorded(id) = first.clone() else {
        panic!("recorded first");
    };
    assert!(id.as_str().starts_with("cand_") && id.as_str().len() == 37);
    let snapshot = run.clone();
    let again = run
        .submit("implementation", &submission(&lease, "tree1"), &at(11))
        .expect("idempotent");
    assert_eq!(again, SubmitOutcome::Unchanged(id.clone()));
    assert_eq!(run, snapshot, "a repeated submission changes nothing");
    // Still idempotent after the lease expired and the candidate was accepted.
    run.validation_passed("implementation", &at(12))
        .expect("accepted");
    let late = run
        .submit("implementation", &submission(&lease, "tree1"), &at(5000))
        .expect("idempotent");
    assert_eq!(late, SubmitOutcome::Unchanged(id));
    assert_eq!(state(&run, "implementation"), TaskState::Accepted);
    // Another tree for the same assignment is not a resubmission.
    let other = run
        .submit("implementation", &submission(&lease, "tree2"), &at(13))
        .expect_err("not the same candidate");
    assert_eq!(other.code().map(|c| c.0), Some("work_stale_submission"));
}

#[test]
fn submission_after_expiry_is_stale_before_the_lease_is_voided() {
    let mut run = pagination_run(3);
    let lease = assign(&mut run, "implementation", &at(0));
    let snapshot = run.clone();
    let error = run
        .submit("implementation", &submission(&lease, "tree"), &at(900))
        .expect_err("expired");
    assert!(stale_reason(error).contains("expired at 2026-10-28T12:15:00Z"));
    assert_eq!(run, snapshot);
    // A submission one second earlier is in time.
    assert!(run
        .submit("implementation", &submission(&lease, "tree"), &at(899))
        .is_ok());
}

#[test]
fn validation_outcomes() {
    let mut run = pagination_run(3);
    let lease = assign(&mut run, "implementation", &at(0));
    run.submit("implementation", &submission(&lease, "tree1"), &at(1))
        .expect("submitted");
    run.validation_failed("implementation", "patch_out_of_scope", &at(2))
        .expect("rejected");
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Ready);
    assert_eq!(
        record.history.last().map(|t| t.reason.as_str()),
        Some("validation_failed: patch_out_of_scope")
    );
    let second = assign(&mut run, "implementation", &at(3));
    assert_eq!(second.attempt, 2);
    run.submit("implementation", &submission(&second, "tree2"), &at(4))
        .expect("submitted");
    run.validation_passed("implementation", &at(5))
        .expect("accepted");
    assert_eq!(state(&run, "implementation"), TaskState::Accepted);
    let not_submitted = run
        .validation_passed("examples", &at(6))
        .expect_err("ready");
    assert!(matches!(
        not_submitted,
        ScheduleError::InvalidTaskState { .. }
    ));
}

#[test]
fn dependents_unblock_when_every_predecessor_is_accepted() {
    let mut run = pagination_run(3);
    let implementation = accept(&mut run, "implementation", "tree-i", &at(0));
    assert!(run.ready_to_unblock().is_empty());
    let pending = run
        .unblock(
            "audit",
            baseline(vec![implementation.clone()], empty_facets()),
            &at(1),
        )
        .expect_err("examples is pending");
    assert_eq!(
        pending,
        ScheduleError::PredecessorsPending {
            task: "audit".into(),
            pending: vec!["examples".into()]
        }
    );
    let examples = accept(&mut run, "examples", "tree-e", &at(2));
    assert_eq!(run.ready_to_unblock(), ["audit"]);
    // A snapshot missing a predecessor's current candidate is refused.
    let stale = run
        .unblock(
            "audit",
            baseline(vec![implementation.clone()], empty_facets()),
            &at(3),
        )
        .expect_err("stale snapshot");
    assert!(matches!(stale, ScheduleError::StaleBaseline { .. }));
    run.unblock(
        "audit",
        baseline(
            vec![implementation.clone(), examples.clone()],
            empty_facets(),
        ),
        &at(4),
    )
    .expect("unblocked");
    let audit = run.task("audit").expect("audit");
    assert_eq!(audit.state, TaskState::Ready);
    assert!(audit.blocked.is_none());
    assert_eq!(
        audit.baseline.as_ref().map(|b| b.predecessors.clone()),
        Some(vec![implementation, examples])
    );
}

#[test]
fn failures_propagate_to_blocked_dependents() {
    let mut run = ChangeRun::open(
        change_record(3, 900),
        vec![
            (spec("a", &[]), Grant::default()),
            (spec("b", &["a"]), Grant::default()),
            (spec("c", &["b"]), Grant::default()),
            (spec("d", &[]), Grant::default()),
        ],
        baseline(Vec::new(), empty_facets()),
    )
    .expect("opens");
    assert!(run.propagate_failures(&at(0)).is_empty());
    run.cancel("a", &at(1)).expect("cancelled");
    assert_eq!(run.propagate_failures(&at(2)), ["b", "c"]);
    assert_eq!(state(&run, "b"), TaskState::Failed);
    assert_eq!(
        run.task("b")
            .and_then(|t| t.history.last())
            .map(|t| t.reason.as_str()),
        Some("predecessor_cancelled: a")
    );
    assert_eq!(
        run.task("c")
            .and_then(|t| t.history.last())
            .map(|t| t.reason.as_str()),
        Some("predecessor_failed: b")
    );
    assert_eq!(state(&run, "d"), TaskState::Ready);
    // An unresolvable assembly conflict fails a blocked task directly.
    let mut run = pagination_run(3);
    run.fail_blocked("audit", "work_conflict: cand_a cand_b", &at(0))
        .expect("failed");
    assert_eq!(state(&run, "audit"), TaskState::Failed);
    assert!(run.fail_blocked("implementation", "x", &at(1)).is_err());
}

#[test]
fn invalidation_blocks_accepted_dependents() {
    let mut run = pagination_run(3);
    let implementation = accept(&mut run, "implementation", "tree-i", &at(0));
    let examples = accept(&mut run, "examples", "tree-e", &at(1));
    run.unblock(
        "audit",
        baseline(
            vec![implementation.clone(), examples.clone()],
            empty_facets(),
        ),
        &at(2),
    )
    .expect("unblocked");
    accept(&mut run, "audit", "tree-a", &at(3));
    // The audit's inputs are current: nothing to invalidate.
    assert!(run
        .invalidate_dependents("implementation", &at(4))
        .is_empty());
    // implementation re-runs: invalidated, reassigned, accepted with a new candidate.
    run.invalidate("implementation", &examples, &at(5))
        .expect("invalidated");
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Blocked);
    assert_eq!(
        record.blocked.as_deref(),
        Some(&*format!("invalidated_by: {examples}"))
    );
    run.unblock(
        "implementation",
        baseline(Vec::new(), empty_facets()),
        &at(6),
    )
    .expect("no predecessors");
    let rerun = accept(&mut run, "implementation", "tree-i2", &at(7));
    assert_ne!(rerun, implementation);
    assert_eq!(
        run.invalidate_dependents("implementation", &at(8)),
        ["audit"]
    );
    let audit = run.task("audit").expect("audit");
    assert_eq!(audit.state, TaskState::Blocked);
    assert_eq!(
        audit.blocked.as_deref(),
        Some(&*format!("invalidated_by: {rerun}"))
    );
    assert!(run.invalidate("examples", &rerun, &at(9)).is_ok());
    assert!(
        run.invalidate("examples", &rerun, &at(10)).is_err(),
        "only from Accepted"
    );
}

#[test]
fn cancellation() {
    let mut run = pagination_run(3);
    let lease = assign(&mut run, "implementation", &at(0));
    run.cancel("implementation", &at(1)).expect("cancelled");
    assert_eq!(state(&run, "implementation"), TaskState::Cancelled);
    let late = run
        .submit("implementation", &submission(&lease, "tree"), &at(2))
        .expect_err("cancelled");
    assert_eq!(late.code().map(|c| c.0), Some("work_stale_submission"));
    assert!(run.cancel("implementation", &at(3)).is_err(), "terminal");
    assert_eq!(
        run.cancel_change(&at(4)).expect("cancelled"),
        ["examples", "audit"]
    );
    assert_eq!(run.change.state, ChangeState::Cancelled);
    for task in ["examples", "audit"] {
        assert_eq!(state(&run, task), TaskState::Cancelled);
    }
    // A closed run refuses every further change.
    let error = run
        .assign(
            "examples",
            "agent",
            worktree("x"),
            &mut FakeSandbox::new(),
            &at(5),
        )
        .expect_err("closed");
    assert!(matches!(error, ScheduleError::InvalidChangeState { .. }));
    assert!(run.cancel_change(&at(6)).is_err());
}

fn accept_all(run: &mut ChangeRun) {
    let implementation = accept(run, "implementation", "tree-i", &at(0));
    let examples = accept(run, "examples", "tree-e", &at(1));
    run.unblock(
        "audit",
        baseline(vec![implementation, examples], empty_facets()),
        &at(2),
    )
    .expect("unblocked");
    accept(run, "audit", "tree-a", &at(3));
}

#[test]
fn integration_needs_authority_and_ends_the_run() {
    let mut run = pagination_run(3);
    accept_all(&mut run);
    let worker = Actor {
        id: "worker".to_string(),
        authority: Authority::default(),
    };
    let snapshot = run.clone();
    let error = run.begin_integration(&worker).expect_err("not authorized");
    assert_eq!(error.code().map(|c| c.0), Some("work_not_authorized"));
    assert_eq!(run, snapshot, "nothing changes");
    let integrator = Actor {
        id: "integrator".to_string(),
        authority: Authority {
            may_integrate: true,
            ..Authority::default()
        },
    };
    run.begin_integration(&integrator).expect("integrating");
    assert_eq!(run.change.state, ChangeState::Integrating);
    assert!(
        run.begin_integration(&integrator).is_err(),
        "already integrating"
    );
    let commit = "9".repeat(40);
    assert_eq!(
        run.finish_integration(&commit, &at(10))
            .expect("integrated"),
        ["implementation", "examples", "audit"]
    );
    assert_eq!(run.change.state, ChangeState::Integrated { commit });
    assert_eq!(state(&run, "audit"), TaskState::Integrated);
    assert!(run.cancel_change(&at(11)).is_err());
}

#[test]
fn stale_candidates_block_or_fail_integration() {
    let integrator = Actor {
        id: "integrator".to_string(),
        authority: Authority {
            may_integrate: true,
            ..Authority::default()
        },
    };
    let mut run = pagination_run(3);
    accept_all(&mut run);
    assert!(
        run.stale_against_target("implementation", &at(9)).is_err(),
        "not integrating"
    );
    run.begin_integration(&integrator).expect("integrating");
    run.stale_against_target("implementation", &at(10))
        .expect("stale");
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Blocked);
    assert_eq!(record.blocked.as_deref(), Some("work_stale_against_target"));
    assert_eq!(run.change.state, ChangeState::Open);

    let mut run = pagination_run(3);
    accept_all(&mut run);
    run.begin_integration(&integrator).expect("integrating");
    let snapshot = run.clone();
    assert!(run
        .fail_integration(
            "target moved 5 times",
            &["implementation", "ghost"],
            &at(10)
        )
        .is_err());
    assert_eq!(run, snapshot, "nothing changes on a refused call");
    run.fail_integration("target moved 5 times", &["implementation"], &at(11))
        .expect("failed");
    assert_eq!(state(&run, "implementation"), TaskState::Failed);
    assert_eq!(state(&run, "examples"), TaskState::Accepted);
    assert_eq!(
        run.change.state,
        ChangeState::Failed {
            reason: "target moved 5 times".into()
        }
    );
}

#[test]
fn worker_failure_returns_the_task_to_ready() {
    let mut run = pagination_run(3);
    let lease = assign(&mut run, "implementation", &at(0));
    run.report_failure("implementation", lease.token, "out of memory", &at(5))
        .expect("reported");
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Ready);
    assert_eq!(
        record.history.last().map(|t| t.reason.as_str()),
        Some("worker_failed: out of memory")
    );
    assert!(run
        .report_failure("implementation", lease.token, "again", &at(6))
        .is_err());
}

#[test]
fn invalid_timestamps_are_refused() {
    let mut run = pagination_run(3);
    let bad = Timestamp("yesterday".to_string());
    let error = run
        .assign(
            "implementation",
            "agent",
            worktree("x"),
            &mut FakeSandbox::new(),
            &bad,
        )
        .expect_err("invalid time");
    assert_eq!(
        error,
        ScheduleError::InvalidTimestamp {
            value: "yesterday".into()
        }
    );
    assert_eq!(run.change.next_token, 1);
    let lease = assign(&mut run, "implementation", &at(0));
    assert!(matches!(
        run.submit("implementation", &submission(&lease, "t"), &bad),
        Err(ScheduleError::InvalidTimestamp { .. })
    ));
    assert!(run.expire_leases(&bad).is_err());
    assert_eq!(state(&run, "implementation"), TaskState::Running);
}

#[test]
fn errors_become_diagnostics_with_their_codes() {
    let stale = ScheduleError::StaleSubmission {
        task: "implementation".into(),
        reason: "the lease expired".into(),
    };
    let d = stale.to_diagnostic();
    assert_eq!(d.code, "work_stale_submission");
    assert_eq!(d.category, cpl_diag::Category::Work);
    assert_eq!(d.data["task"], serde_json::json!("implementation"));
    assert_eq!(
        d.message,
        "stale submission for task `implementation`: the lease expired"
    );
    let busy = ScheduleError::WorkersBusy {
        change: "chg_x".into(),
        workers: 2,
    };
    assert_eq!(busy.code(), None);
    assert_eq!(busy.to_diagnostic().category, cpl_diag::Category::Internal);
}

// ---------------------------------------------------------------------------
// Persistence (10.1, T33)
// ---------------------------------------------------------------------------

#[test]
fn change_run_round_trips_through_json() {
    let reader = map_of(&reader_tree());
    let mut run = ChangeRun::open(
        change_record(3, 900),
        vec![
            (spec("implementation", &[]), Grant::default()),
            (spec("audit", &["implementation"]), Grant::default()),
        ],
        baseline(Vec::new(), reader),
    )
    .expect("opens");
    let lease = assign(&mut run, "implementation", &at(0));
    run.submit("implementation", &submission(&lease, "tree"), &at(1))
        .expect("submitted");
    let text = serde_json::to_string_pretty(&run).expect("serializes");
    let back: ChangeRun = serde_json::from_str(&text).expect("deserializes");
    assert_eq!(back, run);
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["change"]["state"], "open");
    assert_eq!(value["tasks"][0]["state"], "submitted");
    assert_eq!(value["tasks"][1]["state"], "blocked");
    assert_eq!(value["tasks"][0]["lease"]["token"], 1);
    assert_eq!(
        value["tasks"][0]["lease"]["expires_at"],
        "2026-10-28T12:15:00Z"
    );
}

#[test]
fn coordinator_restart_voids_expired_leases() {
    let root = temp_dir("restart");
    let mut coordinator = Coordinator::new(&root);
    let mut run = pagination_run(3);
    let short = assign(&mut run, "implementation", &at(0));
    let id = run.change.id.clone();
    coordinator.insert(run);
    coordinator
        .runs
        .get_mut(&id)
        .expect("run")
        .renew("implementation", short.token, &at(100))
        .expect("renewed");
    {
        let run = coordinator.runs.get_mut(&id).expect("run");
        // `examples` gets a lease that outlives the restart.
        assign(run, "examples", &at(500));
    }
    coordinator.save().expect("saved");
    let file = coordinator.state_file(&id);
    assert_eq!(file, root.join(".cpl/work/chg_mzxw6ytbmzxw6ytb.json"));
    assert!(std::fs::read_to_string(&file)
        .expect("written")
        .ends_with("}\n"));

    // Restart at T0 + 1100: implementation's lease (until 1000) is voided,
    // examples' (until 1400) survives.
    let (restarted, voided) = Coordinator::restart(&root, &at(1100)).expect("restarted");
    assert_eq!(voided.len(), 1);
    assert_eq!(voided[0].to_string(), "chg_mzxw6ytbmzxw6ytb/implementation");
    let run = &restarted.runs[&id];
    assert_eq!(state(run, "implementation"), TaskState::Ready);
    assert_eq!(state(run, "examples"), TaskState::Running);
    // The worktree of the voided lease is still recorded for reassignment.
    assert_eq!(
        run.task("implementation")
            .and_then(|t| t.lease.as_ref())
            .map(|l| l.worktree.clone()),
        Some(worktree("implementation"))
    );
    // Without voiding, the loaded state equals the saved one.
    assert_eq!(Coordinator::load(&root).expect("loaded"), coordinator);
    // No state directory: no runs.
    assert!(Coordinator::load(&temp_dir("empty"))
        .expect("loaded")
        .runs
        .is_empty());
}

#[test]
fn coordinator_refuses_a_misnamed_state_file() {
    let root = temp_dir("misnamed");
    let mut coordinator = Coordinator::new(&root);
    let run = pagination_run(1);
    let id = run.change.id.clone();
    coordinator.insert(run);
    coordinator.save().expect("saved");
    let other = Coordinator::state_dir(&root).join("chg_aaaaaaaaaaaaaaaa.json");
    std::fs::rename(coordinator.state_file(&id), &other).expect("renamed");
    let error = Coordinator::load(&root).expect_err("misnamed");
    assert!(
        matches!(error, CoordinatorError::Misnamed { .. }),
        "{error}"
    );
    std::fs::write(&other, "{ not json").expect("written");
    assert!(matches!(
        Coordinator::load(&root),
        Err(CoordinatorError::Json { .. })
    ));
}
