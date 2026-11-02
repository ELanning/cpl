//! Adversarial tests of the work coordinator model (`spec/toolchain.md`
//! 10.1-10.4 and 10.6-10.8; design-doc.md "Changes and agent tasks", "Swarm
//! authority and lifecycle", and "Integration semantics"): every row of the
//! 10.2 transition table and the transitions outside it, lease renewal and
//! expiry, fencing tokens (an expired or superseded assignment cannot
//! submit), idempotent resubmission of one attempt and candidate, cycles and
//! unordered overlapping writes, dependents unblocked from snapshots of their
//! `after` predecessors, execution boundaries, the `.cpl/work/` state file,
//! and compare-and-swap ref updates against temporary repositories, a moved
//! target included.
//!
//! Every expectation is the specification's answer, not the current
//! behaviour. Where the specification is silent or contradicts itself, the
//! test follows the design document and its comment says so.

mod common;

use common::temp_dir;
use cpl_diag::json::{Digest, Revision, RevisionState, ToolchainInfo};
use cpl_diag::{Category, Facet, FacetSelector, FileId, Span, StableId};
use cpl_index::FacetMap;
use cpl_work::{
    commit_tree, update_ref_cas, worktree_add, write_tree, Actor, Authority, Baseline, CandidateId,
    ChangeRecord, ChangeRun, ChangeRunId, ChangeState, Coordinator, ExecutionBoundary,
    FencingToken, FrozenInputs, FsAccess, GitRepo, Grant, Lease, NetworkAccess, ProcessAccess,
    ResolvedNamespace, ResolvedSelector, RunPolicy, Sandbox, SandboxError, ScheduleError,
    Submission, SubmitOutcome, TaskId, TaskSpec, TaskState, Timestamp,
};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

// ===========================================================================
// Fixtures
// ===========================================================================

/// The commit every change run below was opened on.
const BASE: &str = "3f2e0c41a5f8c2e2b0f5e7a3d9c1b4a6e8f0a2c4";
/// 2026-10-28T12:00:00Z.
const T0: i64 = 1_793_188_800;
/// `[work] lease_seconds` of the frozen policy unless a test says otherwise.
const LEASE: u64 = 900;

/// `T0 + seconds`, spelled in UTC.
fn at(seconds: i64) -> Timestamp {
    Timestamp::from_unix_seconds(T0 + seconds).expect("in range")
}

/// A timestamp spelled exactly as given.
fn ts(text: &str) -> Timestamp {
    Timestamp(text.to_string())
}

fn change_record(workers: u32, lease_seconds: u64) -> ChangeRecord {
    let mut policy = RunPolicy::default();
    policy.work.lease_seconds = lease_seconds;
    ChangeRecord {
        id: ChangeRunId::from_random_bytes(*b"adversary!"),
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

fn span() -> Span {
    Span::new(FileId(0), 0, 0)
}

/// A task that requests nothing.
fn spec(name: &str, after: &[&str]) -> TaskSpec {
    TaskSpec {
        name: name.to_string(),
        span: span(),
        after: after.iter().map(|s| s.to_string()).collect(),
        write: Vec::new(),
        add: Vec::new(),
        write_files: Vec::new(),
        depend: Vec::new(),
        preserve: Vec::new(),
        goal: None,
    }
}

/// Tasks that request nothing, with empty grants.
fn plain(specs: Vec<TaskSpec>) -> Vec<(TaskSpec, Grant)> {
    specs.into_iter().map(|s| (s, Grant::default())).collect()
}

/// `reader.pages.paginate`'s `@id`.
fn paginate() -> StableId {
    StableId::parse("pgn_7c2d").expect("an @id")
}

/// A task with `write { paginate.<facet>, ... }`; its declaration's
/// selectors and the grant computed from them (10.3) agree.
fn writer(name: &str, after: &[&str], facets: &[Facet]) -> (TaskSpec, Grant) {
    let mut task = spec(name, after);
    task.write = facets
        .iter()
        .map(|facet| ResolvedSelector {
            text: format!("paginate.{}", facet.as_str()),
            span: span(),
            targets: vec![(paginate(), FacetSelector::Facet(*facet))],
        })
        .collect();
    let grant = Grant {
        write: facets.iter().map(|facet| (paginate(), *facet)).collect(),
        ..Grant::default()
    };
    (task, grant)
}

/// A task with `write { paginate }`: 10.3 grants `.definition` as every
/// applicable facet except `identity`.
fn definition_writer(name: &str, after: &[&str]) -> (TaskSpec, Grant) {
    let mut task = spec(name, after);
    task.write = vec![ResolvedSelector {
        text: "paginate".to_string(),
        span: span(),
        targets: vec![(paginate(), FacetSelector::Definition)],
    }];
    let grant = Grant {
        write: Facet::ALL
            .iter()
            .filter(|facet| **facet != Facet::Identity)
            .map(|facet| (paginate(), *facet))
            .collect(),
        ..Grant::default()
    };
    (task, grant)
}

/// A task with `add { <namespace>.* }`.
fn adder(name: &str, after: &[&str], namespace: &str) -> (TaskSpec, Grant) {
    let prefix: Vec<String> = namespace.split('.').map(str::to_string).collect();
    let mut task = spec(name, after);
    task.add = vec![ResolvedNamespace {
        text: format!("{namespace}.*"),
        span: span(),
        module_prefix: prefix.clone(),
    }];
    let grant = Grant {
        add: vec![prefix],
        ..Grant::default()
    };
    (task, grant)
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

/// A snapshot of the base commit with `predecessors` layered on (10.4, 5.5).
fn baseline(predecessors: Vec<CandidateId>) -> Baseline {
    let facets = empty_facets();
    Baseline {
        revision: facets.revision.clone(),
        snapshot_commit: BASE.to_string(),
        predecessors,
        facets,
    }
}

fn open(workers: u32, tasks: Vec<(TaskSpec, Grant)>) -> Result<ChangeRun, ScheduleError> {
    ChangeRun::open(change_record(workers, LEASE), tasks, baseline(Vec::new()))
}

/// The tasks of the design's `pagination_update`: `implementation` and
/// `examples` independent, `audit` after both.
fn pagination_tasks() -> Vec<(TaskSpec, Grant)> {
    plain(vec![
        spec("implementation", &[]),
        spec("examples", &[]),
        spec("audit", &["implementation", "examples"]),
    ])
}

fn pagination() -> ChangeRun {
    open(3, pagination_tasks()).expect("the run opens")
}

fn boundary(
    filesystem: FsAccess,
    process: ProcessAccess,
    network: NetworkAccess,
) -> ExecutionBoundary {
    ExecutionBoundary {
        filesystem,
        process,
        network,
    }
}

/// host/any/any: what a host without a sandbox mechanism reports (10.8).
fn unconfined() -> ExecutionBoundary {
    boundary(FsAccess::Host, ProcessAccess::Any, NetworkAccess::Any)
}

/// The per-field maximum (the wider of each pair).
fn widen(a: &ExecutionBoundary, b: &ExecutionBoundary) -> ExecutionBoundary {
    boundary(
        a.filesystem.max(b.filesystem),
        a.process.max(b.process),
        a.network.max(b.network),
    )
}

/// A host sandbox adapter (10.8). `enforceable()` reports `strictest`, the
/// strictest boundary the host can enforce; `launch` is honest: it reports
/// the boundary it actually enforces, which is the requested one widened,
/// field by field, to what the host can enforce.
struct HostSandbox {
    strictest: ExecutionBoundary,
    fail: Option<String>,
    /// Every launch: the lease handed over and the boundary requested.
    launches: Vec<(Lease, ExecutionBoundary)>,
}

impl HostSandbox {
    /// A host that can confine a worker to its worktree, with no child
    /// processes and no network: it can enforce every boundary.
    fn full() -> HostSandbox {
        HostSandbox::enforcing(ExecutionBoundary::default())
    }

    fn enforcing(strictest: ExecutionBoundary) -> HostSandbox {
        HostSandbox {
            strictest,
            fail: None,
            launches: Vec::new(),
        }
    }

    fn failing(message: &str) -> HostSandbox {
        HostSandbox {
            fail: Some(message.to_string()),
            ..HostSandbox::full()
        }
    }
}

impl Sandbox for HostSandbox {
    fn enforceable(&self) -> ExecutionBoundary {
        self.strictest
    }

    fn launch(
        &mut self,
        lease: &Lease,
        boundary: &ExecutionBoundary,
    ) -> Result<ExecutionBoundary, SandboxError> {
        self.launches.push((lease.clone(), *boundary));
        match &self.fail {
            Some(message) => Err(SandboxError {
                message: message.clone(),
            }),
            None => Ok(widen(boundary, &self.strictest)),
        }
    }
}

/// Assign `task` on a full sandbox.
#[track_caller]
fn assign(run: &mut ChangeRun, task: &str, now: &Timestamp) -> Lease {
    run.assign(
        task,
        &format!("{task}-worker"),
        PathBuf::from(format!("/wt/{task}")),
        &mut HostSandbox::full(),
        now,
    )
    .unwrap_or_else(|e| panic!("assign {task}: {e}"))
}

fn submission(lease: &Lease, tree: &str) -> Submission {
    Submission {
        attempt: lease.attempt,
        token: lease.token,
        tree: tree.to_string(),
    }
}

#[track_caller]
fn submit(
    run: &mut ChangeRun,
    task: &str,
    lease: &Lease,
    tree: &str,
    now: &Timestamp,
) -> CandidateId {
    run.submit(task, &submission(lease, tree), now)
        .unwrap_or_else(|e| panic!("submit {task}: {e}"))
        .id()
        .clone()
}

/// `task` from `Ready` to `Accepted`; its candidate.
#[track_caller]
fn accept(run: &mut ChangeRun, task: &str, tree: &str, now: &Timestamp) -> CandidateId {
    let lease = assign(run, task, now);
    let candidate = submit(run, task, &lease, tree, now);
    run.validation_passed(task, now)
        .unwrap_or_else(|e| panic!("accept {task}: {e}"));
    candidate
}

/// Every task of [`pagination`] accepted, `audit` from the snapshot of the
/// other two (predecessors in 10.4's order: topological, ties by task name).
/// Returns the candidates of implementation, examples, and audit.
fn accept_all(run: &mut ChangeRun) -> (CandidateId, CandidateId, CandidateId) {
    let implementation = accept(run, "implementation", "tree-i", &at(0));
    let examples = accept(run, "examples", "tree-e", &at(1));
    run.unblock(
        "audit",
        baseline(vec![examples.clone(), implementation.clone()]),
        &at(2),
    )
    .expect("audit unblocked");
    let audit = accept(run, "audit", "tree-a", &at(3));
    (implementation, examples, audit)
}

fn state(run: &ChangeRun, task: &str) -> TaskState {
    run.task(task).expect("a task").state
}

fn code(error: &ScheduleError) -> Option<&'static str> {
    error.code().map(|c| c.0)
}

/// The error of an operation the specification refuses.
#[track_caller]
fn refused<T>(result: Result<T, ScheduleError>, what: &str) -> ScheduleError {
    match result {
        Ok(_) => panic!("{what}: accepted, but the specification refuses it"),
        Err(error) => error,
    }
}

fn integrator() -> Actor {
    Actor {
        id: "integrator".to_string(),
        authority: Authority {
            may_integrate: true,
            ..Authority::default()
        },
    }
}

/// The operations a worker or the coordinator applies to one task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Unblock,
    FailBlocked,
    Assign,
    Renew,
    ReportFailure,
    Submit,
    Pass,
    Reject,
    Invalidate,
}

const OPS: [Op; 9] = [
    Op::Unblock,
    Op::FailBlocked,
    Op::Assign,
    Op::Renew,
    Op::ReportFailure,
    Op::Submit,
    Op::Pass,
    Op::Reject,
    Op::Invalidate,
];

/// Apply `op` to `task` at `now`, with the task's current lease (token and
/// attempt) when it has one, else token 1 and attempt 1; a submission is of a
/// tree no earlier submission used.
fn apply(run: &mut ChangeRun, task: &str, op: Op, now: &Timestamp) -> Result<(), ScheduleError> {
    let record = run.task(task).expect("a task");
    let (token, attempt) = record
        .lease
        .as_ref()
        .map_or((FencingToken(1), 1), |l| (l.token, l.attempt));
    let by = CandidateId::derive(&record.id, 99, FencingToken(99), "elsewhere");
    match op {
        Op::Unblock => run.unblock(task, baseline(Vec::new()), now),
        Op::FailBlocked => run.fail_blocked(task, "work_conflict: cand_a cand_b", now),
        Op::Assign => run
            .assign(
                task,
                "agent",
                PathBuf::from("/wt/op"),
                &mut HostSandbox::full(),
                now,
            )
            .map(|_| ()),
        Op::Renew => run.renew(task, token, now).map(|_| ()),
        Op::ReportFailure => run.report_failure(task, token, "crashed", now),
        Op::Submit => run
            .submit(
                task,
                &Submission {
                    attempt,
                    token,
                    tree: "tree-never-submitted".to_string(),
                },
                now,
            )
            .map(|_| ()),
        Op::Pass => run.validation_passed(task, now),
        Op::Reject => run.validation_failed(task, "type_mismatch", now),
        Op::Invalidate => run.invalidate(task, &by, now),
    }
}

/// One task in every non-terminal state of 10.2, in a run of six workers:
/// `ready` (never assigned), `running`, `submitted`, `accepted`,
/// `invalidated` (Accepted -> Blocked, no predecessors), and `waiting`
/// (Blocked after `ready`).
fn every_state() -> ChangeRun {
    let mut run = open(
        6,
        plain(vec![
            spec("ready", &[]),
            spec("running", &[]),
            spec("submitted", &[]),
            spec("accepted", &[]),
            spec("invalidated", &[]),
            spec("waiting", &["ready"]),
        ]),
    )
    .expect("the run opens");
    assign(&mut run, "running", &at(0));
    let lease = assign(&mut run, "submitted", &at(0));
    submit(&mut run, "submitted", &lease, "tree-submitted", &at(1));
    let accepted = accept(&mut run, "accepted", "tree-accepted", &at(2));
    accept(&mut run, "invalidated", "tree-invalidated", &at(3));
    run.invalidate("invalidated", &accepted, &at(4))
        .expect("invalidated");
    run
}

// ===========================================================================
// Opening: the `after` graph (grammar 4.1, 7.2, 10.2 "(new)" rows)
// ===========================================================================

#[test]
fn open_rejects_a_task_after_itself() {
    // Grammar 4.1 (work coordinator): "the graph must be acyclic"; 7.2 and
    // Appendix B `work_cycle`; design: "The scheduler rejects cycles".
    let error = refused(
        open(1, plain(vec![spec("a", &["a"])])),
        "a task after itself",
    );
    assert_eq!(code(&error), Some("work_cycle"));
    let diagnostic = error.to_diagnostic();
    assert_eq!(diagnostic.code, "work_cycle");
    assert_eq!(diagnostic.category, Category::Work);
}

#[test]
fn open_rejects_a_cycle_that_no_initial_task_reaches() {
    let error = refused(
        open(
            4,
            plain(vec![
                spec("x", &[]),
                spec("w", &["x"]),
                spec("y", &["z"]),
                spec("z", &["y"]),
            ]),
        ),
        "y and z wait for each other",
    );
    assert_eq!(code(&error), Some("work_cycle"));
}

#[test]
fn open_accepts_after_naming_a_task_declared_later() {
    // Grammar 4.1: `after` names "must refer to tasks of the same change"; the
    // order of the declarations is irrelevant.
    let run = open(
        2,
        plain(vec![
            spec("audit", &["implementation"]),
            spec("implementation", &[]),
        ]),
    )
    .expect("a later declaration is a task of the change");
    assert_eq!(state(&run, "audit"), TaskState::Blocked);
    assert_eq!(state(&run, "implementation"), TaskState::Ready);
}

#[test]
fn open_accepts_a_diamond_and_blocks_every_dependent() {
    // 10.2: (new) -> Ready without predecessors, (new) -> Blocked with the
    // reason `waiting_for: [...]`; 10.1 TaskRecord.baseline is "set when the
    // task becomes Ready".
    let run = open(
        4,
        plain(vec![
            spec("a", &[]),
            spec("b", &["a"]),
            spec("c", &["a"]),
            spec("d", &["b", "c"]),
        ]),
    )
    .expect("a diamond has no cycle");
    let a = run.task("a").expect("a");
    assert_eq!(a.state, TaskState::Ready);
    assert!(a
        .baseline
        .as_ref()
        .is_some_and(|b| b.predecessors.is_empty()));
    for (task, reason) in [
        ("b", "waiting_for: [a]"),
        ("c", "waiting_for: [a]"),
        ("d", "waiting_for: [b, c]"),
    ] {
        let record = run.task(task).expect("a task");
        assert_eq!(record.state, TaskState::Blocked, "{task}");
        assert_eq!(record.blocked.as_deref(), Some(reason), "{task}");
        assert!(record.baseline.is_none(), "{task} is not Ready yet");
    }
}

// ===========================================================================
// Opening: unordered overlapping writes
// ===========================================================================
//
// Design "Changes and agent tasks": "Overlapping write sets are rejected
// among unordered tasks by default"; "Swarm authority and lifecycle": "The
// scheduler rejects cycles and unordered overlapping edits; it does not
// resolve those conflicts by silently choosing the last writer". The
// specification states the rule, with its code `work_overlapping_writes`,
// only for `cpl work check` (7.2) and is silent about the scheduler; these
// tests follow the design, as `ChangeRun::open` already does for cycles.

#[test]
fn open_rejects_unordered_tasks_writing_one_facet() {
    let error = refused(
        open(
            2,
            vec![
                writer("a", &[], &[Facet::Body]),
                writer("b", &[], &[Facet::Body]),
            ],
        ),
        "two unordered tasks writing paginate.body",
    );
    assert_eq!(code(&error), Some("work_overlapping_writes"));
}

#[test]
fn open_rejects_overlapping_writes_of_unordered_siblings() {
    // 7.2: unordered means "neither reaches the other through `after`"; two
    // tasks after one root do not reach each other.
    let error = refused(
        open(
            3,
            vec![
                (spec("root", &[]), Grant::default()),
                writer("left", &["root"], &[Facet::Body]),
                writer("right", &["root"], &[Facet::Body]),
            ],
        ),
        "two siblings writing paginate.body",
    );
    assert_eq!(code(&error), Some("work_overlapping_writes"));
}

#[test]
fn open_rejects_a_definition_write_overlapping_one_facet_write() {
    // 7.2 compares "at facet level"; 10.3: `.definition` grants every
    // applicable facet except `identity`, so `body` is in both sets.
    let error = refused(
        open(
            2,
            vec![
                definition_writer("whole", &[]),
                writer("part", &[], &[Facet::Body]),
            ],
        ),
        "paginate and paginate.body written by unordered tasks",
    );
    assert_eq!(code(&error), Some("work_overlapping_writes"));
}

#[test]
fn open_rejects_unordered_tasks_adding_to_overlapping_namespaces() {
    // 7.2: "add namespaces of unordered tasks do not overlap"; T43: `tests.*`
    // covers `tests.pagination`.
    let error = refused(
        open(
            2,
            vec![
                adder("all_tests", &[], "tests"),
                adder("pagination_tests", &[], "tests.pagination"),
            ],
        ),
        "add { tests.* } and add { tests.pagination.* } in unordered tasks",
    );
    assert_eq!(code(&error), Some("work_overlapping_writes"));
}

#[test]
fn open_accepts_overlapping_writes_ordered_by_after() {
    // Design: "A later task can edit the output of an explicit predecessor."
    let run = open(
        2,
        vec![
            writer("first", &[], &[Facet::Body]),
            writer("second", &["first"], &[Facet::Body]),
        ],
    )
    .expect("ordered writers of one facet");
    assert_eq!(state(&run, "second"), TaskState::Blocked);
}

#[test]
fn open_accepts_overlapping_writes_ordered_through_a_chain() {
    // 7.2: `c` reaches `a` through `b`, so the two are ordered.
    open(
        3,
        vec![
            writer("a", &[], &[Facet::Body]),
            (spec("b", &["a"]), Grant::default()),
            writer("c", &["b"], &[Facet::Body]),
        ],
    )
    .expect("c runs after a through b");
}

#[test]
fn open_accepts_unordered_writes_of_different_facets() {
    // 7.2: write sets "do not overlap at facet level".
    open(
        2,
        vec![
            writer("code", &[], &[Facet::Body]),
            writer("prose", &[], &[Facet::Docs]),
        ],
    )
    .expect("body and docs of one declaration are different facets");
}

#[test]
fn open_accepts_unordered_disjoint_namespaces() {
    // T43 compares module paths segment by segment.
    open(
        2,
        vec![adder("a", &[], "tests.a"), adder("b", &[], "tests.b")],
    )
    .expect("tests.a and tests.b are disjoint");
    open(
        2,
        vec![adder("c", &[], "tests"), adder("d", &[], "testsuite")],
    )
    .expect("tests does not cover testsuite");
}

// ===========================================================================
// Ready -> Running: assignment and execution boundaries (10.2, 10.8)
// ===========================================================================

#[test]
fn assignment_launches_the_worker_with_the_fresh_lease_and_the_granted_boundary() {
    // 10.2 Ready -> Running: a lease with a fresh token, the worker started
    // through `Sandbox::launch(lease, grant.boundary)` (10.8 "launch"); the
    // lease time is `[work] lease_seconds` of the frozen policy (10.1), here 60.
    let mut run = ChangeRun::open(
        change_record(2, 60),
        plain(vec![spec("implementation", &[])]),
        baseline(Vec::new()),
    )
    .expect("the run opens");
    let mut host = HostSandbox::full();
    let lease = run
        .assign(
            "implementation",
            "agent-7",
            PathBuf::from("/wt/one"),
            &mut host,
            &at(5),
        )
        .expect("assigned");
    let record = run.task("implementation").expect("implementation");
    assert_eq!(host.launches.len(), 1, "exactly one worker is started");
    let (launched, requested) = &host.launches[0];
    assert_eq!(*requested, record.grant.boundary);
    assert_eq!(launched.task, record.id);
    assert_eq!(
        (launched.attempt, launched.token),
        (lease.attempt, lease.token)
    );
    assert_eq!(launched.holder, "agent-7");
    assert_eq!(launched.worktree, PathBuf::from("/wt/one"));
    assert_eq!(launched.expires_at, lease.expires_at);
    assert_eq!((&lease.issued_at, &lease.expires_at), (&at(5), &at(65)));
    assert_eq!(record.lease.as_ref(), Some(&lease));
}

// 10.8 contradicts itself about the direction of the enforceability check.
// Its "refusal" row writes `grant.boundary.within(sandbox.enforceable())`,
// but its "host" row says that a host with no mechanism, which reports
// `enforceable()` as host/any/any, "can therefore run only tasks whose policy
// grants exactly that", and `cpl_work::Sandbox::enforceable` is documented as
// "the strictest boundary the host can enforce". Only the check
// `sandbox.enforceable().within(&grant.boundary)` has those consequences; the
// written formula would let a host without any sandbox run every task and a
// full sandbox only the strictest ones. The next four tests follow the host
// row and design line 361 ("The scheduler must enforce filesystem, process,
// network, and integration boundaries. Merely parsing a declaration does not
// sandbox an agent.").

#[test]
fn a_host_without_a_sandbox_refuses_a_confined_task_before_launching_it() {
    let mut run = pagination();
    let mut host = HostSandbox::enforcing(unconfined());
    let snapshot = run.clone();
    let error = refused(
        run.assign(
            "implementation",
            "agent",
            PathBuf::from("/wt/i"),
            &mut host,
            &at(0),
        ),
        "a worktree/none/none task on a host that can confine nothing",
    );
    assert_eq!(code(&error), Some("work_boundary_unenforceable"));
    // 10.8 "refusal": "no lease, worktree, or agent is created".
    assert!(
        host.launches.is_empty(),
        "the worker of a task whose boundary the host cannot enforce was launched"
    );
    assert_eq!(
        run, snapshot,
        "the task stays Ready with no lease, token, or attempt"
    );
}

#[test]
fn a_host_without_a_sandbox_runs_a_task_granted_exactly_host_any_any() {
    let mut run = pagination();
    run.tasks[0].grant.boundary = unconfined();
    let mut host = HostSandbox::enforcing(unconfined());
    let lease = run
        .assign(
            "implementation",
            "agent",
            PathBuf::from("/wt/i"),
            &mut host,
            &at(0),
        )
        .expect("the policy grants exactly what the host enforces");
    assert_eq!(lease.boundary, unconfined());
}

#[test]
fn a_full_sandbox_enforces_a_wider_grant() {
    let granted = boundary(
        FsAccess::WorktreeAndRepository,
        ProcessAccess::Toolchain,
        NetworkAccess::Adapters,
    );
    let mut run = pagination();
    run.tasks[0].grant.boundary = granted;
    let mut host = HostSandbox::full();
    let lease = run
        .assign(
            "implementation",
            "agent",
            PathBuf::from("/wt/i"),
            &mut host,
            &at(0),
        )
        .unwrap_or_else(|e| {
            panic!("a host that confines workers to their worktree can enforce {granted:?}: {e}")
        });
    assert_eq!(host.launches.first().map(|(_, b)| *b), Some(granted));
    assert_eq!(
        lease.boundary, granted,
        "the boundary the sandbox reported (10.8 launch)"
    );
}

#[test]
fn enforceability_is_judged_per_field() {
    // A host that confines the filesystem and the network but cannot
    // restrict child processes.
    let partial = boundary(FsAccess::Worktree, ProcessAccess::Any, NetworkAccess::None);
    let mut host = HostSandbox::enforcing(partial);
    let mut run = pagination();
    let snapshot = run.clone();
    let error = refused(
        run.assign(
            "implementation",
            "agent",
            PathBuf::from("/wt/i"),
            &mut host,
            &at(0),
        ),
        "process=none on a host that cannot restrict processes",
    );
    assert_eq!(code(&error), Some("work_boundary_unenforceable"));
    assert!(
        host.launches.is_empty(),
        "the worker of an unenforceable task was launched"
    );
    assert_eq!(run, snapshot);
    // A grant that allows child processes is enforceable there, whatever
    // its other fields.
    let wide = boundary(FsAccess::Host, ProcessAccess::Any, NetworkAccess::Adapters);
    run.tasks[1].grant.boundary = wide;
    let lease = run
        .assign(
            "examples",
            "agent",
            PathBuf::from("/wt/e"),
            &mut host,
            &at(1),
        )
        .unwrap_or_else(|e| panic!("every field of {wide:?} is enforceable on {partial:?}: {e}"));
    assert_eq!(lease.boundary, wide);
}

#[test]
fn cancelled_and_expired_assignments_free_their_worker_slot() {
    // Design: "workers is a concurrency ceiling"; 10.2 Ready -> Running needs
    // "`workers` not exceeded", and a refused assignment changes nothing.
    let mut run = open(
        1,
        plain(vec![spec("a", &[]), spec("b", &[]), spec("c", &[])]),
    )
    .expect("the run opens");
    assign(&mut run, "a", &at(0));
    let snapshot = run.clone();
    let mut host = HostSandbox::full();
    assert!(
        run.assign("b", "agent", PathBuf::from("/wt/b"), &mut host, &at(1))
            .is_err(),
        "a second worker under `workers 1`"
    );
    assert!(host.launches.is_empty());
    assert_eq!(run, snapshot);
    run.cancel("a", &at(2)).expect("cancelled");
    let b = assign(&mut run, "b", &at(3));
    run.expire_leases(&at(903)).expect("a valid time");
    assert_eq!(state(&run, "b"), TaskState::Ready);
    let c = assign(&mut run, "c", &at(904));
    assert!(b.token < c.token);
}

// ===========================================================================
// Running -> Running and Running -> Ready: renewal and expiry (10.2)
// ===========================================================================

#[test]
fn renewal_extends_the_window_in_which_the_worker_may_submit() {
    // 10.2 Running -> Running: "same token, before expiry"; the renewed lease
    // runs `lease_seconds` from the renewal.
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    assert_eq!(
        run.renew("implementation", lease.token, &at(800))
            .expect("before expiry"),
        at(1700)
    );
    assert!(
        run.expire_leases(&at(1000))
            .expect("a valid time")
            .is_empty(),
        "the first expiry no longer applies"
    );
    // At the renewed expiry itself the lease has expired: renewal is "before expiry".
    let mut late = run.clone();
    let error = refused(
        late.renew("implementation", lease.token, &at(1700)),
        "a renewal at expires_at",
    );
    assert_eq!(code(&error), Some("work_stale_submission"));
    assert_eq!(late, run, "a refused renewal changes nothing");
    let outcome = run
        .submit("implementation", &submission(&lease, "tree"), &at(1000))
        .expect("in time thanks to the renewal");
    assert!(matches!(outcome, SubmitOutcome::Recorded(_)));
}

#[test]
fn renewal_moves_only_the_expiry() {
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    run.renew("implementation", lease.token, &at(100))
        .expect("renewed");
    let record = run.task("implementation").expect("implementation");
    assert_eq!(
        record.lease.as_ref(),
        Some(&Lease {
            expires_at: at(1000),
            ..lease
        })
    );
    assert_eq!(record.attempt, 1, "a renewal is not a reassignment");
    assert_eq!(run.change.next_token, 2, "a renewal issues no token");
}

#[test]
fn a_superseded_worker_cannot_renew_the_new_lease() {
    // Design: "Lease renewal and fencing prevent stale submissions"; Appendix
    // B `work_stale_submission`: "fencing token superseded".
    let mut run = pagination();
    let first = assign(&mut run, "implementation", &at(0));
    run.expire_leases(&at(900)).expect("a valid time");
    let second = assign(&mut run, "implementation", &at(901));
    let snapshot = run.clone();
    let error = refused(
        run.renew("implementation", first.token, &at(902)),
        "a renewal with the superseded token",
    );
    assert_eq!(code(&error), Some("work_stale_submission"));
    assert_eq!(
        run, snapshot,
        "the current lease keeps its expiry {}",
        second.expires_at
    );
}

#[test]
fn a_failed_workers_worktree_stays_recorded_until_reassignment() {
    // 10.2 Running -> Ready (worker reported failure): "the old worktree is
    // kept until reassigned"; design: a crashed worker "leaves an isolated
    // worktree that can be inspected or reassigned", so the run still knows
    // where it is.
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    run.report_failure("implementation", lease.token, "out of memory", &at(1))
        .expect("reported");
    let record = run.task("implementation").expect("implementation");
    assert_eq!(record.state, TaskState::Ready);
    assert_eq!(
        record.lease.as_ref().map(|l| &l.worktree),
        Some(&lease.worktree),
        "the old worktree is still recorded"
    );
    let next = run
        .assign(
            "implementation",
            "agent-2",
            PathBuf::from("/wt/implementation-2"),
            &mut HostSandbox::full(),
            &at(2),
        )
        .expect("reassigned");
    assert_eq!(
        run.task("implementation")
            .and_then(|t| t.lease.as_ref())
            .map(|l| &l.worktree),
        Some(&next.worktree)
    );
    assert_eq!(next.worktree, PathBuf::from("/wt/implementation-2"));
}

#[test]
fn expiry_compares_instants_not_spellings() {
    // 10.1: timestamps are RFC 3339. The lease expires at 2026-10-28T12:15:00Z.
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    // 12:14:59Z written at +02:00 (a string comparison would call it late).
    run.submit(
        "implementation",
        &submission(&lease, "tree"),
        &ts("2026-10-28T14:14:59+02:00"),
    )
    .expect("one second before expiry");
    // 12:15:00Z written at -01:00 (a string comparison would call it in time).
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let snapshot = run.clone();
    let error = refused(
        run.submit(
            "implementation",
            &submission(&lease, "tree"),
            &ts("2026-10-28T11:15:00-01:00"),
        ),
        "a submission at the expiry instant",
    );
    assert_eq!(code(&error), Some("work_stale_submission"));
    assert_eq!(run, snapshot);
}

#[test]
fn expiry_honours_fractions_of_a_second() {
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    run.submit(
        "implementation",
        &submission(&lease, "tree"),
        &ts("2026-10-28T12:14:59.999999999Z"),
    )
    .expect("a nanosecond before expiry");
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let error = refused(
        run.submit(
            "implementation",
            &submission(&lease, "tree"),
            &ts("2026-10-28T12:15:00.000000001Z"),
        ),
        "a submission a nanosecond after expiry",
    );
    assert_eq!(code(&error), Some("work_stale_submission"));
}

#[test]
fn lease_expiry_leaves_a_submitted_task_alone() {
    // 10.2 Running -> Ready: "lease expired without submission".
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    submit(&mut run, "implementation", &lease, "tree", &at(10));
    let snapshot = run.clone();
    assert!(run
        .expire_leases(&at(5000))
        .expect("a valid time")
        .is_empty());
    assert_eq!(run, snapshot);
    // The submission was in time; validating it later is Submitted -> Accepted.
    run.validation_passed("implementation", &at(5001))
        .expect("accepted after the lease ran out");
    assert_eq!(state(&run, "implementation"), TaskState::Accepted);
}

// ===========================================================================
// Running -> Submitted: fencing and idempotent resubmission (10.2)
// ===========================================================================

#[test]
fn a_token_issued_to_another_task_cannot_submit() {
    // 10.2: a submission needs the task's "current, unexpired token"; one
    // lower than the task's current token is `work_stale_submission`.
    let mut run = pagination();
    let implementation = assign(&mut run, "implementation", &at(0));
    let examples = assign(&mut run, "examples", &at(1));
    assert!(implementation.token < examples.token);
    let snapshot = run.clone();
    let higher = Submission {
        attempt: implementation.attempt,
        token: examples.token,
        tree: "tree".to_string(),
    };
    assert!(
        run.submit("implementation", &higher, &at(2)).is_err(),
        "examples' token submitted for implementation"
    );
    let lower = Submission {
        attempt: examples.attempt,
        token: implementation.token,
        tree: "tree".to_string(),
    };
    let error = refused(
        run.submit("examples", &lower, &at(2)),
        "implementation's token submitted for examples",
    );
    assert_eq!(code(&error), Some("work_stale_submission"));
    assert_eq!(run, snapshot);
}

#[test]
fn the_current_token_with_another_attempt_is_not_the_current_assignment() {
    // 10.1: a candidate is the result of one assignment, identified by (task,
    // attempt, token, tree); 10.2 records it for the current assignment only.
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let snapshot = run.clone();
    for attempt in [0, 2, u32::MAX] {
        let forged = Submission {
            attempt,
            token: lease.token,
            tree: "tree".to_string(),
        };
        assert!(
            run.submit("implementation", &forged, &at(1)).is_err(),
            "attempt {attempt} with the token of attempt {}",
            lease.attempt
        );
    }
    assert_eq!(run, snapshot);
}

#[test]
fn a_superseded_repeat_never_replaces_the_current_candidate() {
    // Design: "a result from an expired or superseded assignment cannot
    // overwrite current work". 10.2 makes a repeat of one candidate a no-op
    // returning its id and a lower token `work_stale_submission`; both rules
    // apply to this repeat, and either answer leaves the run unchanged.
    let mut run = pagination();
    let first = assign(&mut run, "implementation", &at(0));
    let replaced = submit(&mut run, "implementation", &first, "tree-1", &at(1));
    run.validation_failed("implementation", "patch_stale", &at(2))
        .expect("rejected");
    let second = assign(&mut run, "implementation", &at(3));
    let current = submit(&mut run, "implementation", &second, "tree-2", &at(4));
    let snapshot = run.clone();
    match run.submit("implementation", &submission(&first, "tree-1"), &at(5)) {
        Ok(outcome) => assert_eq!(outcome, SubmitOutcome::Unchanged(replaced)),
        Err(error) => assert_eq!(code(&error), Some("work_stale_submission")),
    }
    assert_eq!(run, snapshot);
    // A new result of the superseded assignment is fenced: its token is lower.
    let error = refused(
        run.submit("implementation", &submission(&first, "tree-1b"), &at(5)),
        "a new tree from the superseded assignment",
    );
    assert_eq!(code(&error), Some("work_stale_submission"));
    assert_eq!(run, snapshot);
    assert_eq!(
        run.task("implementation").and_then(|t| t.candidate.clone()),
        Some(current)
    );
}

#[test]
fn after_failed_validation_the_old_assignment_cannot_submit_a_new_candidate() {
    // 10.2 Submitted -> Ready: "attempt incremented on reassignment", so a new
    // candidate needs a new assignment, even with an unexpired lease.
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let rejected = submit(&mut run, "implementation", &lease, "tree-1", &at(1));
    run.validation_failed("implementation", "patch_out_of_scope", &at(2))
        .expect("rejected");
    let snapshot = run.clone();
    assert!(
        run.submit("implementation", &submission(&lease, "tree-fixed"), &at(3))
            .is_err(),
        "a Ready task took a candidate without an assignment"
    );
    assert_eq!(run, snapshot);
    // Repeating the rejected submission itself is the idempotent no-op.
    assert_eq!(
        run.submit("implementation", &submission(&lease, "tree-1"), &at(3))
            .expect("a repeat"),
        SubmitOutcome::Unchanged(rejected)
    );
    assert_eq!(run, snapshot);
}

#[test]
fn a_repeat_after_integration_is_still_a_no_op() {
    // Design: "Repeated submission of the same task attempt and candidate
    // must be idempotent"; 10.2: it "returns the existing CandidateId".
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let candidate = submit(&mut run, "implementation", &lease, "tree-i", &at(0));
    run.validation_passed("implementation", &at(0))
        .expect("accepted");
    let examples = accept(&mut run, "examples", "tree-e", &at(1));
    run.unblock("audit", baseline(vec![examples, candidate.clone()]), &at(2))
        .expect("unblocked");
    accept(&mut run, "audit", "tree-a", &at(3));
    run.begin_integration(&integrator()).expect("integrating");
    run.finish_integration(&"9".repeat(40), &at(4))
        .expect("integrated");
    let snapshot = run.clone();
    assert_eq!(
        run.submit("implementation", &submission(&lease, "tree-i"), &at(50))
            .expect("a repeat"),
        SubmitOutcome::Unchanged(candidate)
    );
    assert!(
        run.submit("implementation", &submission(&lease, "tree-x"), &at(50))
            .is_err(),
        "a new candidate in an integrated run"
    );
    assert_eq!(run, snapshot);
}

#[test]
fn a_submission_records_its_candidate_and_one_transition() {
    // 10.2 Running -> Submitted: "candidate recorded"; 10.1 TaskRecord.history.
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let before = run
        .task("implementation")
        .expect("implementation")
        .history
        .len();
    let outcome = run
        .submit("implementation", &submission(&lease, "tree"), &at(7))
        .expect("submitted");
    let record = run.task("implementation").expect("implementation");
    let expected = CandidateId::derive(&record.id, lease.attempt, lease.token, "tree");
    assert_eq!(outcome, SubmitOutcome::Recorded(expected.clone()));
    assert_eq!(record.candidate.as_ref(), Some(&expected));
    assert_eq!(record.history.len(), before + 1);
    let last = record.history.last().expect("a transition");
    assert_eq!(
        (last.from, last.to, last.token),
        (TaskState::Running, TaskState::Submitted, Some(lease.token))
    );
    assert_eq!(last.at, at(7));
}

// ===========================================================================
// Fencing tokens and attempts (10.1)
// ===========================================================================

#[test]
fn fencing_tokens_strictly_increase_across_tasks_failures_and_restarts() {
    // 10.1 FencingToken: "strictly increasing per change run"; 10.2 every
    // assignment gets "a fresh token".
    let mut run = pagination();
    let mut issued = Vec::new();
    // A launch that fails was still handed a token; the next one is fresher.
    let mut failing = HostSandbox::failing("no sandbox slots");
    assert!(run
        .assign(
            "implementation",
            "agent",
            PathBuf::from("/wt/x"),
            &mut failing,
            &at(0)
        )
        .is_err());
    issued.extend(failing.launches.iter().map(|(lease, _)| lease.token));
    issued.push(assign(&mut run, "implementation", &at(1)).token);
    issued.push(assign(&mut run, "examples", &at(2)).token);
    run.expire_leases(&at(1000)).expect("a valid time");
    issued.push(assign(&mut run, "implementation", &at(1001)).token);
    // The counter survives a coordinator restart (10.1 persistence).
    let root = temp_dir("tokens");
    let id = run.change.id.clone();
    let mut coordinator = Coordinator::new(&root);
    coordinator.insert(run);
    coordinator.save().expect("saved");
    let (mut restarted, voided) = Coordinator::restart(&root, &at(5000)).expect("restarted");
    assert_eq!(voided.len(), 1, "{voided:?}");
    let run = restarted.runs.get_mut(&id).expect("the run");
    issued.push(assign(run, "examples", &at(5001)).token);
    issued.push(assign(run, "implementation", &at(5002)).token);
    assert!(
        issued.windows(2).all(|pair| pair[0] < pair[1]),
        "tokens in issue order: {issued:?}"
    );
}

#[test]
fn the_attempt_counts_assignments_not_tokens() {
    // 10.1 TaskRecord.attempt: "incremented on every (re)assignment"; 10.2
    // Running -> Ready and Submitted -> Ready: "attempt incremented on the
    // next assignment" (not before).
    let mut run = pagination();
    let mut failing = HostSandbox::failing("no sandbox slots");
    assert!(run
        .assign(
            "implementation",
            "agent",
            PathBuf::from("/wt/x"),
            &mut failing,
            &at(0)
        )
        .is_err());
    let attempt = |run: &ChangeRun| run.task("implementation").expect("implementation").attempt;
    assert_eq!(attempt(&run), 0, "a launch that failed is no assignment");
    let first = assign(&mut run, "implementation", &at(1));
    assert_eq!(first.attempt, 1);
    run.report_failure("implementation", first.token, "crashed", &at(2))
        .expect("reported");
    assert_eq!(attempt(&run), 1);
    let second = assign(&mut run, "implementation", &at(3));
    assert_eq!(second.attempt, 2);
    submit(&mut run, "implementation", &second, "tree-2", &at(4));
    run.validation_failed("implementation", "type_mismatch", &at(5))
        .expect("rejected");
    assert_eq!(attempt(&run), 2);
    let third = assign(&mut run, "implementation", &at(6));
    assert_eq!(third.attempt, 3);
    run.expire_leases(&at(906)).expect("a valid time");
    assert_eq!(attempt(&run), 3);
    assert_eq!(assign(&mut run, "implementation", &at(1000)).attempt, 4);
}

#[test]
fn the_token_counter_never_wraps_around() {
    // 10.1: strictly increasing, so no token at or below an issued one is
    // ever handed out, also at the end of the u64 range.
    let mut change = change_record(3, LEASE);
    change.next_token = u64::MAX - 1;
    let mut run = ChangeRun::open(
        change,
        plain(vec![spec("a", &[]), spec("b", &[]), spec("c", &[])]),
        baseline(Vec::new()),
    )
    .expect("the run opens");
    let mut last = assign(&mut run, "a", &at(0)).token;
    assert_eq!(last, FencingToken(u64::MAX - 1));
    for task in ["b", "c"] {
        let mut host = HostSandbox::full();
        let snapshot = run.clone();
        match run.assign(task, "agent", PathBuf::from("/wt/x"), &mut host, &at(1)) {
            Ok(lease) => {
                assert!(lease.token > last, "{} after {}", lease.token, last);
                last = lease.token;
            }
            Err(_) => {
                assert!(host.launches.is_empty(), "{task}: launched without a token");
                assert_eq!(run, snapshot);
            }
        }
    }
}

// ===========================================================================
// The transition table (10.2)
// ===========================================================================

#[test]
fn every_row_of_the_table_is_accepted_from_its_state() {
    let run = every_state();
    let rows = [
        ("ready", Op::Assign, TaskState::Running),
        ("running", Op::Renew, TaskState::Running),
        ("running", Op::ReportFailure, TaskState::Ready),
        ("running", Op::Submit, TaskState::Submitted),
        ("submitted", Op::Pass, TaskState::Accepted),
        ("submitted", Op::Reject, TaskState::Ready),
        ("accepted", Op::Invalidate, TaskState::Blocked),
        ("invalidated", Op::Unblock, TaskState::Ready),
        ("invalidated", Op::FailBlocked, TaskState::Failed),
        ("waiting", Op::FailBlocked, TaskState::Failed),
    ];
    for (task, op, to) in rows {
        let mut next = run.clone();
        let before = next.task(task).expect("a task").history.len();
        apply(&mut next, task, op, &at(10)).unwrap_or_else(|e| panic!("{op:?} on {task}: {e}"));
        let record = next.task(task).expect("a task");
        assert_eq!(record.state, to, "{op:?} on {task}");
        assert_eq!(
            record.history.len(),
            before + 1,
            "{op:?} on {task}: one transition"
        );
        let last = record.history.last().expect("a transition");
        assert_eq!(
            (last.from, last.to),
            (state(&run, task), to),
            "{op:?} on {task}"
        );
    }
}

#[test]
fn transitions_outside_the_table_are_refused_and_change_nothing() {
    // 10.2 lists every transition; a refused operation changes nothing.
    let run = every_state();
    let legal: [(&str, &[Op]); 6] = [
        ("ready", &[Op::Assign]),
        ("running", &[Op::Renew, Op::ReportFailure, Op::Submit]),
        ("submitted", &[Op::Pass, Op::Reject]),
        ("accepted", &[Op::Invalidate]),
        ("invalidated", &[Op::Unblock, Op::FailBlocked]),
        // Its predecessor `ready` is neither Accepted nor Integrated.
        ("waiting", &[Op::FailBlocked]),
    ];
    for (task, allowed) in legal {
        for op in OPS.into_iter().filter(|op| !allowed.contains(op)) {
            let mut next = run.clone();
            assert!(
                apply(&mut next, task, op, &at(10)).is_err(),
                "{op:?} accepted on the {} task {task}",
                state(&run, task)
            );
            assert_eq!(next, run, "{op:?} on {task} changed the run");
        }
    }
}

#[test]
fn failed_and_cancelled_tasks_admit_no_transition() {
    // 10.2: no row leaves Failed or Cancelled; "any non-terminal -> Cancelled".
    let mut run = every_state();
    run.cancel("running", &at(5)).expect("cancelled");
    run.fail_blocked("waiting", "work_conflict: cand_a cand_b", &at(5))
        .expect("failed");
    for task in ["running", "waiting"] {
        for op in OPS {
            let mut next = run.clone();
            assert!(
                apply(&mut next, task, op, &at(10)).is_err(),
                "{op:?} left the terminal state of {task}"
            );
            assert_eq!(next, run, "{op:?} on {task}");
        }
        let mut next = run.clone();
        assert!(next.cancel(task, &at(10)).is_err(), "{task} is terminal");
        assert_eq!(next, run);
    }
}

#[test]
fn an_integrated_run_admits_no_transition() {
    let mut run = pagination();
    accept_all(&mut run);
    run.begin_integration(&integrator()).expect("integrating");
    run.finish_integration(&"9".repeat(40), &at(20))
        .expect("integrated");
    for task in ["implementation", "examples", "audit"] {
        assert_eq!(state(&run, task), TaskState::Integrated);
        for op in OPS {
            let mut next = run.clone();
            assert!(
                apply(&mut next, task, op, &at(21)).is_err(),
                "{op:?} on the integrated {task}"
            );
            assert_eq!(next, run, "{op:?} on {task}");
        }
        let mut next = run.clone();
        assert!(next.cancel(task, &at(21)).is_err(), "{task} is terminal");
        assert_eq!(next, run);
    }
    let snapshot = run.clone();
    assert!(run.cancel_change(&at(22)).is_err());
    assert!(run.begin_integration(&integrator()).is_err());
    assert_eq!(run, snapshot);
}

#[test]
fn every_non_terminal_state_can_be_cancelled() {
    // 10.2 "any non-terminal -> Cancelled | operator request"; 10.1
    // TaskRecord.blocked is the "reason while Blocked".
    let run = every_state();
    for task in [
        "ready",
        "running",
        "submitted",
        "accepted",
        "invalidated",
        "waiting",
    ] {
        let mut next = run.clone();
        next.cancel(task, &at(20))
            .unwrap_or_else(|e| panic!("cancel {task}: {e}"));
        let record = next.task(task).expect("a task");
        assert_eq!(record.state, TaskState::Cancelled, "{task}");
        let last = record.history.last().expect("a transition");
        assert_eq!(
            (last.from, last.to),
            (state(&run, task), TaskState::Cancelled),
            "{task}"
        );
        assert!(record.blocked.is_none(), "{task} is no longer Blocked");
        for other in run.tasks.iter().filter(|t| t.id.name != task) {
            assert_eq!(next.task(&other.id.name), Some(other), "cancelling {task}");
        }
    }
}

#[test]
fn a_cancelled_assignment_cannot_submit_renew_or_report() {
    // Design: "a result from an expired or superseded assignment cannot
    // overwrite current work"; a cancelled attempt "ended without integration".
    let mut run = open(1, plain(vec![spec("a", &[]), spec("b", &[])])).expect("the run opens");
    let lease = assign(&mut run, "a", &at(0));
    run.cancel("a", &at(1)).expect("cancelled");
    let snapshot = run.clone();
    assert!(run
        .submit("a", &submission(&lease, "tree"), &at(2))
        .is_err());
    assert!(run.renew("a", lease.token, &at(2)).is_err());
    assert!(run
        .report_failure("a", lease.token, "crashed", &at(2))
        .is_err());
    assert_eq!(run, snapshot);
    // Its worker slot is free again (`workers 1`).
    assign(&mut run, "b", &at(3));
}

// ===========================================================================
// Blocked: snapshots of the predecessors, failure propagation (10.2, 10.4)
// ===========================================================================

#[test]
fn integrated_predecessors_unblock_their_dependents() {
    // 10.2 Blocked -> Ready: "every predecessor `Accepted` or `Integrated`".
    let mut run = pagination();
    let implementation = accept(&mut run, "implementation", "tree-i", &at(0));
    let examples = accept(&mut run, "examples", "tree-e", &at(1));
    run.tasks[0].state = TaskState::Integrated;
    assert_eq!(run.ready_to_unblock(), ["audit"]);
    run.unblock("audit", baseline(vec![examples, implementation]), &at(2))
        .expect("Integrated counts like Accepted");
    assert_eq!(state(&run, "audit"), TaskState::Ready);
}

#[test]
fn a_snapshot_that_also_layers_a_replaced_candidate_does_not_unblock() {
    // 10.4 layers `p.candidate` of each predecessor, so the snapshot's
    // Baseline.predecessors (5.5) are exactly the predecessors' current
    // candidates, and 10.2 Blocked -> Ready requires that snapshot. A baseline
    // that also lists a candidate implementation has since replaced was not
    // assembled by 10.4.
    let mut run = pagination();
    let replaced = accept(&mut run, "implementation", "tree-i1", &at(0));
    let examples = accept(&mut run, "examples", "tree-e", &at(1));
    // implementation re-runs and is accepted with a new candidate.
    run.invalidate("implementation", &examples, &at(2))
        .expect("invalidated");
    run.unblock("implementation", baseline(Vec::new()), &at(3))
        .expect("ready again");
    let current = accept(&mut run, "implementation", "tree-i2", &at(4));
    assert_ne!(replaced, current);
    let snapshot = run.clone();
    let result = run.unblock(
        "audit",
        baseline(vec![examples.clone(), replaced.clone(), current.clone()]),
        &at(5),
    );
    assert!(
        result.is_err(),
        "audit became Ready from a snapshot layering the replaced candidate {replaced}"
    );
    assert_eq!(run, snapshot);
    run.unblock("audit", baseline(vec![examples, current]), &at(6))
        .expect("the snapshot of the current candidates");
}

#[test]
fn a_snapshot_layering_a_task_that_is_not_a_predecessor_does_not_unblock() {
    // Design "Changes and agent tasks": the dependent's snapshot is prepared
    // "from the named predecessors"; 10.4 layers the predecessors only.
    let mut run = open(
        3,
        plain(vec![
            spec("implementation", &[]),
            spec("examples", &[]),
            spec("audit", &["implementation"]),
        ]),
    )
    .expect("the run opens");
    let implementation = accept(&mut run, "implementation", "tree-i", &at(0));
    let examples = accept(&mut run, "examples", "tree-e", &at(1));
    let snapshot = run.clone();
    let result = run.unblock(
        "audit",
        baseline(vec![examples.clone(), implementation.clone()]),
        &at(2),
    );
    assert!(
        result.is_err(),
        "audit (after implementation only) became Ready from a snapshot layering {examples}"
    );
    assert_eq!(run, snapshot);
    run.unblock("audit", baseline(vec![implementation]), &at(3))
        .expect("the snapshot of its predecessor");
}

#[test]
fn an_invalidated_dependent_unblocks_only_from_the_new_candidate() {
    // 10.2 Accepted -> Blocked "invalidated_by: <candidate id>" when a
    // predecessor's accepted candidate changed; Blocked -> Ready from 10.4's
    // snapshot of the current candidates.
    let mut run = pagination();
    let (old, examples, _) = accept_all(&mut run);
    run.invalidate("implementation", &examples, &at(10))
        .expect("re-run");
    run.unblock("implementation", baseline(Vec::new()), &at(11))
        .expect("ready again");
    let new = accept(&mut run, "implementation", "tree-i2", &at(12));
    assert_eq!(
        run.invalidate_dependents("implementation", &at(13)),
        ["audit"]
    );
    let reason = format!("invalidated_by: {new}");
    assert_eq!(
        run.task("audit").and_then(|t| t.blocked.as_deref()),
        Some(reason.as_str())
    );
    let snapshot = run.clone();
    assert!(
        run.unblock("audit", baseline(vec![examples.clone(), old]), &at(14))
            .is_err(),
        "the snapshot of the replaced candidate"
    );
    assert_eq!(run, snapshot);
    run.unblock(
        "audit",
        baseline(vec![examples.clone(), new.clone()]),
        &at(15),
    )
    .expect("the snapshot of the new candidate");
    let audit = run.task("audit").expect("audit");
    assert_eq!(audit.state, TaskState::Ready);
    assert!(audit.blocked.is_none());
    assert_eq!(
        audit.baseline.as_ref().map(|b| b.predecessors.clone()),
        Some(vec![examples, new])
    );
}

#[test]
fn failure_propagates_through_a_diamond_but_not_to_independent_tasks() {
    // 10.2 Blocked -> Failed: "a predecessor `Failed`/`Cancelled`".
    let mut run = open(
        5,
        plain(vec![
            spec("a", &[]),
            spec("b", &["a"]),
            spec("c", &["a"]),
            spec("d", &["b", "c"]),
            spec("e", &[]),
        ]),
    )
    .expect("the run opens");
    run.cancel("a", &at(1)).expect("cancelled");
    let mut failed = run.propagate_failures(&at(2));
    failed.sort();
    assert_eq!(failed, ["b", "c", "d"]);
    for task in ["b", "c", "d"] {
        let record = run.task(task).expect("a task");
        assert_eq!(record.state, TaskState::Failed, "{task}");
        assert!(record.blocked.is_none(), "{task} is no longer Blocked");
    }
    assert_eq!(state(&run, "e"), TaskState::Ready);
}

#[test]
fn one_failed_predecessor_fails_a_dependent_whose_other_predecessor_was_accepted() {
    let mut run = pagination();
    accept(&mut run, "implementation", "tree-i", &at(0));
    assign(&mut run, "examples", &at(1));
    run.cancel("examples", &at(2)).expect("cancelled");
    assert_eq!(run.propagate_failures(&at(3)), ["audit"]);
    assert_eq!(state(&run, "audit"), TaskState::Failed);
    assert!(run.ready_to_unblock().is_empty());
}

#[test]
fn an_invalidated_task_fails_when_a_predecessor_is_cancelled() {
    let mut run = pagination();
    let (implementation, _, _) = accept_all(&mut run);
    run.invalidate("audit", &implementation, &at(10))
        .expect("invalidated");
    run.cancel("examples", &at(11)).expect("cancelled");
    assert_eq!(run.propagate_failures(&at(12)), ["audit"]);
    assert_eq!(state(&run, "audit"), TaskState::Failed);
}

// ===========================================================================
// Integration (10.6)
// ===========================================================================

#[test]
fn integration_authority_is_the_integrating_actors_not_the_submitters() {
    // Design "Swarm authority and lifecycle": "Integration authority belongs
    // to the coordinator or an authorized actor, not to every worker"; 10.6
    // checks `actor.authority.may_integrate` of the actor that integrates.
    let mut change = change_record(3, LEASE);
    change.opened_by.authority.may_integrate = true;
    let mut run =
        ChangeRun::open(change, pagination_tasks(), baseline(Vec::new())).expect("the run opens");
    accept_all(&mut run);
    let worker = Actor {
        id: "implementation-worker".to_string(),
        authority: Authority::default(),
    };
    let snapshot = run.clone();
    let error = refused(run.begin_integration(&worker), "integration by a worker");
    assert_eq!(code(&error), Some("work_not_authorized"));
    assert_eq!(error.to_diagnostic().category, Category::Work);
    assert_eq!(run, snapshot, "nothing changes (10.6)");
    run.begin_integration(&integrator())
        .expect("an authorized actor");
}

#[test]
fn integration_steps_need_an_integrating_change() {
    let mut run = pagination();
    accept_all(&mut run);
    let snapshot = run.clone();
    assert!(run.finish_integration(&"9".repeat(40), &at(20)).is_err());
    assert!(run.fail_integration("checks failed", &[], &at(20)).is_err());
    assert!(run.stale_against_target("implementation", &at(20)).is_err());
    assert_eq!(run, snapshot);
}

#[test]
fn a_candidate_stale_against_the_target_blocks_only_its_task() {
    // 10.6: "stale -> t.state = Blocked (reason work_stale_against_target);
    // return Blocked (operator or rescheduling re-validates)".
    let mut run = pagination();
    accept_all(&mut run);
    run.begin_integration(&integrator()).expect("integrating");
    run.stale_against_target("examples", &at(20))
        .expect("stale");
    let examples = run.task("examples").expect("examples");
    assert_eq!(examples.state, TaskState::Blocked);
    assert_eq!(
        examples.blocked.as_deref(),
        Some("work_stale_against_target")
    );
    assert_eq!(state(&run, "implementation"), TaskState::Accepted);
    assert_eq!(state(&run, "audit"), TaskState::Accepted);
    assert!(
        !run.change.state.is_terminal(),
        "the change is re-validated, not failed"
    );
}

#[test]
fn finishing_an_integration_integrates_exactly_the_accepted_tasks() {
    // 10.2 Accepted -> Integrated: "the integration commit including this
    // candidate advanced the target".
    let mut run = open(
        3,
        plain(vec![
            spec("implementation", &[]),
            spec("examples", &[]),
            spec("notes", &[]),
        ]),
    )
    .expect("the run opens");
    accept(&mut run, "implementation", "tree-i", &at(0));
    accept(&mut run, "examples", "tree-e", &at(1));
    run.cancel("notes", &at(2)).expect("cancelled");
    run.begin_integration(&integrator()).expect("integrating");
    let commit = "9".repeat(40);
    let mut integrated = run.finish_integration(&commit, &at(3)).expect("integrated");
    integrated.sort();
    assert_eq!(integrated, ["examples", "implementation"]);
    assert_eq!(state(&run, "notes"), TaskState::Cancelled);
    assert_eq!(run.change.state, ChangeState::Integrated { commit });
}

#[test]
fn a_failed_check_fails_the_change_but_no_task() {
    // 10.6: "if any check fails: change.state = Failed { reason }"; 10.2 moves
    // a task Accepted -> Failed only when "integration retries [are]
    // exhausted with this candidate stale".
    let mut run = pagination();
    accept_all(&mut run);
    run.begin_integration(&integrator()).expect("integrating");
    let reason = "test(tests.pagination.*): failed";
    run.fail_integration(reason, &[], &at(20)).expect("failed");
    assert_eq!(
        run.change.state,
        ChangeState::Failed {
            reason: reason.to_string()
        }
    );
    for task in ["implementation", "examples", "audit"] {
        assert_eq!(state(&run, task), TaskState::Accepted, "{task}");
    }
}

// ===========================================================================
// The state file and restarts (10.1, T33)
// ===========================================================================

/// Save `run` alone under a fresh project root.
fn saved(run: ChangeRun, name: &str) -> (PathBuf, ChangeRunId) {
    let root = temp_dir(name);
    let id = run.change.id.clone();
    let mut coordinator = Coordinator::new(&root);
    coordinator.insert(run);
    coordinator.save().expect("saved");
    (root, id)
}

#[test]
fn a_restart_keeps_an_unexpired_lease_whose_worker_then_submits() {
    // 10.1: a restart "voids every lease whose `expires_at` has passed", and
    // no other.
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let (root, id) = saved(run, "restart-live");
    let (mut restarted, voided) = Coordinator::restart(&root, &at(600)).expect("restarted");
    assert!(voided.is_empty(), "{voided:?}");
    let run = restarted.runs.get_mut(&id).expect("the run");
    assert_eq!(state(run, "implementation"), TaskState::Running);
    let outcome = run
        .submit("implementation", &submission(&lease, "tree"), &at(601))
        .expect("the lease is still current");
    assert!(matches!(outcome, SubmitOutcome::Recorded(_)));
}

#[test]
fn a_restart_fences_the_worker_of_a_voided_lease() {
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let (root, id) = saved(run, "restart-voided");
    let (mut restarted, voided) = Coordinator::restart(&root, &at(1000)).expect("restarted");
    let names: Vec<&str> = voided.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["implementation"]);
    let run = restarted.runs.get_mut(&id).expect("the run");
    assert_eq!(state(run, "implementation"), TaskState::Ready);
    let snapshot = run.clone();
    let error = refused(
        run.submit("implementation", &submission(&lease, "late"), &at(1001)),
        "a submission of the voided assignment",
    );
    assert_eq!(code(&error), Some("work_stale_submission"));
    assert_eq!(*run, snapshot);
    let next = assign(run, "implementation", &at(1002));
    assert!(next.token > lease.token);
    assert_eq!(next.attempt, lease.attempt + 1);
}

#[test]
fn resubmission_is_idempotent_across_a_restart() {
    // Design: "Repeated submission of the same task attempt and candidate must
    // be idempotent", also when the coordinator restarted in between.
    let mut run = pagination();
    let lease = assign(&mut run, "implementation", &at(0));
    let candidate = submit(&mut run, "implementation", &lease, "tree", &at(1));
    let (root, id) = saved(run, "restart-idempotent");
    let (mut restarted, _) = Coordinator::restart(&root, &at(5000)).expect("restarted");
    let run = restarted.runs.get_mut(&id).expect("the run");
    let snapshot = run.clone();
    assert_eq!(
        run.submit("implementation", &submission(&lease, "tree"), &at(5001))
            .expect("a repeat"),
        SubmitOutcome::Unchanged(candidate)
    );
    assert_eq!(*run, snapshot);
}

#[test]
fn a_leftover_temporary_file_is_not_change_run_state() {
    // 10.1 (T33): a run's state is `.cpl/work/<change run id>.json`; a file of
    // another name, such as the remains of an interrupted write, is not.
    let mut run = pagination();
    assign(&mut run, "implementation", &at(0));
    let (root, id) = saved(run, "restart-leftover");
    let state_before = Coordinator::load(&root).expect("loads");
    let leftover = Coordinator::state_dir(&root).join(format!("{id}.json.tmp"));
    std::fs::write(leftover, "{ \"change\": ").expect("write");
    assert_eq!(
        Coordinator::load(&root).expect("the leftover is not state"),
        state_before
    );
}

// ===========================================================================
// Records: timestamps, boundaries, grants (10.1, 10.3, 10.8)
// ===========================================================================

#[test]
fn the_coordinator_records_times_in_utc() {
    // 10.1: `Timestamp(pub String); // RFC 3339, UTC`. A `now` with an offset
    // is an RFC 3339 instant; what the coordinator records from it is that
    // instant spelled in UTC, unless it refuses such a `now` altogether.
    let steps = [
        (Op::Assign, ts("2026-10-28T14:00:00+02:00")),
        (Op::Renew, ts("2026-10-28T14:05:00+02:00")),
        (Op::Submit, ts("2026-10-28T07:06:00-05:00")),
    ];
    let mut run = pagination();
    let mut applied = 0;
    for (op, now) in &steps {
        let before = run.clone();
        if apply(&mut run, "implementation", *op, now).is_err() {
            assert_eq!(run, before, "a refused {op:?} changes nothing");
            break;
        }
        applied += 1;
    }
    let record = run.task("implementation").expect("implementation");
    let mut offenders: Vec<String> = record
        .history
        .iter()
        .zip(&steps)
        .filter_map(|(transition, (op, now))| {
            not_utc(&format!("Transition.at of {op:?}"), &transition.at, now)
        })
        .collect();
    if let Some(lease) = &record.lease {
        offenders.extend(not_utc("Lease.issued_at", &lease.issued_at, &steps[0].1));
        let renewed = steps[..applied]
            .iter()
            .rev()
            .find(|(op, _)| matches!(op, Op::Assign | Op::Renew))
            .map(|(_, now)| now)
            .expect("assigned");
        let expiry = renewed.add_seconds(LEASE).expect("in range");
        offenders.extend(not_utc("Lease.expires_at", &lease.expires_at, &expiry));
    }
    assert!(
        offenders.is_empty(),
        "times recorded outside UTC:\n{}",
        offenders.join("\n")
    );
}

/// Why `value` is not `instant` spelled in UTC, if it is not.
fn not_utc(field: &str, value: &Timestamp, instant: &Timestamp) -> Option<String> {
    let utc = value.0.ends_with('Z') && value.cmp_instant(instant) == Some(Ordering::Equal);
    (!utc).then(|| format!("{field} is `{value}`, not {instant} in UTC"))
}

#[test]
fn no_transition_records_a_time_that_is_not_rfc3339() {
    // 10.1: Transition.at is a Timestamp (RFC 3339, UTC). Assignment, renewal,
    // submission, and lease expiry refuse a `now` that is not one; every other
    // operation that records a transition must refuse it as well (or, with no
    // error to return, record nothing), or the state file holds a time that
    // no later comparison can read.
    let bad = ts("yesterday");
    let mut offenders = Vec::new();
    let base = every_state();
    for (task, op) in [
        ("ready", Op::Assign),
        ("running", Op::Renew),
        ("running", Op::ReportFailure),
        ("running", Op::Submit),
        ("submitted", Op::Pass),
        ("submitted", Op::Reject),
        ("accepted", Op::Invalidate),
        ("invalidated", Op::Unblock),
        ("waiting", Op::FailBlocked),
    ] {
        let mut run = base.clone();
        let accepted = apply(&mut run, task, op, &bad).is_ok();
        offenders.extend(time_problem(
            &format!("{op:?} on {task}"),
            &run,
            &base,
            accepted,
        ));
    }
    let mut run = base.clone();
    let accepted = run.cancel("ready", &bad).is_ok();
    offenders.extend(time_problem("cancel", &run, &base, accepted));
    let mut run = base.clone();
    let accepted = run.cancel_change(&bad).is_ok();
    offenders.extend(time_problem("cancel_change", &run, &base, accepted));
    // propagate_failures has no error to return.
    let mut cancelled = base.clone();
    cancelled.cancel("ready", &at(5)).expect("cancelled");
    let mut run = cancelled.clone();
    run.propagate_failures(&bad);
    offenders.extend(time_problem("propagate_failures", &run, &cancelled, true));
    // Nor has invalidate_dependents.
    let mut rerun = pagination();
    let (_, examples, _) = accept_all(&mut rerun);
    rerun
        .invalidate("implementation", &examples, &at(10))
        .expect("re-run");
    rerun
        .unblock("implementation", baseline(Vec::new()), &at(11))
        .expect("ready again");
    accept(&mut rerun, "implementation", "tree-i2", &at(12));
    let mut run = rerun.clone();
    run.invalidate_dependents("implementation", &bad);
    offenders.extend(time_problem("invalidate_dependents", &run, &rerun, true));
    // The integration steps (10.6).
    let mut integrating = pagination();
    accept_all(&mut integrating);
    integrating
        .begin_integration(&integrator())
        .expect("integrating");
    let mut run = integrating.clone();
    let accepted = run.stale_against_target("implementation", &bad).is_ok();
    offenders.extend(time_problem(
        "stale_against_target",
        &run,
        &integrating,
        accepted,
    ));
    let mut run = integrating.clone();
    let accepted = run.finish_integration(&"9".repeat(40), &bad).is_ok();
    offenders.extend(time_problem(
        "finish_integration",
        &run,
        &integrating,
        accepted,
    ));
    let mut run = integrating.clone();
    let accepted = run
        .fail_integration("retries exhausted", &["implementation"], &bad)
        .is_ok();
    offenders.extend(time_problem(
        "fail_integration",
        &run,
        &integrating,
        accepted,
    ));
    assert!(
        offenders.is_empty(),
        "operations that took the time `yesterday`:\n{}",
        offenders.join("\n")
    );
}

/// What is wrong with `run` after `what` ran at a time that is not RFC 3339:
/// a recorded time that is not one, or a change made by a refused operation.
fn time_problem(what: &str, run: &ChangeRun, before: &ChangeRun, accepted: bool) -> Option<String> {
    if accepted {
        run.tasks
            .iter()
            .flat_map(|t| &t.history)
            .find(|t| !t.at.is_valid())
            .map(|t| format!("{what} recorded `{}` ({} -> {})", t.at, t.from, t.to))
    } else {
        (run != before).then(|| format!("{what} was refused but changed the run"))
    }
}

#[test]
fn the_boundary_minimum_is_taken_per_field() {
    // 10.3: grant.boundary is "the per-field minimum of policy.boundary and
    // authority.boundary"; 10.8 `ExecutionBoundary::min`.
    let policy = boundary(FsAccess::Host, ProcessAccess::None, NetworkAccess::Any);
    let authority = boundary(
        FsAccess::Worktree,
        ProcessAccess::Any,
        NetworkAccess::Adapters,
    );
    let expected = boundary(
        FsAccess::Worktree,
        ProcessAccess::None,
        NetworkAccess::Adapters,
    );
    assert_eq!(policy.min(&authority), expected);
    assert_eq!(authority.min(&policy), expected);
    assert!(expected.within(&policy) && expected.within(&authority));
}

#[test]
fn within_is_judged_per_field_not_in_order() {
    // 10.8: `within` is per field. worktree/any/none precedes host/none/none
    // in field order, yet its process field is the wider one.
    let a = boundary(FsAccess::Worktree, ProcessAccess::Any, NetworkAccess::None);
    let b = boundary(FsAccess::Host, ProcessAccess::None, NetworkAccess::None);
    assert!(!a.within(&b));
    assert!(!b.within(&a));
    assert!(a.within(&a) && b.within(&b));
    let strictest = ExecutionBoundary::default();
    assert!(strictest.within(&a) && strictest.within(&b));
    assert!(a.within(&unconfined()) && b.within(&unconfined()));
}

#[test]
fn grants_never_change_during_a_run() {
    // 10.3: "Grant is computed when the change is opened and never changes for
    // the run".
    let tasks = vec![
        writer("implementation", &[], &[Facet::Body]),
        adder("examples", &[], "tests.pagination"),
        (
            spec("audit", &["implementation", "examples"]),
            Grant::default(),
        ),
    ];
    let grants: Vec<Grant> = tasks.iter().map(|(_, grant)| grant.clone()).collect();
    let mut run = open(3, tasks).expect("the run opens");
    accept_all(&mut run);
    run.begin_integration(&integrator()).expect("integrating");
    run.finish_integration(&"9".repeat(40), &at(30))
        .expect("integrated");
    let after: Vec<Grant> = run.tasks.iter().map(|t| t.grant.clone()).collect();
    assert_eq!(after, grants);
}

// ===========================================================================
// Git: compare-and-swap and isolated worktrees (10.4, 10.6, 10.7)
// ===========================================================================

fn git_available() -> bool {
    let found = Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("skipped: git is not installed");
    }
    found
}

/// Run git in `dir` for the test's own setup and checks; panics on failure.
#[track_caller]
fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .trim()
        .to_string()
}

/// Whether git exits with status 0.
fn git_succeeds(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("run git")
        .status
        .success()
}

/// A repository with one commit on `main` (`cpl.toml`, `src/m/m.cpl`), a
/// fixed identity, and no hooks, signing, or line-ending conversion, in a
/// fresh directory whose other entries are the worktrees of the test.
struct Repo {
    root: PathBuf,
    repo: GitRepo,
}

impl Repo {
    fn new(name: &str) -> Repo {
        let root = temp_dir(name);
        let dir = root.join("repo");
        std::fs::create_dir_all(dir.join("src/m")).expect("create the repository");
        git_in(&dir, &["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "cpl test"),
            ("user.email", "cpl@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git_in(&dir, &["config", key, value]);
        }
        std::fs::write(
            dir.join("cpl.toml"),
            "[project]\nname = \"t\"\nedition = \"0.1\"\n",
        )
        .expect("write");
        std::fs::write(dir.join("src/m/m.cpl"), "module m\n").expect("write");
        git_in(&dir, &["add", "-A"]);
        git_in(&dir, &["commit", "-q", "-m", "base"]);
        Repo {
            root,
            repo: GitRepo {
                toplevel: dir,
                project_prefix: String::new(),
            },
        }
    }

    fn dir(&self) -> &Path {
        &self.repo.toplevel
    }

    fn rev(&self, revision: &str) -> String {
        git_in(self.dir(), &["rev-parse", revision])
    }

    /// Someone else advances `main`: a commit adding `path` in the main checkout.
    fn advance_main(&self, path: &str) -> String {
        std::fs::write(self.dir().join(path), "module m\n").expect("write");
        git_in(self.dir(), &["add", "-A"]);
        git_in(self.dir(), &["commit", "-q", "-m", "concurrent change"]);
        self.rev("refs/heads/main")
    }

    /// An integration commit: an isolated worktree of `head` (10.7) with
    /// `path` added, snapshotted, and committed with `head` as its parent
    /// (10.6 `commit-tree new_tree -p head`).
    fn integration_commit(&self, worktree: &str, head: &str, path: &str) -> String {
        let wt = self.root.join(worktree);
        worktree_add(&self.repo, &wt, head).expect("worktree added");
        std::fs::write(wt.join(path), "module m\n").expect("write");
        let tree = write_tree(&self.repo, &wt).expect("tree written");
        commit_tree(&self.repo, &tree, &[head], "Integrate pagination_update").expect("committed")
    }
}

#[test]
fn compare_and_swap_refuses_a_target_that_moved_and_keeps_the_newer_head() {
    // 10.6: "if git update-ref <target_ref> <commit> <head> succeeds ... else:
    // continue # the target moved after validation"; design "Integration
    // semantics": the target advances "only if the head still equals the
    // revision just checked".
    if !git_available() {
        return;
    }
    let r = Repo::new("cas-moved");
    let checked = r.rev("refs/heads/main");
    let ours = r.integration_commit("wt-integrate", &checked, "src/m/ours.cpl");
    let theirs = r.advance_main("src/m/theirs.cpl");
    assert!(
        update_ref_cas(&r.repo, "refs/heads/main", &ours, &checked).is_err(),
        "the target moved after it was checked"
    );
    assert_eq!(
        r.rev("refs/heads/main"),
        theirs,
        "the newer head is never overwritten"
    );
    assert!(!git_succeeds(
        r.dir(),
        &["merge-base", "--is-ancestor", &ours, "refs/heads/main"]
    ));
}

#[test]
fn a_retry_against_the_new_head_advances_the_target_linearly() {
    if !git_available() {
        return;
    }
    let r = Repo::new("cas-retry");
    let checked = r.rev("refs/heads/main");
    let first = r.integration_commit("wt-attempt-1", &checked, "src/m/ours.cpl");
    let moved = r.advance_main("src/m/theirs.cpl");
    assert!(update_ref_cas(&r.repo, "refs/heads/main", &first, &checked).is_err());
    // The loop continues against the new head: rebuild on it, swap from it.
    let second = r.integration_commit("wt-attempt-2", &moved, "src/m/ours.cpl");
    update_ref_cas(&r.repo, "refs/heads/main", &second, &moved)
        .expect("the head equals the revision just checked");
    assert_eq!(r.rev("refs/heads/main"), second);
    assert_eq!(
        r.rev(&format!("{second}^")),
        moved,
        "the integration commit's parent is the checked head"
    );
    let files = git_in(
        r.dir(),
        &["ls-tree", "-r", "--name-only", "refs/heads/main"],
    );
    assert!(
        files.lines().any(|f| f == "src/m/ours.cpl")
            && files.lines().any(|f| f == "src/m/theirs.cpl"),
        "{files}"
    );
}

#[test]
fn of_two_integrations_of_one_head_only_the_first_advances_the_target() {
    if !git_available() {
        return;
    }
    let r = Repo::new("cas-race");
    let checked = r.rev("refs/heads/main");
    let a = r.integration_commit("wt-a", &checked, "src/m/a.cpl");
    let b = r.integration_commit("wt-b", &checked, "src/m/b.cpl");
    update_ref_cas(&r.repo, "refs/heads/main", &a, &checked).expect("the first swap");
    assert!(
        update_ref_cas(&r.repo, "refs/heads/main", &b, &checked).is_err(),
        "the second integrator checked a head that is gone"
    );
    assert_eq!(r.rev("refs/heads/main"), a);
}

#[test]
fn a_head_that_moved_away_and_back_still_equals_the_checked_revision() {
    // 10.6 compares the ref's value ("only if the head still equals the
    // revision just checked"), not its history.
    if !git_available() {
        return;
    }
    let r = Repo::new("cas-aba");
    let checked = r.rev("refs/heads/main");
    let ours = r.integration_commit("wt-integrate", &checked, "src/m/ours.cpl");
    let away = r.advance_main("src/m/theirs.cpl");
    git_in(r.dir(), &["update-ref", "refs/heads/main", &checked, &away]);
    update_ref_cas(&r.repo, "refs/heads/main", &ours, &checked)
        .expect("the head equals the checked revision again");
    assert_eq!(r.rev("refs/heads/main"), ours);
}

#[test]
fn a_deleted_target_is_neither_advanced_nor_recreated() {
    if !git_available() {
        return;
    }
    let r = Repo::new("cas-deleted");
    git_in(r.dir(), &["branch", "release"]);
    let checked = r.rev("refs/heads/release");
    let ours = r.integration_commit("wt-integrate", &checked, "src/m/ours.cpl");
    git_in(r.dir(), &["update-ref", "-d", "refs/heads/release"]);
    assert!(update_ref_cas(&r.repo, "refs/heads/release", &ours, &checked).is_err());
    assert!(
        !git_succeeds(
            r.dir(),
            &["rev-parse", "--verify", "--quiet", "refs/heads/release"]
        ),
        "the deleted target was recreated"
    );
}

#[test]
fn a_crashed_workers_worktree_never_updates_the_target() {
    // Design "Swarm authority and lifecycle": a crashed worker "leaves an
    // isolated worktree that can be inspected or reassigned; it does not
    // partially update the target branch"; 10.7 isolated checkout.
    if !git_available() {
        return;
    }
    let r = Repo::new("wt-crashed");
    let head = r.rev("refs/heads/main");
    let wt = r.root.join("wt-worker");
    worktree_add(&r.repo, &wt, &head).expect("worktree added");
    std::fs::write(
        wt.join("src/m/m.cpl"),
        "module m\n\nfn f() -> Int {\n    1\n}\n",
    )
    .expect("write");
    std::fs::write(wt.join("src/m/half.cpl"), "module m\n\nfn g(").expect("write");
    write_tree(&r.repo, &wt).expect("snapshotted");
    // The worker even commits on its detached HEAD before it dies.
    git_in(&wt, &["commit", "-q", "-m", "partial"]);
    assert_eq!(
        r.rev("refs/heads/main"),
        head,
        "the target branch is untouched"
    );
    assert_eq!(
        std::fs::read_to_string(r.dir().join("src/m/m.cpl")).expect("read"),
        "module m\n"
    );
    assert!(!r.dir().join("src/m/half.cpl").exists());
    assert_eq!(
        git_in(r.dir(), &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
    assert!(
        wt.join("src/m/half.cpl").is_file(),
        "the worktree is left for inspection"
    );
}

#[test]
fn a_worktree_snapshot_records_additions_changes_and_deletions() {
    // 10.7 "snapshot a worktree": `add -A` then `write-tree`.
    if !git_available() {
        return;
    }
    let r = Repo::new("wt-snapshot");
    let head = r.rev("refs/heads/main");
    let wt = r.root.join("wt-worker");
    worktree_add(&r.repo, &wt, &head).expect("worktree added");
    std::fs::write(wt.join("src/m/m.cpl"), "module m\n\n// edited\n").expect("write");
    std::fs::write(wt.join("src/m/added.cpl"), "module m\n").expect("write");
    std::fs::remove_file(wt.join("cpl.toml")).expect("remove");
    let tree = write_tree(&r.repo, &wt).expect("tree");
    assert_eq!(
        git_in(r.dir(), &["ls-tree", "-r", "--name-only", &tree]),
        "src/m/added.cpl\nsrc/m/m.cpl"
    );
    assert_eq!(
        git_in(r.dir(), &["show", &format!("{tree}:src/m/m.cpl")]),
        "module m\n\n// edited"
    );
}

#[test]
fn a_reassignment_gets_a_fresh_checkout_beside_the_kept_one() {
    // 10.2 Running -> Ready: "the old worktree is kept until reassigned"; the
    // next attempt works in its own isolated checkout of the baseline.
    if !git_available() {
        return;
    }
    let r = Repo::new("wt-reassign");
    let head = r.rev("refs/heads/main");
    let first = r.root.join("wt-attempt-1");
    let second = r.root.join("wt-attempt-2");
    worktree_add(&r.repo, &first, &head).expect("the first attempt");
    std::fs::write(first.join("src/m/partial.cpl"), "module m\n").expect("write");
    worktree_add(&r.repo, &second, &head).expect("the second attempt");
    assert!(
        first.join("src/m/partial.cpl").is_file(),
        "the old worktree is kept"
    );
    assert!(
        !second.join("src/m/partial.cpl").exists(),
        "the new checkout is the clean baseline"
    );
    assert_eq!(git_in(&second, &["rev-parse", "HEAD"]), head);
    let list = git_in(r.dir(), &["worktree", "list", "--porcelain"]);
    assert!(
        list.contains("wt-attempt-1") && list.contains("wt-attempt-2"),
        "{list}"
    );
}

#[test]
fn an_occupied_worktree_path_is_refused_and_left_alone() {
    if !git_available() {
        return;
    }
    let r = Repo::new("wt-occupied");
    let head = r.rev("refs/heads/main");
    let wt = r.root.join("wt-worker");
    worktree_add(&r.repo, &wt, &head).expect("added");
    std::fs::write(wt.join("src/m/partial.cpl"), "module m\n").expect("write");
    assert!(
        worktree_add(&r.repo, &wt, &head).is_err(),
        "a second checkout into a live worktree"
    );
    assert!(
        wt.join("src/m/partial.cpl").is_file(),
        "the worker's files are untouched"
    );
}

#[test]
fn snapshots_and_candidates_are_commits_on_their_baselines_under_their_refs() {
    // 10.4: "snapshot = commit-tree(write-tree(tree), parent = base_commit)
    // stored at refs/cpl/snapshots/<task id>/<attempt>"; 10.1
    // Candidate.snapshot_commit: "refs/cpl/candidates/<id>, parent =
    // baseline.snapshot_commit". The zero object id as `old` means "must not
    // exist yet" (git update-ref), so a stored candidate is never replaced.
    if !git_available() {
        return;
    }
    let r = Repo::new("snapshot-refs");
    let base = r.rev("refs/heads/main");
    let task = TaskId {
        change: ChangeRunId::from_random_bytes(*b"adversary!"),
        name: "audit".to_string(),
    };
    let wt = r.root.join("wt-audit");
    worktree_add(&r.repo, &wt, &base).expect("added");
    // The assembled snapshot: a predecessor's output layered on the base.
    std::fs::write(wt.join("src/m/predecessor.cpl"), "module m\n").expect("write");
    let snapshot_tree = write_tree(&r.repo, &wt).expect("tree");
    let snapshot = commit_tree(&r.repo, &snapshot_tree, &[&base], "snapshot").expect("committed");
    let snapshot_ref = format!("refs/cpl/snapshots/{task}/1");
    update_ref_cas(&r.repo, &snapshot_ref, &snapshot, "").expect("a task id is a ref path");
    assert_eq!(r.rev(&snapshot_ref), snapshot);
    assert_eq!(r.rev(&format!("{snapshot_ref}^")), base);
    // The task's candidate, on that snapshot.
    std::fs::write(wt.join("src/m/audit.cpl"), "module m\n").expect("write");
    let candidate_tree = write_tree(&r.repo, &wt).expect("tree");
    let candidate =
        commit_tree(&r.repo, &candidate_tree, &[&snapshot], "candidate").expect("committed");
    let id = CandidateId::derive(&task, 1, FencingToken(3), &candidate_tree);
    let candidate_ref = format!("refs/cpl/candidates/{id}");
    update_ref_cas(&r.repo, &candidate_ref, &candidate, "").expect("stored");
    assert_eq!(r.rev(&format!("{candidate_ref}^")), snapshot);
    assert!(update_ref_cas(&r.repo, &candidate_ref, &snapshot, &"0".repeat(40)).is_err());
    assert_eq!(r.rev(&candidate_ref), candidate);
}
