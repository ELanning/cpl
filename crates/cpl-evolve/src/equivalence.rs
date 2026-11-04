//! Behavior preservation and measured improvements of a change (`spec/evolution.md`, change
//! contracts).
//!
//! `preserve_behavior` holds when every function the change modifies behaves like its base
//! version on every input of the base version's domain. Each such function is compared by the
//! equivalence prover (`cpl_prove::equivalent`), with every function that is unchanged together
//! with everything it calls shared between the two sides; inputs on which the prover could not
//! rule out a difference are run on both versions, so a difference is reported only with inputs
//! that show it.
//!
//! An `improve` requirement is measured: both versions run on inputs of growing size, counting
//! interpreter steps (function activations and loop iterations), and the requirement is
//! `measured` when the new version is never slower and its advantage grows with the input size.

use crate::differential::{self, Outcome, Rng};
use crate::verify::VerifyConfig;
use crate::Snapshot;
use cpl_diag::{Diagnostic, Facet, FileOrigin, StableId};
use cpl_hir::{DefId, DefKind, FnBody, Program, RefKind, Ty};
use num_bigint::BigInt;
use serde_json::{json, Value as Json};
use std::collections::BTreeSet;

const EXECUTABLE: [Facet; 5] = [
    Facet::Name,
    Facet::Signature,
    Facet::Contract,
    Facet::Effects,
    Facet::Body,
];

/// The identities of a program's definitions and the identities each one calls.
fn callees(p: &Program, def: DefId) -> Vec<StableId> {
    let mut out: Vec<StableId> = p
        .references_from(def)
        .iter()
        .filter(|r| r.kind == RefKind::Call || r.kind == RefKind::CapabilityOp)
        .map(|r| p.def(r.to).stable_id.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// What the change modified, and what it left identical on both sides.
pub struct Delta {
    /// Functions present on both sides whose own text or resolution differs.
    pub changed: Vec<StableId>,
    /// Definitions identical on both sides, together with everything they call.
    pub same: BTreeSet<StableId>,
    /// Public functions the change removed.
    pub removed: Vec<StableId>,
    /// Types, refinements, and capabilities whose definition differs.
    pub types: Vec<StableId>,
}

pub fn delta(base: &Snapshot, candidate: &Snapshot) -> Delta {
    let (old, new) = (&base.checked.program, &candidate.checked.program);
    let (old_index, new_index) = (base.index(), candidate.index());
    let (old_map, new_map) = (old_index.facet_map(), new_index.facet_map());
    let std = |p: &Program, d: DefId| p.module(p.def(d).module).origin == FileOrigin::Std;
    let mut local = BTreeSet::new();
    let mut changed = Vec::new();
    let mut types = Vec::new();
    for d in new.defs() {
        let Some(o) = old.by_stable_id(&d.stable_id) else {
            continue;
        };
        let facets = if std(new, d.id) && std(old, o) {
            true
        } else {
            match (old_map.get(&d.stable_id), new_map.get(&d.stable_id)) {
                (Some(a), Some(b)) => EXECUTABLE.iter().all(|f| a.facet(*f) == b.facet(*f)),
                _ => false,
            }
        };
        if facets && callees(old, o) == callees(new, d.id) {
            local.insert(d.stable_id.clone());
        } else if d.kind.is_test() {
        } else if matches!(d.kind, DefKind::Fn(_)) {
            changed.push(d.stable_id.clone());
        } else {
            types.push(d.stable_id.clone());
        }
    }
    // Identical on both sides: unchanged, and so is everything it calls.
    let mut same = local;
    loop {
        let drop: Vec<StableId> = same
            .iter()
            .filter(|id| {
                new.by_stable_id(id)
                    .is_some_and(|d| callees(new, d).iter().any(|c| !same.contains(c)))
            })
            .cloned()
            .collect();
        if drop.is_empty() {
            break;
        }
        for id in drop {
            same.remove(&id);
        }
    }
    let removed = old
        .defs()
        .filter(|d| !std(old, d.id) && d.is_pub && matches!(d.kind, DefKind::Fn(_)))
        .filter(|d| new.by_stable_id(&d.stable_id).is_none())
        .map(|d| d.stable_id.clone())
        .collect();
    Delta {
        changed,
        same,
        removed,
        types,
    }
}

/// The `behavior` obligation of `preserve_behavior`.
pub fn preserve_behavior(
    base: &Snapshot,
    candidate: &Snapshot,
    config: &VerifyConfig,
) -> Result<Json, Diagnostic> {
    let delta = delta(base, candidate);
    let (old, new) = (&base.checked.program, &candidate.checked.program);
    let mut functions = Vec::new();
    for id in &delta.removed {
        functions.push(json!({"symbol": id, "status": "failed", "reason": "removed"}));
    }
    for id in &delta.types {
        functions.push(json!({"symbol": id, "status": "unknown", "reason": "type_changed"}));
    }
    for id in &delta.changed {
        let (Some(o), Some(n)) = (old.by_stable_id(id), new.by_stable_id(id)) else {
            continue;
        };
        let same = |s: &StableId| delta.same.contains(s);
        let e = cpl_prove::equivalent(old, o, new, n, &same);
        let mut record = json!({"symbol": id, "status": e.status.as_str()});
        match e.status {
            cpl_prove::Status::Proved => record["evidence"] = e.evidence,
            cpl_prove::Status::Failed => {
                record["evidence"] = e.evidence;
                record["evidence"]["by"] = json!("symbolic_equivalence");
            }
            cpl_prove::Status::Unknown => {
                record["reason"] = json!(e.reason);
                // Inputs on which the versions may differ: run both to decide.
                if let Some(difference) = confirm(base, candidate, id, o, &e.candidates, config)? {
                    record["status"] = json!("failed");
                    record["evidence"] = difference;
                    record.as_object_mut().map(|m| m.remove("reason"));
                }
            }
        }
        functions.push(record);
    }
    let status = if functions.iter().any(|f| f["status"] == "failed") {
        "failed"
    } else if functions.iter().all(|f| f["status"] == "proved") {
        "proved"
    } else {
        "unknown"
    };
    let mut out = json!({"requirement": "behavior", "status": status, "functions": functions});
    if status == "proved" && delta.changed.is_empty() && delta.types.is_empty() {
        out["reason"] = json!("unchanged_executable_facets");
    }
    Ok(out)
}

/// Run `candidates` (inputs by parameter name) on both versions; the first input on which they
/// differ, with both outcomes.
fn confirm(
    base: &Snapshot,
    candidate: &Snapshot,
    id: &StableId,
    def: DefId,
    candidates: &[Json],
    config: &VerifyConfig,
) -> Result<Option<Json>, Diagnostic> {
    let p = &base.checked.program;
    let Some(f) = p.def(def).kind.as_fn() else {
        return Ok(None);
    };
    if !f.generics.is_empty() || !f.using.is_empty() {
        return Ok(None);
    }
    cpl_eval::on_eval_stack(|| {
        for inputs in candidates {
            let Some(args) = f
                .params
                .iter()
                .map(|param| inputs.get(&param.name).filter(|v| !v.is_null()).cloned())
                .collect::<Option<Vec<Json>>>()
            else {
                continue;
            };
            // A version that runs out of a resource on a candidate shows no difference there.
            let before = differential::bounded(base, id, &args, config)?;
            if before == Outcome::Outside || before.exhausted() {
                continue;
            }
            let after = differential::bounded(candidate, id, &args, config)?;
            if !after.exhausted() && !before.same(&after) {
                return Ok(Some(json!({
                    "by": "execution",
                    "inputs": inputs,
                    "before": before.json(),
                    "after": after.json(),
                })));
            }
        }
        Ok(None)
    })
}

// ---- measured improvements ----------------------------------------------------------------

/// The input sizes an improvement is measured at.
const SIZES: [u32; 7] = [1, 2, 4, 8, 16, 32, 64];
/// Inputs per size.
const SAMPLES: u32 = 3;
/// The step budget of one run; a version that exceeds it at some size is slower there than any
/// version that does not.
const STEP_BUDGET: u64 = 2_000_000;

/// A value of `ty` whose size is `size`: an integer of that magnitude, a text, bytes, or list of
/// that length, and structs, enums, and wrappers of such parts.
fn scaled(p: &Program, ty: &Ty, rng: &mut Rng, size: u32, depth: u32) -> Option<cpl_eval::Value> {
    use cpl_eval::Value;
    if depth > 6 {
        return differential::arbitrary(p, ty, rng, 0, depth);
    }
    Some(match ty {
        Ty::Int => Value::Int(BigInt::from(size)),
        Ty::Text => Value::text(&"ab".repeat(size as usize)[..size as usize]),
        Ty::Bytes => Value::bytes(&vec![7u8; size as usize]),
        Ty::List(element) => {
            let items = (0..size)
                .map(|i| {
                    differential::arbitrary(p, element, rng, size.min(16).max(i % 16), depth + 1)
                })
                .collect::<Option<Vec<_>>>()?;
            Value::list(items)
        }
        Ty::Wrapper(def) => {
            let inner = &p.def(*def).kind.as_wrapper()?.inner;
            Value::wrap(*def, scaled(p, inner, rng, size, depth + 1)?)
        }
        Ty::Refined(_, def) => {
            let base = cpl_eval::types::refinement_base(p, *def)?.clone();
            scaled(p, &base, rng, size, depth + 1)?
        }
        Ty::Adt(def, args) => match &p.def(*def).kind {
            DefKind::Struct(s) => {
                let fields = s
                    .fields
                    .iter()
                    .map(|f| {
                        let ty = cpl_eval::types::instantiate(&f.ty, args);
                        scaled(p, &ty, rng, size, depth + 1)
                    })
                    .collect::<Option<Vec<_>>>()?;
                Value::new_struct(*def, fields)
            }
            _ => differential::arbitrary(p, ty, rng, size, depth)?,
        },
        _ => differential::arbitrary(p, ty, rng, size, depth)?,
    })
}

/// One `improve` requirement: `measure` of function `id`.
pub fn measure(
    base: &Snapshot,
    candidate: &Snapshot,
    id: &StableId,
    measure: &str,
    config: &VerifyConfig,
) -> Result<Json, Diagnostic> {
    let record = |status: &str, extra: Json| {
        let mut r = json!({"requirement": measure, "symbol": id, "status": status});
        if let (Some(m), Json::Object(e)) = (r.as_object_mut(), extra) {
            m.extend(e);
        }
        r
    };
    if measure != "time_complexity" && measure != "steps" {
        return Ok(record("unknown", json!({"reason": "unsupported_measure"})));
    }
    let p = &base.checked.program;
    let Some(def) = p.by_stable_id(id) else {
        return Ok(record("unknown", json!({"reason": "not_in_base"})));
    };
    let Some(f) = p
        .def(def)
        .kind
        .as_fn()
        .filter(|f| matches!(f.body, FnBody::Block(_)))
    else {
        return Ok(record("unknown", json!({"reason": "not_a_function"})));
    };
    if !f.generics.is_empty() || !f.using.is_empty() {
        return Ok(record("unknown", json!({"reason": "not_runnable"})));
    }
    let mut rows = Vec::new();
    cpl_eval::on_eval_stack(|| -> Result<(), Diagnostic> {
        for size in SIZES {
            let (mut old, mut new, mut inside) = (Some(0u64), Some(0u64), 0);
            for sample in 0..SAMPLES {
                let mut rng = Rng::new(config.seed, id, size * SAMPLES + sample);
                let Some(values) = f
                    .params
                    .iter()
                    .map(|param| scaled(p, &param.ty, &mut rng, size, 0))
                    .collect::<Option<Vec<_>>>()
                else {
                    return Ok(());
                };
                let args: Vec<Json> = values
                    .iter()
                    .zip(&f.params)
                    .map(|(v, param)| cpl_eval::value_to_json(p, v, &param.ty))
                    .collect();
                let (before, a) =
                    differential::run_counting(base, id, &args, config, Some(STEP_BUDGET))?;
                if before == Outcome::Outside {
                    continue;
                }
                let (after, b) =
                    differential::run_counting(candidate, id, &args, config, Some(STEP_BUDGET))?;
                // A version out of budget did not finish: only the other's outcome is known.
                if a.is_some() && b.is_some() && !before.same(&after) {
                    rows.push(
                        json!({"size": size, "behavior_changed": true, "arguments": args,
                        "before": before.json(), "after": after.json()}),
                    );
                    return Ok(());
                }
                inside += 1;
                old = old.zip(a).map(|(x, y)| x + y);
                new = new.zip(b).map(|(x, y)| x + y);
            }
            if inside > 0 {
                rows.push(json!({"size": size, "inputs": inside, "before": old, "after": new}));
            }
        }
        Ok(())
    })?;
    if let Some(changed) = rows.iter().find(|r| r.get("behavior_changed").is_some()) {
        return Ok(record(
            "failed",
            json!({"reason": "behavior_changed", "evidence": changed}),
        ));
    }
    // A size at which both versions exhaust the budget compares nothing.
    let usable: Vec<&Json> = rows
        .iter()
        .filter(|r| !(r["before"].is_null() && r["after"].is_null()))
        .collect();
    if usable.len() < 2 {
        return Ok(record(
            "unknown",
            json!({"reason": "not_measurable", "measurements": rows}),
        ));
    }
    // Steps per size; an exhausted budget counts as more than the budget.
    let steps = |r: &Json, key: &str| r[key].as_u64().map_or(f64::INFINITY, |s| s as f64);
    let worse = usable
        .iter()
        .find(|r| steps(r, "after") > steps(r, "before"));
    let ratio = |r: &Json| steps(r, "before") / steps(r, "after").max(1.0);
    let (first, last) = (ratio(usable[0]), ratio(usable[usable.len() - 1]));
    // The empirical exponent of the steps' growth between the two largest sizes, where fixed
    // costs matter least.
    let exponent = |key: &str| -> Json {
        let (a, b) = (usable[usable.len() - 2], usable[usable.len() - 1]);
        let (sa, sb) = (
            a["size"].as_f64().unwrap_or(1.0),
            b["size"].as_f64().unwrap_or(1.0),
        );
        let (ya, yb) = (steps(a, key).max(1.0), steps(b, key).max(1.0));
        if yb.is_infinite() || ya.is_infinite() {
            return Json::Null;
        }
        let k = (yb / ya).ln() / (sb / sa).ln();
        json!((k * 100.0).round() / 100.0)
    };
    let evidence = json!({
        "measurements": rows,
        "growth_exponent": {"before": exponent("before"), "after": exponent("after")},
        "step_budget": STEP_BUDGET,
    });
    let improved = match measure {
        // The advantage must grow with the input: a constant factor is not a complexity change.
        "time_complexity" => last > 1.0 && last >= 2.0 * first.max(1.0),
        _ => last > 1.0,
    };
    let status = if worse.is_some() || !improved {
        "failed"
    } else {
        "measured"
    };
    let mut out = record(status, json!({"evidence": evidence}));
    if let Some(r) = worse {
        out["reason"] = json!(format!("slower at size {}", r["size"]));
    } else if !improved {
        out["reason"] = json!(if measure == "time_complexity" {
            "no_asymptotic_improvement"
        } else {
            "no_improvement"
        });
    }
    Ok(out)
}

/// The functions an unscoped `improve` entry applies to: those the change modified.
pub fn changed_functions(base: &Snapshot, candidate: &Snapshot) -> Vec<StableId> {
    delta(base, candidate).changed
}
