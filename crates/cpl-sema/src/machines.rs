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
    /// The `context` statements: each starts a state machine (its handle's local).
    contexts: Vec<cpl_hir::LocalId>,
    /// `app.dispatch(event)`: a call of the program's root machine.
    dispatches: Vec<(DefId, Span)>,
}

impl<'h> Visitor<'h> for Uses<'h> {
    fn stmt(&mut self, s: &'h cpl_hir::Stmt, _: Site<'h>) {
        if let cpl_hir::Stmt::Context { local, .. } = s {
            self.contexts.push(*local);
        }
    }

    fn expr(&mut self, e: &'h Expr, site: Site<'h>) {
        match &e.kind {
            ExprKind::ContextOp {
                program,
                op: cpl_hir::ContextOp::Dispatch,
                ..
            } => self.dispatches.push((*program, e.span)),
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
                    _ => matches!(e.ty, Ty::Capability(_) | Ty::Context(_)),
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
            // a function whose only handles are the state machine's changes nothing outside it: passing the
            // context down the call stack is the design (spec/state-machines.md), no effect to pull up
            let context_only = |d: DefId| {
                p.def(d).kind.as_fn().is_some_and(|f| {
                    !f.using.is_empty() && f.using.iter().all(|x| matches!(x.ty, Ty::Context(_)))
                })
            };
            let effectful =
                |c: &&Call| !c.contract && p.function_mode(c.callee) == FunctionMode::Mutate && !context_only(c.callee);
            for first in u.calls.iter().filter(effectful) {
                if let Some(second) = uses[first.callee.0 as usize].calls.iter().find(effectful) {
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
    one_machine_per_stack(p, &uses, &mut diagnostics);
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
        } else if d.code == codes::STATE_CONSTRUCTION_OUTSIDE_MACHINE
            || d.code == codes::CONTEXT_NESTED
            || d.code == codes::CONTEXT_IN_REDUCER
            || d.code == codes::MACHINE_CALL_OUTSIDE_MACHINE
            || d.code == codes::MACHINE_UNROOTED
        {
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

/// One state machine per call stack (spec/state-machines.md): a function that holds a context (its `context`
/// statement made one, or a caller passed its handle) reaches no function whose `context` statement makes another,
/// through calls and through `dispatch` (a call of the program's root machine); machines and initializers start
/// none; only machines, tests, and properties call a machine; and a machine no root machine reaches is reported.
fn one_machine_per_stack(p: &Program, uses: &[Uses<'_>], out: &mut Vec<(DefId, Diagnostic)>) {
    let n = p.defs.len();
    let roots: BTreeMap<DefId, DefId> = p
        .defs()
        .filter(|d| p.is_program_state(d.id))
        .filter_map(|d| p.root_machine(d.id).map(|m| (d.id, m)))
        .collect();
    // every call edge, a dispatch being a call of the root machine
    let edges = |d: DefId| -> Vec<(DefId, Span)> {
        let u = &uses[d.0 as usize];
        let mut e: Vec<(DefId, Span)> = u.calls.iter().map(|c| (c.callee, c.span)).collect();
        e.extend(u.dispatches.iter().filter_map(|(prog, span)| roots.get(prog).map(|m| (*m, *span))));
        e
    };
    let mut reverse = vec![Vec::new(); n];
    for d in p.defs() {
        for (to, _) in edges(d.id) {
            reverse[to.0 as usize].push(d.id);
        }
    }
    // `next[d]`: the next function on a shortest path from `d` to one that starts a context
    let mut next: Vec<Option<(DefId, DefId)>> = vec![None; n];
    let mut queue = VecDeque::new();
    for d in p.defs() {
        if !uses[d.id.0 as usize].contexts.is_empty() {
            next[d.id.0 as usize] = Some((d.id, d.id));
            queue.push_back(d.id);
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
    let local_span = |d: DefId, l: cpl_hir::LocalId| {
        p.def(d).kind.locals().get(l.0 as usize).map_or(p.def(d).span, |x| x.span)
    };
    for def in p.defs() {
        let u = &uses[def.id.0 as usize];
        let mode = p.function_mode(def.id);
        let received: Vec<&cpl_hir::ParamDef> = def
            .kind
            .as_fn()
            .map(|f| f.using.iter().filter(|x| matches!(x.ty, Ty::Context(_))).collect())
            .unwrap_or_default();
        if matches!(mode, FunctionMode::Machine | FunctionMode::Initial) {
            for &l in &u.contexts {
                out.push((def.id, diag(codes::CONTEXT_IN_REDUCER, Some(local_span(def.id, l)),
                    "a machine or an initializer computes one step of a state machine and starts none; start the context in the function that runs it")));
            }
        }
        // a second context in the function itself, or one made while a caller's is held
        let mut second: Vec<Span> = u.contexts.iter().skip(1).map(|&l| local_span(def.id, l)).collect();
        if !received.is_empty() {
            second.extend(u.contexts.iter().map(|&l| local_span(def.id, l)));
        }
        second.extend(received.iter().skip(1).map(|x| x.span));
        for span in second {
            out.push((def.id, diag(codes::CONTEXT_NESTED, Some(span),
                "a second state machine on one call stack: this function already has one; pass its handle (`using app`) instead of starting another")));
        }
        if u.contexts.is_empty() && received.is_empty() {
            continue;
        }
        // calls, while holding the context, of functions that start another
        let mut seen = BTreeSet::new();
        for (callee, span) in edges(def.id) {
            if next[callee.0 as usize].is_none() || !seen.insert(callee) {
                continue;
            }
            let mut path = vec![p.display_path(def.id)];
            let (witness, truncated) = chain(p, callee, &next);
            path.extend(witness);
            let mut d = diag(codes::CONTEXT_NESTED, Some(span),
                format!("a second state machine on one call stack: {} starts one while this function's is running; pass this function's handle down (`using app`) instead", path.join(" -> ")))
                .with_data("call_chain", json!(path));
            if truncated {
                d = d.with_data("call_chain_truncated", json!(true));
            }
            out.push((def.id, d));
        }
    }
    // machines are called by machines (composition) and by tests; everything else dispatches to a context
    for def in p.defs() {
        let caller = p.function_mode(def.id);
        let checks = matches!(def.kind, DefKind::Test(_) | DefKind::Property(_));
        if caller == FunctionMode::Machine || checks {
            continue;
        }
        for c in &uses[def.id.0 as usize].calls {
            if p.function_mode(c.callee) == FunctionMode::Machine {
                let name = p.def(c.callee).name.display();
                out.push((def.id, diag(codes::MACHINE_CALL_OUTSIDE_MACHINE, Some(c.span),
                    format!("`{name}` is a state machine: only a machine (its parent) or a test calls it; run its program in a context (`context app = start(...)`) and send events with `app.dispatch(event)`"))
                    .with_data("machine", json!(p.display_path(c.callee)))));
            }
        }
    }
    // the machines each root machine reaches by composition
    let mut reached: BTreeSet<DefId> = roots.values().copied().collect();
    let mut work: Vec<DefId> = reached.iter().copied().collect();
    while let Some(m) = work.pop() {
        for c in &uses[m.0 as usize].calls {
            if p.function_mode(c.callee) == FunctionMode::Machine && reached.insert(c.callee) {
                work.push(c.callee);
            }
        }
    }
    for def in p.defs() {
        if p.function_mode(def.id) != FunctionMode::Machine || reached.contains(&def.id) {
            continue;
        }
        let state_is_program = p
            .machine_signature(def.id)
            .is_some_and(|s| matches!(s.state, Ty::Adt(d, _) if p.is_program_state(d)));
        if state_is_program {
            continue; // a second root: `program_machine_count`
        }
        out.push((def.id, diag(codes::MACHINE_UNROOTED, Some(def.span),
            format!("machine `{}` is no program's root and no root machine calls it, so no context ever runs it; call it from its parent machine", def.name.display()))));
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
