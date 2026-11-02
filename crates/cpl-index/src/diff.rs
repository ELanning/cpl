//! Semantic diff (`spec/toolchain.md` 9): the statements of 9.1 between two
//! facet maps, as the `DiffOutput` of 9.2. Computed from the facet maps
//! alone (T31), so it works between any two revisions, including ones that
//! do not typecheck.

use crate::facet::{facet_hash, FacetEntry, FacetMap};
use cpl_diag::json::{
    DiffDuplicates, DiffOutput, DiffStatement, DiffStatementKind, DuplicateId, FacetHash,
};
use cpl_diag::Facet;
use serde_json::{json, Value};

/// The statements of Section 9.1 for every stable id of `old` and/or `new`.
///
/// | Statement | When | `facet` | `detail` |
/// | - | - | - | - |
/// | `identity_added` | only in `new` | `null` | `{ "version" }` (new) |
/// | `identity_removed` | only in `old` | `null` | `{ "version" }` (old) |
/// | `identity_renamed` | `name` hashes differ | `name` | `{ "from", "to" }` (paths) |
/// | `moved` | modules differ | `null` | `{ "from_module", "to_module" }` |
/// | `signature_changed`, `contract_changed`, `body_changed`, `docs_changed` | that facet's hashes differ | that facet | `{ "old", "new" }` (hashes) |
/// | `effect_bound_widened` / `_narrowed` | `new \ old` / `old \ new` effect sets non-empty | `effects` | `{ "added" }` / `{ "removed" }` (sorted `param.op`) |
/// | `verification_policy_changed` | the policy hashes differ | `verification_policy` | `{ "old", "new" }` (texts, `@prove` or empty) |
///
/// An id whose two entries have different facet sets (the declaration
/// changed kind, e.g. a refinement became a struct) is `identity_removed`
/// plus `identity_added`, each with `"note": "kind_changed"` in its
/// `detail`, because the facets are not comparable. The facet sets decide,
/// as 9.1 states the rule: a kind change that keeps them (a wrapper `type`
/// that became a `struct`) is compared facet by facet. The `identity` facet
/// never produces a statement.
///
/// A statement's `path` is the new side's when the id is in `new`, else the
/// old side's ("ordered by `path` (new side, else old)", 9.1), so both
/// statements of a kind change carry the new path, the removed one with
/// the old side's version, and so its old path, in its `detail`.
/// Statements are ordered by `path`, then by the table order of 9.1, then
/// by id, so the order is the visible order of the `path`s, as 7.2's human
/// lines show them. `duplicates` lists the ids declared more than once in
/// either map: their statements describe the first occurrences.
pub fn semantic_diff(old: &FacetMap, new: &FacetMap) -> DiffOutput {
    let mut statements: Vec<DiffStatement> = Vec::new();
    for (id, o) in &old.entries {
        if !new.entries.contains_key(id) {
            statements.push(removed(o, None));
        }
    }
    for (id, n) in &new.entries {
        match old.entries.get(id) {
            None => statements.push(added(n, None)),
            Some(o) => compare(o, n, &mut statements),
        }
    }
    statements.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.statement.cmp(&b.statement))
            .then_with(|| a.id.cmp(&b.id))
    });
    DiffOutput {
        old: old.revision.clone(),
        new: new.revision.clone(),
        statements,
        duplicates: DiffDuplicates {
            old: duplicate_ids(old),
            new: duplicate_ids(new),
        },
    }
}

/// The note on the two statements of a kind change.
const KIND_CHANGED: &str = "kind_changed";

fn statement(
    statement: DiffStatementKind,
    entry: &FacetEntry,
    facet: Option<Facet>,
    detail: Value,
) -> DiffStatement {
    DiffStatement {
        statement,
        id: entry.version.id.clone(),
        path: entry.version.path.clone(),
        kind: entry.kind.prefix().to_string(),
        facet,
        detail,
    }
}

fn version_detail(entry: &FacetEntry, note: Option<&str>) -> Value {
    let version = serde_json::to_value(&entry.version).expect("a SymbolVersion serializes");
    match note {
        Some(note) => json!({ "version": version, "note": note }),
        None => json!({ "version": version }),
    }
}

fn added(n: &FacetEntry, note: Option<&str>) -> DiffStatement {
    statement(
        DiffStatementKind::IdentityAdded,
        n,
        None,
        version_detail(n, note),
    )
}

fn removed(o: &FacetEntry, note: Option<&str>) -> DiffStatement {
    statement(
        DiffStatementKind::IdentityRemoved,
        o,
        None,
        version_detail(o, note),
    )
}

/// The statements for an id present on both sides, each at the new
/// side's path (9.1).
fn compare(o: &FacetEntry, n: &FacetEntry, out: &mut Vec<DiffStatement>) {
    if !o.version.facets.keys().eq(n.version.facets.keys()) {
        // The old declaration is gone (its kind and version stay the old
        // side's), but the id lives on at the new path.
        let gone = DiffStatement {
            path: n.version.path.clone(),
            ..removed(o, Some(KIND_CHANGED))
        };
        out.push(gone);
        out.push(added(n, Some(KIND_CHANGED)));
        return;
    }
    let differs = |facet: Facet| o.facet(facet) != n.facet(facet);
    if differs(Facet::Name) {
        out.push(statement(
            DiffStatementKind::IdentityRenamed,
            n,
            Some(Facet::Name),
            json!({ "from": o.version.path, "to": n.version.path }),
        ));
    }
    if o.module != n.module {
        out.push(statement(
            DiffStatementKind::Moved,
            n,
            None,
            json!({ "from_module": o.module, "to_module": n.module }),
        ));
    }
    let hashes = |facet: Facet| json!({ "old": o.facet(facet), "new": n.facet(facet) });
    for (facet, kind) in [
        (Facet::Signature, DiffStatementKind::SignatureChanged),
        (Facet::Contract, DiffStatementKind::ContractChanged),
    ] {
        if differs(facet) {
            out.push(statement(kind, n, Some(facet), hashes(facet)));
        }
    }
    let widened: Vec<&String> = n.effects.difference(&o.effects).collect();
    if !widened.is_empty() {
        out.push(statement(
            DiffStatementKind::EffectBoundWidened,
            n,
            Some(Facet::Effects),
            json!({ "added": widened }),
        ));
    }
    let narrowed: Vec<&String> = o.effects.difference(&n.effects).collect();
    if !narrowed.is_empty() {
        out.push(statement(
            DiffStatementKind::EffectBoundNarrowed,
            n,
            Some(Facet::Effects),
            json!({ "removed": narrowed }),
        ));
    }
    for (facet, kind) in [
        (Facet::Body, DiffStatementKind::BodyChanged),
        (Facet::Docs, DiffStatementKind::DocsChanged),
    ] {
        if differs(facet) {
            out.push(statement(kind, n, Some(facet), hashes(facet)));
        }
    }
    if differs(Facet::VerificationPolicy) {
        out.push(statement(
            DiffStatementKind::VerificationPolicyChanged,
            n,
            Some(Facet::VerificationPolicy),
            json!({
                "old": o.facet(Facet::VerificationPolicy).map(policy_text),
                "new": n.facet(Facet::VerificationPolicy).map(policy_text),
            }),
        ));
    }
}

/// The `verification_policy` text a hash stands for: the facet's canonical
/// text is `@prove` or empty (Section 5.2), so the hash identifies it. A
/// hash of neither (a facet map from a later toolchain) is shown as is.
fn policy_text(hash: &FacetHash) -> String {
    ["@prove", ""]
        .into_iter()
        .find(|text| facet_hash(text) == *hash)
        .map_or_else(|| hash.as_str().to_string(), str::to_string)
}

fn duplicate_ids(map: &FacetMap) -> Vec<DuplicateId> {
    map.duplicates
        .iter()
        .map(|(id, files)| DuplicateId {
            id: id.clone(),
            files: files.clone(),
        })
        .collect()
}
