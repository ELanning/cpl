//! The containment graph of `spec/semantics.md` S7: one node per struct,
//! enum, and wrapper declaration, an edge `A -> B` for every occurrence of
//! `B` in a field, payload, or wrapper payload of `A` that is not nested
//! under `List`, `Option`, or `Result`, with the type arguments of generic
//! declarations substituted before the edges are read (`struct Box<T> { v:
//! T }` and `struct Node { next: Box<Node> }` give `Node -> Node`). Every
//! declaration on a cycle is `infinite_type`, with the cycle in `notes`.
//! A refinement in a field contributes the edges of its root base.

use crate::checker::{diag, Checker, Stage};
use crate::codes;
use crate::refine::{find_cycle, tarjan};
use cpl_diag::Note;
use cpl_hir::{DefId, DefKind, Program, Ty};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Report `infinite_type` on every declaration of every containment cycle.
pub(crate) fn check_containment(cx: &mut Checker<'_>) {
    let nodes: Vec<DefId> = cx
        .program
        .defs()
        .filter(|d| {
            matches!(
                d.kind,
                DefKind::Struct(_) | DefKind::Enum(_) | DefKind::Wrapper(_)
            )
        })
        .map(|d| d.id)
        .collect();
    let mut edges: BTreeMap<DefId, BTreeSet<DefId>> = BTreeMap::new();
    for &n in &nodes {
        let mut out = BTreeSet::new();
        let mut seen = HashSet::new();
        for ty in contained_types(&cx.program, n) {
            walk(cx, &ty, &mut out, &mut seen, 0);
        }
        edges.insert(n, out);
    }
    let mut on_cycle: BTreeSet<DefId> = BTreeSet::new();
    for scc in tarjan(&nodes, &edges) {
        if scc.len() > 1 || edges[&scc[0]].contains(&scc[0]) {
            on_cycle.extend(scc);
        }
    }
    for &n in &on_cycle {
        let cycle = find_cycle(n, &edges, &on_cycle);
        let names: Vec<String> = cycle
            .iter()
            .map(|d| cx.program.def(*d).name.display())
            .collect();
        let name = cx.program.def(n).name.display();
        let mut d = diag(
            codes::INFINITE_TYPE,
            Some(cx.decls[n.0 as usize].name_span),
            format!("`{name}` contains itself without a `List`, `Option`, or `Result` in between, so it has no finite values"),
        )
        .with_data("type", json!(name));
        d.notes
            .push(Note::new(format!("cycle: {}", names.join(" -> "))));
        cx.emit(Stage::Resolve, d);
        cx.fail(n);
    }
}

/// Report `polymorphic_recursion` on every struct or enum declaration that refers to its own
/// recursive group (through any type, `List`, `Option`, and `Result` included) with a type
/// argument that holds one of its type parameters inside a constructor (`S<List<T>>`,
/// `Pair<T, T>` as an argument): its instantiations then grow without end (`S<Int>`,
/// `S<List<Int>>`, ...), and every compiled target needs them all. A bare type parameter
/// (`S<T>`, a permutation `S<U, T>`) or a closed type (`S<Int>`) as an argument keeps them finite.
pub(crate) fn check_regular_recursion(cx: &mut Checker<'_>) {
    let program = &cx.program;
    let nodes: Vec<DefId> = program
        .defs()
        .filter(|d| matches!(d.kind, DefKind::Struct(_) | DefKind::Enum(_)))
        .map(|d| d.id)
        .collect();
    // A -> B for every mention of B in the types of A
    let mut edges: BTreeMap<DefId, BTreeSet<DefId>> = BTreeMap::new();
    for &n in &nodes {
        let mut out = BTreeSet::new();
        for ty in contained_types(program, n) {
            mentions(&ty, &mut |d, _| {
                out.insert(d);
            });
        }
        edges.insert(n, out);
    }
    let mut found: Vec<(DefId, String)> = Vec::new();
    for scc in tarjan(&nodes, &edges) {
        if scc.len() == 1 && !edges[&scc[0]].contains(&scc[0]) {
            continue;
        }
        let group: BTreeSet<DefId> = scc.iter().copied().collect();
        for &n in &scc {
            // a declaration already on a containment cycle (`A<A<T>>`) has its error
            if program.def(n).status == cpl_hir::DefStatus::HasErrors {
                continue;
            }
            let mut bad: Option<Ty> = None;
            for ty in contained_types(program, n) {
                mentions(&ty, &mut |d, args| {
                    let grows = |a: &Ty| !matches!(a, Ty::Param(_)) && has_param(a);
                    if bad.is_none() && group.contains(&d) && args.iter().any(grows) {
                        bad = Some(Ty::Adt(d, args.to_vec()));
                    }
                });
            }
            if let Some(t) = bad {
                let generics: Vec<String> = match &program.def(n).kind {
                    DefKind::Struct(s) => s.generics.clone(),
                    DefKind::Enum(e) => e.generics.clone(),
                    _ => Vec::new(),
                };
                found.push((n, program.display_ty_in(&t, &generics)));
            }
        }
    }
    for (n, t) in found {
        let name = cx.program.def(n).name.display();
        let d = diag(
            codes::POLYMORPHIC_RECURSION,
            Some(cx.decls[n.0 as usize].name_span),
            format!("`{name}` refers to its own recursive declarations as `{t}`, whose type arguments grow at every level, so it has no finite set of instantiations to compile"),
        )
        .with_data("type", json!(name))
        .with_data("reference", json!(t));
        cx.emit(Stage::Resolve, d);
        cx.fail(n);
    }
}

/// Every declaration a type mentions, with its type arguments, outermost first.
fn mentions(ty: &Ty, f: &mut dyn FnMut(DefId, &[Ty])) {
    match ty {
        Ty::Adt(d, args) => {
            f(*d, args);
            for a in args {
                mentions(a, f);
            }
        }
        Ty::List(t) | Ty::Gen(t) => mentions(t, f),
        _ => {}
    }
}

/// Whether a type holds a type parameter.
fn has_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param(_) => true,
        Ty::Adt(_, args) => args.iter().any(has_param),
        Ty::List(t) | Ty::Gen(t) => has_param(t),
        _ => false,
    }
}

/// The field, payload, or wrapper-payload types of a declaration.
fn contained_types(program: &Program, def: DefId) -> Vec<Ty> {
    match &program.def(def).kind {
        DefKind::Struct(s) => s.fields.iter().map(|f| f.ty.clone()).collect(),
        DefKind::Enum(e) => e.variants.iter().flat_map(|v| v.payload.clone()).collect(),
        DefKind::Wrapper(w) => vec![w.inner.clone()],
        _ => Vec::new(),
    }
}

/// How deep generic instantiations are expanded. Polymorphic recursion
/// (`struct A<T> { a: A<A<T>> }`) has no fixed point; the declaration it
/// passes through is already an edge at the first level, so the cycle is
/// found before the bound is reached.
const MAX_EXPANSION_DEPTH: usize = 16;

/// The declarations a contained type reaches (see the module docs).
/// `seen` holds the generic instantiations already expanded.
fn walk(
    cx: &Checker<'_>,
    ty: &Ty,
    out: &mut BTreeSet<DefId>,
    seen: &mut HashSet<(DefId, Vec<Ty>)>,
    depth: usize,
) {
    let program = &cx.program;
    let std = program.std_defs;
    match ty {
        Ty::List(_) => {}
        Ty::Adt(d, _) if *d == std.option || *d == std.result => {}
        Ty::Adt(d, args) => {
            out.insert(*d);
            if args.is_empty() || depth >= MAX_EXPANSION_DEPTH {
                return;
            }
            if !seen.insert((*d, args.clone())) {
                return;
            }
            for inner in contained_types(program, *d) {
                walk(cx, &substitute(&inner, args), out, seen, depth + 1);
            }
        }
        Ty::Wrapper(d) => {
            out.insert(*d);
        }
        Ty::Refined(..) => {
            let root = cx.refinements.strip(ty);
            if !matches!(root, Ty::Refined(..)) {
                walk(cx, &root, out, seen, depth);
            }
        }
        _ => {}
    }
}

/// `ty[Param(i) := args[i]]`.
pub(crate) fn substitute(ty: &Ty, args: &[Ty]) -> Ty {
    match ty {
        Ty::Param(i) => args.get(*i as usize).cloned().unwrap_or(Ty::Error),
        Ty::List(t) => Ty::List(Box::new(substitute(t, args))),
        Ty::Gen(t) => Ty::Gen(Box::new(substitute(t, args))),
        Ty::Adt(d, a) => Ty::Adt(*d, a.iter().map(|x| substitute(x, args)).collect()),
        other => other.clone(),
    }
}
