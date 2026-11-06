//! The task state machine of `spec/toolchain.md` 10.2 over one change run,
//! with leases and fencing tokens, as pure functions: every operation
//! takes the current time as an argument, touches nothing outside the
//! [`ChangeRun`] value (the sandbox of [`ChangeRun::assign`] is the one
//! injected adapter), and a rejected operation changes nothing, except
//! where its documentation says otherwise.
//!
//! Every time the run records (a transition's `at`, a lease's `issued_at`
//! and `expires_at`) is the operation's `now`, checked to be RFC 3339 and
//! spelled in UTC (Section 10.1: `Timestamp` is "RFC 3339, UTC"). An
//! operation given a `now` that is not RFC 3339 is refused with
//! [`ScheduleError::InvalidTimestamp`], or, when it has no error to return
//! ([`ChangeRun::propagate_failures`], [`ChangeRun::invalidate_dependents`]),
//! records nothing.
//!
//! | 10.2 transition | Operation |
//! | - | - |
//! | (new) -> `Ready` / `Blocked` | [`ChangeRun::open`] |
//! | `Blocked` -> `Ready` | [`ChangeRun::unblock`] (see [`ChangeRun::ready_to_unblock`]) |
//! | `Blocked` -> `Failed` | [`ChangeRun::propagate_failures`], [`ChangeRun::fail_blocked`] |
//! | `Ready` -> `Running` | [`ChangeRun::assign`] |
//! | `Running` -> `Running` | [`ChangeRun::renew`] |
//! | `Running` -> `Ready` | [`ChangeRun::expire_leases`], [`ChangeRun::report_failure`] |
//! | `Running` -> `Submitted` | [`ChangeRun::submit`] (fencing, idempotent resubmission) |
//! | `Submitted` -> `Accepted` / `Ready` | [`ChangeRun::validation_passed`] / [`ChangeRun::validation_failed`] |
//! | `Accepted` -> `Blocked` | [`ChangeRun::invalidate`], [`ChangeRun::invalidate_dependents`], [`ChangeRun::stale_against_target`] (10.6) |
//! | `Accepted` -> `Integrated` | [`ChangeRun::finish_integration`] (10.6) |
//! | `Accepted` -> `Failed` | [`ChangeRun::fail_integration`] (10.6) |
//! | any non-terminal -> `Cancelled` | [`ChangeRun::cancel`], [`ChangeRun::cancel_change`] |

use super::{boundary_text, ExecutionBoundary, Grant, Sandbox, TaskState};
use crate::codes;
use crate::model::{
    namespace_covers, Actor, Baseline, CandidateId, ChangeRecord, ChangeState, FencingToken, Lease,
    TaskId, TaskRecord, TaskSpec, Timestamp, Transition,
};
use cpl_diag::{Category, DiagCode, Diagnostic, Facet, StableId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// One change run: the document `<project root>/.cpl/work/<change run
/// id>.json` (Section 10.1, T33), the change record and its task records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRun {
    pub change: ChangeRecord,
    /// In the order of `change.tasks`.
    pub tasks: Vec<TaskRecord>,
}

/// What a worker submits for its assignment: the identity of the
/// candidate (Section 10.1 `CandidateId`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub attempt: u32,
    pub token: FencingToken,
    /// The git tree oid of the candidate source tree.
    pub tree: String,
}

/// The answer to a submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// The candidate was recorded and the task is `Submitted`.
    Recorded(CandidateId),
    /// The same candidate (task, attempt, token, tree) was submitted
    /// before: its existing id, and nothing changed (Section 10.2).
    Unchanged(CandidateId),
}

impl SubmitOutcome {
    pub fn id(&self) -> &CandidateId {
        match self {
            SubmitOutcome::Recorded(id) | SubmitOutcome::Unchanged(id) => id,
        }
    }
}

/// Why a state-machine operation was refused. [`ScheduleError::code`]
/// names the Appendix B code where the spec defines one.
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub enum ScheduleError {
    /// A change whose `workers` ceiling is 0 (Section 10.2, as `cpl work
    /// check` refuses it, 7.2): no task of it could ever be assigned.
    #[error(
        "change `{change}` has `workers {workers}`; the concurrency ceiling must be at least 1"
    )]
    BadWorkers { change: String, workers: u32 },
    #[error("change run {change} has no task `{task}`")]
    UnknownTask { change: String, task: String },
    #[error("task `{task}` is declared more than once")]
    DuplicateTask { task: String },
    #[error("task `{task}` runs after `{predecessor}`, which is not a task of the change")]
    UnknownPredecessor { task: String, predecessor: String },
    #[error("the `after` edges form a cycle: {}", .tasks.join(" -> "))]
    Cycle { tasks: Vec<String> },
    /// A change of more than `reach::MAX_TASKS_PER_CHANGE` tasks.
    #[error("change `{change}` declares {tasks} tasks; a change may declare at most {limit}")]
    TooManyTasks {
        change: String,
        tasks: usize,
        limit: usize,
    },
    /// Two tasks that neither runs after the other (directly or through a
    /// chain of `after`) write one facet or add to overlapping namespaces;
    /// `overlap` names the shared facet or the two namespaces.
    #[error("tasks `{first}` and `{second}` are unordered (neither runs after the other), but both {overlap}")]
    OverlappingWrites {
        first: String,
        second: String,
        overlap: String,
    },
    #[error("change run {change} is {state}; it cannot {action}")]
    InvalidChangeState {
        change: String,
        state: String,
        action: &'static str,
    },
    #[error("task `{task}` is {state}; it cannot {action}")]
    InvalidTaskState {
        task: String,
        state: TaskState,
        action: &'static str,
    },
    #[error("task `{task}` still waits for {}", .pending.join(", "))]
    PredecessorsPending { task: String, pending: Vec<String> },
    /// The snapshot does not layer exactly the current candidates of the
    /// task's predecessors (Section 10.4).
    #[error("the snapshot of task `{task}` was not assembled from exactly the current candidates of its predecessors ({})", baseline_mismatch(.missing, .unexpected))]
    StaleBaseline {
        task: String,
        /// Current candidates of predecessors the snapshot does not layer
        /// (`<task> (no candidate)` for a predecessor that has none).
        missing: Vec<String>,
        /// Candidates the snapshot layers that are no current candidate of
        /// a predecessor, or that it layers twice.
        unexpected: Vec<String>,
    },
    #[error("all {workers} workers of change run {change} are busy")]
    WorkersBusy { change: String, workers: u32 },
    #[error("change run {change} has issued every fencing token")]
    TokensExhausted { change: String },
    #[error("task `{task}`: the host cannot enforce the granted boundary ({}); the strictest boundary it can enforce is {}", boundary_text(.granted), boundary_text(.enforceable))]
    BoundaryUnenforceable {
        task: String,
        granted: ExecutionBoundary,
        enforceable: ExecutionBoundary,
    },
    #[error("task `{task}`: the sandbox reported a boundary ({}) wider than the granted one ({})", boundary_text(.reported), boundary_text(.granted))]
    LaunchExceededGrant {
        task: String,
        granted: ExecutionBoundary,
        reported: ExecutionBoundary,
    },
    #[error("task `{task}`: the sandbox could not launch the worker: {message}")]
    LaunchFailed { task: String, message: String },
    #[error("stale submission for task `{task}`: {reason}")]
    StaleSubmission { task: String, reason: String },
    #[error("actor `{actor}` has no integration authority")]
    NotAuthorized { actor: String },
    #[error("`{value}` is not an RFC 3339 timestamp")]
    InvalidTimestamp { value: String },
}

impl ScheduleError {
    /// The Appendix B code: `work_stale_submission`,
    /// `work_boundary_unenforceable`, `work_not_authorized`,
    /// `work_bad_workers`, `work_unknown_task`, `work_cycle`,
    /// `work_overlapping_writes`; `None` for misuse of the state machine,
    /// which has no code of its own.
    pub fn code(&self) -> Option<DiagCode> {
        Some(match self {
            ScheduleError::BadWorkers { .. } => codes::WORK_BAD_WORKERS,
            ScheduleError::StaleSubmission { .. } => codes::WORK_STALE_SUBMISSION,
            ScheduleError::BoundaryUnenforceable { .. }
            | ScheduleError::LaunchExceededGrant { .. } => codes::WORK_BOUNDARY_UNENFORCEABLE,
            ScheduleError::NotAuthorized { .. } => codes::WORK_NOT_AUTHORIZED,
            ScheduleError::UnknownPredecessor { .. } => codes::WORK_UNKNOWN_TASK,
            ScheduleError::Cycle { .. } => codes::WORK_CYCLE,
            ScheduleError::TooManyTasks { .. } => codes::WORK_TOO_MANY_TASKS,
            ScheduleError::OverlappingWrites { .. } => codes::WORK_OVERLAPPING_WRITES,
            _ => return None,
        })
    }

    /// The error as a diagnostic: its code (category `work`), or
    /// `internal_error` (category `internal`) when it has none. The
    /// payload carries `task` when one is involved (the first of two
    /// overlapping tasks), and `change` and `found` for `work_bad_workers`,
    /// as `cpl work check` reports it.
    pub fn to_diagnostic(&self) -> Diagnostic {
        let mut diagnostic = match self.code() {
            Some(code) => Diagnostic::error(code, codes::WORK, self.to_string()),
            None => Diagnostic::error(codes::INTERNAL_ERROR, Category::Internal, self.to_string()),
        };
        if let Some(task) = self.task() {
            diagnostic = diagnostic.with_data("task", json!(task));
        }
        if let ScheduleError::BadWorkers { change, workers } = self {
            diagnostic = diagnostic
                .with_data("change", json!(change))
                .with_data("found", json!(workers));
        }
        diagnostic
    }

    fn task(&self) -> Option<&str> {
        match self {
            ScheduleError::UnknownTask { task, .. }
            | ScheduleError::DuplicateTask { task }
            | ScheduleError::UnknownPredecessor { task, .. }
            | ScheduleError::OverlappingWrites { first: task, .. }
            | ScheduleError::InvalidTaskState { task, .. }
            | ScheduleError::PredecessorsPending { task, .. }
            | ScheduleError::StaleBaseline { task, .. }
            | ScheduleError::BoundaryUnenforceable { task, .. }
            | ScheduleError::LaunchExceededGrant { task, .. }
            | ScheduleError::LaunchFailed { task, .. }
            | ScheduleError::StaleSubmission { task, .. } => Some(task),
            _ => None,
        }
    }
}

/// The parenthesis of [`ScheduleError::StaleBaseline`]'s message.
fn baseline_mismatch(missing: &[String], unexpected: &[String]) -> String {
    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!("missing {}", missing.join(", ")));
    }
    if !unexpected.is_empty() {
        parts.push(format!(
            "layering {}, which is no current candidate of a predecessor",
            unexpected.join(", ")
        ));
    }
    parts.join("; ")
}

impl ChangeRun {
    // -----------------------------------------------------------------------
    // Opening and queries
    // -----------------------------------------------------------------------

    /// Open a change run (10.2 "change opened"): one record per task, in
    /// order; a task without `after` predecessors is `Ready` with the base
    /// snapshot `initial` as its baseline ("All initial tasks use that
    /// snapshot", design line 351), a task with predecessors is `Blocked`
    /// with the reason `waiting_for: [a, b]`. `change.tasks` is set to the
    /// tasks' ids. The change's `workers` ceiling must be at least 1
    /// (`work_bad_workers`, the rule of `cpl work check`, Section 7.2: a run
    /// with no worker could assign no task), the task names must be
    /// unique, every `after` name must be a task of the change
    /// (`work_unknown_task`), the `after` graph must be acyclic
    /// (`work_cycle`), and no two unordered tasks may write one facet or
    /// add to overlapping namespaces (`work_overlapping_writes`).
    ///
    /// The last rule is the design's: "Overlapping write sets are rejected
    /// among unordered tasks by default" ("Changes and agent tasks") and
    /// "The scheduler rejects cycles and unordered overlapping edits"
    /// ("Swarm authority and lifecycle"). The specification states it, with
    /// its code, only for `cpl work check` (Section 7.2); the run checks it
    /// as 7.2 words it, as it checks the cycle rule. Two tasks are
    /// unordered when neither reaches the other through `after`. A task's
    /// write set is the facets its `write` selectors request (`.definition`
    /// as every facet but `identity`, Section 10.3) and the facets its grant
    /// writes, which 10.3 makes a subset of them; its namespaces are its
    /// `add` namespaces and its grant's, and two namespaces overlap when one
    /// covers the other (T43).
    pub fn open(
        mut change: ChangeRecord,
        tasks: Vec<(TaskSpec, Grant)>,
        initial: Baseline,
    ) -> Result<ChangeRun, ScheduleError> {
        if change.workers == 0 {
            return Err(ScheduleError::BadWorkers {
                change: change.name.clone(),
                workers: change.workers,
            });
        }
        if tasks.len() > crate::reach::MAX_TASKS_PER_CHANGE {
            return Err(ScheduleError::TooManyTasks {
                change: change.name.clone(),
                tasks: tasks.len(),
                limit: crate::reach::MAX_TASKS_PER_CHANGE,
            });
        }
        let mut names = BTreeSet::new();
        for (spec, _) in &tasks {
            if !names.insert(spec.name.as_str()) {
                return Err(ScheduleError::DuplicateTask {
                    task: spec.name.clone(),
                });
            }
        }
        for (spec, _) in &tasks {
            if let Some(unknown) = spec.after.iter().find(|p| !names.contains(p.as_str())) {
                return Err(ScheduleError::UnknownPredecessor {
                    task: spec.name.clone(),
                    predecessor: unknown.clone(),
                });
            }
        }
        let specs: Vec<&TaskSpec> = tasks.iter().map(|(spec, _)| spec).collect();
        if let Some(cycle) = find_cycle(&specs) {
            return Err(ScheduleError::Cycle { tasks: cycle });
        }
        if let Some(overlap) = find_overlap(&tasks) {
            return Err(overlap);
        }
        change.tasks = tasks
            .iter()
            .map(|(spec, _)| TaskId {
                change: change.id.clone(),
                name: spec.name.clone(),
            })
            .collect();
        let records = tasks
            .into_iter()
            .zip(change.tasks.clone())
            .map(|((spec, grant), id)| {
                let (state, baseline, blocked) = if spec.after.is_empty() {
                    (TaskState::Ready, Some(initial.clone()), None)
                } else {
                    let waiting = format!("waiting_for: [{}]", spec.after.join(", "));
                    (TaskState::Blocked, None, Some(waiting))
                };
                TaskRecord {
                    id,
                    spec,
                    grant,
                    state,
                    attempt: 0,
                    lease: None,
                    baseline,
                    candidate: None,
                    blocked,
                    history: Vec::new(),
                }
            })
            .collect();
        Ok(ChangeRun {
            change,
            tasks: records,
        })
    }

    /// The record of the task with this name.
    pub fn task(&self, name: &str) -> Option<&TaskRecord> {
        self.tasks.iter().find(|t| t.id.name == name)
    }

    /// The number of `Running` tasks (each holds a lease and a worker).
    pub fn running(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.state == TaskState::Running)
            .count()
    }

    /// `[work] lease_seconds` of the frozen policy.
    pub fn lease_seconds(&self) -> u64 {
        self.change.frozen_inputs.policy.work.lease_seconds
    }

    /// The `Blocked` tasks whose predecessors are all `Accepted` or
    /// `Integrated`: those whose candidate snapshot can be assembled
    /// (Section 10.4) and which [`ChangeRun::unblock`] then makes `Ready`.
    pub fn ready_to_unblock(&self) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|t| t.state == TaskState::Blocked && self.pending(t).is_empty())
            .map(|t| t.id.name.clone())
            .collect()
    }

    // -----------------------------------------------------------------------
    // Blocked
    // -----------------------------------------------------------------------

    /// `Blocked -> Ready`: every predecessor is `Accepted` or `Integrated`
    /// (else nothing changes), and `baseline` is the candidate snapshot the
    /// caller assembled from them and checked (Section 10.4). Its
    /// `predecessors` must be exactly the current candidates of the task's
    /// predecessors, each once: 10.4 layers `p.candidate` of every
    /// predecessor `p` and nothing else, so a snapshot that misses one,
    /// still layers a candidate that was since replaced, or layers the
    /// output of a task that is not a predecessor was not assembled for
    /// this task, and is refused ([`ScheduleError::StaleBaseline`]). Their
    /// order (10.4: topological by `after`, ties by task name) is not
    /// checked.
    pub fn unblock(
        &mut self,
        name: &str,
        baseline: Baseline,
        now: &Timestamp,
    ) -> Result<(), ScheduleError> {
        self.ensure_open("unblock a task")?;
        let i = self.index(name)?;
        let now = utc(now)?;
        self.expect_state(i, &[TaskState::Blocked], "be unblocked")?;
        let pending = self.pending(&self.tasks[i]);
        if !pending.is_empty() {
            return Err(ScheduleError::PredecessorsPending {
                task: name.to_string(),
                pending,
            });
        }
        let (missing, unexpected) = self.predecessor_mismatch(&self.tasks[i], &baseline);
        if !missing.is_empty() || !unexpected.is_empty() {
            return Err(ScheduleError::StaleBaseline {
                task: name.to_string(),
                missing,
                unexpected,
            });
        }
        self.tasks[i].baseline = Some(baseline);
        self.transition(
            i,
            TaskState::Ready,
            &now,
            None,
            "predecessors_accepted".to_string(),
        );
        Ok(())
    }

    /// `Blocked -> Failed` for every `Blocked` task with a `Failed` or
    /// `Cancelled` predecessor, repeated until no more fail (so a failure
    /// propagates through chains of `after`). Returns the failed tasks in
    /// the order they failed; nothing in a closed change run, and nothing
    /// (no task fails) when `now` is not RFC 3339.
    pub fn propagate_failures(&mut self, now: &Timestamp) -> Vec<String> {
        let mut failed = Vec::new();
        let Ok(now) = utc(now) else {
            return failed;
        };
        if self.change.state.is_terminal() {
            return failed;
        }
        loop {
            let next = self.tasks.iter().enumerate().find_map(|(i, t)| {
                if t.state != TaskState::Blocked {
                    return None;
                }
                t.spec.after.iter().find_map(|p| {
                    let state = self.task(p)?.state;
                    matches!(state, TaskState::Failed | TaskState::Cancelled)
                        .then(|| (i, format!("predecessor_{state}: {p}")))
                })
            });
            let Some((i, reason)) = next else {
                return failed;
            };
            self.transition(i, TaskState::Failed, &now, None, reason);
            failed.push(self.tasks[i].id.name.clone());
        }
    }

    /// `Blocked -> Failed` with a reason, for an assembly conflict that
    /// cannot be resolved (Section 10.4, reason `work_conflict: ...`).
    pub fn fail_blocked(
        &mut self,
        name: &str,
        reason: &str,
        now: &Timestamp,
    ) -> Result<(), ScheduleError> {
        self.ensure_open("fail a task")?;
        let i = self.index(name)?;
        let now = utc(now)?;
        self.expect_state(i, &[TaskState::Blocked], "fail while blocked")?;
        self.transition(i, TaskState::Failed, &now, None, reason.to_string());
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Leases
    // -----------------------------------------------------------------------

    /// `Ready -> Running` (Sections 10.2 and 10.8): the change's `workers`
    /// ceiling must not be reached, and the host must be able to enforce
    /// the task's granted boundary, else `work_boundary_unenforceable` and
    /// the task stays `Ready` with no lease, token, or worker. Then a lease
    /// is issued with a fresh fencing token (`change.next_token`, strictly
    /// increasing), the next attempt number, `issued_at = now`, and
    /// `expires_at = now + [work] lease_seconds` of the frozen policy, and
    /// the worker is started through `sandbox.launch(lease,
    /// grant.boundary)`; the boundary it reports is recorded in the lease.
    /// `worktree` is the isolated checkout of the baseline snapshot the
    /// caller created; the previous lease, whose worktree was kept until
    /// this reassignment, is replaced.
    ///
    /// Enforceability: [`Sandbox::enforceable`] is the strictest boundary
    /// the host can enforce, and a host that can confine a worker that far
    /// can confine it to any wider boundary (and may enforce a stricter one
    /// than granted, 10.8 "launch"), so the grant is enforceable iff
    /// `sandbox.enforceable().within(&grant.boundary)`, field by field.
    /// This is 10.8's "host" row: a host with no mechanism reports
    /// `host`/`any`/`any` "and can therefore run only tasks whose policy
    /// grants exactly that". The "refusal" row of 10.8 writes the reverse,
    /// `grant.boundary.within(sandbox.enforceable())`, which would let a
    /// host without any sandbox run every task and a full sandbox only the
    /// strictest ones, against design line 361 ("The scheduler must
    /// enforce filesystem, process, network, and integration boundaries").
    ///
    /// A token handed to the sandbox is never issued again: when the launch
    /// fails, or reports a boundary wider than the grant (which the
    /// coordinator refuses rather than records, `work_boundary_unenforceable`),
    /// the task stays `Ready` but the token is spent.
    pub fn assign(
        &mut self,
        name: &str,
        holder: &str,
        worktree: PathBuf,
        sandbox: &mut dyn Sandbox,
        now: &Timestamp,
    ) -> Result<Lease, ScheduleError> {
        self.ensure_open("assign a task")?;
        let i = self.index(name)?;
        let now = utc(now)?;
        self.expect_state(i, &[TaskState::Ready], "be assigned")?;
        if self.running() >= self.change.workers as usize {
            return Err(ScheduleError::WorkersBusy {
                change: self.change.id.to_string(),
                workers: self.change.workers,
            });
        }
        let granted = self.tasks[i].grant.boundary;
        let enforceable = sandbox.enforceable();
        if !enforceable.within(&granted) {
            return Err(ScheduleError::BoundaryUnenforceable {
                task: name.to_string(),
                granted,
                enforceable,
            });
        }
        let expires_at = self.expiry(&now)?;
        let next_token = self.change.next_token.checked_add(1).ok_or_else(|| {
            ScheduleError::TokensExhausted {
                change: self.change.id.to_string(),
            }
        })?;
        let token = FencingToken(self.change.next_token);
        self.change.next_token = next_token;
        let attempt = self.tasks[i].attempt.saturating_add(1);
        let mut lease = Lease {
            task: self.tasks[i].id.clone(),
            attempt,
            token,
            holder: holder.to_string(),
            worktree,
            boundary: granted,
            issued_at: now.0.clone(),
            expires_at,
        };
        let reported =
            sandbox
                .launch(&lease, &granted)
                .map_err(|e| ScheduleError::LaunchFailed {
                    task: name.to_string(),
                    message: e.message,
                })?;
        if !reported.within(&granted) {
            return Err(ScheduleError::LaunchExceededGrant {
                task: name.to_string(),
                granted,
                reported,
            });
        }
        lease.boundary = reported;
        self.tasks[i].attempt = attempt;
        self.tasks[i].lease = Some(lease.clone());
        self.transition(
            i,
            TaskState::Running,
            &now,
            Some(token),
            format!("assigned to {holder}"),
        );
        Ok(lease)
    }

    /// `Running -> Running`: renew the lease of the current assignment
    /// (same token, before expiry) to `now + lease_seconds`; returns the new
    /// expiry. A superseded token or an expired lease is
    /// `work_stale_submission`.
    pub fn renew(
        &mut self,
        name: &str,
        token: FencingToken,
        now: &Timestamp,
    ) -> Result<Timestamp, ScheduleError> {
        self.ensure_open("renew a lease")?;
        let i = self.index(name)?;
        let now = utc(now)?;
        self.check_assignment(i, token, None, &now)?;
        let expires_at = self.expiry(&now)?;
        if let Some(lease) = self.tasks[i].lease.as_mut() {
            lease.expires_at = expires_at.clone();
        }
        self.transition(
            i,
            TaskState::Running,
            &now,
            Some(token),
            "lease_renewed".to_string(),
        );
        Ok(expires_at)
    }

    /// `Running -> Ready` for every task whose lease has expired at `now`
    /// without a submission (also what a coordinator restart does, Section
    /// 10.1). The lease stays recorded, so its worktree is kept until the
    /// task is reassigned and its token stays superseded. Returns the tasks
    /// in order; none in a closed run.
    pub fn expire_leases(&mut self, now: &Timestamp) -> Result<Vec<String>, ScheduleError> {
        let now = utc(now)?;
        if self.change.state.is_terminal() {
            return Ok(Vec::new());
        }
        let expired: Vec<usize> = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.state == TaskState::Running
                    && t.lease.as_ref().is_some_and(|l| l.is_expired(&now.0))
            })
            .map(|(i, _)| i)
            .collect();
        let mut names = Vec::with_capacity(expired.len());
        for i in expired {
            let token = self.tasks[i].lease.as_ref().map(|l| l.token);
            self.transition(
                i,
                TaskState::Ready,
                &now,
                token,
                "lease_expired".to_string(),
            );
            names.push(self.tasks[i].id.name.clone());
        }
        Ok(names)
    }

    /// `Running -> Ready`: the worker of the current assignment (its token)
    /// reported failure. A report with a superseded token is
    /// `work_stale_submission`, so a stale worker cannot reset a newer
    /// assignment; an expired but not yet voided lease may still report.
    pub fn report_failure(
        &mut self,
        name: &str,
        token: FencingToken,
        reason: &str,
        now: &Timestamp,
    ) -> Result<(), ScheduleError> {
        self.ensure_open("record a worker failure")?;
        let i = self.index(name)?;
        let now = utc(now)?;
        self.check_token(i, token, None)?;
        self.transition(
            i,
            TaskState::Ready,
            &now,
            Some(token),
            format!("worker_failed: {reason}"),
        );
        Ok(())
    }

    /// `Running -> Submitted` (Section 10.2): the submission's candidate id
    /// is derived from (task, attempt, token, tree). When it equals the
    /// task's recorded candidate, the submission repeats an earlier one: the
    /// existing id is returned and nothing changes, whatever the task's
    /// state now. Otherwise the fencing rule applies: the task must be
    /// `Running`, the token and attempt must be those of its current lease
    /// (a lower token is a superseded assignment), and the lease must not
    /// have expired at `now`; any failure is `work_stale_submission` and
    /// changes nothing.
    pub fn submit(
        &mut self,
        name: &str,
        submission: &Submission,
        now: &Timestamp,
    ) -> Result<SubmitOutcome, ScheduleError> {
        let i = self.index(name)?;
        let id = CandidateId::derive(
            &self.tasks[i].id,
            submission.attempt,
            submission.token,
            &submission.tree,
        );
        if self.tasks[i].candidate.as_ref() == Some(&id) {
            return Ok(SubmitOutcome::Unchanged(id));
        }
        if self.change.state.is_terminal() {
            return Err(ScheduleError::StaleSubmission {
                task: name.to_string(),
                reason: format!("change run {} is {}", self.change.id, self.change.state),
            });
        }
        let now = utc(now)?;
        self.check_assignment(i, submission.token, Some(submission.attempt), &now)?;
        self.tasks[i].candidate = Some(id.clone());
        self.transition(
            i,
            TaskState::Submitted,
            &now,
            Some(submission.token),
            format!("submitted {id}"),
        );
        Ok(SubmitOutcome::Recorded(id))
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    /// `Submitted -> Accepted`: task-level validation passed (scope
    /// enforcement, Section 10.5, typecheck, `expect` checks, the task's own
    /// tests).
    pub fn validation_passed(&mut self, name: &str, now: &Timestamp) -> Result<(), ScheduleError> {
        self.ensure_open("accept a candidate")?;
        let i = self.index(name)?;
        let now = utc(now)?;
        self.expect_state(i, &[TaskState::Submitted], "be accepted")?;
        let token = self.tasks[i].lease.as_ref().map(|l| l.token);
        self.transition(
            i,
            TaskState::Accepted,
            &now,
            token,
            "validation_passed".to_string(),
        );
        Ok(())
    }

    /// `Submitted -> Ready`: task-level validation failed; `reason` (the
    /// diagnostics' codes, for example) is attached to the transition. The
    /// next assignment increments the attempt.
    pub fn validation_failed(
        &mut self,
        name: &str,
        reason: &str,
        now: &Timestamp,
    ) -> Result<(), ScheduleError> {
        self.ensure_open("reject a candidate")?;
        let i = self.index(name)?;
        let now = utc(now)?;
        self.expect_state(i, &[TaskState::Submitted], "be rejected")?;
        let token = self.tasks[i].lease.as_ref().map(|l| l.token);
        self.transition(
            i,
            TaskState::Ready,
            &now,
            token,
            format!("validation_failed: {reason}"),
        );
        Ok(())
    }

    /// `Accepted -> Blocked` with the reason `invalidated_by: <candidate>`:
    /// a predecessor's accepted candidate changed, or a review of this
    /// task's candidate became stale.
    pub fn invalidate(
        &mut self,
        name: &str,
        by: &CandidateId,
        now: &Timestamp,
    ) -> Result<(), ScheduleError> {
        self.ensure_open("invalidate a task")?;
        let i = self.index(name)?;
        let now = utc(now)?;
        self.expect_state(i, &[TaskState::Accepted], "be invalidated")?;
        let reason = format!("invalidated_by: {by}");
        self.tasks[i].blocked = Some(reason.clone());
        self.transition(i, TaskState::Blocked, &now, None, reason);
        Ok(())
    }

    /// [`ChangeRun::invalidate`] every `Accepted` task that runs directly
    /// after `name` and whose baseline does not include `name`'s current
    /// candidate: the predecessor was re-run and its accepted candidate
    /// changed. Returns the invalidated tasks; none when `name` has no
    /// candidate, the run is closed, or `now` is not RFC 3339.
    pub fn invalidate_dependents(&mut self, name: &str, now: &Timestamp) -> Vec<String> {
        let Ok(now) = utc(now) else {
            return Vec::new();
        };
        let Some(candidate) = self.task(name).and_then(|t| t.candidate.clone()) else {
            return Vec::new();
        };
        let stale: Vec<String> = self
            .tasks
            .iter()
            .filter(|t| {
                t.state == TaskState::Accepted
                    && t.spec.after.iter().any(|p| p == name)
                    && !t
                        .baseline
                        .as_ref()
                        .is_some_and(|b| b.predecessors.contains(&candidate))
            })
            .map(|t| t.id.name.clone())
            .collect();
        stale
            .into_iter()
            .filter(|t| self.invalidate(t, &candidate, &now.0).is_ok())
            .collect()
    }

    // -----------------------------------------------------------------------
    // Cancellation
    // -----------------------------------------------------------------------

    /// Any non-terminal state `-> Cancelled` (operator request). A running
    /// task's lease stays recorded (its worktree is kept for inspection),
    /// and its token can no longer submit.
    pub fn cancel(&mut self, name: &str, now: &Timestamp) -> Result<(), ScheduleError> {
        let i = self.index(name)?;
        let now = utc(now)?;
        if self.tasks[i].state.is_terminal() {
            return Err(self.state_error(i, "be cancelled"));
        }
        self.transition(i, TaskState::Cancelled, &now, None, "cancelled".to_string());
        Ok(())
    }

    /// Cancel the change run: every non-terminal task becomes `Cancelled`
    /// and the change `Cancelled`. Returns the cancelled tasks.
    pub fn cancel_change(&mut self, now: &Timestamp) -> Result<Vec<String>, ScheduleError> {
        self.ensure_open("be cancelled")?;
        let now = utc(now)?;
        let mut cancelled = Vec::new();
        for i in 0..self.tasks.len() {
            if !self.tasks[i].state.is_terminal() {
                self.transition(i, TaskState::Cancelled, &now, None, "cancelled".to_string());
                cancelled.push(self.tasks[i].id.name.clone());
            }
        }
        self.change.state = ChangeState::Cancelled;
        Ok(cancelled)
    }

    // -----------------------------------------------------------------------
    // Integration (Section 10.6)
    // -----------------------------------------------------------------------

    /// `Open -> Integrating` for an actor with integration authority; any
    /// other actor is `work_not_authorized` and nothing changes (design
    /// line 365).
    pub fn begin_integration(&mut self, actor: &Actor) -> Result<(), ScheduleError> {
        if !actor.authority.may_integrate {
            return Err(ScheduleError::NotAuthorized {
                actor: actor.id.clone(),
            });
        }
        if self.change.state != ChangeState::Open {
            return Err(self.change_error("begin integration"));
        }
        self.change.state = ChangeState::Integrating;
        Ok(())
    }

    /// A candidate's `expect base` failed against the target head (Section
    /// 10.6): the task goes `Accepted -> Blocked` with the reason
    /// `work_stale_against_target`, and integration stops (the change is
    /// `Open` again, for an operator or rescheduling to re-validate).
    pub fn stale_against_target(
        &mut self,
        name: &str,
        now: &Timestamp,
    ) -> Result<(), ScheduleError> {
        if self.change.state != ChangeState::Integrating {
            return Err(self.change_error("mark a candidate stale against the target"));
        }
        let i = self.index(name)?;
        let now = utc(now)?;
        self.expect_state(i, &[TaskState::Accepted], "become stale against the target")?;
        let reason = codes::WORK_STALE_AGAINST_TARGET.to_string();
        self.tasks[i].blocked = Some(reason.clone());
        self.transition(i, TaskState::Blocked, &now, None, reason);
        self.change.state = ChangeState::Open;
        Ok(())
    }

    /// The integration commit advanced the target: every `Accepted` task
    /// becomes `Integrated` and the change `Integrated { commit }`. Returns
    /// the integrated tasks.
    pub fn finish_integration(
        &mut self,
        commit: &str,
        now: &Timestamp,
    ) -> Result<Vec<String>, ScheduleError> {
        if self.change.state != ChangeState::Integrating {
            return Err(self.change_error("finish integration"));
        }
        let now = utc(now)?;
        let mut integrated = Vec::new();
        for i in 0..self.tasks.len() {
            if self.tasks[i].state == TaskState::Accepted {
                self.transition(
                    i,
                    TaskState::Integrated,
                    &now,
                    None,
                    format!("integrated: {commit}"),
                );
                integrated.push(self.tasks[i].id.name.clone());
            }
        }
        self.change.state = ChangeState::Integrated {
            commit: commit.to_string(),
        };
        Ok(integrated)
    }

    /// Integration failed: the change becomes `Failed { reason }`, and each
    /// task of `stale` (integration retries exhausted with its candidate
    /// stale) goes `Accepted -> Failed`. Every named task must be
    /// `Accepted`, else nothing changes.
    pub fn fail_integration(
        &mut self,
        reason: &str,
        stale: &[&str],
        now: &Timestamp,
    ) -> Result<(), ScheduleError> {
        if self.change.state != ChangeState::Integrating {
            return Err(self.change_error("fail integration"));
        }
        let now = utc(now)?;
        let mut indices = Vec::with_capacity(stale.len());
        for name in stale {
            let i = self.index(name)?;
            self.expect_state(i, &[TaskState::Accepted], "fail integration")?;
            indices.push(i);
        }
        for i in indices {
            self.transition(
                i,
                TaskState::Failed,
                &now,
                None,
                format!("integration_failed: {reason}"),
            );
        }
        self.change.state = ChangeState::Failed {
            reason: reason.to_string(),
        };
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn index(&self, name: &str) -> Result<usize, ScheduleError> {
        self.tasks
            .iter()
            .position(|t| t.id.name == name)
            .ok_or_else(|| ScheduleError::UnknownTask {
                change: self.change.id.to_string(),
                task: name.to_string(),
            })
    }

    fn change_error(&self, action: &'static str) -> ScheduleError {
        ScheduleError::InvalidChangeState {
            change: self.change.id.to_string(),
            state: self.change.state.as_str().to_string(),
            action,
        }
    }

    fn ensure_open(&self, action: &'static str) -> Result<(), ScheduleError> {
        if self.change.state.is_terminal() {
            Err(self.change_error(action))
        } else {
            Ok(())
        }
    }

    fn state_error(&self, i: usize, action: &'static str) -> ScheduleError {
        ScheduleError::InvalidTaskState {
            task: self.tasks[i].id.name.clone(),
            state: self.tasks[i].state,
            action,
        }
    }

    fn expect_state(
        &self,
        i: usize,
        expected: &[TaskState],
        action: &'static str,
    ) -> Result<(), ScheduleError> {
        if expected.contains(&self.tasks[i].state) {
            Ok(())
        } else {
            Err(self.state_error(i, action))
        }
    }

    /// The predecessors of `task` that are neither `Accepted` nor `Integrated`.
    fn pending(&self, task: &TaskRecord) -> Vec<String> {
        task.spec
            .after
            .iter()
            .filter(|p| {
                !self
                    .task(p)
                    .is_some_and(|t| matches!(t.state, TaskState::Accepted | TaskState::Integrated))
            })
            .cloned()
            .collect()
    }

    /// How `baseline.predecessors` differs from the current candidates of
    /// `task`'s predecessors: the candidates it lacks, then those it layers
    /// beyond them (another task's output, a replaced candidate, or a
    /// repeat), both in order.
    fn predecessor_mismatch(
        &self,
        task: &TaskRecord,
        baseline: &Baseline,
    ) -> (Vec<String>, Vec<String>) {
        let mut missing = Vec::new();
        let mut current: Vec<&CandidateId> = Vec::new();
        let predecessors: BTreeSet<&str> = task.spec.after.iter().map(String::as_str).collect();
        for p in predecessors {
            match self.task(p).and_then(|t| t.candidate.as_ref()) {
                Some(candidate) => current.push(candidate),
                None => missing.push(format!("{p} (no candidate)")),
            }
        }
        let mut layered: BTreeSet<&CandidateId> = BTreeSet::new();
        let mut unexpected = Vec::new();
        for candidate in &baseline.predecessors {
            if !current.contains(&candidate) || !layered.insert(candidate) {
                unexpected.push(candidate.to_string());
            }
        }
        missing.extend(
            current
                .into_iter()
                .filter(|c| !layered.contains(c))
                .map(|c| c.to_string()),
        );
        (missing, unexpected)
    }

    /// `now + lease_seconds`.
    fn expiry(&self, now: &Utc) -> Result<Timestamp, ScheduleError> {
        now.0
            .add_seconds(self.lease_seconds())
            .ok_or_else(|| ScheduleError::InvalidTimestamp {
                value: now.0 .0.clone(),
            })
    }

    /// The fencing check without expiry: the task is `Running` and `token`
    /// (and `attempt`, when given) are those of its current lease.
    fn check_token(
        &self,
        i: usize,
        token: FencingToken,
        attempt: Option<u32>,
    ) -> Result<(), ScheduleError> {
        let task = &self.tasks[i];
        let stale = |reason: String| ScheduleError::StaleSubmission {
            task: task.id.name.clone(),
            reason,
        };
        let Some(lease) = &task.lease else {
            return Err(stale(format!(
                "fencing token {token} was never issued for this task (it is {})",
                task.state
            )));
        };
        if token < lease.token {
            return Err(stale(format!(
                "fencing token {token} is lower than the current token {}",
                lease.token
            )));
        }
        if token != lease.token {
            return Err(stale(format!(
                "fencing token {token} was never issued for this task (the current token is {})",
                lease.token
            )));
        }
        if task.state != TaskState::Running {
            return Err(stale(format!(
                "the assignment with fencing token {token} has ended (the task is {})",
                task.state
            )));
        }
        if let Some(attempt) = attempt {
            if attempt != lease.attempt {
                return Err(stale(format!(
                    "attempt {attempt} is not the current attempt {}",
                    lease.attempt
                )));
            }
        }
        Ok(())
    }

    /// [`ChangeRun::check_token`], and the lease has not expired at `now`.
    fn check_assignment(
        &self,
        i: usize,
        token: FencingToken,
        attempt: Option<u32>,
        now: &Utc,
    ) -> Result<(), ScheduleError> {
        self.check_token(i, token, attempt)?;
        let task = &self.tasks[i];
        match &task.lease {
            Some(lease) if lease.is_expired(&now.0) => Err(ScheduleError::StaleSubmission {
                task: task.id.name.clone(),
                reason: format!("the lease expired at {}", lease.expires_at),
            }),
            _ => Ok(()),
        }
    }

    /// Record `from -> to` at `now`, the one place a transition is made.
    fn transition(
        &mut self,
        i: usize,
        to: TaskState,
        now: &Utc,
        token: Option<FencingToken>,
        reason: String,
    ) {
        let task = &mut self.tasks[i];
        task.history.push(Transition {
            from: task.state,
            to,
            at: now.0.clone(),
            token,
            reason,
        });
        task.state = to;
        if to != TaskState::Blocked {
            task.blocked = None;
        }
    }
}

/// An operation's `now`, checked to be RFC 3339 and spelled in UTC
/// (Section 10.1: `Timestamp` is "RFC 3339, UTC"): the only time a
/// transition or a lease records.
struct Utc(Timestamp);

/// `now` in UTC ([`Timestamp::to_utc`]), or
/// [`ScheduleError::InvalidTimestamp`].
fn utc(now: &Timestamp) -> Result<Utc, ScheduleError> {
    now.to_utc()
        .map(Utc)
        .ok_or_else(|| ScheduleError::InvalidTimestamp {
            value: now.0.clone(),
        })
}

/// A cycle of `after` edges, as the task names along it with the first
/// repeated at the end (`a -> b -> a`), or `None`. Tasks are visited in
/// order, so the cycle reported is deterministic.
fn find_cycle(specs: &[&TaskSpec]) -> Option<Vec<String>> {
    let index: BTreeMap<&str, usize> = specs
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i))
        .collect();
    // 0: unvisited, 1: on the current path, 2: done.
    let mut marks = vec![0u8; specs.len()];
    let mut path = Vec::new();
    fn visit(
        i: usize,
        specs: &[&TaskSpec],
        index: &BTreeMap<&str, usize>,
        marks: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Option<Vec<String>> {
        marks[i] = 1;
        path.push(i);
        for predecessor in &specs[i].after {
            let Some(&j) = index.get(predecessor.as_str()) else {
                continue;
            };
            if marks[j] == 1 {
                let start = path.iter().position(|&k| k == j).unwrap_or(0);
                let mut cycle: Vec<String> = path[start..]
                    .iter()
                    .map(|&k| specs[k].name.clone())
                    .collect();
                cycle.push(specs[j].name.clone());
                return Some(cycle);
            }
            if marks[j] == 0 {
                if let Some(cycle) = visit(j, specs, index, marks, path) {
                    return Some(cycle);
                }
            }
        }
        path.pop();
        marks[i] = 2;
        None
    }
    (0..specs.len()).find_map(|i| {
        if marks[i] == 0 {
            visit(i, specs, &index, &mut marks, &mut path)
        } else {
            None
        }
    })
}

/// The first pair of unordered tasks, in declaration order, whose write
/// sets share a facet or whose `add` namespaces overlap, as
/// [`ScheduleError::OverlappingWrites`] (Section 7.2's rule; see
/// [`ChangeRun::open`]). The `after` graph must be acyclic.
fn find_overlap(tasks: &[(TaskSpec, Grant)]) -> Option<ScheduleError> {
    let specs: Vec<&TaskSpec> = tasks.iter().map(|(spec, _)| spec).collect();
    let index: BTreeMap<&str, usize> = specs
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i))
        .collect();
    let edges: Vec<Vec<usize>> = specs
        .iter()
        .map(|s| s.after.iter().filter_map(|p| index.get(p.as_str()).copied()).collect())
        .collect();
    let reach = crate::reach::Reach::new(&edges);
    let writes: Vec<BTreeSet<(StableId, Facet)>> = tasks
        .iter()
        .map(|(spec, grant)| write_set(spec, grant))
        .collect();
    let adds: Vec<BTreeSet<Vec<String>>> = tasks
        .iter()
        .map(|(spec, grant)| add_namespaces(spec, grant))
        .collect();
    for (i, first) in specs.iter().enumerate() {
        for (j, second) in specs.iter().enumerate().skip(i + 1) {
            if reach.reaches(i, j) || reach.reaches(j, i) {
                continue;
            }
            let overlap = if let Some((id, facet)) = writes[i].intersection(&writes[j]).next() {
                format!("write `{id}.{facet}`")
            } else if let Some((a, b)) = adds[i]
                .iter()
                .flat_map(|a| adds[j].iter().map(move |b| (a, b)))
                .find(|(a, b)| namespace_covers(a, b) || namespace_covers(b, a))
            {
                format!(
                    "add to the overlapping namespaces `{}` and `{}`",
                    namespace_text(a),
                    namespace_text(b)
                )
            } else {
                continue;
            };
            return Some(ScheduleError::OverlappingWrites {
                first: first.name.clone(),
                second: second.name.clone(),
                overlap,
            });
        }
    }
    None
}

/// A task's write set at facet level for the overlap rule: every facet its
/// `write` selectors request (`.definition` as every facet but `identity`,
/// which no task writes, Section 10.3) and every facet its grant writes.
fn write_set(spec: &TaskSpec, grant: &Grant) -> BTreeSet<(StableId, Facet)> {
    spec.write
        .iter()
        .flat_map(|selector| &selector.targets)
        .flat_map(|(id, selector)| {
            selector
                .facets()
                .into_iter()
                .map(move |facet| (id.clone(), facet))
        })
        .chain(grant.write.iter().cloned())
        .filter(|(_, facet)| *facet != Facet::Identity)
        .collect()
}

/// A task's `add` namespaces (module-path prefixes, T43): those it
/// requests and those it is granted.
fn add_namespaces(spec: &TaskSpec, grant: &Grant) -> BTreeSet<Vec<String>> {
    spec.add
        .iter()
        .map(|namespace| namespace.module_prefix.clone())
        .chain(grant.add.iter().cloned())
        .collect()
}

/// `tests.pagination.*`, the spelling of a namespace wildcard.
fn namespace_text(prefix: &[String]) -> String {
    if prefix.is_empty() {
        "*".to_string()
    } else {
        format!("{}.*", prefix.join("."))
    }
}
