//! Nominal protocol states and versioned schemas use ordinary CPL types on both backends.
//! The annotations add graph and endpoint checks, never unchecked runtime conversions.
use crate::checker::{Checker, Stage};
use cpl_diag::{Category, DiagCode, Diagnostic};
use cpl_hir::{DefId, DefKind, Ty};
use std::collections::{BTreeMap, BTreeSet};

fn nominal(ty: &Ty) -> Option<DefId> {
    match ty {
        Ty::Adt(id, args) if args.is_empty() => Some(*id),
        Ty::Wrapper(id) => Some(*id),
        _ => None,
    }
}

pub(crate) fn check(cx: &mut Checker<'_>) {
    let p = &cx.program;
    let mut errors = Vec::new();
    let mut fail = |id: DefId, code: &'static str, message: String| {
        errors.push((
            id,
            Diagnostic::error(DiagCode(code), Category::Type, message)
                .with_span(p.def(id).span)
                .with_symbol(p.def(id).stable_id.clone(), p.display_path(id), None),
        ));
    };
    let mut states = BTreeMap::new();
    let mut schemas = BTreeMap::new();
    let mut roles = BTreeMap::new();
    for d in p.defs() {
        for (annotation, table, count) in
            [("protocol", &mut states, 3), ("schema", &mut schemas, 2)]
        {
            let a = p.annotation_texts(d.id, annotation);
            if a.len() != count {
                continue;
            }
            if !matches!(
                &d.kind,
                DefKind::Wrapper(_) | DefKind::Struct(_) | DefKind::Enum(_)
            ) {
                continue;
            }
            if table
                .insert((a[0].to_string(), a[1].to_string()), d.id)
                .is_some()
            {
                fail(
                    d.id,
                    "evolution_duplicate_version",
                    format!("duplicate @{annotation} identity {} / {}", a[0], a[1]),
                );
            }
            if annotation == "protocol" {
                if !["initial", "state", "terminal", "initial_terminal"].contains(&a[2]) {
                    fail(
                        d.id,
                        "protocol_role_invalid",
                        "protocol role must be initial, state, terminal, or initial_terminal"
                            .into(),
                    );
                }
                roles.insert(d.id, a[2].to_string());
            }
        }
    }
    let mut edges = BTreeMap::<DefId, BTreeSet<DefId>>::new();
    let mut migrations = BTreeSet::new();
    for d in p.defs() {
        if !p.annotation_texts(d.id, "message").is_empty()
            && p.annotation_texts(d.id, "transition").is_empty()
        {
            fail(
                d.id,
                "protocol_message_without_transition",
                "@message requires @transition on the same function".into(),
            );
        }
        for (annotation, table) in [("transition", &states), ("migration", &schemas)] {
            let a = p.annotation_texts(d.id, annotation);
            if a.len() != 3 {
                continue;
            }
            let Some(f) = d.kind.as_fn() else {
                continue;
            };
            let from = table.get(&(a[0].into(), a[1].into())).copied();
            let to = table.get(&(a[0].into(), a[2].into())).copied();
            if from.is_none() || to.is_none() {
                fail(
                    d.id,
                    "evolution_unknown_endpoint",
                    format!("@{annotation} refers to an undeclared state or schema version"),
                );
                continue;
            }
            if !f.generics.is_empty()
                || f.params.first().and_then(|x| nominal(&x.ty)) != from
                || nominal(&f.ret) != to
            {
                fail(d.id, "evolution_endpoint_mismatch", format!("@{annotation} requires a monomorphic function taking the source type first and returning the destination type"));
                continue;
            }
            if annotation == "migration" {
                if f.params.len() != 1
                    || !f.using.is_empty()
                    || p.is_mutating(d.id)
                    || !f.effects.inferred.is_empty()
                {
                    fail(d.id, "migration_not_pure", "a migration must be pure with exactly one value parameter and no capabilities".into());
                }
                if from == to || !migrations.insert((a[0], a[1], a[2])) {
                    fail(d.id, "migration_duplicate_edge", "migration endpoints must differ and each directed version pair has one migration".into());
                }
            } else {
                edges.entry(from.unwrap()).or_default().insert(to.unwrap());
            }
        }
    }
    for protocol in states.keys().map(|(name, _)| name).collect::<BTreeSet<_>>() {
        let members: BTreeSet<_> = states
            .iter()
            .filter(|((name, _), _)| name == protocol)
            .map(|(_, id)| *id)
            .collect();
        let initial: BTreeSet<_> = members
            .iter()
            .copied()
            .filter(|id| {
                matches!(
                    roles.get(id).map(String::as_str),
                    Some("initial" | "initial_terminal")
                )
            })
            .collect();
        let terminal: BTreeSet<_> = members
            .iter()
            .copied()
            .filter(|id| {
                matches!(
                    roles.get(id).map(String::as_str),
                    Some("terminal" | "initial_terminal")
                )
            })
            .collect();
        if initial.len() != 1 || terminal.is_empty() {
            fail(*members.first().unwrap(), "protocol_endpoints_invalid", format!("protocol {protocol} needs exactly one initial state and at least one terminal state"));
            continue;
        }
        let reachable = closure(&initial, &edges);
        let mut reverse = BTreeMap::<DefId, BTreeSet<DefId>>::new();
        for (from, targets) in &edges {
            for to in targets {
                reverse.entry(*to).or_default().insert(*from);
            }
        }
        let can_finish = closure(&terminal, &reverse);
        for id in members {
            if !reachable.contains(&id) {
                fail(
                    id,
                    "protocol_unreachable_state",
                    format!("protocol state {} is unreachable", p.display_path(id)),
                );
            } else if !can_finish.contains(&id) {
                fail(
                    id,
                    "protocol_no_completion",
                    "this protocol state has no path to a terminal state".into(),
                );
            }
        }
    }
    role_implementations(p, &mut fail);
    for (id, error) in errors {
        cx.fail(id);
        cx.emit(Stage::Type, error);
    }
}

/// `@role(protocol, role)` marks a function as part of that role's implementation. A role's
/// functions perform only the transitions whose `@message` sender is the role, and together they
/// perform every one of them, directly or by being the transition.
fn role_implementations(p: &cpl_hir::Program, fail: &mut impl FnMut(DefId, &'static str, String)) {
    // Labeled transitions: (protocol, function, sender).
    let mut transitions: Vec<(String, DefId, String)> = Vec::new();
    let mut named: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for d in p.defs() {
        let t = p.annotation_texts(d.id, "transition");
        let m = p.annotation_texts(d.id, "message");
        if t.len() == 3 && m.len() == 2 {
            transitions.push((t[0].to_string(), d.id, m[0].to_string()));
            let roles = named.entry(t[0].to_string()).or_default();
            roles.insert(m[0].to_string());
            roles.insert(m[1].to_string());
        }
    }
    let mut members: BTreeMap<(String, String), Vec<DefId>> = BTreeMap::new();
    for d in p.defs() {
        let r = p.annotation_texts(d.id, "role");
        if r.len() != 2 {
            continue;
        }
        if !named.get(r[0]).is_some_and(|roles| roles.contains(r[1])) {
            fail(
                d.id,
                "protocol_role_unknown",
                format!("no message of protocol {} names the role {}", r[0], r[1]),
            );
            continue;
        }
        members
            .entry((r[0].to_string(), r[1].to_string()))
            .or_default()
            .push(d.id);
    }
    for ((protocol, role), functions) in &members {
        let mut performed = BTreeSet::new();
        for f in functions {
            for r in p.references_from(*f) {
                if r.kind != cpl_hir::RefKind::Call {
                    continue;
                }
                let Some((_, _, sender)) = transitions
                    .iter()
                    .find(|(proto, t, _)| proto == protocol && *t == r.to)
                else {
                    continue;
                };
                if sender != role {
                    fail(
                        *f,
                        "protocol_role_violation",
                        format!(
                            "role {role} performs {}, which {sender} sends in protocol {protocol}",
                            p.display_path(r.to)
                        ),
                    );
                }
                performed.insert(r.to);
            }
            performed.insert(*f);
        }
        let missing: Vec<String> = transitions
            .iter()
            .filter(|(proto, t, sender)| {
                proto == protocol && sender == role && !performed.contains(t)
            })
            .map(|(_, t, _)| p.display_path(*t))
            .collect();
        if !missing.is_empty() {
            fail(
                functions[0],
                "protocol_role_incomplete",
                format!(
                    "role {role} of protocol {protocol} never performs {}",
                    missing.join(", ")
                ),
            );
        }
    }
}

fn closure(roots: &BTreeSet<DefId>, edges: &BTreeMap<DefId, BTreeSet<DefId>>) -> BTreeSet<DefId> {
    let mut seen = roots.clone();
    let mut todo: Vec<_> = roots.iter().copied().collect();
    while let Some(id) = todo.pop() {
        for next in edges.get(&id).into_iter().flatten() {
            if seen.insert(*next) {
                todo.push(*next);
            }
        }
    }
    seen
}
