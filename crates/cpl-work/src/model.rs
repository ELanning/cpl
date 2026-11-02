//! The work model (`spec/toolchain.md` 1.9, `cpl_work::model`) and the
//! coordinator records of Section 10.1.
//!
//! The first half is what `cpl work check` produces from a `.work` file
//! (Section 7.2): [`WorkModel`], [`ChangeSpec`], [`TaskSpec`], resolved
//! selectors and namespaces, [`FileSelector`] (T42), and
//! [`RequirementSpec`], built by [`crate::validate::validate_work_file`],
//! which resolves the selectors against the checked program.
//!
//! The second half is the Milestone 6 coordinator state of Section 10.1:
//! identifiers ([`ChangeRunId`], [`TaskId`], [`CandidateId`],
//! [`FencingToken`], [`Timestamp`]), the actor and its [`Authority`], the
//! [`ChangeRecord`] and [`TaskRecord`] persisted under
//! `<project root>/.cpl/work/<change run id>.json` (T33), [`Lease`],
//! [`Baseline`] (Section 5.5), and [`Candidate`] with its patch summaries
//! and review records. Every record is `Serialize` and `Deserialize`; the
//! state machine over them is [`crate::scheduler::ChangeRun`].

use crate::scheduler::{ExecutionBoundary, Grant, RunPolicy, TaskState};
use crate::time;
use cpl_diag::json::{
    DiffStatement, Digest, DuplicateId, FacetHash, Revision, SymbolVersion, ToolchainInfo,
};
use cpl_diag::{Facet, FacetSelector, Span, StableId};
use cpl_index::{facet_hash, FacetEntry, FacetMap, IdKind};
use cpl_project::Project;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

// ===========================================================================
// 1.9: the work model of a `.work` file
// ===========================================================================

/// A validated `.work` file (Section 7.2): its changes in source order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkModel {
    pub changes: Vec<ChangeSpec>,
}

impl WorkModel {
    /// The change with this name (change names are unique across a project, D34).
    pub fn change(&self, name: &str) -> Option<&ChangeSpec> {
        self.changes.iter().find(|c| c.name == name)
    }
}

/// One `change` declaration (grammar 4.1) after validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSpec {
    pub name: String,
    pub span: Span,
    /// The text inside `base git("...")`, passed to git unchanged (D35).
    pub base: String,
    /// The text inside `target git("...")`; a branch (D35).
    pub target: String,
    /// The concurrency ceiling; default 1.
    pub workers: u32,
    /// In source order; the `after` edges form a DAG.
    pub tasks: Vec<TaskSpec>,
    /// The `integrate` requirements in source order.
    pub integrate: Vec<RequirementSpec>,
}

impl ChangeSpec {
    /// The task with this name.
    pub fn task(&self, name: &str) -> Option<&TaskSpec> {
        self.tasks.iter().find(|t| t.name == name)
    }
}

/// One `task` declaration (grammar 4.2) after validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub name: String,
    pub span: Span,
    /// The predecessors named by `after`, in source order.
    pub after: Vec<String>,
    /// The declaration and facet selectors of the `write` set.
    pub write: Vec<ResolvedSelector>,
    /// The `add` namespaces.
    pub add: Vec<ResolvedNamespace>,
    /// The file selectors found in the `write` set (Section 10.3, T42).
    pub write_files: Vec<FileSelector>,
    pub depend: Vec<ResolvedSelector>,
    pub preserve: Vec<ResolvedSelector>,
    pub goal: Option<String>,
}

/// A declaration or facet selector resolved to stable ids: one target for
/// `paginate.body`, one per declaration for a namespace wildcard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSelector {
    /// The selector as written (`paginate.body`).
    pub text: String,
    pub span: Span,
    pub targets: Vec<(StableId, FacetSelector)>,
}

/// A namespace wildcard `m.*` (T43): module `m` and every module below it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedNamespace {
    /// The selector as written (`tests.pagination.*`).
    pub text: String,
    pub span: Span,
    /// The module path before `.*` (`["tests", "pagination"]`).
    pub module_prefix: Vec<String>,
}

impl ResolvedNamespace {
    /// Whether the namespace covers `module` (T43).
    pub fn covers(&self, module: &[String]) -> bool {
        namespace_covers(&self.module_prefix, module)
    }
}

/// T43: a namespace `m` covers module `m` and every module whose path has
/// `m` as a proper prefix, segment by segment (`tests` covers
/// `tests.pagination`, never `testsuite`).
pub fn namespace_covers(namespace: &[String], module: &[String]) -> bool {
    module.starts_with(namespace)
}

/// A project file a task may be granted (Section 10.3, T42): `cpl.toml`,
/// `cpl.lock`, or a `work/<stem>.work` file, named in a `write` set by the
/// selectors `project.manifest`, `project.lock`, and `work.<stem>`.
///
/// Serialized as the selector spelling (`"project.manifest"`,
/// `"work.pagination"`), the form `[work.policy] files` uses.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileSelector {
    Manifest,
    Lock,
    Work(String),
}

impl FileSelector {
    /// `project.manifest`, `project.lock`, or `work.<stem>` with a
    /// non-empty stem without `/` (the rule `cpl.toml`'s `[work.policy]
    /// files` is validated with).
    pub fn parse(spelling: &str) -> Option<FileSelector> {
        match spelling {
            "project.manifest" => Some(FileSelector::Manifest),
            "project.lock" => Some(FileSelector::Lock),
            _ => spelling
                .strip_prefix("work.")
                .filter(|stem| !stem.is_empty() && !stem.contains('/'))
                .map(|stem| FileSelector::Work(stem.to_string())),
        }
    }

    /// The selector spelling.
    pub fn spelling(&self) -> String {
        match self {
            FileSelector::Manifest => "project.manifest".to_string(),
            FileSelector::Lock => "project.lock".to_string(),
            FileSelector::Work(stem) => format!("work.{stem}"),
        }
    }

    /// The project-relative file: `cpl.toml`, `cpl.lock`, `work/<stem>.work`.
    pub fn path(&self) -> String {
        match self {
            FileSelector::Manifest => "cpl.toml".to_string(),
            FileSelector::Lock => "cpl.lock".to_string(),
            FileSelector::Work(stem) => format!("work/{stem}.work"),
        }
    }

    /// The selector of a project-relative path, when one names it: a
    /// `.work` file directly under `work/` with a non-empty stem.
    pub fn for_path(path: &str) -> Option<FileSelector> {
        match path {
            "cpl.toml" => Some(FileSelector::Manifest),
            "cpl.lock" => Some(FileSelector::Lock),
            _ => path
                .strip_prefix("work/")
                .and_then(|name| name.strip_suffix(".work"))
                .filter(|stem| !stem.is_empty() && !stem.contains('/'))
                .map(|stem| FileSelector::Work(stem.to_string())),
        }
    }
}

impl fmt::Display for FileSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.spelling())
    }
}

impl Serialize for FileSelector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.spelling())
    }
}

impl<'de> Deserialize<'de> for FileSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let spelling = String::deserialize(deserializer)?;
        FileSelector::parse(&spelling).ok_or_else(|| {
            de::Error::custom(format!(
                "`{spelling}` is not a file selector (project.manifest, project.lock, or work.<stem>)"
            ))
        })
    }
}

/// One `integrate` requirement (grammar 4.3, D36).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementSpec {
    Typecheck,
    NoHoles,
    NoUnresolvedAssumptions,
    Prove(ResolvedSelector),
    Test(ResolvedNamespaceOrSelector),
    /// `review(task)`: a task of the same change.
    Review(String),
}

/// The argument of `require test(...)`: a namespace selects the tests in
/// it (T43), a declaration selector the tests of that declaration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedNamespaceOrSelector {
    Namespace(ResolvedNamespace),
    Selector(ResolvedSelector),
}

// ===========================================================================
// 10.1: identifiers
// ===========================================================================

/// RFC 4648 base32, lowercase, no padding (the alphabet of generated `@id`s).
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// A change run: `chg_` and 16 base32 characters, issued when a change is
/// opened. Validated on construction and deserialization.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ChangeRunId(String);

impl ChangeRunId {
    pub const PREFIX: &'static str = "chg_";

    /// The id of 80 random bits, encoded like a generated `@id` (Section
    /// 4.1): 16 base32 characters, most significant bit first. The caller
    /// draws the bytes (this crate has no random source, Section 1.3).
    pub fn from_random_bytes(bytes: [u8; 10]) -> ChangeRunId {
        let mut bits = bytes
            .iter()
            .fold(0u128, |acc, &b| (acc << 8) | u128::from(b));
        let mut encoded = [0u8; 16];
        for slot in encoded.iter_mut().rev() {
            *slot = BASE32[(bits & 31) as usize];
            bits >>= 5;
        }
        let mut text = String::from(Self::PREFIX);
        text.extend(encoded.iter().map(|&c| char::from(c)));
        ChangeRunId(text)
    }

    /// `^chg_[a-z2-7]{16}$`.
    pub fn parse(text: &str) -> Result<ChangeRunId, String> {
        let valid = text
            .strip_prefix(Self::PREFIX)
            .is_some_and(|rest| rest.len() == 16 && rest.bytes().all(|c| BASE32.contains(&c)));
        if valid {
            Ok(ChangeRunId(text.to_string()))
        } else {
            Err(format!(
                "`{text}` is not a change run id (`chg_` and 16 base32 characters)"
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChangeRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ChangeRunId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        ChangeRunId::parse(&text).map_err(de::Error::custom)
    }
}

/// A task of a change run. Displayed as `<change run id>/<task name>`
/// (`chg_.../implementation`), the form [`CandidateId::derive`] hashes and
/// a ref path can hold (`refs/cpl/snapshots/<task id>/<attempt>`, 10.4).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskId {
    pub change: ChangeRunId,
    pub name: String,
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.change, self.name)
    }
}

/// A candidate: `cand_` and the first 32 hex characters of blake3 over the
/// task id, the attempt, the fencing token, and the tree oid (Section 10.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CandidateId(String);

impl CandidateId {
    pub const PREFIX: &'static str = "cand_";

    /// The id of a submission. Section 10.1 names the hashed fields but
    /// not their encoding; this reading hashes the UTF-8 text
    /// `<task id>\n<attempt>\n<token>\n<tree oid>` (decimal numbers, the
    /// task id as displayed), so equal submissions always get equal ids
    /// and resubmission is idempotent (10.2).
    pub fn derive(task: &TaskId, attempt: u32, token: FencingToken, tree: &str) -> CandidateId {
        let text = format!("{task}\n{attempt}\n{}\n{tree}", token.0);
        CandidateId(format!("{}{}", Self::PREFIX, facet_hash(&text).as_str()))
    }

    /// `^cand_[0-9a-f]{32}$`.
    pub fn parse(text: &str) -> Result<CandidateId, String> {
        let valid = text.strip_prefix(Self::PREFIX).is_some_and(|rest| {
            rest.len() == 32
                && rest
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        });
        if valid {
            Ok(CandidateId(text.to_string()))
        } else {
            Err(format!(
                "`{text}` is not a candidate id (`cand_` and 32 lowercase hex digits)"
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CandidateId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        CandidateId::parse(&text).map_err(de::Error::custom)
    }
}

/// A scheduler-issued fencing token, strictly increasing per change run
/// ([`ChangeRecord::next_token`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FencingToken(pub u64);

impl fmt::Display for FencingToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An RFC 3339 timestamp; the coordinator writes UTC
/// (`2026-10-28T12:30:05Z`). Comparisons go through the instant, never the
/// string, so `...+02:00` and `...Z` spellings of one instant are equal in
/// time ([`Timestamp::cmp_instant`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub String);

impl Timestamp {
    /// The current time in whole seconds, UTC.
    pub fn now() -> Timestamp {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Timestamp::from_unix_seconds(i64::try_from(seconds).unwrap_or(i64::MAX))
            .unwrap_or_else(|| Timestamp("9999-12-31T23:59:59Z".to_string()))
    }

    /// The UTC timestamp of a Unix time; `None` outside the years 0000-9999.
    pub fn from_unix_seconds(seconds: i64) -> Option<Timestamp> {
        time::format(time::Instant { seconds, nanos: 0 }).map(Timestamp)
    }

    /// Seconds since the Unix epoch (the fraction dropped toward the past);
    /// `None` when this is not an RFC 3339 timestamp.
    pub fn unix_seconds(&self) -> Option<i64> {
        time::parse(&self.0).map(|instant| instant.seconds)
    }

    /// Whether this is an RFC 3339 timestamp.
    pub fn is_valid(&self) -> bool {
        time::parse(&self.0).is_some()
    }

    /// The same instant spelled in UTC, keeping the fraction
    /// (`2026-10-28T14:00:00+02:00` is `2026-10-28T12:00:00Z`): the form
    /// the coordinator records (Section 10.1, "RFC 3339, UTC"). `None` when
    /// this is not an RFC 3339 timestamp or the instant leaves the years
    /// 0000-9999.
    pub fn to_utc(&self) -> Option<Timestamp> {
        time::parse(&self.0).and_then(time::format).map(Timestamp)
    }

    /// This instant plus `seconds`, in UTC, keeping the fraction; `None`
    /// when this is not an RFC 3339 timestamp or the result leaves the
    /// years 0000-9999.
    pub fn add_seconds(&self, seconds: u64) -> Option<Timestamp> {
        let instant = time::parse(&self.0)?;
        let seconds = instant.seconds.checked_add(i64::try_from(seconds).ok()?)?;
        time::format(time::Instant {
            seconds,
            nanos: instant.nanos,
        })
        .map(Timestamp)
    }

    /// The order of the two instants; `None` when either is not an RFC
    /// 3339 timestamp.
    pub fn cmp_instant(&self, other: &Timestamp) -> Option<Ordering> {
        Some(time::parse(&self.0)?.cmp(&time::parse(&other.0)?))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ===========================================================================
// 10.1: actors and frozen inputs
// ===========================================================================

/// The host-supplied identity and authority of the actor who opens a change
/// or integrates it; never read from a `.work` file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub authority: Authority,
}

/// What an actor may authorize (Section 10.3): every grant of a change it
/// opens is bounded by it. The derived default is unlimited `write` and
/// `add` (within policy), no files, the strictest boundary, and no
/// integration authority.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authority {
    /// `None`: unlimited (within policy).
    pub write: Option<BTreeSet<(StableId, Facet)>>,
    /// `None`: unlimited (within policy).
    pub add: Option<Vec<Vec<String>>>,
    /// The project files the actor may authorize (Section 10.3).
    pub files: BTreeSet<FileSelector>,
    /// Section 10.8.
    pub boundary: ExecutionBoundary,
    /// Design line 365: integration authority is not every worker's.
    pub may_integrate: bool,
}

/// What a change run froze when it opened (design line 351): digests of
/// `cpl.toml`, `cpl.lock`, and the `.work` file, the toolchain, and the
/// run's policy, which every check of the run reads instead of the
/// assembled tree's manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenInputs {
    pub manifest: Digest,
    pub lock: Option<Digest>,
    pub work_file: Digest,
    pub toolchain: ToolchainInfo,
    /// `[verification]`, `[test]`, and `[work]` as they were when the change opened.
    pub policy: RunPolicy,
}

impl FrozenInputs {
    /// The inputs of a change opened on `project` (the base snapshot) from
    /// the `.work` file whose bytes have the digest `work_file`.
    pub fn freeze(project: &Project, work_file: Digest) -> FrozenInputs {
        FrozenInputs {
            manifest: project.manifest_digest.clone(),
            lock: project.lock_digest.clone(),
            work_file,
            toolchain: ToolchainInfo::current(),
            policy: RunPolicy::from_manifest(&project.manifest),
        }
    }
}

// ===========================================================================
// 10.1: change and task records
// ===========================================================================

/// One opened change (a change run).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub id: ChangeRunId,
    /// The `change` declaration's name (`pagination_update`).
    pub name: String,
    /// The project-relative `.work` file.
    pub work_file: String,
    pub work_digest: Digest,
    /// `base git("main")` resolved when the change opened; frozen.
    pub base_commit: String,
    /// The branch to advance (`refs/heads/main`).
    pub target_ref: String,
    pub workers: u32,
    /// The submitter; its authority bounds every grant (Section 10.3).
    pub opened_by: Actor,
    pub frozen_inputs: FrozenInputs,
    pub state: ChangeState,
    /// The run's tasks in source order.
    pub tasks: Vec<TaskId>,
    /// The next fencing token to issue; tokens strictly increase per run.
    pub next_token: u64,
    pub opened_at: Timestamp,
}

/// The state of a change run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    Open,
    Integrating,
    Integrated { commit: String },
    Failed { reason: String },
    Cancelled,
}

impl ChangeState {
    /// `Integrated`, `Failed`, and `Cancelled` end the run.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ChangeState::Integrated { .. } | ChangeState::Failed { .. } | ChangeState::Cancelled
        )
    }

    /// `open`, `integrating`, `integrated`, `failed`, `cancelled`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ChangeState::Open => "open",
            ChangeState::Integrating => "integrating",
            ChangeState::Integrated { .. } => "integrated",
            ChangeState::Failed { .. } => "failed",
            ChangeState::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for ChangeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One task of a change run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: TaskId,
    /// Section 1.9.
    pub spec: TaskSpec,
    /// Section 10.3; computed when the change opened, never changed.
    pub grant: Grant,
    pub state: TaskState,
    /// Incremented on every (re)assignment; 0 before the first.
    pub attempt: u32,
    /// The current or, once it ended, the last lease: its worktree is kept
    /// until the task is reassigned (Section 10.2).
    pub lease: Option<Lease>,
    /// Section 5.5; set when the task becomes `Ready`.
    pub baseline: Option<Baseline>,
    /// The latest accepted or submitted candidate.
    pub candidate: Option<CandidateId>,
    /// The reason while `Blocked`.
    pub blocked: Option<String>,
    pub history: Vec<Transition>,
}

/// One state change of a task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub from: TaskState,
    pub to: TaskState,
    pub at: Timestamp,
    /// The fencing token of the assignment the transition concerns.
    pub token: Option<FencingToken>,
    pub reason: String,
}

/// An assignment of a task to a worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub task: TaskId,
    pub attempt: u32,
    pub token: FencingToken,
    /// Opaque worker identity.
    pub holder: String,
    /// The isolated checkout of the baseline.
    pub worktree: PathBuf,
    /// The boundary the `Sandbox` reported as enforced for this worker (Section 10.8).
    pub boundary: ExecutionBoundary,
    pub issued_at: Timestamp,
    pub expires_at: Timestamp,
}

impl Lease {
    /// Whether the lease has expired at `now`: `expires_at` is not after
    /// `now`. A timestamp that is not RFC 3339 counts as expired, so that a
    /// corrupt record fails closed (it can never accept a submission).
    pub fn is_expired(&self, now: &Timestamp) -> bool {
        !matches!(self.expires_at.cmp_instant(now), Some(Ordering::Greater))
    }
}

/// A task's baseline (Section 5.5): the facet map of the snapshot the task
/// was given, with the candidates layered onto the base commit for it
/// (Section 10.4; empty for initial tasks).
///
/// JSON: the object of Section 5.5, `{ revision, snapshot_commit,
/// predecessors, facets }`, where `facets` maps each stable id to its
/// [`SymbolVersion`] without `id` (the key) followed by the rest of its
/// [`FacetEntry`] (`kind`, `module`, `name`, `file`, and `effects` when
/// non-empty), which enforcement needs after a coordinator restart (the
/// semantic diff reads effect sets and modules, T31); a `duplicates` list
/// `[{ id, files }]` follows when the map has any. `facets.revision` is
/// `revision`: Section 10.4 builds the map from the snapshot the revision
/// names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Baseline {
    pub revision: Revision,
    pub snapshot_commit: String,
    pub predecessors: Vec<CandidateId>,
    pub facets: FacetMap,
}

/// The JSON form of [`Baseline`].
#[derive(Serialize, Deserialize)]
struct BaselineJson {
    revision: Revision,
    snapshot_commit: String,
    predecessors: Vec<CandidateId>,
    facets: BTreeMap<StableId, FacetEntryJson>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    duplicates: Vec<DuplicateId>,
}

/// The JSON form of one [`FacetEntry`] inside [`BaselineJson`].
#[derive(Serialize, Deserialize)]
struct FacetEntryJson {
    path: String,
    provisional: bool,
    facets: BTreeMap<Facet, FacetHash>,
    definition: FacetHash,
    kind: String,
    module: String,
    name: String,
    file: String,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    effects: BTreeSet<String>,
}

impl Serialize for Baseline {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let facets = self
            .facets
            .entries
            .iter()
            .map(|(id, entry)| {
                let json = FacetEntryJson {
                    path: entry.version.path.clone(),
                    provisional: entry.version.provisional,
                    facets: entry.version.facets.clone(),
                    definition: entry.version.definition.clone(),
                    kind: entry.kind.prefix().to_string(),
                    module: entry.module.clone(),
                    name: entry.name.clone(),
                    file: entry.file.clone(),
                    effects: entry.effects.clone(),
                };
                (id.clone(), json)
            })
            .collect();
        BaselineJson {
            revision: self.revision.clone(),
            snapshot_commit: self.snapshot_commit.clone(),
            predecessors: self.predecessors.clone(),
            facets,
            duplicates: self
                .facets
                .duplicates
                .iter()
                .map(|(id, files)| DuplicateId {
                    id: id.clone(),
                    files: files.clone(),
                })
                .collect(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Baseline {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = BaselineJson::deserialize(deserializer)?;
        let mut entries = BTreeMap::new();
        for (id, entry) in json.facets {
            let kind = IdKind::from_prefix(&entry.kind).ok_or_else(|| {
                de::Error::custom(format!(
                    "`{}` is not a declaration kind (fn, ty, cap, test, prop)",
                    entry.kind
                ))
            })?;
            if entry.provisional != id.is_provisional() {
                return Err(de::Error::custom(format!(
                    "`provisional` of `{id}` is {}, but the id is {}",
                    entry.provisional,
                    if id.is_provisional() {
                        "a qualified name"
                    } else {
                        "an @id value"
                    }
                )));
            }
            let version = SymbolVersion {
                id: id.clone(),
                path: entry.path,
                provisional: entry.provisional,
                facets: entry.facets,
                definition: entry.definition,
            };
            entries.insert(
                id,
                FacetEntry {
                    version,
                    kind,
                    module: entry.module,
                    name: entry.name,
                    file: entry.file,
                    effects: entry.effects,
                },
            );
        }
        Ok(Baseline {
            facets: FacetMap {
                revision: json.revision.clone(),
                entries,
                duplicates: json
                    .duplicates
                    .into_iter()
                    .map(|d| (d.id, d.files))
                    .collect(),
            },
            revision: json.revision,
            snapshot_commit: json.snapshot_commit,
            predecessors: json.predecessors,
        })
    }
}

// ===========================================================================
// 10.1: candidates and reviews
// ===========================================================================

/// A submitted result of one assignment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: CandidateId,
    pub task: TaskId,
    pub attempt: u32,
    pub token: FencingToken,
    pub baseline: Baseline,
    /// The git tree oid of the candidate source tree.
    pub tree: String,
    /// `refs/cpl/candidates/<id>`, whose parent is `baseline.snapshot_commit`.
    pub snapshot_commit: String,
    /// The submitted `.patch` forms, resolved and consumed (Section 10.5).
    pub patches: Vec<PatchSummary>,
    /// Copied from the lease: what the worker that produced this tree ran under.
    pub boundary: ExecutionBoundary,
    /// `semantic_diff(baseline.facets, facet_map(tree))`.
    pub facet_diff: Vec<DiffStatement>,
    /// Every symbol the changed facets reference, at the candidate.
    pub dependency_versions: Vec<SymbolVersion>,
    /// Receipts produced at task-level validation.
    pub evidence: Vec<Digest>,
    /// For review tasks.
    pub review: Option<ReviewRecord>,
    pub submitted_at: Timestamp,
}

/// One applied patch form: its target and the `body` hashes before
/// (`expected`, the baseline's) and after (`applied`, the patch body's).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchSummary {
    pub target: (StableId, Facet),
    pub expected: FacetHash,
    pub applied: FacetHash,
}

/// What a review task emits about the exact candidates it inspected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub reviewer_task: TaskId,
    pub reviewed: Vec<CandidateId>,
    pub verdict: Verdict,
    pub notes: String,
    pub token: FencingToken,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approve,
    Reject,
    NeedsChanges,
}
