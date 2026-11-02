//! Receipts (`spec/toolchain.md` 8): the inputs every receipt records, the
//! verification (8.1) and test (8.2) receipt documents, and the receipt
//! digest, blake3 of the receipt's canonical JSON, which names it: two
//! receipts with equal digests are the same evidence.

use crate::source::{digest, source_set};
use cpl_diag::json::{
    Assumption, Digest, Fixture, Obligation, ProofStatus, ReceiptCore, ReceiptInputsJson,
    ReceiptKind, Revision, SymbolVersion, TestReceipt, TestReceiptEntry, TestReport, TestRunConfig,
    TestStatus, TestSummary, ToolchainInfo, VerificationConfig, VerificationReceipt, VerifierInfo,
};
use cpl_diag::version;
use cpl_project::Project;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// What every receipt records about its inputs (Section 8, `inputs`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReceiptInputs {
    pub toolchain: ToolchainInfo,
    pub revision: Revision,
    /// Every file of the source set, by project-relative path, `cpl.toml`
    /// included.
    pub sources: BTreeMap<String, Digest>,
    /// The digest of the standard library the project was loaded with
    /// (Section 3.6).
    pub std_digest: Digest,
    /// blake3 of `cpl.lock`; `None` without a lock.
    pub lock_digest: Option<Digest>,
    pub manifest_digest: Digest,
}

impl ReceiptInputs {
    /// The `inputs` object of a receipt document.
    pub fn to_json(&self) -> ReceiptInputsJson {
        ReceiptInputsJson {
            manifest: self.manifest_digest.clone(),
            lock: self.lock_digest.clone(),
            std: self.std_digest.clone(),
            sources: self.sources.clone(),
        }
    }
}

/// The receipt inputs of a loaded project at `revision`: this toolchain,
/// the digest of every source-set file, the std digest, and the manifest
/// and lock digests.
pub fn receipt_inputs(project: &Project, revision: Revision) -> ReceiptInputs {
    ReceiptInputs {
        toolchain: ToolchainInfo::current(),
        revision,
        sources: source_set(project).into_iter().collect(),
        std_digest: project.std_digest(),
        lock_digest: project.lock_digest.clone(),
        manifest_digest: project.manifest_digest.clone(),
    }
}

/// A verification receipt (Section 8.1). `dependencies` are ordered by
/// display path (Section 6.5) and deduplicated by id. `produced_at` is the
/// current UTC time (RFC 3339, whole seconds) and `duration_ms` is 0: both
/// are informational, outside the digest, and the caller sets
/// `duration_ms` once the check is timed.
#[allow(clippy::too_many_arguments)] // the signature of spec/toolchain.md 1.8
pub fn verification_receipt(
    inputs: &ReceiptInputs,
    core: ReceiptCore,
    target: SymbolVersion,
    dependencies: Vec<SymbolVersion>,
    config: VerificationConfig,
    status: ProofStatus,
    obligations: Vec<Obligation>,
    assumptions: Vec<Assumption>,
    verifier: VerifierInfo,
) -> VerificationReceipt {
    VerificationReceipt {
        receipt: ReceiptKind::Verification,
        schema: version::JSON_SCHEMA,
        toolchain: inputs.toolchain.clone(),
        verifier,
        revision: inputs.revision.clone(),
        core,
        target,
        dependencies: by_path(dependencies),
        inputs: inputs.to_json(),
        configuration: config,
        status,
        obligations,
        assumptions,
        produced_at: now_rfc3339(),
        duration_ms: 0,
    }
}

/// A test receipt (Section 8.2) for `report`: `tests` holds the
/// deterministic part of every record (no durations, no per-record seed or
/// receipt), `summary` the report's counts, and `status` is `tested` when at
/// least one test was selected and every selected test is `tested`, else
/// `failed` (an empty selection is a configuration failure, design line
/// 421, so it is never evidence that something was tested). `tested` is
/// ordered by display path and deduplicated by id; `produced_at` and
/// `duration_ms` as for [`verification_receipt`].
#[allow(clippy::too_many_arguments)] // the signature of spec/toolchain.md 1.8
pub fn test_receipt(
    inputs: &ReceiptInputs,
    core: ReceiptCore,
    report: &TestReport,
    tested: Vec<SymbolVersion>,
    config: TestRunConfig,
    fixtures: Vec<Fixture>,
    assumptions: Vec<Assumption>,
    environment: BTreeMap<String, String>,
) -> TestReceipt {
    let tests: Vec<TestReceiptEntry> = report
        .results
        .iter()
        .map(|record| TestReceiptEntry {
            symbol: record.symbol.clone(),
            qualified_name: record.qualified_name.clone(),
            kind: record.kind,
            status: record.status,
            property_seed: record.property_seed.clone(),
            cases: record.cases,
        })
        .collect();
    let all_tested = report.selected > 0
        && report.failed == 0
        && report.tested == report.selected
        && tests.iter().all(|t| t.status == TestStatus::Tested);
    TestReceipt {
        receipt: ReceiptKind::Test,
        schema: version::JSON_SCHEMA,
        toolchain: inputs.toolchain.clone(),
        revision: inputs.revision.clone(),
        inputs: inputs.to_json(),
        core,
        configuration: config,
        tested: by_path(tested),
        tests,
        summary: TestSummary {
            selected: report.selected,
            tested: report.tested,
            failed: report.failed,
        },
        fixtures,
        assumptions,
        environment,
        status: if all_tested {
            TestStatus::Tested
        } else {
            TestStatus::Failed
        },
        produced_at: now_rfc3339(),
        duration_ms: 0,
    }
}

/// Symbol versions in display-path order, one per id.
fn by_path(mut versions: Vec<SymbolVersion>) -> Vec<SymbolVersion> {
    versions.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.id.cmp(&b.id)));
    versions.dedup_by(|a, b| a.id == b.id);
    versions
}

/// The informational top-level fields every receipt carries and its digest
/// leaves out (Section 8).
const INFORMATIONAL: [&str; 2] = ["produced_at", "duration_ms"];

/// The receipt's name (Section 8): blake3 (64 hex) of its canonical JSON,
/// the receipt object with the informational top-level keys `produced_at`
/// and `duration_ms` removed, serialized with every object's keys sorted
/// (by their UTF-8 bytes) and no whitespace. Keys of the same names nested
/// deeper are part of the evidence and are kept. Strings and numbers are
/// written as `serde_json` writes them (integers digit for digit).
pub fn receipt_digest(receipt: &Value) -> Digest {
    let mut out = String::new();
    match receipt {
        Value::Object(map) => write_object(
            map.iter()
                .filter(|(key, _)| !INFORMATIONAL.contains(&key.as_str())),
            &mut out,
        ),
        other => write_canonical(other, &mut out),
    }
    digest(out.as_bytes())
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&serde_json::to_string(s).expect("a string serializes")),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => write_object(map.iter(), out),
    }
}

/// An object with its keys sorted explicitly, whatever order the map
/// iterates in (`serde_json`'s `preserve_order` feature would keep
/// insertion order).
fn write_object<'a>(entries: impl Iterator<Item = (&'a String, &'a Value)>, out: &mut String) {
    let mut entries: Vec<(&String, &Value)> = entries.collect();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    out.push('{');
    for (i, (key, value)) in entries.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(key).expect("a string serializes"));
        out.push(':');
        write_canonical(value, out);
    }
    out.push('}');
}

/// The current time as RFC 3339 UTC with whole seconds
/// (`2026-10-27T21:00:00Z`).
fn now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    rfc3339_utc(seconds)
}

/// Seconds since the Unix epoch as RFC 3339 UTC (proleptic Gregorian
/// calendar; the civil-from-days conversion of H. Hinnant).
fn rfc3339_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let of_day = seconds % 86_400;
    let (hour, minute, second) = (of_day / 3600, of_day % 3600 / 60, of_day % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::rfc3339_utc;

    #[test]
    fn rfc3339_of_known_instants() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(1_793_134_800), "2026-10-27T21:00:00Z");
        assert_eq!(rfc3339_utc(4_107_542_399), "2100-02-28T23:59:59Z");
        assert_eq!(rfc3339_utc(4_107_542_400), "2100-03-01T00:00:00Z");
        assert_eq!(rfc3339_utc(253_402_300_799), "9999-12-31T23:59:59Z");
    }
}
