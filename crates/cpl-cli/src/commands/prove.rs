//! `cpl prove <symbol> [--json]` (`spec/toolchain.md` 7.2, 8.1; semantics
//! S36, S59; T29, T44).
//!
//! Loads and checks the project, resolves the symbol with the semantic
//! index, enumerates its proof obligations, and reports what the verifier
//! established (`cpl_index::Index::prove`): the `ProveOutput` with its
//! verification receipt (Section 8.1), and one `verification_result`
//! diagnostic (info, semantics S36) whose `receipt` is the receipt's
//! `core`. Milestone 1 has no verifier: every function's result is
//! `unknown`, every obligation `unknown` with `reason:
//! "verifier_unavailable"` (except the refinements the type checker
//! established, `proved` with `{"by": "literal"}`), and the receipt names
//! the verifier `{ "name": "none", "version": null }`. The receipt's
//! `duration_ms` is the time the obligations and the receipt took.
//!
//! | Outcome | Code | Exit |
//! | - | - | - |
//! | every obligation proved (`proved`, `conditional`; from Milestone 5) | `verification_result` | 0 |
//! | `unknown` or `failed` (always `unknown` in Milestone 1) | `verification_result`; envelope `status: unsuccessful`, `result` tells them apart (T44) | 1 |
//! | the project does not check | its errors and warnings; nothing is proved | 1 |
//! | the target is not a function | `usage_not_provable` | 2 |
//! | it names nothing, or a bare name names several declarations | `index_unknown_symbol`, `index_ambiguous_symbol` | 2 |
//!
//! Human output (stdout), the verification_result diagnostic on stderr:
//!
//! ```text
//! prove reader.pages.paginate (pgn_7c2d): unknown
//!   10 obligations: 0 proved, 0 failed, 10 unknown (verifier unavailable in cpl 0.1.0)
//!   receipt: 7e1c…  (blake3 of the receipt)
//! ```
//!
//! where the receipt line holds the whole `cpl_index::receipt_digest` of the
//! receipt, its name (Section 8).
//!
//! Readings: like `cpl run` and `cpl test`, `cpl prove` needs a project
//! that checks without errors (a verifier reasons about the checked
//! program, and an obligation of a definition that failed to check is not
//! well defined): with check errors it reports them (exit 1) and proves
//! nothing. An unresolved or ambiguous target is a usage error (exit 2),
//! as for `cpl run`, the other command that acts on one function: 7.4
//! gives `prove` exit 2 for a target that is not a function, and a name
//! that names nothing names no function.

use super::{check_diagnostics_for_execution, Ctx};
use crate::cli::ProveArgs;
use crate::output::{plural, Failure, Report};
use cpl_diag::json::{Digest, ObligationStatus, ProveOutput, Status};
use cpl_index::{Index, Target};
use cpl_sema::CheckConfig;
use std::time::Instant;

pub fn run(ctx: &Ctx, args: &ProveArgs) -> Result<Report<ProveOutput>, Failure> {
    let opened = ctx.session.open(false)?;
    let project = &opened.project;
    let sm = project.source_map.clone();
    let revision = Some(opened.revision.clone());
    let checked = cpl_sema::check_with(project, &CheckConfig::default());
    let mut diagnostics = check_diagnostics_for_execution(&checked.diagnostics);
    if checked.has_errors() {
        return Err(Failure::new(
            Status::Unsuccessful,
            revision,
            &diagnostics,
            Some(sm),
        ));
    }
    let index = Index::from_check(&checked, project, opened.revision.clone());
    let started = Instant::now();
    let proof = match index.prove(&Target::parse(&args.symbol)) {
        Ok(proof) => proof,
        Err(d) => {
            diagnostics.push(d);
            return Err(Failure::new(
                Status::Error,
                revision,
                &diagnostics,
                Some(sm),
            ));
        }
    };
    let status = proof.status();
    let mut output = proof.output;
    output.receipt.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    diagnostics.push(proof.diagnostic);
    let receipt = serde_json::to_value(&output.receipt).expect("a receipt serializes");
    let stdout = human(&output, &cpl_index::receipt_digest(&receipt));
    let json = diagnostics.iter().map(|d| d.to_json(&sm)).collect();
    Ok(Report::new("prove", revision, status, json, output)
        .with_stdout(stdout)
        .with_sources(sm))
}

/// The three lines of the human output (module docs).
fn human(output: &ProveOutput, digest: &Digest) -> String {
    let count = |status: ObligationStatus| {
        output
            .obligations
            .iter()
            .filter(|o| o.status == status)
            .count()
    };
    let receipt = &output.receipt;
    let verifier = if receipt.verifier.name == "none" {
        format!(
            " (verifier unavailable in cpl {})",
            receipt.toolchain.version
        )
    } else {
        String::new()
    };
    format!(
        "prove {} ({}): {}\n  {}: {} proved, {} failed, {} unknown{verifier}\n  receipt: {digest}  (blake3 of the receipt)\n",
        output.symbol.path,
        output.symbol.id,
        output.result.as_str(),
        plural(
            output.obligations.len() as u64,
            "obligation",
            "obligations"
        ),
        count(ObligationStatus::Proved),
        count(ObligationStatus::Failed),
        count(ObligationStatus::Unknown),
    )
}
