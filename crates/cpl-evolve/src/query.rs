use crate::{error, Snapshot};
use cpl_diag::{Diagnostic, FileOrigin, StableId};
use cpl_hir::{DefId, DefKind, FnBody, Program};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Impact {
    pub changed: BTreeSet<StableId>,
    pub affected: BTreeSet<StableId>,
    pub tests: BTreeSet<StableId>,
    pub full_recheck: bool,
}

pub fn project_ids(p: &Program) -> BTreeSet<StableId> {
    p.defs()
        .filter(|d| p.module(d.module).origin != FileOrigin::Std)
        .map(|d| d.stable_id.clone())
        .collect()
}

/// Reverse closure over *all* resolved references, including contracts, types, constructors, and fields.
/// Every snapshot's graph is included so removal or rebinding cannot hide an old dependent; the
/// graphs are cached per snapshot.
pub fn affected(snapshots: &[&Snapshot], roots: &BTreeSet<StableId>) -> BTreeSet<StableId> {
    let graphs: Vec<_> = snapshots.iter().map(|s| s.dependents()).collect();
    let mut seen = roots.clone();
    let mut todo: Vec<_> = roots.iter().cloned().collect();
    while let Some(id) = todo.pop() {
        for graph in &graphs {
            for next in graph.get(&id).into_iter().flatten() {
                if seen.insert(next.clone()) {
                    todo.push(next.clone());
                }
            }
        }
    }
    seen
}

pub fn impact(before: &Snapshot, after: &Snapshot) -> Impact {
    let old = before.index();
    let new = after.index();
    let delta = cpl_index::semantic_diff(old.facet_map(), new.facet_map());
    let mut changed: BTreeSet<_> = delta.statements.into_iter().map(|d| d.id).collect();
    // Import changes, new bindings, and configuration may change meaning without changing any facet.
    let headers = |s: &Snapshot| -> BTreeMap<String, String> {
        s.checked
            .program
            .asts
            .iter()
            .filter(|(f, _)| s.project.source_map.file(*f).origin != FileOrigin::Std)
            .map(|(f, ast)| {
                // The header alone: cloning the module would copy every declaration first.
                let header = cpl_syntax::ast::Module {
                    span: ast.span,
                    trivia: ast.trivia.clone(),
                    docs: ast.docs.clone(),
                    header: ast.header.clone(),
                    uses: ast.uses.clone(),
                    declarations: Vec::new(),
                    trailing_comments: Vec::new(),
                };
                (
                    s.project.source_map.file(*f).path.clone(),
                    cpl_syntax::format_module(&header),
                )
            })
            .collect()
    };
    let full_recheck = headers(before) != headers(after)
        || before.tree.files.get("cpl.toml") != after.tree.files.get("cpl.toml")
        || before.tree.files.get("cpl.lock") != after.tree.files.get("cpl.lock")
        || before.project.std_digest() != after.project.std_digest();
    let p = &after.checked.program;
    let scope = if full_recheck {
        changed.extend(project_ids(&before.checked.program));
        changed.extend(project_ids(p));
        changed.clone()
    } else {
        affected(&[before, after], &changed)
    };
    let tests = p
        .defs()
        .filter(|d| {
            scope.contains(&d.stable_id)
                && matches!(d.kind, DefKind::Test(_) | DefKind::Property(_))
        })
        .map(|d| d.stable_id.clone())
        .collect();
    Impact {
        changed,
        affected: scope,
        tests,
        full_recheck,
    }
}

pub fn metadata(p: &Program, id: DefId) -> BTreeMap<String, Vec<String>> {
    [
        "intent",
        "preserve",
        "protocol",
        "transition",
        "message",
        "schema",
        "migration",
    ]
    .into_iter()
    .filter_map(|name| {
        let args = p.annotation_texts(id, name);
        (!args.is_empty()).then(|| (name.into(), args.into_iter().map(str::to_string).collect()))
    })
    .collect()
}

pub fn symbol(s: &Snapshot, id: DefId) -> Value {
    let p = &s.checked.program;
    let d = p.def(id);
    json!({"version": s.index().version(id), "metadata": metadata(p, id), "location": p.source_map.location(d.span)})
}

/// A deliberately small, exact query grammar; no natural-language guesswork or hidden external calls.
pub fn query(s: &Snapshot, expression: &str, base: Option<&Snapshot>) -> Result<Value, Diagnostic> {
    let p = &s.checked.program;
    let index = s.index();
    let expr = expression.trim();
    let all: Vec<_> = p
        .defs()
        .filter(|d| p.module(d.module).origin != FileOrigin::Std)
        .map(|d| d.id)
        .collect();
    if expr == "symbols" {
        return Ok(json!({"symbols": all.iter().map(|id| symbol(s, *id)).collect::<Vec<_>>()}));
    }
    if expr == "unproven contracts" {
        return Ok(
            json!({"obligations": all.iter().flat_map(|id| index.obligations(*id)).filter(|o| o.status != cpl_diag::json::ObligationStatus::Proved).collect::<Vec<_>>()}),
        );
    }
    for (word, annotation) in [
        ("protocols", "protocol"),
        ("migrations", "migration"),
        ("schemas", "schema"),
    ] {
        if expr == word {
            return Ok(
                json!({"symbols": all.iter().filter(|id| !p.annotation_texts(**id, annotation).is_empty()).map(|id| symbol(s, *id)).collect::<Vec<_>>()}),
            );
        }
    }
    if expr == "holes" {
        // Structured synthesis requests: each hole's expected type, bindings, and clauses.
        return Ok(json!({"holes": s.checked.holes.iter().map(|h| {
            let mut report = json!(index.hole_report_json(h));
            report["symbol"] = json!(p.def(h.def).stable_id);
            report["location"] = json!(p.source_map.location(h.span));
            report
        }).collect::<Vec<_>>()}));
    }
    if let Some(name) = expr.strip_prefix("protocol ") {
        return protocol(s, name.trim());
    }
    if expr == "changed" {
        return Ok(
            json!({"impact": impact(base.ok_or_else(|| error("query_base_required", "changed requires --since REV"))?, s)}),
        );
    }
    if let Some(effect) = expr
        .strip_prefix("functions with ")
        .and_then(|s| s.strip_suffix(" effects"))
    {
        let ids: Vec<_> = all
            .iter()
            .filter(|id| {
                p.def(**id).kind.as_fn().is_some_and(|f| {
                    f.effects
                        .declared
                        .iter()
                        .flatten()
                        .chain(f.effects.inferred.iter())
                        .any(|e| {
                            let param = f.using.iter().find(|u| u.local == e.param);
                            let cap = param.and_then(|u| {
                                if let cpl_hir::Ty::Capability(c) = u.ty {
                                    Some(p.display_path(c))
                                } else {
                                    None
                                }
                            });
                            e.op == effect
                                || param.is_some_and(|u| format!("{}.{}", u.name, e.op) == effect)
                                || cap.is_some_and(|c| {
                                    c == effect || format!("{c}.{}", e.op) == effect
                                })
                        })
                })
            })
            .map(|id| symbol(s, *id))
            .collect();
        return Ok(json!({"symbols": ids}));
    }
    for prefix in [
        "callers of ",
        "dependencies of ",
        "affected by ",
        "tests of ",
        "symbol ",
    ] {
        if let Some(target) = expr.strip_prefix(prefix) {
            let id = s.resolve(target)?;
            let ids = match prefix {
                "callers of " => index.callers(id),
                "tests of " => index.tests_for(id),
                "dependencies of " => index.dependencies(id).iter().map(|d| d.def).collect(),
                "affected by " => {
                    affected(&[s], &[p.def(id).stable_id.clone()].into_iter().collect())
                        .iter()
                        .filter_map(|id| p.by_stable_id(id))
                        .collect()
                }
                _ => {
                    let mut nodes = Vec::new();
                    let mut visit = |e: &cpl_hir::Expr| {
                        nodes.push(json!({"type": p.display_ty(&e.ty), "location": p.source_map.location(e.span)}))
                    };
                    match &p.def(id).kind {
                        DefKind::Fn(f) => {
                            for c in f.requires.iter().chain(&f.ensures) {
                                c.expr.walk(&mut visit);
                            }
                            if let FnBody::Block(b) = &f.body {
                                b.walk(&mut visit);
                            }
                        }
                        DefKind::Test(t) => t.body.walk(&mut visit),
                        DefKind::Property(t) => t.body.walk(&mut visit),
                        _ => (),
                    }
                    return Ok(
                        json!({"symbol": symbol(s, id), "syntax": p.ast(id), "typed_expressions": nodes,
                        "references": p.references_from(id).iter().map(|r| json!({"target": p.def(r.to).stable_id, "kind": r.kind, "interface": r.interface, "location": p.source_map.location(r.span)})).collect::<Vec<_>>(),
                        "obligations": index.obligations(id)}),
                    );
                }
            };
            return Ok(
                json!({"symbols": ids.into_iter().map(|id| symbol(s, id)).collect::<Vec<_>>()}),
            );
        }
    }
    Err(error("query_invalid", "expected symbols, symbol NAME, callers of NAME, dependencies of NAME, affected by NAME, tests of NAME, functions with EFFECT effects, unproven contracts, holes, protocols, protocol NAME, schemas, migrations, or changed"))
}

/// The checked state graph of one protocol: states with roles, and transitions with participants.
fn protocol(s: &Snapshot, name: &str) -> Result<Value, Diagnostic> {
    let p = &s.checked.program;
    let mut states = Vec::new();
    let mut transitions = Vec::new();
    for d in p
        .defs()
        .filter(|d| p.module(d.module).origin != FileOrigin::Std)
    {
        let a = p.annotation_texts(d.id, "protocol");
        if a.len() == 3 && a[0] == name {
            states.push(json!({"state": a[1], "role": a[2], "type": p.display_path(d.id), "symbol": d.stable_id}));
        }
        let t = p.annotation_texts(d.id, "transition");
        if t.len() == 3 && t[0] == name {
            let message = p.annotation_texts(d.id, "message");
            transitions.push(json!({"from": t[1], "to": t[2], "function": p.display_path(d.id), "symbol": d.stable_id,
                "sender": message.first(), "receiver": message.get(1)}));
        }
    }
    if states.is_empty() {
        return Err(error(
            "query_protocol_unknown",
            format!("no protocol named {name}"),
        ));
    }
    let roles = |wanted: &[&str]| -> Vec<Value> {
        states
            .iter()
            .filter(|st| wanted.contains(&st["role"].as_str().unwrap_or_default()))
            .map(|st| st["state"].clone())
            .collect()
    };
    let initial = roles(&["initial", "initial_terminal"]);
    let terminal = roles(&["terminal", "initial_terminal"]);
    // Informational: after a message, a non-terminal state from which its receiver never sends
    // may lack a response. One-way notifications are legitimate, so this is not a checker error.
    let unanswered: Vec<Value> = transitions
        .iter()
        .filter(|t| t["receiver"].is_string() && !terminal.contains(&t["to"]))
        .filter(|t| {
            let out: Vec<_> = transitions
                .iter()
                .filter(|u| u["from"] == t["to"])
                .collect();
            out.iter().any(|u| u["sender"].is_string())
                && !out.iter().any(|u| u["sender"] == t["receiver"])
        })
        .map(|t| json!({"transition": t["function"], "state": t["to"], "receiver": t["receiver"]}))
        .collect();
    Ok(
        json!({"protocol": name, "initial": initial, "terminal": terminal, "states": states,
        "transitions": transitions, "unanswered": unanswered}),
    )
}
