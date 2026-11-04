use crate::verify::VerifyConfig;
use crate::{error, Snapshot};
use cpl_diag::Diagnostic;
use cpl_hir::{DefId, DefKind, Ty};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoredValue {
    pub schema: String,
    pub version: String,
    pub schema_digest: String,
    pub value: Value,
}
fn ty(s: &Snapshot, id: DefId) -> Result<Ty, Diagnostic> {
    match &s.checked.program.def(id).kind {
        DefKind::Wrapper(_) => Ok(Ty::Wrapper(id)),
        DefKind::Struct(t) if t.generics.is_empty() => Ok(Ty::Adt(id, vec![])),
        DefKind::Enum(t) if t.generics.is_empty() => Ok(Ty::Adt(id, vec![])),
        _ => Err(error(
            "schema_type_invalid",
            "a persisted schema must be a monomorphic nominal value type",
        )),
    }
}

/// The schema and all predicates and types it depends on, including std and stable identities.
pub fn fingerprint(s: &Snapshot, id: DefId) -> String {
    let p = &s.checked.program;
    let index = s.index();
    let mut seen = BTreeSet::from([id]);
    let mut todo = vec![id];
    while let Some(id) = todo.pop() {
        for r in p.references_from(id) {
            if seen.insert(r.to) {
                todo.push(r.to);
            }
        }
    }
    let mut versions: Vec<_> = seen.iter().map(|id| index.version(*id)).collect();
    versions.sort_by_key(|v| &v.id);
    blake3::hash(
        &serde_json::to_vec(
            &json!({"format": 1, "std": s.project.std_digest(), "types": versions}),
        )
        .unwrap(),
    )
    .to_hex()
    .to_string()
}

pub fn pack(
    s: &Snapshot,
    symbol: &str,
    value: &Value,
    config: &VerifyConfig,
) -> Result<StoredValue, Diagnostic> {
    config.validate()?;
    cpl_eval::on_eval_stack(|| {
        let id = s.resolve(symbol)?;
        let p = &s.checked.program;
        let annotation = p.annotation_texts(id, "schema");
        if annotation.len() != 2 {
            return Err(error(
                "schema_annotation_required",
                "select a type annotated with @schema(family, version)",
            ));
        }
        let ty = ty(s, id)?;
        let value = cpl_eval::value_from_json(p, value, &ty)
            .map_err(|e| error("schema_value_invalid", e.to_string()))?;
        let mut engine = cpl_eval::Interpreter::new(p, config.eval());
        for position in cpl_eval::json::refined_positions(p, &value, &ty) {
            if !engine
                .satisfies_refinement(position.refined, &position.value)
                .map_err(|e| e.to_diagnostic(p))?
            {
                return Err(error(
                    "schema_invariant_failed",
                    format!("schema invariant failed at {}", position.pointer),
                ));
            }
        }
        Ok(StoredValue {
            schema: annotation[0].into(),
            version: annotation[1].into(),
            schema_digest: fingerprint(s, id),
            value: cpl_eval::value_to_json(p, &value, &ty),
        })
    })
}

/// One typed migration, with a versioned input and validated output. The source value is never modified.
pub fn migrate(
    s: &Snapshot,
    function: &str,
    input: &StoredValue,
    config: &VerifyConfig,
) -> Result<StoredValue, Diagnostic> {
    config.validate()?;
    let id = s.resolve(function)?;
    let p = &s.checked.program;
    let annotation = p.annotation_texts(id, "migration");
    if annotation.len() != 3 {
        return Err(error(
            "migration_annotation_required",
            "select a checked @migration function",
        ));
    }
    let f = p.def(id).kind.as_fn().unwrap();
    let nominal = |t: &Ty| match t {
        Ty::Adt(id, _) | Ty::Wrapper(id) => *id,
        _ => unreachable!("checked migration endpoint"),
    };
    let from = nominal(&f.params[0].ty);
    let to = nominal(&f.ret);
    if annotation[0] != input.schema
        || annotation[1] != input.version
        || fingerprint(s, from) != input.schema_digest
    {
        return Err(error(
            "migration_source_mismatch",
            "stored data does not match the migration's schema, version, or schema fingerprint",
        ));
    }
    let result = cpl_eval::run_function(
        p,
        &p.symbol_path(id),
        &[cpl_eval::RunArg::Json(input.value.clone())],
        &config.eval(),
    )
    .map_err(|e| error("migration_input_invalid", e.to_string()))?;
    let output = result.value.map_err(|e| e.to_diagnostic(p))?;
    pack(
        s,
        &p.display_path(to),
        &cpl_eval::value_to_json(p, &output, &f.ret),
        config,
    )
}

/// Migrate through the shortest chain of declared migrations from the stored version to `version`.
/// Every step validates its output, so each intermediate value satisfies its own schema.
pub fn migrate_to(
    s: &Snapshot,
    input: &StoredValue,
    version: &str,
    config: &VerifyConfig,
) -> Result<(StoredValue, Vec<String>), Diagnostic> {
    let p = &s.checked.program;
    let edges: Vec<(String, String, DefId)> = p
        .defs()
        .filter_map(|d| {
            let a = p.annotation_texts(d.id, "migration");
            (a.len() == 3 && a[0] == input.schema)
                .then(|| (a[1].to_string(), a[2].to_string(), d.id))
        })
        .collect();
    let pairs: Vec<_> = edges
        .iter()
        .map(|(f, t, _)| (f.clone(), t.clone()))
        .collect();
    let chain = path(&pairs, &input.version, version).ok_or_else(|| {
        error(
            "migration_path_missing",
            format!(
                "no migration path for schema {} from {} to {version}",
                input.schema, input.version
            ),
        )
    })?;
    let mut value = input.clone();
    let mut steps = Vec::new();
    for pair in chain.windows(2) {
        let (_, _, id) = edges
            .iter()
            .find(|(f, t, _)| *f == pair[0] && *t == pair[1])
            .expect("the path follows declared edges");
        let name = p.display_path(*id);
        value = migrate(s, &name, &value, config)?;
        steps.push(name);
    }
    if steps.is_empty() {
        // Already at the requested version: still validate shape, invariants, and fingerprint.
        let schema = p
            .defs()
            .find(|d| p.annotation_texts(d.id, "schema") == [input.schema.as_str(), version])
            .ok_or_else(|| {
                error(
                    "schema_unknown",
                    "the stored schema version is not declared",
                )
            })?;
        if fingerprint(s, schema.id) != input.schema_digest {
            return Err(error(
                "migration_source_mismatch",
                "stored data does not match the schema fingerprint",
            ));
        }
        value = pack(s, &p.display_path(schema.id), &input.value, config)?;
    }
    Ok((value, steps))
}

/// Published versions are immutable. Add a new nominal version and a directed migration path instead.
pub fn require_migrations(before: &Snapshot, after: &Snapshot) -> Result<(), Diagnostic> {
    match schema_issues(before, after).into_iter().next() {
        None => Ok(()),
        Some(issue) => Err(error(
            if issue["status"] == "changed_in_place" {
                "schema_version_changed"
            } else {
                "migration_required"
            },
            issue["message"].as_str().unwrap_or_default().to_string(),
        )
        .with_data("schema", issue)),
    }
}

/// Every persisted-data incompatibility between two snapshots: a schema version that disappeared
/// or changed in place, and a new version of an existing family that some older version cannot
/// reach through declared migrations.
pub fn schema_issues(before: &Snapshot, after: &Snapshot) -> Vec<Value> {
    let schemas = |s: &Snapshot| -> BTreeMap<(String, String), DefId> {
        s.checked
            .program
            .defs()
            .filter_map(|d| {
                let a = s.checked.program.annotation_texts(d.id, "schema");
                (a.len() == 2).then(|| ((a[0].into(), a[1].into()), d.id))
            })
            .collect()
    };
    let old = schemas(before);
    let new = schemas(after);
    let mut issues = Vec::new();
    for (key, id) in &old {
        let status = match new.get(key) {
            None => "removed",
            Some(next) if fingerprint(before, *id) != fingerprint(after, *next) => {
                "changed_in_place"
            }
            Some(_) => continue,
        };
        issues.push(json!({"family": key.0, "version": key.1, "status": status,
            "message": format!("schema {} version {} must remain available and unchanged; introduce a new version", key.0, key.1)}));
    }
    for key in new.keys().filter(|key| !old.contains_key(*key)) {
        let roots: BTreeSet<_> = old
            .keys()
            .filter(|k| k.0 == key.0)
            .map(|k| k.1.clone())
            .collect();
        let edges: Vec<_> = after
            .checked
            .program
            .defs()
            .filter_map(|d| {
                let a = after.checked.program.annotation_texts(d.id, "migration");
                (a.len() == 3 && a[0] == key.0).then(|| (a[1].to_string(), a[2].to_string()))
            })
            .collect();
        // Every old persisted version needs a path, not merely the newest one.
        for root in roots {
            if path(&edges, &root, &key.1).is_none() {
                issues.push(json!({"family": key.0, "version": key.1, "from": root, "status": "migration_missing",
                    "message": format!("schema {} needs a migration path from {root} to {}", key.0, key.1)}));
            }
        }
    }
    issues
}

/// A shortest chain of versions from `from` to `to` over directed migration edges.
fn path(edges: &[(String, String)], from: &str, to: &str) -> Option<Vec<String>> {
    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    let mut queue = std::collections::VecDeque::from([from.to_string()]);
    let mut seen = BTreeSet::from([from.to_string()]);
    while let Some(at) = queue.pop_front() {
        if at == to {
            let mut chain = vec![at.clone()];
            let mut node = at;
            while let Some(p) = parent.get(&node) {
                chain.push(p.clone());
                node = p.clone();
            }
            chain.reverse();
            return Some(chain);
        }
        for (_, next) in edges.iter().filter(|(f, _)| *f == at) {
            if seen.insert(next.clone()) {
                parent.insert(next.clone(), at.clone());
                queue.push_back(next.clone());
            }
        }
    }
    None
}
