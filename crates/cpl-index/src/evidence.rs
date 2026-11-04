//! The evidence the commands attach to what the checker and the test
//! runner report (`spec/toolchain.md` 1.6, 7.2, 8; semantics S48, S59), so
//! that `cpl-cli` assembles it without logic of its own (1.10, T3):
//!
//! | Command | Item | Specification |
//! | - | - | - |
//! | `cpl check` | [`Index::hole_dependencies`], [`Index::hole_report_json`], [`Index::verification_receipt_core`], [`Index::complete_check_diagnostics`], [`Index::check_output`] | 7.2, S48, S59 |
//! | `cpl prove` | [`Index::prove`] ([`Proof`]) | 7.2, 8.1, S36 |
//! | `cpl test` | [`Index::test_receipt_core`], [`Index::complete_test_report`], [`test_check_configuration`], [`test_environment`] | 7.2, 8.2, S46, S59 |
//! | all three | [`Index::modules_of`], [`Index::versions_of`] (with [`Index::call_closure`]) | S48, S59, 8 |
//!
//! Readings taken here:
//!
//! - `source_revision` is the revision's commit, or `"unversioned"` without
//!   one (no repository, or a repository without commits), and `dirty` is
//!   present (`true`) exactly when there is a commit and the state is
//!   `worktree`: it qualifies a commit, so an unversioned tree has none.
//! - A hole's `symbols` are the versions of `Index::dependencies` of its
//!   declaration (6.5: every reference, a recursive declaration's reference
//!   to itself included), which is "every definition the enclosing
//!   declaration references" (1.6, 7.2): `HoleReport::dependencies`, which
//!   `cpl-sema` builds without that self-reference, is not read.
//! - A hole's `modules` are those of the definitions its declaration
//!   references and of the declaration itself (whose module is part of the
//!   hole's context), and a receipt's `modules` those of the static call
//!   closure of the checked declaration, itself included ([`Index::call_closure`]);
//!   std modules are never listed, `toolchain.std` covering them, as the
//!   examples of 7.2 and 8.1 show.
//! - A test receipt's `tested` is the static call closure of the selected
//!   tests, the tests included, so that a test's own version is part of
//!   the evidence a reused receipt must match (8.2).
//! - A verification status is the verifier's (`cpl_prove`,
//!   `spec/verification.md`): a function whose obligations are all proved
//!   (or that has none), whose termination is known, and that has no typed
//!   hole is `proved`, and `cpl prove` exits 0 for it (T29).

use crate::codes;
use crate::query::{name_span, Index, Target};
use crate::receipt::{test_receipt, verification_receipt};
use cpl_diag::json::{
    CheckOutput, CheckSummary, HoleBindingJson, HoleCapabilityJson, HoleDependenciesJson,
    HoleOperationJson, HoleReportJson, JsonMap, ModuleHash, Obligation, ObligationStatus,
    ProofStatus, ProveOutput, ReceiptCore, ReceiptToolchain, Revision, RevisionState, Status,
    SymbolVersion, TestReceipt, TestReport, TestRunConfig, VerificationConfig,
    VerificationPolicyJson, VerificationStatusJson, VerifierInfo,
};
use cpl_diag::{Category, Diagnostic, Facet, FileOrigin, Severity};
use cpl_sema::hir::{DefId, DefKind};
use cpl_sema::{CheckResult, HoleReport};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// `cpl prove`'s result (Section 7.2): the `ProveOutput` of the envelope
/// and the one `verification_result` diagnostic of the symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub output: ProveOutput,
    pub diagnostic: Diagnostic,
}

impl Proof {
    /// The envelope status of `cpl prove` (T29, Section 7.4): `ok` (exit 0)
    /// only when every obligation is proved, a `proved` or `conditional`
    /// result; `unsuccessful` (exit 1) for `unknown` and `failed`, which
    /// `result` tells apart.
    pub fn status(&self) -> Status {
        match self.output.result {
            ProofStatus::Proved | ProofStatus::Conditional => Status::Ok,
            ProofStatus::Unknown | ProofStatus::Failed => Status::Unsuccessful,
        }
    }
}

/// The `check_configuration` of a `test_result` receipt (semantics S59):
/// `{ cases, max_shrinks, max_call_depth, seed }`, the values in effect.
pub fn test_check_configuration(config: &TestRunConfig, max_call_depth: u32) -> Value {
    json!({
        "cases": config.cases,
        "max_shrinks": config.max_shrinks,
        "max_call_depth": max_call_depth,
        "seed": config.seed,
    })
}

/// The environment inputs of a test run (Section 8.2): `os` and `arch`
/// (`linux`, `x86_64`), the Milestone 1 set.
pub fn test_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("arch".to_string(), std::env::consts::ARCH.to_string()),
        ("os".to_string(), std::env::consts::OS.to_string()),
    ])
}

/// A function's status from its obligations (Section 8.1): `failed` when one failed, `unknown`
/// (the first unknown obligation's reason, or `no_measure` when only termination is unknown)
/// when one is not proved or termination is unknown, `proved` otherwise.
fn proof_status(
    obligations: &[Obligation],
    termination_unknown: bool,
) -> (ProofStatus, Option<String>) {
    if obligations
        .iter()
        .any(|o| o.status == ObligationStatus::Failed)
    {
        return (ProofStatus::Failed, None);
    }
    if let Some(o) = obligations
        .iter()
        .find(|o| o.status == ObligationStatus::Unknown)
    {
        return (ProofStatus::Unknown, o.reason.clone());
    }
    if termination_unknown {
        return (ProofStatus::Unknown, Some("no_measure".to_string()));
    }
    (ProofStatus::Proved, None)
}

/// The verifier of every receipt (`spec/verification.md`).
fn verifier() -> VerifierInfo {
    let mut configuration = JsonMap::new();
    configuration.insert("theory".into(), json!("linear_integer_arithmetic"));
    VerifierInfo {
        name: cpl_prove::VERIFIER.to_string(),
        version: Some(cpl_prove::VERSION.to_string()),
        configuration,
    }
}

/// `source_revision` and `dirty` of a revision (module docs).
fn revision_fields(revision: &Revision) -> (String, Option<bool>) {
    match &revision.commit {
        Some(commit) => (
            commit.clone(),
            (revision.state == RevisionState::Worktree).then_some(true),
        ),
        None => ("unversioned".to_string(), None),
    }
}

impl<'p> Index<'p> {
    /// The project modules of `defs`, each with its content hash, in
    /// module-path order (std modules are left out: `toolchain.std` covers
    /// them).
    pub fn modules_of(&self, defs: impl IntoIterator<Item = DefId>) -> Vec<ModuleHash> {
        let program = self.program();
        let modules: BTreeMap<String, ModuleHash> = defs
            .into_iter()
            .map(|d| program.def(d).module)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|m| {
                let hash = self.module_hash(m)?;
                let path = program.module(m).path.join(".");
                Some((
                    path.clone(),
                    ModuleHash {
                        module: path,
                        content_hash: hash.clone(),
                    },
                ))
            })
            .collect();
        modules.into_values().collect()
    }

    /// The versions of `defs` in display-path order, one per id: the form
    /// of a receipt's `dependencies` and `tested` (Section 8) and of a hole
    /// report's `symbols` (`index.versions_of(index.call_closure(&selected))`
    /// is a test receipt's `tested`).
    pub fn versions_of(&self, defs: impl IntoIterator<Item = DefId>) -> Vec<SymbolVersion> {
        let mut defs: Vec<DefId> = defs.into_iter().collect();
        self.sort_by_path(&mut defs);
        let mut out: Vec<SymbolVersion> = defs.iter().map(|&d| self.version(d).clone()).collect();
        out.dedup_by(|a, b| a.id == b.id);
        out
    }

    /// The receipt object of semantics S59 with the given modules and
    /// configuration; `assumptions` are `[]` in Milestone 1.
    fn core(&self, modules: Vec<ModuleHash>, check_configuration: Value) -> ReceiptCore {
        let (source_revision, dirty) = revision_fields(self.revision());
        ReceiptCore {
            source_revision,
            dirty,
            toolchain: ReceiptToolchain::current(self.inputs.std_digest.clone()),
            modules,
            check_configuration,
            assumptions: Vec::new(),
            fixtures: None,
            environment: None,
        }
    }

    /// The `dependencies` object of a hole report (semantics S48, Section
    /// 7.2): `source_revision`, `dirty` (only when `true`), `toolchain`
    /// `{ cpl, edition, std }`, the `modules` its declaration refers to
    /// with their content hashes, and `symbols`, the versions of every
    /// definition the declaration references in display-path order: its
    /// [`Index::dependencies`] (Section 6.5), so a recursive declaration
    /// lists itself (module docs).
    pub fn hole_dependencies(&self, report: &HoleReport) -> HoleDependenciesJson {
        self.declaration_dependencies(report.def)
    }

    /// The `dependencies` object of a hole in `def` ([`Index::hole_dependencies`]).
    fn declaration_dependencies(&self, def: DefId) -> HoleDependenciesJson {
        let (source_revision, dirty) = revision_fields(self.revision());
        let referenced: Vec<DefId> = self.dependencies(def).iter().map(|d| d.def).collect();
        HoleDependenciesJson {
            source_revision,
            dirty,
            toolchain: ReceiptToolchain::current(self.inputs.std_digest.clone()),
            modules: self.modules_of(referenced.iter().copied().chain([def])),
            symbols: self.versions_of(referenced),
        }
    }

    /// The hole report of semantics S48 as `CheckOutput::holes` and an
    /// inspect `holes` entry carry it: the `hole` diagnostic's payload with
    /// its `dependencies` ([`Index::hole_dependencies`]). Types render as
    /// the checker renders them (type parameters by name, `?` for an
    /// erroneous part, S69); `capability` is the qualified name.
    pub fn hole_report_json(&self, report: &HoleReport) -> HoleReportJson {
        let program = self.program();
        let generics = program.def(report.def).kind.generics();
        let ty = |t| program.display_ty_in(t, generics);
        HoleReportJson {
            hole: report.name.clone(),
            expected_type: ty(&report.expected),
            bindings: report
                .bindings
                .iter()
                .map(|b| HoleBindingJson {
                    name: b.name.clone(),
                    ty: ty(&b.ty),
                    kind: b.kind,
                    mutable: b.mutable,
                })
                .collect(),
            capabilities: report
                .capabilities
                .iter()
                .map(|c| HoleCapabilityJson {
                    parameter: c.parameter.clone(),
                    capability: program.display_path(c.capability),
                    operations: c
                        .operations
                        .iter()
                        .map(|o| HoleOperationJson {
                            name: o.name.clone(),
                            signature: o.signature.clone(),
                            in_bound: o.in_bound,
                        })
                        .collect(),
                })
                .collect(),
            requires: report.requires.clone(),
            ensures: report.ensures.clone(),
            invariants: report.invariants.clone(),
            dependencies: self.hole_dependencies(report),
        }
    }

    /// The `receipt` of a `verification_result` (semantics S59): the
    /// modules of the function's static call closure and
    /// `check_configuration: { release }`. `cpl prove`'s receipt has this
    /// object as its `core` (Section 8).
    pub fn verification_receipt_core(&self, def: DefId, release: bool) -> ReceiptCore {
        self.core(
            self.modules_of(self.call_closure(&[def])),
            json!({ "release": release }),
        )
    }

    /// The `receipt` of a `test_result` (semantics S59), and the `core` of
    /// a test receipt for a whole run (Section 8.2): the modules of the
    /// static call closure of `tests` (the union over the selected tests),
    /// `check_configuration` ([`test_check_configuration`]), `fixtures: []`
    /// and `environment` ([`test_environment`]).
    pub fn test_receipt_core(&self, tests: &[DefId], check_configuration: Value) -> ReceiptCore {
        ReceiptCore {
            fixtures: Some(Vec::new()),
            environment: Some(test_environment()),
            ..self.core(
                self.modules_of(self.call_closure(tests)),
                check_configuration,
            )
        }
    }

    /// The check's diagnostics with what `cpl-cli` adds (Section 7.2): the
    /// `dependencies` of every `hole` diagnostic (of the declaration of the
    /// report with its span, or else of its `symbol`, so that an index
    /// without the reports completes it too) and the `receipt` of every
    /// `verification_result` (matched by its `symbol`), built from the same
    /// values; every other diagnostic, and the order, unchanged. `release`
    /// is `CheckConfig::release`.
    pub fn complete_check_diagnostics(
        &self,
        diagnostics: &[Diagnostic],
        release: bool,
    ) -> Vec<Diagnostic> {
        let program = self.program();
        diagnostics
            .iter()
            .cloned()
            .map(|mut d| {
                if d.code == "hole" {
                    let def = d
                        .span
                        .and_then(|span| self.holes().iter().find(|h| h.span == span))
                        .map(|report| report.def)
                        .or_else(|| d.symbol.as_ref().and_then(|id| program.by_stable_id(id)));
                    if let Some(def) = def {
                        d.data.insert(
                            "dependencies".to_string(),
                            to_value(&self.declaration_dependencies(def)),
                        );
                    }
                } else if d.code == "verification_result" {
                    if let Some(def) = d.symbol.as_ref().and_then(|id| program.by_stable_id(id)) {
                        d.data.insert(
                            "receipt".to_string(),
                            to_value(&self.verification_receipt_core(def, release)),
                        );
                    }
                }
                d
            })
            .collect()
    }

    /// `CheckOutput` (Section 7.2): the mode, the summary (`errors` and
    /// `warnings` counted over the check's diagnostics, `holes` its hole
    /// reports, and the `definitions`, `modules`, and `.cpl` files of the
    /// project, std left out), the hole payloads, and one entry per
    /// verification status.
    pub fn check_output(&self, check: &CheckResult, release: bool) -> CheckOutput {
        let program = self.program();
        let count = |severity| {
            check
                .diagnostics
                .iter()
                .filter(|d| d.severity == severity)
                .count() as u32
        };
        let in_project = |m| program.module(m).origin != FileOrigin::Std;
        CheckOutput {
            release,
            summary: CheckSummary {
                errors: count(Severity::Error),
                warnings: count(Severity::Warning),
                holes: check.holes.len() as u32,
                definitions: program.defs().filter(|d| in_project(d.module)).count() as u32,
                modules: program.modules().filter(|m| in_project(m.id)).count() as u32,
                files: self.source_files,
            },
            holes: check
                .holes
                .iter()
                .map(|h| self.hole_report_json(h))
                .collect(),
            verification: check
                .verification
                .iter()
                .map(|v| VerificationStatusJson {
                    symbol: program.def(v.def).stable_id.clone(),
                    qualified_name: program.display_path(v.def),
                    status: v.status,
                    reason: v.reason.clone(),
                })
                .collect(),
        }
    }

    /// `cpl prove <symbol>` (Section 7.2): resolve the target, enumerate its
    /// obligations (Section 8.1), and report the verification status with
    /// the receipt of Section 8.1 and the `verification_result` diagnostic
    /// of semantics S36 (`info`, at the function's name, `facet:
    /// "verification_policy"`, `status`, `reason`, and its `receipt`, the
    /// receipt's `core`; a note when the function's termination is unknown
    /// as well, semantics 5.5: recursive without a measure, or a `while`
    /// loop without one). The status is the verifier's report on the
    /// function (with `obligation` and `model` for a failed one), the
    /// verifier is `cpl-prove` with its version, and the receipt's
    /// `dependencies` are the function's dependencies, interface and body.
    ///
    /// Errors: the resolution's (`index_unknown_symbol`,
    /// `index_ambiguous_symbol`), and `usage_not_provable` for a target that
    /// is not a function.
    pub fn prove(&self, target: &Target) -> Result<Proof, Diagnostic> {
        let program = self.program();
        let def = self.resolve(target)?;
        let d = program.def(def);
        let qualified_name = program.display_path(def);
        if !matches!(d.kind, DefKind::Fn(_)) {
            return Err(Diagnostic::error(
                codes::USAGE_NOT_PROVABLE,
                Category::Usage,
                format!(
                    "`{qualified_name}` is a {}, not a function: only functions have proof obligations",
                    d.kind.keyword()
                ),
            )
            .with_symbol(d.stable_id.clone(), qualified_name, None));
        }
        let obligations = self.obligations(def);
        let termination_unknown = crate::obligations::termination_unknown(self, def);
        // The verifier's status of a function with a body (it also knows about holes); from the
        // obligations for a `@builtin`, whose clauses are axioms.
        let (status, reason) = match self.proof(program, def) {
            Some(report) => (
                match report.status {
                    cpl_prove::Status::Proved => ProofStatus::Proved,
                    cpl_prove::Status::Failed => ProofStatus::Failed,
                    cpl_prove::Status::Unknown => ProofStatus::Unknown,
                },
                report.reason.map(str::to_string),
            ),
            None => proof_status(&obligations, termination_unknown),
        };
        let core = self.verification_receipt_core(def, false);
        let config = VerificationConfig {
            release: false,
            policy: VerificationPolicyJson {
                prove: d.policy.prove,
            },
            proof_timeout_ms: self.proof_timeout_ms,
            admitted: self.admitted.clone(),
        };
        let dependencies = self
            .dependencies(def)
            .iter()
            .map(|dep| self.version(dep.def).clone())
            .collect();
        let receipt = verification_receipt(
            &self.inputs,
            core.clone(),
            self.version(def).clone(),
            dependencies,
            config,
            status,
            obligations.clone(),
            Vec::new(),
            verifier(),
        );
        // The same message as the check's `verification_result`.
        let failed = obligations
            .iter()
            .find(|o| o.status == ObligationStatus::Failed);
        let message = match (&reason, failed) {
            (_, Some(o)) if status == ProofStatus::Failed => {
                format!("{}: failed ({} obligation)", d.name.display(), o.kind)
            }
            (Some(r), _) => format!(
                "{}: {} ({})",
                d.name.display(),
                status.as_str(),
                r.replace('_', " ")
            ),
            (None, _) => format!("{}: {}", d.name.display(), status.as_str()),
        };
        let mut diagnostic =
            Diagnostic::info(codes::VERIFICATION_RESULT, Category::Contract, message)
                .with_span(name_span(program.ast(def)))
                .with_symbol(
                    d.stable_id.clone(),
                    qualified_name,
                    Some(Facet::VerificationPolicy),
                )
                .with_data("status", json!(status.as_str()))
                .with_data("reason", json!(reason))
                .with_data("receipt", to_value(&core));
        if let Some(o) = obligations
            .iter()
            .find(|o| o.status == ObligationStatus::Failed)
        {
            diagnostic = diagnostic.with_data("obligation", json!(o.id)).with_data(
                "model",
                o.evidence
                    .as_ref()
                    .map(|e| e["model"].clone())
                    .unwrap_or_default(),
            );
        }
        if termination_unknown {
            diagnostic = diagnostic.with_note(
                "termination is unknown as well: a `decreases` measure is missing (`missing_decreases`)",
                None,
            );
        }
        Ok(Proof {
            output: ProveOutput {
                symbol: self.version(def).clone(),
                result: status,
                obligations,
                receipt,
            },
            diagnostic,
        })
    }

    /// Fill the `receipt` of every record of a test run (semantics S46,
    /// S59; Section 7.2: `cpl-cli` fills each record's receipt before it
    /// emits the diagnostics) and build the run's test receipt (Section
    /// 8.2). `selected` is the run's selection (`cpl_eval::select_tests`,
    /// every selected test even when `fail_fast` stopped early), `config`
    /// the run's configuration, and `max_call_depth` the evaluator's
    /// (`EvalConfig::max_call_depth`). Each record's receipt covers the
    /// static call closure of its own test; the run's `core` and `tested`
    /// cover the selection's.
    pub fn complete_test_report(
        &self,
        report: &mut TestReport,
        selected: &[DefId],
        config: &TestRunConfig,
        max_call_depth: u32,
    ) -> TestReceipt {
        let check_configuration = test_check_configuration(config, max_call_depth);
        let program = self.program();
        for record in &mut report.results {
            let tests: Vec<DefId> = program.by_stable_id(&record.symbol).into_iter().collect();
            record.receipt = Some(self.test_receipt_core(&tests, check_configuration.clone()));
        }
        let core = self.test_receipt_core(selected, check_configuration);
        let tested = self.versions_of(self.call_closure(selected));
        test_receipt(
            &self.inputs,
            core,
            report,
            tested,
            config.clone(),
            Vec::new(),
            Vec::new(),
            test_environment(),
        )
    }
}

/// A wire value as JSON (wire types always serialize).
fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("wire types serialize")
}
