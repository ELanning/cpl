//! `cpl_diag::json`: every JSON document the toolchain emits is a type of
//! this module or composed of them (`spec/toolchain.md` T3, 1.4.2,
//! Appendix C). Field order is the key order of the spec's examples;
//! optional keys are omitted when absent unless the spec's example shows
//! them as `null`; every JSON map is a `BTreeMap` (T4).
//!
//! A few vocabulary enums used by these documents are defined here because
//! `cpl-diag` is the only crate below all of their users, and are
//! re-exported by the crate the spec places them in: [`ProofStatus`]
//! (`cpl_sema::check`), [`BindingKind`] (`cpl_sema::hir`), [`Include`]
//! (`cpl_index::query`), and [`ExecutionBoundary`] with its three access
//! enums (`cpl_work::scheduler`, needed by `cpl_sema::project`'s
//! `[work.policy]` section).
//!
//! Milestone 2 adds the `cpl build` documents ([`BuildOutput`] with
//! [`CompilerInfo`], [`RuntimeInfo`], [`BuildExport`], [`BuildBinding`],
//! [`BuildArtifact`], [`BuildInput`], [`BuildLock`]) and the `--engine`
//! wrappers ([`EngineKind`], [`RunBinding`], [`EngineRunOutput`],
//! [`EngineTestOutput`]), Section 7.2; no Milestone 1 type changes, so
//! [`version::JSON_SCHEMA`] stays 1 (T73).

use crate::diag::{Category, Severity};
use crate::facet::{Facet, FacetSelector};
use crate::id::StableId;
use crate::version;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub use crate::source::Location;

/// A JSON object (a sorted map, T4).
pub type JsonMap = serde_json::Map<String, serde_json::Value>;

/// The JSON object a serializable payload produces, for flattening into a
/// diagnostic's [`crate::Diagnostic::data`] (a hole report, a test record).
/// Panics when `value` does not serialize to a JSON object.
pub fn to_data<T: Serialize + ?Sized>(value: &T) -> JsonMap {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::Object(map)) => map,
        Ok(other) => panic!("to_data: expected a JSON object, found {other}"),
        Err(e) => panic!("to_data: {e}"),
    }
}

/// `Option<Option<T>>` fields: absent is `None`, `null` is `Some(None)`.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ===========================================================================
// 1.4.2: the envelope and the shared documents
// ===========================================================================

/// Common envelope of every `--json` output (Section 7.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// `"check"`, `"test"`, `"id assign"`, ...
    pub command: String,
    pub toolchain: ToolchainInfo,
    /// `None` (`null`) when no project is involved (e.g. `fmt` on loose files).
    pub revision: Option<Revision>,
    /// `ok` | `unsuccessful` | `error` (exit code 0 | 1 | 2); never a proof
    /// or test status (T44).
    pub status: Status,
    /// In `Diagnostics::sorted` order (Section 7.1, T46).
    pub diagnostics: Vec<DiagnosticJson>,
    /// The command's own keys, flattened into the envelope.
    #[serde(flatten)]
    pub result: T,
}

/// The result of a command whose envelope carries no keys of its own (a
/// usage error, a load failure).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoOutput {}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolchainInfo {
    pub version: String,
    pub edition: String,
    pub json_schema: u32,
}

impl ToolchainInfo {
    /// This toolchain: [`version::TOOLCHAIN`], [`version::EDITION`], [`version::JSON_SCHEMA`].
    pub fn current() -> ToolchainInfo {
        ToolchainInfo {
            version: version::TOOLCHAIN.to_string(),
            edition: version::EDITION.to_string(),
            json_schema: version::JSON_SCHEMA,
        }
    }
}

/// The envelope's command outcome (T44).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Unsuccessful,
    Error,
}

impl Status {
    /// 0, 1, or 2 (Section 7.1).
    pub fn exit_code(self) -> u8 {
        match self {
            Status::Ok => 0,
            Status::Unsuccessful => 1,
            Status::Error => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Unsuccessful => "unsuccessful",
            Status::Error => "error",
        }
    }
}

/// Exactly the object of semantics S49. Optional fields are omitted when
/// absent, never `null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticJson {
    pub code: String,
    pub category: Category,
    pub severity: Severity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Location>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<StableId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<Facet>,
    /// `{ "message", "span"? }`; omitted when empty (S49).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteJson>,
    /// `{ "qualified_name", "span"? }`, innermost first; omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backtrace: Vec<BacktraceJson>,
    /// The code-specific payload, flattened (T5).
    #[serde(flatten)]
    pub data: JsonMap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteJson {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Location>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktraceJson {
    pub qualified_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Location>,
}

/// Section 6.3. `commit` is `None` (`null`) when the project is not in a git
/// repository or the repository has no commits.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Revision {
    pub commit: Option<String>,
    pub state: RevisionState,
    pub digest: Digest,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionState {
    Clean,
    Worktree,
}

/// blake3-256 of some bytes, 64 lowercase hex characters.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest(pub String);

impl Digest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// blake3-256 of a facet's canonical text, truncated to 32 lowercase hex
/// characters (Section 5.3).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FacetHash(pub String);

impl FacetHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FacetHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One symbol's facet versions, as embedded in inspect responses, receipts,
/// hole reports, and candidates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolVersion {
    pub id: StableId,
    /// The display symbol path (Section 4.4).
    pub path: String,
    pub provisional: bool,
    /// Only the facets applicable to the declaration kind, in canonical order.
    pub facets: BTreeMap<Facet, FacetHash>,
    /// The `.definition` hash (Section 5.4).
    pub definition: FacetHash,
}

// ===========================================================================
// Shared vocabularies (re-exported by the crates the spec names)
// ===========================================================================

/// Verification status (semantics S50; `cpl_sema::check::ProofStatus`).
/// `Conditional` is never produced in Milestone 1.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofStatus {
    Proved,
    Failed,
    Unknown,
    Conditional,
}

impl ProofStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProofStatus::Proved => "proved",
            ProofStatus::Failed => "failed",
            ProofStatus::Unknown => "unknown",
            ProofStatus::Conditional => "conditional",
        }
    }
}

/// How a binding visible at a hole was introduced (semantics S48;
/// `cpl_sema::hir::BindingKind`, Section 1.6). `SelfValue` serializes as
/// `"self"`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    Parameter,
    Using,
    Let,
    Var,
    Pattern,
    Forall,
    Parallel,
    For,
    Result,
    #[serde(rename = "self")]
    SelfValue,
    /// `context app = e`: the handle of the call stack's state machine.
    Context,
}

impl BindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BindingKind::Parameter => "parameter",
            BindingKind::Using => "using",
            BindingKind::Let => "let",
            BindingKind::Var => "var",
            BindingKind::Pattern => "pattern",
            BindingKind::Forall => "forall",
            BindingKind::Parallel => "parallel",
            BindingKind::For => "for",
            BindingKind::Result => "result",
            BindingKind::SelfValue => "self",
            BindingKind::Context => "context",
        }
    }
}

/// One item of an inspect include set (Section 6.2; `cpl_index::query::Include`).
/// The derived order is the order of the `include` key of [`InspectOutput`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Include {
    Signature,
    Contract,
    Effects,
    Implementation,
    Docs,
    Callers,
    Dependencies,
    Obligations,
    Tests,
    Changes,
    /// Implied by the default set, never written on the command line (Section 6.2).
    InterfaceDependencies,
}

impl Include {
    pub fn as_str(self) -> &'static str {
        match self {
            Include::Signature => "signature",
            Include::Contract => "contract",
            Include::Effects => "effects",
            Include::Implementation => "implementation",
            Include::Docs => "docs",
            Include::Callers => "callers",
            Include::Dependencies => "dependencies",
            Include::Obligations => "obligations",
            Include::Tests => "tests",
            Include::Changes => "changes",
            Include::InterfaceDependencies => "interface_dependencies",
        }
    }
}

/// The host boundary a task runs under (Section 10.8; `cpl_work::scheduler`).
/// The `[work.policy]` keys `filesystem`, `process`, `network` (Section 11.1).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExecutionBoundary {
    pub filesystem: FsAccess,
    pub process: ProcessAccess,
    pub network: NetworkAccess,
}

impl ExecutionBoundary {
    /// The per-field minimum (the stricter of each pair).
    pub fn min(&self, other: &Self) -> Self {
        ExecutionBoundary {
            filesystem: self.filesystem.min(other.filesystem),
            process: self.process.min(other.process),
            network: self.network.min(other.network),
        }
    }

    /// True when every field is at most as permissive as `limit`'s.
    pub fn within(&self, limit: &Self) -> bool {
        self.filesystem <= limit.filesystem
            && self.process <= limit.process
            && self.network <= limit.network
    }
}

/// Ordered strictest first: write only inside the task's worktree (default);
/// plus read access to the repository; unrestricted.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FsAccess {
    #[default]
    Worktree,
    WorktreeAndRepository,
    Host,
}

/// No child processes (default); only the `cpl` binary; unrestricted.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProcessAccess {
    #[default]
    None,
    Toolchain,
    Any,
}

/// No network (default); only the hosts the bound adapters declare; unrestricted.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccess {
    #[default]
    None,
    Adapters,
    Any,
}

/// [`SymbolBlock::relation`]: a reference kind in dependency lists
/// (Section 6.5, the HIR's `RefKind`) or `direct`/`transitive` in `tests`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Call,
    TypeUse,
    Ctor,
    FieldAccess,
    CapabilityOp,
    Check,
    Direct,
    Transitive,
}

// ===========================================================================
// 6.4: cpl inspect
// ===========================================================================

/// `InspectOutput` (Section 6.4), flattened into the envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectOutput {
    /// The effective include set, sorted.
    pub include: Vec<Include>,
    pub symbol: SymbolBlock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_dependencies: Option<Vec<SymbolBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<SymbolBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callers: Option<Vec<SymbolBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obligations: Option<Vec<Obligation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<Vec<SymbolBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<InspectChanges>,
}

/// The `changes` key of an inspect response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectChanges {
    pub since: Revision,
    pub statements: Vec<DiffStatement>,
}

/// A symbol block (Section 6.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolBlock {
    /// `id`, `path`, `provisional`, `facets`, `definition`.
    #[serde(flatten)]
    pub version: SymbolVersion,
    /// `"fn"` | `"ty"` | `"cap"` | `"test"` | `"prop"`.
    pub kind: String,
    pub module: String,
    pub public: bool,
    /// Std symbols use `std/<file>.cpl`.
    pub location: Location,
    /// fn only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_policy: Option<VerificationPolicyJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<String>>,
    /// Absent unless `docs` is included; then a string or `null`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_some"
    )]
    pub docs: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holes: Option<Vec<HoleReportJson>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<Relation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_sites: Option<Vec<Location>>,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VerificationPolicyJson {
    pub prove: bool,
}

// ===========================================================================
// 7.2: cpl check
// ===========================================================================

/// `CheckOutput` (Section 7.2), flattened into the envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckOutput {
    pub release: bool,
    pub summary: CheckSummary,
    /// The hole payloads without the diagnostic envelope fields.
    pub holes: Vec<HoleReportJson>,
    pub verification: Vec<VerificationStatusJson>,
}

/// `summary` of [`CheckOutput`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckSummary {
    pub errors: u32,
    pub warnings: u32,
    pub holes: u32,
    pub definitions: u32,
    pub modules: u32,
    pub files: u32,
}

/// The hole report of semantics S48 plus `dependencies.symbols` (Section
/// 7.2): the payload of the `hole` diagnostic, and an entry of
/// [`CheckOutput::holes`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoleReportJson {
    pub hole: String,
    pub expected_type: String,
    /// Innermost scope first; within a scope the most recent binding first.
    pub bindings: Vec<HoleBindingJson>,
    pub capabilities: Vec<HoleCapabilityJson>,
    pub requires: Vec<String>,
    pub ensures: Vec<String>,
    pub invariants: Vec<String>,
    pub dependencies: HoleDependenciesJson,
}

/// `{ name, type, kind, mutable? }`; `mutable` only when `true`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoleBindingJson {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub kind: BindingKind,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mutable: bool,
}

/// `{ parameter, capability, operations }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoleCapabilityJson {
    pub parameter: String,
    /// The capability's qualified name.
    pub capability: String,
    pub operations: Vec<HoleOperationJson>,
}

/// `{ name, signature, in_bound }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoleOperationJson {
    pub name: String,
    pub signature: String,
    pub in_bound: bool,
}

/// Semantics S48's `dependencies` object plus `symbols` (Section 7.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoleDependenciesJson {
    /// A commit id, or `"unversioned"`.
    pub source_revision: String,
    /// `Some(true)` only; omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    pub toolchain: ReceiptToolchain,
    pub modules: Vec<ModuleHash>,
    pub symbols: Vec<SymbolVersion>,
}

/// One entry of [`CheckOutput::verification`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationStatusJson {
    pub symbol: StableId,
    pub qualified_name: String,
    pub status: ProofStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ===========================================================================
// 7.2: cpl fmt
// ===========================================================================

/// `FmtOutput` (Section 7.2), flattened into the envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FmtOutput {
    pub check: bool,
    pub files: Vec<FmtFileJson>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FmtFileJson {
    pub path: String,
    pub result: FmtResult,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FmtResult {
    Unchanged,
    Formatted,
    /// With `--check`.
    WouldFormat,
    Error,
}

// ===========================================================================
// 7.2: cpl run
// ===========================================================================

/// `RunOutput` (Section 7.2), flattened into the envelope. On a fault
/// `result` and `result_text` are `null` and `fault` is the diagnostic;
/// otherwise `fault` is `null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutput {
    pub function: SymbolVersion,
    /// Value JSON (Section 7.3), one per positional parameter.
    pub arguments: Vec<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub result_text: Option<String>,
    pub fault: Option<DiagnosticJson>,
}

// ===========================================================================
// 7.2 and 1.7: cpl test
// ===========================================================================

/// The `cpl test` result: the [`TestReport`] flattened, plus the test
/// receipt of Section 8.2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestOutput {
    #[serde(flatten)]
    pub report: TestReport,
    pub receipt: TestReceipt,
}

/// What `cpl_eval::run_tests` returns (Section 1.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestReport {
    /// The run seed, always reported (a decimal `u64`).
    pub seed: u64,
    /// Cases per property in effect.
    pub cases: u32,
    pub selected: u32,
    pub tested: u32,
    pub failed: u32,
    pub results: Vec<TestCaseRecord>,
}

/// One test or property outcome: the payload of the `test_result`
/// diagnostic of semantics S46, plus `kind` and `counterexamples`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCaseRecord {
    pub symbol: StableId,
    pub qualified_name: String,
    pub kind: TestKind,
    pub status: TestStatus,
    pub duration_ms: u64,
    /// Property only: the run seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Property only: the 64-hex-digit derived seed (T45).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property_seed: Option<String>,
    /// Property only: cases executed (the failing index plus one on failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cases: Option<u32>,
    /// Failed property only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failing_case: Option<FailingCase>,
    /// Failed property only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shrunk: Option<ShrunkCase>,
    /// The fault (for a property, the one the shrunk tuple raised).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<DiagnosticJson>,
    /// The `contract_counterexample` diagnostics the failure produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counterexamples: Vec<DiagnosticJson>,
    /// The S59 receipt object; built by the caller of `run_tests` (it needs
    /// revision and module hashes from `cpl-index`), absent until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ReceiptCore>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestKind {
    Test,
    Property,
}

/// Semantics S50's test vocabulary (T44).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Tested,
    Failed,
}

/// `{ index, input }`: the original failing binder tuple, by binder name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailingCase {
    pub index: u32,
    pub input: JsonMap,
}

/// `{ input, shrink_steps, shrink_exhausted }`: the final binder tuple.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShrunkCase {
    pub input: JsonMap,
    pub shrink_steps: u32,
    pub shrink_exhausted: bool,
}

// ===========================================================================
// 7.2 and 8.1: cpl prove, obligations, assumptions
// ===========================================================================

/// `ProveOutput` (Section 7.2), flattened into the envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProveOutput {
    pub symbol: SymbolVersion,
    pub result: ProofStatus,
    pub obligations: Vec<Obligation>,
    pub receipt: VerificationReceipt,
}

/// A proof obligation (Section 8.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation {
    /// `<id>#ensures/1`, `<id>#call/2/requires/1`, ...
    pub id: String,
    /// `ensures`, `decreases`, `termination`, `invariant_init`,
    /// `invariant_preserve`, `loop_decreases`, `call_requires`, `division`,
    /// `refinement`, `assert`.
    pub kind: String,
    pub clause: String,
    pub location: Location,
    pub status: ObligationStatus,
    /// For `unknown`: the verifier's reason (`unsupported_expression`,
    /// `invariant_too_weak`, `nonlinear_arithmetic`, `integer_reasoning`,
    /// `limit`, `indirect_recursion`, `has_errors`) or `no_measure`;
    /// `null` otherwise (`spec/verification.md`).
    pub reason: Option<String>,
    pub evidence: Option<serde_json::Value>,
    /// Assumption ids.
    pub assumptions: Vec<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationStatus {
    Proved,
    Failed,
    Unknown,
}

/// A named assumption (Section 8.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assumption {
    /// `asm_<n>`.
    pub id: String,
    pub kind: AssumptionKind,
    pub symbol: StableId,
    /// A facet or `null`.
    pub facet: Option<Facet>,
    pub description: String,
    pub admitted: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssumptionKind {
    UncheckedCallee,
    UnprovedDependency,
    Hole,
    ForeignAdapter,
}

// ===========================================================================
// 8: receipts
// ===========================================================================

/// The `receipt` key of a receipt document.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    Verification,
    Test,
}

/// The `inputs` object of a receipt (Section 8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptInputsJson {
    pub manifest: Digest,
    /// `null` when there is no `cpl.lock` (the lock is not a source-set member).
    pub lock: Option<Digest>,
    pub std: Digest,
    /// Every source-set file by project-relative path, `cpl.toml` included.
    pub sources: BTreeMap<String, Digest>,
}

/// Section 8.1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationReceipt {
    /// Always [`ReceiptKind::Verification`].
    pub receipt: ReceiptKind,
    /// [`version::JSON_SCHEMA`].
    pub schema: u32,
    pub toolchain: ToolchainInfo,
    pub verifier: VerifierInfo,
    pub revision: Revision,
    pub core: ReceiptCore,
    pub target: SymbolVersion,
    pub dependencies: Vec<SymbolVersion>,
    pub inputs: ReceiptInputsJson,
    pub configuration: VerificationConfig,
    pub status: ProofStatus,
    pub obligations: Vec<Obligation>,
    pub assumptions: Vec<Assumption>,
    /// RFC 3339; informational, excluded from the digest.
    pub produced_at: String,
    /// Informational, excluded from the digest.
    pub duration_ms: u64,
}

/// Section 8.2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestReceipt {
    /// Always [`ReceiptKind::Test`].
    pub receipt: ReceiptKind,
    pub schema: u32,
    pub toolchain: ToolchainInfo,
    pub revision: Revision,
    pub inputs: ReceiptInputsJson,
    pub core: ReceiptCore,
    pub configuration: TestRunConfig,
    /// The static call closure of the selected tests, with facet versions.
    pub tested: Vec<SymbolVersion>,
    pub tests: Vec<TestReceiptEntry>,
    pub summary: TestSummary,
    pub fixtures: Vec<Fixture>,
    pub assumptions: Vec<Assumption>,
    pub environment: BTreeMap<String, String>,
    /// `tested` when every selected test is tested, else `failed`.
    pub status: TestStatus,
    pub produced_at: String,
    pub duration_ms: u64,
}

/// One entry of [`TestReceipt::tests`]: the deterministic part of a
/// [`TestCaseRecord`] (no `duration_ms`, `seed`, or nested `receipt`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestReceiptEntry {
    pub symbol: StableId,
    pub qualified_name: String,
    pub kind: TestKind,
    pub status: TestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property_seed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cases: Option<u32>,
}

/// `summary` of [`TestReceipt`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TestSummary {
    pub selected: u32,
    pub tested: u32,
    pub failed: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationConfig {
    pub release: bool,
    pub policy: VerificationPolicyJson,
    pub proof_timeout_ms: u64,
    /// `[verification] admit` ids in effect.
    pub admitted: Vec<StableId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifierInfo {
    /// `"cpl-prove"` (`spec/verification.md`).
    pub name: String,
    pub version: Option<String>,
    pub configuration: JsonMap,
}

/// Section 8.2 `configuration`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestRunConfig {
    pub seed: u64,
    pub cases: u32,
    pub max_shrinks: u32,
    pub timeout_ms: u64,
    pub selector: TestSelectorSpec,
    pub check_contracts: bool,
    pub fail_fast: bool,
}

/// The wire form of `cpl_eval::TestSelector`; absent parts are `null`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSelectorSpec {
    pub namespace: Option<Vec<String>>,
    pub name: Option<String>,
    pub id: Option<StableId>,
}

/// A capability binding supplied to a test run (empty in Milestone 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixture {
    pub capability: String,
    pub binding: String,
    pub digest: Digest,
}

/// The receipt object of semantics S59 (Section 8 `core`; also the
/// `receipt` field of `verification_result` and `test_result`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptCore {
    /// A commit id, or `"unversioned"`.
    pub source_revision: String,
    /// `Some(true)` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    pub toolchain: ReceiptToolchain,
    pub modules: Vec<ModuleHash>,
    /// `{ release }` or `{ cases, max_shrinks, max_call_depth, seed }`.
    pub check_configuration: serde_json::Value,
    pub assumptions: Vec<Assumption>,
    /// Test receipts only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixtures: Option<Vec<Fixture>>,
    /// Test receipts only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<BTreeMap<String, String>>,
}

/// `{ cpl, edition, std }`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReceiptToolchain {
    pub cpl: String,
    pub edition: String,
    pub std: Digest,
}

impl ReceiptToolchain {
    /// This toolchain with the given std digest (Section 3.6).
    pub fn current(std: Digest) -> ReceiptToolchain {
        ReceiptToolchain {
            cpl: version::TOOLCHAIN.to_string(),
            edition: version::EDITION.to_string(),
            std,
        }
    }
}

/// `{ module, content_hash }`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModuleHash {
    pub module: String,
    pub content_hash: Digest,
}

// ===========================================================================
// 9.2: cpl diff
// ===========================================================================

/// `DiffOutput` (Section 9.2), flattened into the envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffOutput {
    pub old: Revision,
    pub new: Revision,
    pub statements: Vec<DiffStatement>,
    pub duplicates: DiffDuplicates,
}

/// One semantic-diff statement (Section 9.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStatement {
    pub statement: DiffStatementKind,
    pub id: StableId,
    pub path: String,
    /// `"fn"` | `"ty"` | `"cap"` | `"test"` | `"prop"`.
    pub kind: String,
    /// The facet the statement is about, or `null` (identity statements).
    pub facet: Option<Facet>,
    /// Statement-specific, per the table of Section 9.1. Written with its
    /// keys in declaration order (Section 7.1), not in the sorted order of
    /// a JSON map: `{ "old", "new" }`, `{ "from", "to" }`, and an embedded
    /// version exactly as a [`SymbolVersion`] is written.
    #[serde(serialize_with = "serialize_detail")]
    pub detail: serde_json::Value,
}

/// The declaration order of every key a [`DiffStatement::detail`] holds,
/// in which it is written (Section 7.1 writes `--json` output "with keys in
/// declaration order", and a `serde_json::Value` would sort them, T4): the
/// details of the Section 9.1 table in table order (`version` with the
/// `note` of a kind change, `from`/`to`, `from_module`/`to_module`,
/// `old`/`new`, `added`, `removed`), then the fields of an embedded
/// [`SymbolVersion`] in their declaration order (1.4.2), then the facet
/// names of its `facets` in [`Facet::ALL`] order, the "canonical order used
/// by every map" (1.4). Any other key follows these, in sorted order.
const DETAIL_KEY_ORDER: [&str; 15] = [
    "version",
    "note",
    "from",
    "to",
    "from_module",
    "to_module",
    "old",
    "new",
    "added",
    "removed",
    "id",
    "path",
    "provisional",
    "facets",
    "definition",
];

/// The position of a detail key in [`DETAIL_KEY_ORDER`], then in
/// [`Facet::ALL`]; `None` for any other key.
fn detail_key_rank(key: &str) -> Option<usize> {
    DETAIL_KEY_ORDER.iter().position(|k| *k == key).or_else(|| {
        Facet::ALL
            .iter()
            .position(|facet| facet.as_str() == key)
            .map(|i| DETAIL_KEY_ORDER.len() + i)
    })
}

/// A JSON value written with the keys of every object in it ordered by
/// [`detail_key_rank`] (unranked keys last, by name).
struct DetailOrder<'a>(&'a serde_json::Value);

impl Serialize for DetailOrder<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self.0 {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
                entries.sort_by_key(|(key, _)| (detail_key_rank(key).unwrap_or(usize::MAX), *key));
                let mut out = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    out.serialize_entry(key, &DetailOrder(value))?;
                }
                out.end()
            }
            serde_json::Value::Array(items) => {
                serializer.collect_seq(items.iter().map(DetailOrder))
            }
            scalar => scalar.serialize(serializer),
        }
    }
}

fn serialize_detail<S: serde::Serializer>(
    detail: &serde_json::Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    DetailOrder(detail).serialize(serializer)
}

/// The statement vocabulary of Section 9.1, in table order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffStatementKind {
    IdentityAdded,
    IdentityRemoved,
    IdentityRenamed,
    Moved,
    SignatureChanged,
    ContractChanged,
    EffectBoundWidened,
    EffectBoundNarrowed,
    BodyChanged,
    DocsChanged,
    VerificationPolicyChanged,
}

/// `duplicates` of [`DiffOutput`]: ids declared more than once (Section 4.5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffDuplicates {
    pub old: Vec<DuplicateId>,
    pub new: Vec<DuplicateId>,
}

/// An id declared more than once in one revision, with its files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateId {
    pub id: StableId,
    pub files: Vec<String>,
}

// ===========================================================================
// 7.2: cpl id assign
// ===========================================================================

/// `IdAssignOutput` (Section 7.2), flattened into the envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdAssignOutput {
    pub check: bool,
    pub assignments: Vec<IdAssignmentJson>,
    pub skipped_files: Vec<SkippedFileJson>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdAssignmentJson {
    pub path: String,
    pub line: u32,
    /// The declaration's display symbol path.
    pub symbol: String,
    /// `"fn"` | `"ty"` | `"cap"` | `"test"` | `"prop"`.
    pub kind: String,
    pub id: StableId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedFileJson {
    pub path: String,
    pub reason: String,
}

// ===========================================================================
// 7.2: cpl work check
// ===========================================================================

/// `WorkCheckOutput` (Section 7.2), flattened into the envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkCheckOutput {
    pub file: String,
    pub changes: Vec<WorkChangeJson>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkChangeJson {
    pub name: String,
    /// The text inside `git("...")`.
    pub base: String,
    /// Omitted when `base` was not resolved (no repository or no `git`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    pub target: String,
    pub workers: u32,
    pub tasks: Vec<WorkTaskJson>,
    pub integrate: Vec<WorkRequirementJson>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkTaskJson {
    pub name: String,
    pub after: Vec<String>,
    pub write: Vec<WorkSelectorJson>,
    pub add: Vec<WorkAddJson>,
    pub depend: Vec<WorkSelectorJson>,
    pub preserve: Vec<WorkSelectorJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
}

/// A resolved selector: `{ selector, targets }` for declarations and
/// facets, `{ selector, file }` for a file selector (Section 10.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSelectorJson {
    pub selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<WorkTargetJson>>,
    /// The project-relative file a file selector names (`cpl.toml`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// `{ id, facet }` where `facet` is a facet name or `"definition"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkTargetJson {
    pub id: StableId,
    pub facet: FacetSelector,
}

/// An `add` namespace: `{ selector, namespace }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkAddJson {
    pub selector: String,
    pub namespace: String,
}

/// One `integrate` requirement: `{ "require": "typecheck" }`,
/// `{ "require": "prove", "targets": [...] }`,
/// `{ "require": "test", "namespace": "...", "matched": 3 }` (or `targets`
/// for a declaration selector), `{ "require": "review", "task": "..." }`,
/// `{ "require": "no_holes" }`, `{ "require": "no_unresolved_assumptions" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkRequirementJson {
    pub require: RequireKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<WorkTargetJson>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl WorkRequirementJson {
    /// A requirement with only its `require` key.
    pub fn new(require: RequireKind) -> WorkRequirementJson {
        WorkRequirementJson {
            require,
            targets: None,
            namespace: None,
            matched: None,
            task: None,
        }
    }
}

/// The six requirement forms (grammar D36).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequireKind {
    Typecheck,
    NoHoles,
    NoUnresolvedAssumptions,
    Prove,
    Test,
    Review,
}

// ===========================================================================
// Milestone 2, 7.2: cpl build
// ===========================================================================

/// `BuildOutput` (Section 7.2 `cpl build`), flattened into the envelope and
/// written as `<out>/build.json`. Every key is always present (Appendix C).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildOutput {
    /// `"c"`, the one target of Milestone 2.
    pub target: String,
    pub release: bool,
    /// `"optimized"` or `"debug"`.
    pub profile: String,
    /// In the order `address`, `undefined`, `thread`.
    pub sanitize: Vec<String>,
    /// Project-relative when under the project, else absolute (Section 7.1).
    pub out_dir: String,
    pub compiler: CompilerInfo,
    pub runtime: RuntimeInfo,
    pub program_digest: Digest,
    pub exports_digest: Digest,
    /// One per export symbol, in export order (T74).
    pub exports: Vec<BuildExport>,
    /// One per backend-`c` binding, in binding-name order.
    pub bindings: Vec<BuildBinding>,
    /// Every file the build produced for a host, in path order.
    pub artifacts: Vec<BuildArtifact>,
    /// Every build input (Section 0), in path order.
    pub inputs: Vec<BuildInput>,
    /// The unresolved holes linked into the package (0 under `--release`).
    pub holes: u32,
    pub lock: BuildLock,
    pub cached: bool,
    pub duration_ms: u64,
    /// Milestone 3 (`spec/python.md` 1): the Python target's interpreter and package; present only when `target` is
    /// `"python"` (the one key of the envelope that is absent rather than null).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<PythonBuildInfo>,
}

/// `python` of [`BuildOutput`] (`spec/python.md` 1): the interpreter whose headers built the extension, its
/// `platform.python_version()`, the import name, the distribution version, and the wheel's tag
/// (`cp310-abi3-linux_<arch>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonBuildInfo {
    pub interpreter: String,
    pub version: String,
    pub package: String,
    pub distribution_version: String,
    pub tag: String,
}

/// `compiler` of [`BuildOutput`]: the resolved driver binary, `family`
/// `"gcc"` or `"clang"`, its version, its `-dumpmachine` target, and the
/// blake3 digest of the binary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerInfo {
    pub path: String,
    pub family: String,
    pub version: String,
    pub target: String,
    pub digest: Digest,
}

/// `runtime` of [`BuildOutput`]: `cpl_rt::RUNTIME_VERSION`, the C API
/// version as `"<major>.<minor>"`, `source` `"embedded"` or `"dir"`
/// (`CPL_RUNTIME_DIR`), and `cpl_rt::runtime_digest`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub version: String,
    pub api: String,
    pub source: String,
    pub digest: Digest,
}

/// One export of [`BuildOutput::exports`]: its C symbol, its target's
/// qualified name and identity, its ABI digest (`spec/cabi.md` Section 9),
/// its integer mapping (`handle`, `int64`, `int32`, `uint64`, `uint32`),
/// and its declared effect bound as sorted `param.op` strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildExport {
    pub symbol: String,
    pub function: String,
    pub id: StableId,
    pub signature: Digest,
    pub int: String,
    pub effects: Vec<String>,
}

/// One backend-`c` binding of [`BuildOutput::bindings`]: `form` is
/// `"wrapper"` or `"declarative"`, `grant` the C function a host calls to
/// obtain the adapter (`cboundary_bind_image_codec`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildBinding {
    pub binding: String,
    pub capability: String,
    pub form: String,
    pub isolation: String,
    pub grant: String,
}

/// One file of [`BuildOutput::artifacts`]: `kind` is `"header"`,
/// `"shared_library"`, `"static_library"`, `"executable"`, or `"worker"` (the
/// C adapter host, `spec/python.md` 6.6); for the Python target
/// `"python_module"` (each `.py` and `.pyi` file and `py.typed`),
/// `"extension"`, `"worker"`, and `"wheel"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildArtifact {
    pub kind: String,
    pub path: String,
    pub digest: Digest,
}

/// One build input of [`BuildOutput::inputs`]: `kind` is `"adapter"` or
/// `"vendored_source"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInput {
    pub kind: String,
    pub binding: String,
    pub path: String,
    pub digest: Digest,
}

/// `lock` of [`BuildOutput`]: `checked`, the comparisons of Section 11.2
/// ran (`--release`); `written`, `cpl.lock` was written (`--lock`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BuildLock {
    pub checked: bool,
    pub written: bool,
}

// ===========================================================================
// Milestone 2, 7.2: --engine
// ===========================================================================

/// The engine of `cpl run` and `cpl test` (`--engine`, Section 7.2, T73).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// The reference evaluator, the default and the oracle.
    #[default]
    Eval,
    /// The program's driver process.
    Native,
    /// Both, compared under semantics S75.
    Both,
}

impl EngineKind {
    /// Every engine, in declaration order.
    pub const ALL: [EngineKind; 3] = [EngineKind::Eval, EngineKind::Native, EngineKind::Both];

    /// `"eval"`, `"native"`, `"both"`.
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::Eval => "eval",
            EngineKind::Native => "native",
            EngineKind::Both => "both",
        }
    }

    /// The engine with this spelling.
    pub fn parse(s: &str) -> Option<EngineKind> {
        EngineKind::ALL.into_iter().find(|e| e.as_str() == s)
    }
}

impl fmt::Display for EngineKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One handle `cpl run` bound for a `using` parameter (semantics S78): the
/// parameter, the `[bindings.<name>]` table, the handle number (`$handle`
/// of value JSON), and the digest of the binding's build inputs (T82).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBinding {
    pub parameter: String,
    pub binding: String,
    pub handle: u32,
    pub inputs: Digest,
}

/// The `cpl run` result from Milestone 2: [`RunOutput`] flattened and
/// unchanged, plus `engine` (omitted under `eval`) and `bindings` (omitted
/// when no handle was bound), so that every Milestone 1 golden stays
/// byte-identical (T73, T77).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineRunOutput {
    #[serde(flatten)]
    pub run: RunOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<RunBinding>,
}

/// The `cpl test` result from Milestone 2: [`TestOutput`] flattened and
/// unchanged, plus `engine` (omitted under `eval`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineTestOutput {
    #[serde(flatten)]
    pub test: TestOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineKind>,
}
