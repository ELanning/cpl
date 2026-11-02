//! The work coordinator of Milestone 6 (`spec/toolchain.md` Section 10;
//! `cpl_work::scheduler` of 1.9): task states, grants, the frozen run
//! policy, the `CheckRunner` and `Sandbox` adapters the host implements
//! (T2, T40), and the pure state machine over the records of 10.1
//! ([`ChangeRun`]) with its persistence under `.cpl/work/` ([`Coordinator`]).
//!
//! The records themselves ([`Lease`], [`TaskId`], [`FencingToken`],
//! [`Timestamp`], [`Actor`], ...) are defined once, in
//! [`crate::model`] (Section 10.1), and re-exported here.

mod run;
mod store;

pub use crate::model::{
    Actor, Authority, Baseline, Candidate, CandidateId, ChangeRecord, ChangeRunId, ChangeState,
    FencingToken, FrozenInputs, Lease, PatchSummary, ReviewRecord, TaskId, TaskRecord, Timestamp,
    Transition, Verdict,
};
pub use cpl_diag::json::{ExecutionBoundary, FsAccess, NetworkAccess, ProcessAccess};
pub use run::{ChangeRun, ScheduleError, Submission, SubmitOutcome};
pub use store::{Coordinator, CoordinatorError};

use crate::model::{namespace_covers, FileSelector};
use cpl_diag::json::{
    DiagnosticJson, TestReceipt, TestReport, TestSelectorSpec, VerificationReceipt,
};
use cpl_diag::{Facet, StableId};
use cpl_project::{Manifest, TestSection, VerificationSection, VirtualTree, WorkSection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// The state of a task (Section 10.2; design "Swarm authority and lifecycle").
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Dependencies are available and scope can be assigned.
    Ready,
    /// A worker holds the current assignment lease.
    Running,
    /// A candidate and provenance are available for validation.
    Submitted,
    /// Dependencies, a conflict, or a required decision prevent progress.
    Blocked,
    /// Required task-level validation passed for the candidate.
    Accepted,
    /// An accepted candidate is included in a checked target commit.
    Integrated,
    /// The attempt ended without integration.
    Failed,
    /// Cancelled by an operator.
    Cancelled,
}

impl TaskState {
    /// `Integrated`, `Failed`, and `Cancelled`: no transition leaves them.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskState::Integrated | TaskState::Failed | TaskState::Cancelled
        )
    }

    /// `ready`, `running`, ..., `cancelled`.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Ready => "ready",
            TaskState::Running => "running",
            TaskState::Submitted => "submitted",
            TaskState::Blocked => "blocked",
            TaskState::Accepted => "accepted",
            TaskState::Integrated => "integrated",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a task may change (Section 10.3), computed when the change opens
/// and never changed for the run: the intersection of the task's requests,
/// `[work.policy]`, and the submitter's authority, minus the fixed
/// exclusions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Writable facets of existing declarations (`.definition` expanded to
    /// every applicable facet except `identity`).
    pub write: BTreeSet<(StableId, Facet)>,
    /// Module-path prefixes under which new declarations may be created (T43).
    pub add: Vec<Vec<String>>,
    /// Facets whose baseline hash the candidate must keep.
    pub preserve: BTreeSet<(StableId, Facet)>,
    /// The project files the task may change (T42).
    pub files: BTreeSet<FileSelector>,
    /// The host boundary the task runs under (Section 10.8).
    pub boundary: ExecutionBoundary,
}

impl Grant {
    /// Whether `(id, facet)` is writable. `identity` never is: it is a
    /// fixed exclusion of every grant (Section 10.3).
    pub fn may_write(&self, id: &StableId, facet: Facet) -> bool {
        facet != Facet::Identity && self.write.contains(&(id.clone(), facet))
    }

    /// Whether some `add` namespace covers `module` (T43).
    pub fn may_add(&self, module: &[String]) -> bool {
        self.add
            .iter()
            .any(|namespace| namespace_covers(namespace, module))
    }

    /// Whether the task may change the file `selector` names.
    pub fn may_change_file(&self, selector: &FileSelector) -> bool {
        self.files.contains(selector)
    }
}

/// The `[verification]`, `[test]`, and `[work]` sections frozen when a
/// change is opened (Section 10.1); every check of the run reads them
/// instead of the assembled tree's manifest (design line 351).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPolicy {
    pub verification: VerificationSection,
    pub test: TestSection,
    pub work: WorkSection,
}

impl RunPolicy {
    /// The policy of a manifest, frozen.
    pub fn from_manifest(manifest: &Manifest) -> RunPolicy {
        RunPolicy {
            verification: manifest.verification.clone(),
            test: manifest.test.clone(),
            work: manifest.work.clone(),
        }
    }
}

/// The checks the scheduler runs (T2): implemented by `cpl-cli` over
/// `cpl-sema`, `cpl-eval`, and the verifier, and by scripted fakes in tests.
pub trait CheckRunner {
    fn typecheck(&mut self, tree: &VirtualTree, policy: &RunPolicy, release: bool) -> CheckOutcome;
    fn no_holes(&mut self, tree: &VirtualTree, policy: &RunPolicy) -> CheckOutcome;
    fn test(
        &mut self,
        tree: &VirtualTree,
        policy: &RunPolicy,
        selector: &TestSelectorSpec,
        seed: u64,
    ) -> TestOutcome;
    fn prove(&mut self, tree: &VirtualTree, policy: &RunPolicy, target: &StableId) -> ProveOutcome;
}

/// The result of `typecheck` or `no_holes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckOutcome {
    pub ok: bool,
    pub diagnostics: Vec<DiagnosticJson>,
    pub receipt: Option<serde_json::Value>,
}

/// The result of a `test(...)` requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestOutcome {
    pub report: TestReport,
    pub receipt: TestReceipt,
}

/// The result of a `prove(...)` requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProveOutcome {
    pub receipt: VerificationReceipt,
}

/// The host's sandbox mechanism (Section 10.8, T40), implemented in
/// `cpl-cli`. A host with no mechanism reports `enforceable()` as
/// `host`/`any`/`any`.
pub trait Sandbox {
    /// The strictest boundary the host can enforce for a worker. A host
    /// that can confine a worker that far can confine it to any wider
    /// boundary, so a task's granted boundary is enforceable iff this one
    /// is within it, field by field (Section 10.8 "host": a host reporting
    /// `host`/`any`/`any` runs only tasks granted exactly that;
    /// [`ChangeRun::assign`]).
    fn enforceable(&self) -> ExecutionBoundary;
    /// Start the worker of `lease` under `boundary`; the boundary actually
    /// enforced (equal or stricter) is recorded in the lease.
    fn launch(
        &mut self,
        lease: &Lease,
        boundary: &ExecutionBoundary,
    ) -> Result<ExecutionBoundary, SandboxError>;
}

/// Why a sandbox could not launch a worker.
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
#[error("{message}")]
pub struct SandboxError {
    pub message: String,
}

/// `filesystem=<fs> process=<process> network=<network>`, the form of the
/// integration commit's `CPL-Boundary:` trailer (Section 10.6), with the
/// `[work.policy]` spellings of Section 11.1.
pub fn boundary_text(boundary: &ExecutionBoundary) -> String {
    let filesystem = match boundary.filesystem {
        FsAccess::Worktree => "worktree",
        FsAccess::WorktreeAndRepository => "worktree_and_repository",
        FsAccess::Host => "host",
    };
    let process = match boundary.process {
        ProcessAccess::None => "none",
        ProcessAccess::Toolchain => "toolchain",
        ProcessAccess::Any => "any",
    };
    let network = match boundary.network {
        NetworkAccess::None => "none",
        NetworkAccess::Adapters => "adapters",
        NetworkAccess::Any => "any",
    };
    format!("filesystem={filesystem} process={process} network={network}")
}
