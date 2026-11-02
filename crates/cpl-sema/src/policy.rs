//! Release policy: the `release_policy` stage (`spec/toolchain.md` 3.1,
//! 3.4, 7.2 `--release`; `spec/semantics.md` 5.5, 8.2, S36, S47, S63).
//!
//! Under `CheckConfig::release` every `@prove` declaration whose
//! verification status (`Checker::verification`, filled by the contract
//! stage) is `unknown` or `failed` is the error `verification_unsatisfied`
//! (S36 (c), S47 (b)) at the function's name (S63 (3)), with `symbol`,
//! `facet: "verification_policy"`, and `status`; `proved` and, from
//! Milestone 5, `conditional` satisfy the policy, and no manifest switch
//! weakens it (T36). In Milestone 1 every status is `unknown`, so every
//! `@prove` declaration fails a release check. Development mode reports
//! nothing here.
//!
//! The other release rules are reported where their facts are known: the
//! typer's hole stage (`crate::types`, its `holes` module) reports a
//! `fn`-body hole as `hole_in_release` in place of `hole` and raises a
//! `test`/`property`-body `hole` to a warning (S47, S64), and the driver
//! (`crate::check`) checks `cpl.lock` (toolchain 11.2, T37:
//! `project_lock_missing` when absent, `project_lock_stale` when it differs
//! from the tree), before resolution, so that a release check of a project
//! whose files fail to parse still reports its lock.

use crate::checker::{blame, diag, Checker, Stage};
use crate::codes;
use cpl_diag::Facet;
use serde_json::json;

/// Apply the release policy (see the module docs).
pub(crate) fn apply_policy(cx: &mut Checker<'_>) {
    if !cx.config.release {
        return;
    }
    let mut diags = Vec::new();
    let mut failed = Vec::new();
    for status in &cx.verification {
        let shown = match status.status {
            crate::check::ProofStatus::Unknown => "unknown",
            crate::check::ProofStatus::Failed => "failed",
            crate::check::ProofStatus::Proved | crate::check::ProofStatus::Conditional => continue,
        };
        let def = status.def;
        let name = cx.program.def(def).name.display();
        let reason = match status.reason.as_deref() {
            Some("verifier_unavailable") => " (no verifier is available in this toolchain)",
            _ => "",
        };
        let d = diag(
            codes::VERIFICATION_UNSATISFIED,
            Some(cx.decls[def.0 as usize].name_span),
            format!(
                "`{name}` is `@prove` but its verification status is {shown}{reason}; a release build requires its obligations to be proved"
            ),
        )
        .with_data("status", json!(shown));
        diags.push(blame(d, cx, def, Some(Facet::VerificationPolicy)));
        failed.push(def);
    }
    for def in failed {
        cx.fail(def);
    }
    cx.emit_all(Stage::ReleasePolicy, diags);
}
