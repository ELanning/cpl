use crate::{
    error,
    query::{impact, project_ids, Impact},
    Snapshot,
};
use cpl_diag::json::{Obligation, ObligationStatus, TestCaseRecord, TestStatus};
use cpl_diag::{Diagnostic, StableId};
use cpl_eval::{EvalConfig, TestConfig};
use cpl_hir::DefKind;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct VerifyConfig {
    pub seed: u64,
    pub cases: u32,
    pub max_shrinks: u32,
    pub timeout_ms: u64,
    pub max_call_depth: u32,
    pub require_proofs: bool,
    /// With a baseline, persisted schema versions must stay available and reachable by migration.
    pub require_migrations: bool,
}
impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            cases: 100,
            max_shrinks: 1000,
            timeout_ms: 5000,
            max_call_depth: EvalConfig::DEFAULT_MAX_CALL_DEPTH,
            require_proofs: false,
            require_migrations: true,
        }
    }
}
impl VerifyConfig {
    pub fn eval(&self) -> EvalConfig {
        EvalConfig {
            time_budget: Some(Duration::from_millis(self.timeout_ms)),
            max_call_depth: self.max_call_depth,
            ..EvalConfig::default()
        }
    }
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if self.cases == 0 || self.timeout_ms == 0 || self.max_call_depth == 0 {
            return Err(error(
                "verification_config_invalid",
                "cases, timeout_ms, and max_call_depth must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub format: u32,
    pub toolchain: String,
    pub snapshot: String,
    pub base: Option<String>,
    pub configuration: VerifyConfig,
    pub impact: Impact,
    pub typecheck: String,
    pub tests: Vec<TestCaseRecord>,
    pub obligations: Vec<Obligation>,
    pub holes: usize,
    /// Persisted-data incompatibilities with the baseline; empty without a baseline.
    pub schemas: Vec<serde_json::Value>,
    /// Each kind of evidence separately: none of them is promoted to another.
    pub summary: serde_json::Value,
    pub accepted: bool,
    pub digest: String,
}

impl Evidence {
    pub fn calculate_digest(&self) -> String {
        let mut copy = self.clone();
        copy.digest.clear();
        blake3::hash(&serde_json::to_vec(&copy).expect("evidence serializes"))
            .to_hex()
            .to_string()
    }
    pub fn unresolved(&self) -> usize {
        self.obligations
            .iter()
            .filter(|o| o.status != ObligationStatus::Proved)
            .count()
    }
}

/// Worker threads for independent evaluations: `CPL_PARALLELISM` (1..=64) when set, else the
/// available processors, at most eight, the runtime's own default.
pub(crate) fn parallelism() -> usize {
    std::env::var("CPL_PARALLELISM")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| (1..=64).contains(n))
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(8)))
}

/// Run tests on a pool of evaluation threads. Tests are independent and properties draw from
/// their own seeded streams, so the records equal a sequential run's, in the given order.
pub(crate) fn run_tests(
    p: &cpl_hir::Program,
    ids: &[cpl_hir::DefId],
    config: &TestConfig,
) -> Vec<TestCaseRecord> {
    let workers = parallelism().min(ids.len());
    if workers <= 1 {
        return cpl_eval::on_eval_stack(|| {
            ids.iter()
                .map(|id| cpl_eval::run_test(p, *id, config))
                .collect()
        });
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<TestCaseRecord>> = vec![None; ids.len()];
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    cpl_eval::on_eval_stack(|| {
                        let mut done = Vec::new();
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(id) = ids.get(i) else { break };
                            done.push((i, cpl_eval::run_test(p, *id, config)));
                        }
                        done
                    })
                })
            })
            .collect();
        for handle in handles {
            for (i, record) in handle.join().expect("a test worker panicked") {
                out[i] = Some(record);
            }
        }
    });
    out.into_iter()
        .map(|r| r.expect("every test ran"))
        .collect()
}

pub(crate) fn test_config(config: &VerifyConfig) -> TestConfig {
    TestConfig {
        cases: config.cases,
        max_shrinks: config.max_shrinks,
        eval: config.eval(),
        ..TestConfig::with_seed(config.seed)
    }
}

/// Always typechecks the entire snapshot. Only execution and obligation enumeration are incremental.
pub fn verify(
    after: &Snapshot,
    before: Option<&Snapshot>,
    config: &VerifyConfig,
) -> Result<Evidence, Diagnostic> {
    config.validate()?;
    let p = &after.checked.program;
    let index = after.index();
    let impact = before.map(|b| impact(b, after)).unwrap_or_else(|| {
        let affected = project_ids(p);
        let tests = p
            .defs()
            .filter(|d| {
                affected.contains(&d.stable_id)
                    && matches!(d.kind, DefKind::Test(_) | DefKind::Property(_))
            })
            .map(|d| d.stable_id.clone())
            .collect();
        Impact {
            changed: affected.clone(),
            affected,
            tests,
            full_recheck: true,
        }
    });
    let test_config = test_config(config);
    let ids: Vec<_> = impact
        .tests
        .iter()
        .filter_map(|id| p.by_stable_id(id))
        .collect();
    let tests = run_tests(p, &ids, &test_config);
    let obligations: Vec<_> = impact
        .affected
        .iter()
        .filter_map(|id| p.by_stable_id(id))
        .flat_map(|id| index.obligations(id))
        .collect();
    let proofs_ok = obligations.iter().all(|o| {
        o.status == ObligationStatus::Proved
            || (!config.require_proofs && o.status == ObligationStatus::Unknown)
    });
    let holes = after.checked.holes.len();
    let schemas = before
        .map(|b| crate::migration::schema_issues(b, after))
        .unwrap_or_default();
    let schemas_ok = schemas.is_empty() || !config.require_migrations;
    let accepted = holes == 0
        && tests.iter().all(|t| t.status == TestStatus::Tested)
        && proofs_ok
        && schemas_ok;
    let count =
        |status: ObligationStatus| obligations.iter().filter(|o| o.status == status).count();
    let declarations = project_ids(p).len();
    let summary = serde_json::json!({
        "typecheck": "passed",
        "tests": {"passed": tests.iter().filter(|t| t.status == TestStatus::Tested).count(),
                  "failed": tests.iter().filter(|t| t.status == TestStatus::Failed).count()},
        "property_cases": tests.iter().filter_map(|t| t.cases).map(u64::from).sum::<u64>(),
        "contracts": "checked_at_runtime_by_tests",
        "obligations": {"proved": count(ObligationStatus::Proved), "unknown": count(ObligationStatus::Unknown),
                        "failed": count(ObligationStatus::Failed)},
        "holes": holes,
        "schemas": match (before, schemas.is_empty()) {
            (None, _) => "not_checked",
            (Some(_), true) => "compatible",
            (Some(_), false) => "incompatible",
        },
        "scope": {"declarations": declarations, "affected": impact.affected.iter().filter(|id| p.by_stable_id(id).is_some()).count(),
                  "full_recheck": impact.full_recheck},
    });
    let mut evidence = Evidence {
        format: 1,
        toolchain: env!("CARGO_PKG_VERSION").into(),
        snapshot: after.digest(),
        base: before.map(Snapshot::digest),
        configuration: config.clone(),
        impact,
        typecheck: "passed".into(),
        tests,
        obligations,
        holes,
        schemas,
        summary,
        accepted,
        digest: String::new(),
    };
    evidence.digest = evidence.calculate_digest();
    Ok(evidence)
}

/// The digest detects tampering; it is not a signature. Acceptance always independently recomputes evidence.
pub fn recheck(
    after: &Snapshot,
    before: Option<&Snapshot>,
    evidence: &Evidence,
) -> Result<Evidence, Diagnostic> {
    if evidence.format != 1
        || evidence.toolchain != env!("CARGO_PKG_VERSION")
        || evidence.snapshot != after.digest()
        || evidence.base != before.map(Snapshot::digest)
        || evidence.digest != evidence.calculate_digest()
    {
        return Err(error(
            "evidence_stale",
            "evidence does not match the snapshots, toolchain, or digest",
        ));
    }
    let actual = verify(after, before, &evidence.configuration)?;
    if actual.impact != evidence.impact
        || actual.accepted != evidence.accepted
        || actual.obligations != evidence.obligations
        || normalized_tests(&actual.tests) != normalized_tests(&evidence.tests)
        || actual.holes != evidence.holes
        || actual.schemas != evidence.schemas
        || actual.summary != evidence.summary
        || actual.typecheck != evidence.typecheck
    {
        return Err(error(
            "evidence_mismatch",
            "independent verification disagrees with the submitted evidence",
        ));
    }
    Ok(actual)
}
fn normalized_tests(tests: &[TestCaseRecord]) -> Vec<TestCaseRecord> {
    tests
        .iter()
        .cloned()
        .map(|mut t| {
            t.duration_ms = 0;
            t
        })
        .collect()
}

pub fn obligations_for(
    s: &Snapshot,
    ids: &std::collections::BTreeSet<StableId>,
) -> Vec<Obligation> {
    ids.iter()
        .filter_map(|id| s.checked.program.by_stable_id(id))
        .flat_map(|id| s.index().obligations(id))
        .collect()
}
