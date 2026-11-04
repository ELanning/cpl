//! `concurrent` blocks (`spec/concurrency.md`): every task must be independent of the order of its
//! siblings. A task cannot assign a variable declared outside it, return, or propagate out of the
//! block, and two tasks cannot perform conflicting operations through one `using` handle: two
//! operations of one handle conflict unless the capability declares both `@read_only`.
//!
//! | Rule | Diagnostic |
//! | - | - |
//! | a task assigns an outer variable, contains `return`, or uses `?` | `concurrent_unsafe` |
//! | two tasks' footprints share a handle with an operation that is not read-only | `concurrent_conflict` |
//! | an `@read_only` argument names no operation of its capability | `read_only_unknown_operation` |
//!
//! The footprint of a task is [`cpl_hir::Program::footprint`]: its operation calls and the
//! declared bounds of the functions it passes handles to. Distinct handles are distinct resources
//! here; a host that binds two parameters to one resource is checked when the block runs.

use crate::bodies::{visit_def, Site, Visitor};
use crate::checker::{blame, diag, Checker, Stage};
use crate::codes;
use cpl_diag::{Diagnostic, Facet};
use cpl_hir::{DefId, DefKind, EffectRef, ExprKind, FnDef, Program, Stmt, Task, Ty};
use serde_json::json;
use std::collections::BTreeSet;

pub(crate) fn check(cx: &mut Checker<'_>) {
    let p = &cx.program;
    let mut found: Vec<(DefId, Diagnostic, Facet)> = Vec::new();
    for def in p.defs() {
        if let DefKind::Capability(c) = &def.kind {
            for op in p.annotation_texts(def.id, "read_only") {
                if !c.ops.iter().any(|o| o.name == op) {
                    let d = diag(
                        codes::READ_ONLY_UNKNOWN_OPERATION,
                        Some(def.span),
                        format!("`@read_only` names `{op}`, which is not an operation of this capability"),
                    )
                    .with_data("operation", json!(op));
                    found.push((def.id, d, Facet::Signature));
                }
            }
        }
    }
    for def in p.defs() {
        struct Blocks<'h>(Vec<&'h [Task]>);
        impl<'h> Visitor<'h> for Blocks<'h> {
            fn stmt(&mut self, s: &'h Stmt, _: Site<'h>) {
                if let Stmt::Concurrent(tasks) = s {
                    self.0.push(tasks);
                }
            }
        }
        let mut blocks = Blocks(Vec::new());
        visit_def(def, &mut blocks);
        for tasks in blocks.0 {
            for d in safety(tasks) {
                found.push((def.id, d, Facet::Body));
            }
            if let Some(f) = def.kind.as_fn() {
                for d in conflicts(p, f, tasks) {
                    found.push((def.id, d, Facet::Effects));
                }
            }
        }
    }
    for (def, d, facet) in found {
        let d = blame(d, cx, def, Some(facet));
        if d.is_error() {
            cx.fail(def);
        }
        cx.emit(Stage::Effect, d);
    }
}

/// `concurrent_unsafe` for tasks whose result could depend on their order.
fn safety(tasks: &[Task]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for task in tasks {
        let mut declared = BTreeSet::new();
        task.stmt.walk_stmts(&mut |s| {
            if let Stmt::Let { local, .. } = s {
                declared.insert(*local);
            }
        });
        let mut why = None;
        task.stmt.walk_stmts(&mut |s| match s {
            Stmt::Assign { local, .. } if !declared.contains(local) => {
                why.get_or_insert("assigns a variable declared outside it");
            }
            Stmt::Return(_) => {
                why.get_or_insert("returns from the enclosing function");
            }
            _ => {}
        });
        task.stmt.walk(&mut |e| {
            if matches!(e.kind, ExprKind::Propagate(_)) {
                why.get_or_insert("propagates an error out of the block with `?`");
            }
        });
        if let Some(why) = why {
            out.push(diag(
                codes::CONCURRENT_UNSAFE,
                Some(task.span),
                format!("a concurrent task cannot depend on the order of its siblings, but this task {why}; compute a value and use it after the block"),
            ));
        }
    }
    out
}

/// `concurrent_conflict` for each pair of tasks sharing a handle with a non-read-only operation.
fn conflicts(p: &Program, f: &FnDef, tasks: &[Task]) -> Vec<Diagnostic> {
    let capability = |e: &EffectRef| {
        f.using
            .iter()
            .find(|u| u.local == e.param)
            .and_then(|u| match u.ty {
                Ty::Capability(c) => Some(c),
                _ => None,
            })
    };
    let commute = |a: &EffectRef, b: &EffectRef| {
        capability(a).is_some_and(|c| p.read_only(c, &a.op) && p.read_only(c, &b.op))
    };
    let prints: Vec<BTreeSet<EffectRef>> = tasks.iter().map(|t| p.footprint(&t.stmt)).collect();
    let mut out = Vec::new();
    for j in 1..tasks.len() {
        for i in 0..j {
            let clashes: Vec<String> = prints[i]
                .iter()
                .flat_map(|a| {
                    prints[j]
                        .iter()
                        .filter(move |b| b.param == a.param && !commute(a, b))
                        .map(move |b| {
                            format!("{} with {}", f.display_effect(a), f.display_effect(b))
                        })
                })
                .collect();
            if clashes.is_empty() {
                continue;
            }
            out.push(
                diag(
                    codes::CONCURRENT_CONFLICT,
                    Some(tasks[j].span),
                    format!(
                        "tasks {} and {} of this concurrent block may conflict through one handle ({}); run them in separate statements to order them, or declare operations that only observe the resource `@read_only`",
                        i + 1,
                        j + 1,
                        clashes.join(", ")
                    ),
                )
                .with_data("tasks", json!([i + 1, j + 1]))
                .with_data("operations", json!(clashes)),
            );
        }
    }
    out
}
