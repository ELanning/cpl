//! Hole reports (`spec/semantics.md` 8.1, 8.3, S47, S48, S64; toolchain
//! 1.6 `HoleReport`, 7.2 `cpl check`).
//!
//! Every hole of a function, test, or property body that has a fully
//! resolved expected type gets a [`HoleReport`] and, in development mode,
//! the `hole` diagnostic (severity `info`) carrying the S48 payload. Under
//! `--release` a function-body hole's single report is `hole_in_release`
//! (error, in place of `hole`), while a test- or property-body hole keeps
//! `hole`, raised to a warning (S47, S64); `CheckResult::holes` lists every
//! report in both modes.
//!
//! A hole without an expected type is `hole_type_unknown` (reported by the
//! typer at the statement's end) and a hole in a contract expression is
//! the contract stage's `hole_in_contract`; neither gets a report (S64).
//!
//! An expected type that is (or contains) the error type of S62, because
//! the position's type comes from a declaration or a form that failed
//! (`takes(?h)` with `fn takes(x: Nope)`, `let x: Nope = ?h`, an argument
//! of an unresolved callee), is still an expected type: S64 reports every
//! hole exactly once and names only the two error holes above as the ones
//! without a report, and the hole is no cascade (fixing the other error
//! leaves it unfinished). It is reported like any other hole, `hole` in
//! development and `hole_in_release` in a release build, with the unknown
//! parts of `expected_type` rendered `?` (the spelling 9.4 gives unknown
//! parts of a type in `cannot_infer_type`).
//!
//! The S48 `dependencies` object (`source_revision`, `toolchain`,
//! per-module content hashes) needs the Git checkout and the semantic
//! index, so the diagnostic leaves it to `cpl-cli`, which builds it from
//! [`HoleReport::dependencies`] (toolchain 1.6: "cpl-cli adds versions via
//! cpl-index").

use super::infer::render;
use super::typer::{HoleState, Owner, Typer};
use crate::check::{HoleBinding, HoleCapability, HoleOperation, HoleReport};
use crate::checker::{blame, diag, diag_with, Checker, Stage};
use crate::codes;
use cpl_diag::{Diagnostic, Facet, Severity};
use cpl_hir::{BindingKind, EffectRef, Ty};
use serde_json::json;

/// Where a hole stands: in the linked program (a `fn` body) or in a test
/// or property body, which release builds do not link (S47).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum HoleSite {
    FnBody,
    TestBody,
}

/// The reports and development-mode diagnostics of a typer's holes.
pub(crate) fn build(
    t: &Typer<'_, '_, '_>,
    site: HoleSite,
) -> Vec<(HoleReport, HoleSite, Diagnostic)> {
    let program = t.program;
    let mut out = Vec::new();
    for h in &t.holes {
        if h.state != HoleState::Pending {
            continue;
        }
        let expected = t.vars.zonk_final(&h.ty);
        let visible = t
            .tcx
            .cx
            .names
            .holes
            .get(&h.span)
            .cloned()
            .unwrap_or_default();
        let mut bindings = Vec::new();
        let mut capabilities = Vec::new();
        let fd = program.def(t.def).kind.as_fn();
        for id in visible {
            let Some(l) = t.locals.get(id.0 as usize) else {
                continue;
            };
            let ty = t.vars.zonk_final(&l.ty);
            if l.kind == BindingKind::Using {
                if let Ty::Capability(cap) = ty {
                    let ops = program
                        .def(cap)
                        .kind
                        .as_capability()
                        .map(|c| c.ops.as_slice())
                        .unwrap_or_default();
                    let operations = ops
                        .iter()
                        .map(|op| {
                            let params: Vec<String> = op
                                .params
                                .iter()
                                .map(|p| format!("{}: {}", p.name, program.display_ty(&p.ty)))
                                .collect();
                            let in_bound = fd
                                .and_then(|f| f.effects.declared.as_ref())
                                .is_some_and(|b| {
                                    b.contains(&EffectRef {
                                        param: id,
                                        op: op.name.clone(),
                                    })
                                });
                            HoleOperation {
                                name: op.name.clone(),
                                signature: format!(
                                    "fn {}({}) -> {}",
                                    op.name,
                                    params.join(", "),
                                    program.display_ty(&op.ret)
                                ),
                                in_bound,
                            }
                        })
                        .collect();
                    capabilities.push(HoleCapability {
                        parameter: l.name.clone(),
                        capability: cap,
                        operations,
                    });
                }
            }
            bindings.push(HoleBinding {
                name: l.name.clone(),
                ty,
                kind: l.kind,
                mutable: l.mutable,
            });
        }
        let (requires, ensures) = match (t.owner, fd) {
            (Owner::Fn, Some(f)) => (
                f.requires.iter().map(|c| c.text.clone()).collect(),
                f.ensures.iter().map(|c| c.text.clone()).collect(),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        let report = HoleReport {
            name: h.name.clone(),
            span: h.span,
            def: t.def,
            expected,
            bindings,
            capabilities,
            requires,
            ensures,
            invariants: h.invariants.clone(),
            dependencies: Vec::new(),
        };
        let d = diagnostic(t, &report);
        out.push((report, site, d));
    }
    out
}

/// The development-mode `hole` diagnostic of a report (S48).
fn diagnostic(t: &Typer<'_, '_, '_>, r: &HoleReport) -> Diagnostic {
    let program = t.program;
    let ty = |ty: &Ty| render(program, ty, t.generics, false);
    let bindings: Vec<serde_json::Value> = r
        .bindings
        .iter()
        .map(|b| {
            let mut v = json!({ "name": b.name, "type": ty(&b.ty), "kind": b.kind });
            if b.mutable {
                v["mutable"] = json!(true);
            }
            v
        })
        .collect();
    let capabilities: Vec<serde_json::Value> = r
        .capabilities
        .iter()
        .map(|c| {
            json!({
                "parameter": c.parameter,
                "capability": program.display_path(c.capability),
                "operations": c.operations.iter().map(|o| json!({
                    "name": o.name,
                    "signature": o.signature,
                    "in_bound": o.in_bound,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let d = diag(
        codes::HOLE,
        Some(r.span),
        format!("unfinished expression ?{}", r.name),
    )
    .with_data("hole", json!(r.name))
    .with_data("expected_type", json!(ty(&r.expected)))
    .with_data("bindings", json!(bindings))
    .with_data("capabilities", json!(capabilities))
    .with_data("requires", json!(r.requires))
    .with_data("ensures", json!(r.ensures))
    .with_data("invariants", json!(r.invariants));
    let mode = program.function_mode(r.def);
    let d = if mode != cpl_hir::FunctionMode::Inferred {
        let mut d = d.with_data("function_mode", json!(mode.prefix()));
        if let Some(sig) = program.machine_signature(r.def) {
            d = d
                .with_data("machine_state", json!(program.display_ty(&sig.state)))
                .with_data("machine_event", json!(program.display_ty(&sig.event)));
            if let Some(command) = sig.command {
                d = d.with_data("machine_command", json!(program.display_ty(&command)));
            }
        }
        d
    } else {
        d
    };
    blame(d, t.tcx.cx, r.def, Some(Facet::Body))
}

/// Record the reports in `CheckResult::holes` (in source order) and emit
/// their diagnostics for the mode of the check (S47, S64).
pub(crate) fn report(cx: &mut Checker<'_>, mut holes: Vec<(HoleReport, HoleSite, Diagnostic)>) {
    holes.sort_by_key(|(r, _, _)| (r.span.file, r.span.start));
    let release = cx.config.release;
    for (report, site, d) in holes {
        if release && site == HoleSite::FnBody {
            let expected = d.data.get("expected_type").cloned().unwrap_or(json!(""));
            let mut e = diag_with(
                codes::HOLE_IN_RELEASE,
                Severity::Error,
                Some(report.span),
                format!(
                    "unfinished expression ?{}: a release build rejects holes in the linked program",
                    report.name
                ),
            )
            .with_data("hole", json!(report.name))
            .with_data("expected_type", expected);
            e = blame(e, cx, report.def, Some(Facet::Body));
            cx.emit(Stage::ReleasePolicy, e);
            cx.fail(report.def);
        } else {
            let mut d = d;
            if release {
                d.severity = Severity::Warning;
            }
            cx.emit(Stage::Hole, d);
        }
        cx.holes.push(report);
    }
}
