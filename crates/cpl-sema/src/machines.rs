//! Explicit purity, deterministic reducers, and ownership of immutable application-state roots.
//! Reachability uses reverse breadth-first searches: recursion cannot hide a mutating call, and each search
//! visits an edge once. Diagnostics carry a concrete shortest call chain rather than only the final callee.

use crate::bodies::{visit_def, Site, Visitor};
use crate::checker::{blame, diag, Checker, Stage};
use crate::codes;
use cpl_diag::{Diagnostic, Facet, Span};
use cpl_hir::{DefId, DefKind, Expr, ExprKind, FunctionMode, Program, Ty};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy)]
struct Call {
    callee: DefId,
    span: Span,
    contract: bool,
}

#[derive(Default)]
struct Uses<'h> {
    calls: Vec<Call>,
    constructors: Vec<(DefId, Span)>,
    parallel: Vec<&'h Expr>,
}

impl<'h> Visitor<'h> for Uses<'h> {
    fn expr(&mut self, e: &'h Expr, site: Site<'h>) {
        match &e.kind {
            ExprKind::Call { callee, .. } => self.calls.push(Call {
                callee: *callee,
                span: e.span,
                contract: site.contract,
            }),
            ExprKind::Parallel { predicate, .. } => self.parallel.push(predicate),
            ExprKind::StructLit { def, .. } => self.constructors.push((*def, e.span)),
            _ => {}
        }
    }
}

/// `next[d]` holds the next function and terminal mutator on a shortest path; a target points to itself.
fn reach(p: &Program, reverse: &[Vec<DefId>], explicit_only: bool) -> Vec<Option<(DefId, DefId)>> {
    let mut next = vec![None; p.defs.len()];
    let mut queue = VecDeque::new();
    for def in p.defs() {
        if p.function_mode(def.id) == FunctionMode::Mutate
            || (!explicit_only && p.is_mutating(def.id))
        {
            next[def.id.0 as usize] = Some((def.id, def.id));
            queue.push_back(def.id);
        }
    }
    while let Some(to) = queue.pop_front() {
        for &from in &reverse[to.0 as usize] {
            if next[from.0 as usize].is_none() {
                next[from.0 as usize] = Some((to, next[to.0 as usize].unwrap().1));
                queue.push_back(from);
            }
        }
    }
    next
}

fn chain(p: &Program, start: DefId, next: &[Option<(DefId, DefId)>]) -> (Vec<String>, bool) {
    let mut path = vec![p.display_path(start)];
    let mut at = start;
    while let Some((to, target)) = next[at.0 as usize] {
        if to == at {
            break;
        }
        // The graph walk is linear, but displaying a complete path for every caller of a long chain would
        // allocate quadratic output. Bound witness rendering while retaining the actual mutating endpoint.
        if path.len() == 64 {
            path.push("... (truncated)".into());
            path.push(p.display_path(target));
            return (path, true);
        }
        path.push(p.display_path(to));
        at = to;
    }
    (path, false)
}

pub(crate) fn check(cx: &mut Checker<'_>) {
    let p = &cx.program;
    let mut uses = Vec::with_capacity(p.defs.len());
    let mut reverse = vec![Vec::new(); p.defs.len()];
    for def in p.defs() {
        let mut u = Uses::default();
        visit_def(def, &mut u);
        for call in &u.calls {
            reverse[call.callee.0 as usize].push(def.id);
        }
        uses.push(u);
    }
    let any = reach(p, &reverse, false);
    let explicit = reach(p, &reverse, true);
    let mut diagnostics: Vec<(DefId, Diagnostic)> = Vec::new();
    let mut roots: BTreeMap<DefId, Vec<DefId>> = BTreeMap::new();

    for def in p.defs() {
        let mode = p.function_mode(def.id);
        let f = def.kind.as_fn();
        if mode.is_pure()
            && f.is_some_and(|f| f.effects.declared.as_ref().is_some_and(|e| !e.is_empty()))
        {
            diagnostics.push((def.id, diag(codes::PURE_DECLARES_EFFECTS, Some(def.span),
                "a pure function, machine, or initializer cannot declare external effects; return commands and handle them in a mutate function")));
        }
        if mode == FunctionMode::Machine {
            match p.machine_signature(def.id) {
                Some(sig) => {
                    if let Ty::Adt(state, _) = sig.state {
                        if p.is_program_state(state) { roots.entry(state).or_default().push(def.id); }
                    }
                }
                None => diagnostics.push((def.id, diag(codes::MACHINE_SIGNATURE, Some(def.span),
                    "a machine takes (state: S, event: E), without generics or capabilities; S is a struct or enum and E is an enum; return S or a record { state: S, commands: List<C> } with enum C"))),
            }
        }
        if mode == FunctionMode::Initial {
            let valid = f.is_some_and(|f| {
                matches!(&f.ret, Ty::Adt(root, args) if args.is_empty() && p.is_program_state(*root))
                    && f.generics.is_empty() && f.using.is_empty()
                    && !f.params.iter().any(|param| contains_root(p, &param.ty))
            });
            if !valid {
                diagnostics.push((def.id, diag(codes::INITIAL_SIGNATURE, Some(def.span),
                    "an initializer returns one program state and takes no program-state snapshots, generic parameters, or capabilities")));
            }
        }
        if p.is_program_state(def.id)
            && def.kind.as_struct().is_some_and(|s| !s.generics.is_empty())
        {
            diagnostics.push((
                def.id,
                diag(
                    codes::PROGRAM_GENERIC,
                    Some(def.span),
                    "a program state has a concrete type; put reusable generic data in its fields",
                ),
            ));
        }
        let u = &uses[def.id.0 as usize];
        for body in &u.parallel {
            struct Safety(bool);
            impl<'h> Visitor<'h> for Safety {
                fn stmt(&mut self, s: &'h cpl_hir::Stmt, _: Site<'h>) {
                    self.0 |= matches!(s, cpl_hir::Stmt::Assign { .. } | cpl_hir::Stmt::Return(_));
                }
            }
            let mut safety = Safety(false);
            crate::bodies::visit_expression(body, &mut safety);
            let mut unsafe_body = safety.0;
            body.walk(&mut |e| {
                unsafe_body |= match &e.kind {
                    ExprKind::CapCall { .. } | ExprKind::Propagate(_) => true,
                    ExprKind::Call { callee, using, .. } => {
                        !using.is_empty() || any[callee.0 as usize].is_some()
                    }
                    _ => matches!(e.ty, Ty::Capability(_)),
                };
            });
            if unsafe_body {
                diagnostics.push((def.id, diag(codes::PARALLEL_UNSAFE, Some(body.span),
                    "parallel tasks must be pure, cannot capture capabilities, assign variables, or return/propagate out of their scope; put local computation in a pure helper and return commands for effects")));
            }
        }
        for call in &u.calls {
            // Old functions keep their existing effect-bound diagnostics. They still cannot hide a newly
            // designated mutator behind a pure helper. Contracts always exclude both kinds of mutation.
            let paths = if mode.is_pure() { &any } else { &explicit };
            let pure_caller = !p.is_mutating(def.id);
            if (pure_caller || call.contract) && paths[call.callee.0 as usize].is_some() {
                let mut path = vec![p.display_path(def.id)];
                let (witness, truncated) = chain(p, call.callee, paths);
                path.extend(witness);
                let code = if call.contract {
                    codes::MUTATE_IN_CONTRACT
                } else {
                    codes::PURE_CALLS_MUTATE
                };
                let mut diagnostic = diag(code, Some(call.span),
                    format!("pure code reaches a mutate function: {}; move the effect to the caller and pass its result as a value", path.join(" -> ")))
                    .with_data("call_chain", json!(path));
                if truncated {
                    diagnostic = diagnostic.with_data("call_chain_truncated", json!(true));
                }
                diagnostics.push((def.id, diagnostic));
            }
        }
        for &(state, span) in &u.constructors {
            if !p.is_program_state(state) {
                continue;
            }
            let allowed = f.is_some_and(|f| match mode {
                FunctionMode::Machine => {
                    matches!(f.params.first().map(|p| &p.ty), Some(Ty::Adt(d, _)) if *d == state)
                }
                FunctionMode::Initial => matches!(&f.ret, Ty::Adt(d, _) if *d == state),
                _ => false,
            });
            if !allowed {
                diagnostics.push((def.id, diag(codes::STATE_CONSTRUCTION_OUTSIDE_MACHINE, Some(span),
                    format!("`{}` may be constructed only by its machine or an initial function; pass immutable snapshots to helpers", p.display_path(state)))
                    .with_data("state", json!(p.display_path(state)))));
            }
        }
        if mode == FunctionMode::Mutate {
            for first in u
                .calls
                .iter()
                .filter(|c| !c.contract && p.function_mode(c.callee) == FunctionMode::Mutate)
            {
                if let Some(second) = uses[first.callee.0 as usize]
                    .calls
                    .iter()
                    .find(|c| !c.contract && p.function_mode(c.callee) == FunctionMode::Mutate)
                {
                    let path = [
                        p.display_path(def.id),
                        p.display_path(first.callee),
                        p.display_path(second.callee),
                    ];
                    diagnostics.push((def.id, diag(codes::MUTATION_DEPTH, Some(first.span),
                        "mutation spans three function levels; keep orchestration near the entry point and extract pure computations")
                        .with_data("call_chain", json!(path))));
                    break;
                }
            }
        }
    }
    for def in p.defs().filter(|d| p.is_program_state(d.id)) {
        let count = roots.get(&def.id).map_or(0, Vec::len);
        if count != 1 {
            diagnostics.push((
                def.id,
                diag(
                    codes::PROGRAM_MACHINE_COUNT,
                    Some(def.span),
                    format!(
                        "program state `{}` needs exactly one root machine, found {count}",
                        p.display_path(def.id)
                    ),
                )
                .with_data(
                    "machines",
                    json!(roots
                        .get(&def.id)
                        .into_iter()
                        .flatten()
                        .map(|id| p.display_path(*id))
                        .collect::<Vec<_>>()),
                ),
            ));
        }
    }
    for (def, d) in diagnostics {
        let facet = if d.category == cpl_diag::Category::Type {
            Facet::Signature
        } else if d.code == codes::MUTATE_IN_CONTRACT {
            Facet::Contract
        } else if d.code == codes::STATE_CONSTRUCTION_OUTSIDE_MACHINE {
            Facet::Body
        } else {
            Facet::Effects
        };
        let d = blame(d, cx, def, Some(facet));
        if d.is_error() {
            cx.fail(def);
        }
        cx.emit(Stage::Effect, d);
    }
}

/// Follow nominal containment once per declaration, so recursive enum/list types cannot make this audit recurse
/// forever. Inspect every generic argument as well as the nominal fields and payloads.
fn contains_root(p: &Program, ty: &Ty) -> bool {
    let mut work = vec![ty.clone()];
    let mut seen = BTreeSet::new();
    while let Some(ty) = work.pop() {
        match ty {
            Ty::List(inner) | Ty::Gen(inner) => work.push(*inner),
            Ty::Adt(id, args) => {
                if p.is_program_state(id) {
                    return true;
                }
                work.extend(args);
                if !seen.insert(id) {
                    continue;
                }
                match &p.def(id).kind {
                    DefKind::Struct(s) => work.extend(s.fields.iter().map(|f| f.ty.clone())),
                    DefKind::Enum(e) => {
                        work.extend(e.variants.iter().flat_map(|v| v.payload.clone()))
                    }
                    _ => {}
                }
            }
            Ty::Wrapper(id) if seen.insert(id) => {
                work.push(p.def(id).kind.as_wrapper().unwrap().inner.clone())
            }
            Ty::Refined(_, id) if seen.insert(id) => {
                work.push(p.def(id).kind.as_refinement().unwrap().base.clone())
            }
            _ => {}
        }
    }
    false
}
