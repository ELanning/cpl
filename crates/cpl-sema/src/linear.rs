//! Protocol states are affine (`spec/evolution.md`, Protocols): a value whose type is a protocol
//! state, or contains one (a struct field, an enum payload, a wrapper, `Option`, `Result`), is
//! used at most once. After a transition consumes a state, the old value cannot be used again,
//! so a program cannot keep operating on a state the protocol has left.
//!
//! | Rule | Diagnostic |
//! | - | - |
//! | a local holding a state is read again after a read consumed it | `protocol_state_reused` |
//! | a list element type contains a state (lists share their elements) | `protocol_state_in_list` |
//! | a generic function that may use a parameter twice is instantiated with a state | `protocol_state_duplicated` |
//!
//! Every read consumes: a call argument, an operand, a match scrutinee, a field access, a
//! returned value. Assigning a `var` makes it usable again. The flow is path-sensitive: a value
//! consumed in one branch of an `if` or `match` is consumed after it, and the body of a loop,
//! `forall`, or `parallel for` is analyzed twice, so a state consumed in one iteration and not
//! reassigned is a reuse in the next. Contract clauses are specifications and consume nothing.

use crate::checker::{blame, diag, Checker, Stage};
use crate::codes;
use cpl_diag::{Diagnostic, Facet, Span};
use cpl_hir::{
    Block, DefId, DefKind, Expr, ExprKind, FnBody, LocalDef, LocalId, Program, Stmt, Ty,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn check(cx: &mut Checker<'_>) {
    let p = &cx.program;
    let states: BTreeSet<DefId> = p
        .defs()
        .filter(|d| {
            p.annotation_texts(d.id, "protocol").len() == 3
                && matches!(
                    d.kind,
                    DefKind::Wrapper(_) | DefKind::Struct(_) | DefKind::Enum(_)
                )
        })
        .map(|d| d.id)
        .collect();
    if states.is_empty() {
        return;
    }
    let affinity = Affinity { p, states: &states };
    // Which type parameters each generic function may use more than once, to a fixpoint over
    // generic calls between them.
    let mut duplicates: BTreeMap<DefId, BTreeSet<u32>> = BTreeMap::new();
    loop {
        let mut changed = false;
        for d in p.defs() {
            let Some(f) = d.kind.as_fn() else { continue };
            if f.generics.is_empty() {
                continue;
            }
            let FnBody::Block(body) = &f.body else {
                continue;
            };
            let mut w = Walk::new(&affinity, &f.locals, &duplicates, true);
            w.block(body);
            let found: BTreeSet<u32> = w
                .reused
                .keys()
                .flat_map(|local| params_in(&f.locals[local.0 as usize].ty))
                .chain(w.duplicating.iter().copied())
                .collect();
            if duplicates.get(&d.id) != Some(&found) && !found.is_empty() {
                duplicates.insert(d.id, found);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out: Vec<(DefId, Diagnostic)> = Vec::new();
    for d in p.defs() {
        let (locals, body): (&[LocalDef], &Block) = match &d.kind {
            DefKind::Fn(f) => match &f.body {
                FnBody::Block(b) => (&f.locals, b),
                FnBody::Builtin(_) => continue,
            },
            DefKind::Test(t) => (&t.locals, &t.body),
            DefKind::Property(t) => (&t.locals, &t.body),
            _ => continue,
        };
        let mut w = Walk::new(&affinity, locals, &duplicates, false);
        w.block(body);
        for (local, (first, again)) in &w.reused {
            let name = &locals[local.0 as usize].name;
            out.push((
                d.id,
                diag(
                    codes::PROTOCOL_STATE_REUSED,
                    Some(*again),
                    format!("`{name}` holds a protocol state that was already consumed; a state is used once, so continue with the value the consuming call returned"),
                )
                .with_data("name", json!(name))
                .with_note(format!("`{name}` was consumed here"), Some(*first)),
            ));
        }
        for (span, callee) in &w.generic_errors {
            out.push((
                d.id,
                diag(
                    codes::PROTOCOL_STATE_DUPLICATED,
                    Some(*span),
                    format!("`{}` may use its argument more than once, so it cannot receive a protocol state", p.display_path(*callee)),
                )
                .with_data("callee", json!(p.display_path(*callee))),
            ));
        }
        if let Some(span) = w.in_list {
            out.push((
                d.id,
                diag(
                    codes::PROTOCOL_STATE_IN_LIST,
                    Some(span),
                    "a list shares its elements, so it cannot hold protocol states; keep each state in its own binding or an Option",
                ),
            ));
        }
    }
    for (def, d) in out {
        let d = blame(d, cx, def, Some(Facet::Body));
        cx.fail(def);
        cx.emit(Stage::Effect, d);
    }
}

/// The type parameters a type mentions.
fn params_in(ty: &Ty) -> Vec<u32> {
    match ty {
        Ty::Param(i) => vec![*i],
        Ty::List(t) | Ty::Gen(t) => params_in(t),
        Ty::Adt(_, args) => args.iter().flat_map(params_in).collect(),
        _ => vec![],
    }
}

struct Affinity<'p> {
    p: &'p Program,
    states: &'p BTreeSet<DefId>,
}

impl Affinity<'_> {
    /// Whether values of `ty` are affine; with `generic`, type parameters count as affine (the
    /// analysis of what a generic body may duplicate).
    fn affine(&self, ty: &Ty, generic: bool) -> bool {
        let mut seen = BTreeSet::new();
        self.walk(ty, generic, &mut seen)
    }
    fn walk(&self, ty: &Ty, generic: bool, seen: &mut BTreeSet<DefId>) -> bool {
        match ty {
            Ty::Param(_) => generic,
            Ty::Wrapper(def) => {
                self.states.contains(def)
                    || (seen.insert(*def)
                        && self
                            .p
                            .def(*def)
                            .kind
                            .as_wrapper()
                            .is_some_and(|w| self.walk(&w.inner, generic, seen)))
            }
            Ty::Adt(def, args) => {
                if self.states.contains(def) || args.iter().any(|a| self.walk(a, generic, seen)) {
                    return true;
                }
                if !seen.insert(*def) {
                    return false;
                }
                match &self.p.def(*def).kind {
                    DefKind::Struct(s) => s.fields.iter().any(|f| self.walk(&f.ty, generic, seen)),
                    DefKind::Enum(e) => e
                        .variants
                        .iter()
                        .any(|v| v.payload.iter().any(|t| self.walk(t, generic, seen))),
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

struct Walk<'a, 'p> {
    affinity: &'a Affinity<'p>,
    locals: &'a [LocalDef],
    duplicates: &'a BTreeMap<DefId, BTreeSet<u32>>,
    /// Type parameters count as affine (the duplication analysis of a generic body).
    generic: bool,
    /// Affine locals consumed on the current path, with the consuming read.
    consumed: BTreeMap<LocalId, Span>,
    /// Each reused local once: the first consumption and the reuse.
    reused: BTreeMap<LocalId, (Span, Span)>,
    /// Generic calls duplicating an affine argument.
    generic_errors: Vec<(Span, DefId)>,
    /// Type parameters of the analyzed generic body passed to a callee that duplicates them.
    duplicating: BTreeSet<u32>,
    in_list: Option<Span>,
}

impl<'a, 'p> Walk<'a, 'p> {
    fn new(
        affinity: &'a Affinity<'p>,
        locals: &'a [LocalDef],
        duplicates: &'a BTreeMap<DefId, BTreeSet<u32>>,
        generic: bool,
    ) -> Self {
        Walk {
            affinity,
            locals,
            duplicates,
            generic,
            consumed: BTreeMap::new(),
            reused: BTreeMap::new(),
            generic_errors: Vec::new(),
            duplicating: BTreeSet::new(),
            in_list: None,
        }
    }

    fn affine(&self, ty: &Ty) -> bool {
        self.affinity.affine(ty, self.generic)
    }

    fn read(&mut self, local: LocalId, span: Span) {
        let Some(def) = self.locals.get(local.0 as usize) else {
            return;
        };
        if !self.affine(&def.ty) {
            return;
        }
        match self.consumed.get(&local) {
            Some(first) => {
                self.reused.entry(local).or_insert((*first, span));
            }
            None => {
                self.consumed.insert(local, span);
            }
        }
    }

    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(t) = &b.tail {
            self.expr(t);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { local, value } => {
                self.expr(value);
                self.consumed.remove(local);
            }
            Stmt::Assign { local, value } | Stmt::Context { local, value } => {
                self.expr(value);
                self.consumed.remove(local);
            }
            Stmt::Discard(e) | Stmt::Expr(e) => self.expr(e),
            Stmt::Assert { expr, .. } => self.expr(expr),
            Stmt::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            // A second pass finds states consumed in one iteration and used in the next.
            Stmt::While { cond, body, .. } => {
                for _ in 0..2 {
                    self.expr(cond);
                    self.block(body);
                }
            }
            Stmt::Concurrent(tasks) => {
                for t in tasks {
                    self.stmt(&t.stmt);
                }
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        if self.in_list.is_none() {
            if let Ty::List(element) = &e.ty {
                if self.affinity.affine(element, false) {
                    self.in_list = Some(e.span);
                }
            }
        }
        match &e.kind {
            ExprKind::Local(id) => self.read(*id, e.span),
            ExprKind::Lit(_) | ExprKind::Hole { .. } | ExprKind::Gen(_) | ExprKind::Error => {}
            ExprKind::Ctor { args, .. } | ExprKind::List(args) => {
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Wrap { arg, .. } | ExprKind::Check { arg, .. } => self.expr(arg),
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::Call {
                callee,
                generic_args,
                args,
                using,
            } => {
                for a in args.iter().chain(using) {
                    self.expr(a);
                }
                if let Some(dup) = self.duplicates.get(callee) {
                    for i in dup {
                        let Some(arg) = generic_args.get(*i as usize) else {
                            continue;
                        };
                        if self.generic {
                            self.duplicating.extend(params_in(arg));
                        } else if self.affinity.affine(arg, false) {
                            self.generic_errors.push((e.span, *callee));
                        }
                    }
                }
            }
            ExprKind::CapCall { handle, args, .. } | ExprKind::ContextOp { handle, args, .. } => {
                self.expr(handle);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Propagate(inner)
            | ExprKind::Unary { operand: inner, .. }
            | ExprKind::Widen { expr: inner, .. }
            | ExprKind::Refine { expr: inner, .. } => self.expr(inner),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            // The body runs once per element: two passes expose a reuse across elements.
            ExprKind::Forall {
                collection,
                predicate,
                ..
            }
            | ExprKind::Parallel {
                collection,
                predicate,
                ..
            } => {
                self.expr(collection);
                self.expr(predicate);
                self.expr(predicate);
            }
            ExprKind::StructLit { spread, fields, .. } => {
                if let Some(s) = spread {
                    self.expr(s);
                }
                for (_, f) in fields {
                    self.expr(f);
                }
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(cond);
                let before = self.consumed.clone();
                self.block(then_block);
                let after_then = std::mem::replace(&mut self.consumed, before);
                if let Some(b) = else_block {
                    self.block(b);
                }
                self.consumed.extend(after_then);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                let before = self.consumed.clone();
                let mut merged = before.clone();
                for arm in arms {
                    self.consumed = before.clone();
                    self.block(&arm.body);
                    merged.extend(std::mem::take(&mut self.consumed));
                }
                self.consumed = merged;
            }
        }
    }
}
