//! The lowering of the typed HIR to the IR of [`crate::lir`]: one [`LFn`] per function of the
//! program and one [`LPred`] per refinement or struct invariant.
//!
//! Evaluation stays strict and left to right (semantics 6.2): an operand that needs statements
//! (a block, `if`, `match`, `?`, a `forall`) runs after every operand to its left, which is
//! first bound to a temporary unless reading it again cannot differ. Contracts are checked where
//! the evaluator checks them (toolchain 1.7.1): `requires` and the header measure on entry,
//! `ensures` on every exit, loop invariants before the first test and after every iteration,
//! the loop measure after the condition held, a struct's invariant at each literal the checker
//! did not establish, and a capability operation's clauses around its call.

use crate::lir::*;
use cpl_hir::{
    Block, Clause, DefId, DefKind, Expr, ExprKind, FnBody, FnDef, LocalDef, LocalId, Pattern,
    PatternKind, Program, RefinementDef, Stmt, Ty,
};
use cpl_syntax::ast::{BinaryOp, UnaryOp};

/// The lowered program.
pub struct Lowered {
    pub fns: Vec<LFn>,
    pub preds: Vec<LPred>,
    /// The definitions a printer declares: the project's, and the standard library's
    /// refinements the project reaches.
    pub defs: std::collections::BTreeSet<DefId>,
}

/// The functions and predicates of the program's own modules (and the standard library's
/// refinements), in definition order.
pub fn lower_program(p: &Program, emit: &dyn Fn(DefId) -> bool) -> Lowered {
    let mut fns = Vec::new();
    let mut preds = Vec::new();
    let mut defs = std::collections::BTreeSet::new();
    for d in p.defs() {
        if !emit(d.id) {
            continue;
        }
        defs.insert(d.id);
        match &d.kind {
            DefKind::Fn(f) if matches!(f.body, FnBody::Block(_)) => fns.push(lower_fn(p, d.id, f)),
            DefKind::Refinement(r) => preds.push(lower_pred(p, d.id, r, &[])),
            DefKind::Struct(s) => {
                if let Some(inv) = &s.invariant {
                    preds.push(lower_pred(p, d.id, inv, &s.generics));
                }
            }
            _ => {}
        }
    }
    for f in &mut fns {
        prune_clause_scopes(&mut f.requires);
        prune_clause_scopes(&mut f.body);
        prune_clause_scopes(&mut f.ensures);
        if let Some((stmts, _, _)) = &mut f.measure {
            prune_clause_scopes(stmts);
        }
    }
    for q in &mut preds {
        prune_clause_scopes(&mut q.body);
    }
    Lowered { fns, preds, defs }
}

/// The declarations whose predicates `check<R>` evaluates, in order (semantics 6.10): a
/// struct's invariant alone, else the refinement chain from the root base upward.
pub fn check_order(p: &Program, refined: DefId) -> Vec<DefId> {
    if p.def(refined)
        .kind
        .as_struct()
        .is_some_and(|s| s.invariant.is_some())
    {
        return vec![refined];
    }
    let mut order = chain(p, refined);
    order.reverse();
    order
}

/// `refined` and the refinements it refines, outermost first.
pub fn chain(p: &Program, refined: DefId) -> Vec<DefId> {
    let mut out = Vec::new();
    let mut current = Some(refined);
    while let Some(def) = current {
        if out.len() >= 64 || out.contains(&def) {
            break;
        }
        let Some(r) = p.def(def).kind.as_refinement() else {
            break;
        };
        out.push(def);
        current = match &r.base {
            Ty::Refined(_, next) => Some(*next),
            _ => None,
        };
    }
    out
}

/// The prelude's types render by bare name, every other declaration by qualified name
/// (`ConstraintError.type_name`, `$type`; stdlib S44, toolchain T30).
pub fn type_name(p: &Program, def: DefId) -> String {
    let d = p.def(def);
    if p.module(d.module).path == ["std", "prelude"] {
        d.name.display()
    } else {
        p.display_path(def)
    }
}

/// The qualified identity of a capability operation (`reader.pages.PageRead.get`).
pub fn operation_identity(p: &Program, capability: DefId, op: &str) -> String {
    format!("{}.{op}", p.display_path(capability))
}

fn lower_fn(p: &Program, def: DefId, f: &FnDef) -> LFn {
    let mut x = Fx::new(p, def, f.ret.clone(), &f.locals);
    let params: Vec<Var> = f.params.iter().map(|q| x.var_of(q.local)).collect();
    let using: Vec<Var> = f.using.iter().map(|q| x.var_of(q.local)).collect();
    let symbol = x.symbol.clone();
    let mut requires = Vec::new();
    for c in &f.requires {
        x.check_clause(c, "requires", &symbol, &mut requires);
    }
    let measure = f.decreases.as_ref().map(|c| {
        let mut stmts = vec![LStmt::ClauseEnter {
            owner: symbol.clone(),
        }];
        let value = x.expr(&c.expr, &mut stmts);
        let value = x.bind(value, &Ty::Int, &mut stmts);
        push(&mut stmts, LStmt::ClauseLeave);
        (stmts, value, c.text.clone())
    });
    let mut body = Vec::new();
    if let FnBody::Block(b) = &f.body {
        let v = x.block(b, &mut body);
        push(&mut body, LStmt::Return(v));
    }
    let mut ensures = Vec::new();
    for c in &f.ensures {
        x.check_clause(c, "ensures", &symbol, &mut ensures);
    }
    let result = f.result_local.map(|l| x.var_of(l));
    LFn {
        def,
        generics: f.generics.clone(),
        params,
        using,
        ret: f.ret.clone(),
        vars: x.vars,
        requires,
        measure,
        body,
        ensures,
        result,
    }
}

fn lower_pred(p: &Program, def: DefId, r: &RefinementDef, generics: &[String]) -> LPred {
    let mut x = Fx::new(p, def, Ty::Bool, &r.locals);
    let self_var = x.var_of(r.self_local);
    // S34: a fault inside the predicate is the refinement's (or the struct's).
    let mut body = vec![LStmt::ClauseEnter {
        owner: x.symbol.clone(),
    }];
    let value = x.expr(&r.predicate.expr, &mut body);
    let value = x.bind(value, &Ty::Bool, &mut body);
    push(&mut body, LStmt::ClauseLeave);
    let self_ty = x.vars[self_var as usize].ty.clone();
    LPred {
        def,
        generics: generics.to_vec(),
        self_var,
        self_ty,
        vars: x.vars,
        body,
        value,
    }
}

/// Whether a statement list cannot complete normally.
pub fn diverges(stmts: &[LStmt]) -> bool {
    match stmts.last() {
        Some(LStmt::Return(_) | LStmt::Fault(_) | LStmt::Break { .. }) => true,
        Some(LStmt::If { then, els, .. }) => diverges(then) && diverges(els),
        Some(LStmt::Match { arms, .. }) => arms.iter().all(|a| diverges(&a.body)),
        Some(LStmt::Loop { label, body }) => !breaks(body, *label),
        _ => false,
    }
}

/// Whether `stmts` contain a `Break` of `label` (a loop ends normally only through one).
pub fn breaks(stmts: &[LStmt], label: u32) -> bool {
    stmts.iter().any(|s| match s {
        LStmt::Break { label: l } => *l == label,
        LStmt::If { then, els, .. } => breaks(then, label) || breaks(els, label),
        LStmt::Match { arms, .. } => arms.iter().any(|a| breaks(&a.body, label)),
        LStmt::Loop { body, .. } | LStmt::ForEach { body, .. } => breaks(body, label),
        _ => false,
    })
}

/// Append `s` unless the list already diverges: nothing after a `return`, a fault, or a
/// `break` would run, and some hosts reject unreachable statements.
fn push(out: &mut Vec<LStmt>, s: LStmt) {
    if !diverges(out) {
        out.push(s);
    }
}

struct Fx<'p> {
    p: &'p Program,
    /// The blamed declaration of faults raised here (the stable identity).
    symbol: String,
    ret: Ty,
    vars: Vec<VarDef>,
    /// The local tables in scope: the definition's own, then an operation's while its clauses
    /// are lowered.
    maps: Vec<Vec<Option<Var>>>,
    labels: u32,
}

impl<'p> Fx<'p> {
    fn new(p: &'p Program, owner: DefId, ret: Ty, locals: &[LocalDef]) -> Fx<'p> {
        let vars: Vec<VarDef> = locals
            .iter()
            .map(|l| VarDef {
                name: l.name.clone(),
                ty: l.ty.clone(),
                mutable: l.mutable,
            })
            .collect();
        let map = (0..locals.len() as u32).map(Some).collect();
        Fx {
            p,
            symbol: p.def(owner).stable_id.as_str().to_string(),
            ret,
            vars,
            maps: vec![map],
            labels: 0,
        }
    }

    fn var_of(&self, local: LocalId) -> Var {
        self.maps
            .last()
            .and_then(|m| m.get(local.0 as usize).copied().flatten())
            .unwrap_or_else(|| panic!("an unmapped local {}", local.0))
    }

    fn temp(&mut self, name: &str, ty: Ty, mutable: bool) -> Var {
        self.vars.push(VarDef {
            name: name.to_string(),
            ty,
            mutable,
        });
        (self.vars.len() - 1) as Var
    }

    fn label(&mut self) -> u32 {
        self.labels += 1;
        self.labels
    }

    /// `value` in a fresh immutable temporary, unless it is already stable.
    fn bind(&mut self, value: LExpr, ty: &Ty, out: &mut Vec<LStmt>) -> LExpr {
        if value.is_stable(&self.vars) {
            return value;
        }
        let t = self.temp("t", ty.clone(), false);
        push(out, LStmt::Let { var: t, value });
        LExpr::Var(t)
    }

    fn std(&self) -> &cpl_hir::StdDefs {
        self.p.std_defs()
    }

    fn fault(
        &self,
        code: &'static str,
        kind: Option<&'static str>,
        clause: &str,
        symbol: &str,
    ) -> Fault {
        let message = match (code, kind) {
            ("contract_violation", Some(k)) => format!("{k} clause failed: {clause}"),
            ("assertion_failed", _) => format!("assertion failed: {clause}"),
            ("unresolved_hole", _) => format!("evaluation reached the unresolved hole ?{clause}"),
            _ => clause.to_string(),
        };
        Fault {
            code,
            clause_kind: kind,
            clause: clause.to_string(),
            symbol: symbol.to_string(),
            message,
        }
    }

    /// A contract clause that must hold: lowered, then `if not c { fault }`.
    fn check_clause(&mut self, c: &Clause, kind: &'static str, symbol: &str, out: &mut Vec<LStmt>) {
        let saved = std::mem::replace(&mut self.symbol, symbol.to_string());
        // S34: a fault escaping the clause is the clause owner's; the clause's own failure,
        // raised after it, is not inside it.
        push(
            out,
            LStmt::ClauseEnter {
                owner: symbol.to_string(),
            },
        );
        let v = self.expr(&c.expr, out);
        let v = self.bind(v, &Ty::Bool, out);
        push(out, LStmt::ClauseLeave);
        self.symbol = saved;
        let fault = self.fault("contract_violation", Some(kind), &c.text, symbol);
        push(
            out,
            LStmt::If {
                cond: not(v),
                then: vec![LStmt::Fault(fault)],
                els: Vec::new(),
            },
        );
    }

    // ---- blocks and statements -----------------------------------------------------------

    fn block(&mut self, b: &Block, out: &mut Vec<LStmt>) -> LExpr {
        for s in &b.stmts {
            if diverges(out) {
                return LExpr::Unit;
            }
            self.stmt(s, out);
        }
        match &b.tail {
            Some(t) if !diverges(out) => self.expr(t, out),
            _ => LExpr::Unit,
        }
    }

    fn stmt(&mut self, s: &Stmt, out: &mut Vec<LStmt>) {
        match s {
            Stmt::Let { local, value } => {
                let v = self.expr(value, out);
                let var = self.var_of(*local);
                push(out, LStmt::Let { var, value: v });
            }
            Stmt::Discard(e) | Stmt::Expr(e) => {
                let v = self.expr(e, out);
                if !v.is_stable(&self.vars) && !matches!(v, LExpr::Var(_)) {
                    push(out, LStmt::Expr(v));
                }
            }
            Stmt::Assign { local, value } => {
                let var = self.var_of(*local);
                // `xs = xs.push(v)`: the host appends in place where it can.
                if let ExprKind::Call { callee, args, .. } = &value.kind {
                    let push_call = self.p.def(*callee).kind.as_fn().is_some_and(
                        |f| matches!(&f.body, FnBody::Builtin(id) if id == "list_push"),
                    );
                    if push_call
                        && args.len() == 2
                        && matches!(args[0].kind, ExprKind::Local(l) if l == *local)
                    {
                        let v = self.expr(&args[1], out);
                        push(
                            out,
                            LStmt::Push {
                                list: var,
                                value: v,
                            },
                        );
                        return;
                    }
                }
                let v = self.expr(value, out);
                push(out, LStmt::Assign { var, value: v });
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => self.expr(e, out),
                    None => LExpr::Unit,
                };
                push(out, LStmt::Return(v));
            }
            Stmt::Assert { expr, text } => {
                let c = self.expr(expr, out);
                let fault = self.fault("assertion_failed", None, text, &self.symbol.clone());
                push(
                    out,
                    LStmt::If {
                        cond: not(c),
                        then: vec![LStmt::Fault(fault)],
                        els: Vec::new(),
                    },
                );
            }
            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
            } => self.while_loop(cond, invariants, decreases.as_ref(), body, out),
            Stmt::Concurrent(tasks) => {
                for task in tasks {
                    if diverges(out) {
                        return;
                    }
                    self.stmt(&task.stmt, out);
                }
            }
            // the call stack's state machine, a cell of the host's (spec/state-machines.md)
            Stmt::Context { local, value } => {
                let v = self.expr(value, out);
                let var = self.var_of(*local);
                push(
                    out,
                    LStmt::Let {
                        var,
                        value: LExpr::ContextNew(Box::new(v)),
                    },
                );
            }
        }
    }

    /// `app.state()`, or `app.dispatch(event)`: the event, then the root machine called on the current state, its
    /// new state committed, its commands (or `Unit`) the value.
    fn context_op(
        &mut self,
        handle: &Expr,
        program: DefId,
        op: cpl_hir::ContextOp,
        args: &[Expr],
        out: &mut Vec<LStmt>,
    ) -> LExpr {
        let p = self.p;
        let mut all: Vec<&Expr> = vec![handle];
        all.extend(args.iter());
        let mut vals = self.exprs(&all, out);
        let rest = vals.split_off(1);
        let h = vals.pop().expect("a handle");
        let machine = p.root_machine(program);
        let (cpl_hir::ContextOp::Dispatch, Some(machine), Some(event)) = (op, machine, rest.into_iter().next())
        else {
            return LExpr::ContextGet(Box::new(h));
        };
        let sig = p.machine_signature(machine).expect("a root machine's signature");
        let ret = p.def(machine).kind.as_fn().expect("a machine").ret.clone();
        let state = self.temp("state", sig.state.clone(), false);
        push(
            out,
            LStmt::Let {
                var: state,
                value: LExpr::ContextGet(Box::new(h.clone())),
            },
        );
        let step = self.temp("step", ret.clone(), false);
        push(
            out,
            LStmt::Let {
                var: step,
                value: LExpr::Call {
                    def: machine,
                    generics: Vec::new(),
                    args: vec![LExpr::Var(state), event],
                    using: Vec::new(),
                },
            },
        );
        let field = |index: u32| -> Option<LExpr> {
            let Ty::Adt(def, _) = &ret else {
                return None;
            };
            Some(LExpr::Field {
                base: Box::new(LExpr::Var(step)),
                def: *def,
                index,
                ty: ret.clone(),
            })
        };
        let next = sig.state_field.and_then(field).unwrap_or(LExpr::Var(step));
        push(out, LStmt::ContextSet { handle: h, value: next });
        sig.commands_field.and_then(field).unwrap_or(LExpr::Unit)
    }

    /// Semantics 5.3: the invariants before the first test, then per iteration the condition,
    /// the measure (`>= 0` and below the previous one), the body, and the invariants again.
    fn while_loop(
        &mut self,
        cond: &Expr,
        invariants: &[Clause],
        decreases: Option<&Clause>,
        body: &Block,
        out: &mut Vec<LStmt>,
    ) {
        let symbol = self.symbol.clone();
        for c in invariants {
            self.check_clause(c, "invariant", &symbol, out);
        }
        let measure = decreases.map(|_| {
            let has = self.temp("has_measure", Ty::Bool, true);
            let prev = self.temp("measure", Ty::Int, true);
            push(
                out,
                LStmt::Let {
                    var: has,
                    value: LExpr::Bool(false),
                },
            );
            push(
                out,
                LStmt::Let {
                    var: prev,
                    value: LExpr::Int(0.into()),
                },
            );
            (has, prev)
        });
        let label = self.label();
        let mut b = Vec::new();
        let c = self.expr(cond, &mut b);
        push(
            &mut b,
            LStmt::If {
                cond: not(c),
                then: vec![LStmt::Break { label }],
                els: Vec::new(),
            },
        );
        if let (Some(clause), Some((has, prev))) = (decreases, measure) {
            push(
                &mut b,
                LStmt::ClauseEnter {
                    owner: symbol.clone(),
                },
            );
            let m = self.expr(&clause.expr, &mut b);
            let m = self.bind(m, &Ty::Int, &mut b);
            push(&mut b, LStmt::ClauseLeave);
            let negative = op(Op::Lt, vec![m.clone(), LExpr::Int(0.into())], Ty::Int);
            let not_below = op(
                Op::And,
                vec![
                    LExpr::Var(has),
                    op(Op::Ge, vec![m.clone(), LExpr::Var(prev)], Ty::Int),
                ],
                Ty::Bool,
            );
            let fault = self.fault(
                "contract_violation",
                Some("decreases"),
                &clause.text,
                &symbol,
            );
            push(
                &mut b,
                LStmt::If {
                    cond: op(Op::Or, vec![negative, not_below], Ty::Bool),
                    then: vec![LStmt::Fault(fault)],
                    els: Vec::new(),
                },
            );
            push(
                &mut b,
                LStmt::Assign {
                    var: prev,
                    value: m,
                },
            );
            push(
                &mut b,
                LStmt::Assign {
                    var: has,
                    value: LExpr::Bool(true),
                },
            );
        }
        let v = self.block(body, &mut b);
        if !v.is_stable(&self.vars) && !matches!(v, LExpr::Var(_)) {
            push(&mut b, LStmt::Expr(v));
        }
        for c in invariants {
            if diverges(&b) {
                break;
            }
            self.check_clause(c, "invariant", &symbol, &mut b);
        }
        push(out, LStmt::Loop { label, body: b });
    }

    // ---- expressions ---------------------------------------------------------------------

    /// Operands left to right: when an operand needs statements, every operand before it that
    /// could read differently afterwards is bound to a temporary first.
    fn exprs(&mut self, es: &[&Expr], out: &mut Vec<LStmt>) -> Vec<LExpr> {
        let mut vals: Vec<(LExpr, Ty)> = Vec::with_capacity(es.len());
        for e in es {
            let mut buf = Vec::new();
            let v = self.expr(e, &mut buf);
            if !buf.is_empty() {
                for (prev, ty) in vals.iter_mut() {
                    if !prev.is_stable(&self.vars) {
                        let value = std::mem::replace(prev, LExpr::Unit);
                        let ty = ty.clone();
                        *prev = self.bind(value, &ty, out);
                    }
                }
                for s in buf {
                    push(out, s);
                }
            }
            vals.push((v, e.ty.clone()));
        }
        vals.into_iter().map(|(v, _)| v).collect()
    }

    fn expr(&mut self, e: &Expr, out: &mut Vec<LStmt>) -> LExpr {
        let p = self.p;
        match &e.kind {
            ExprKind::Lit(cpl_hir::Lit::Int(n)) => LExpr::Int(n.clone()),
            ExprKind::Lit(cpl_hir::Lit::Text(s)) => LExpr::Text(s.clone()),
            ExprKind::Lit(cpl_hir::Lit::Bool(b)) => LExpr::Bool(*b),
            ExprKind::Local(l) => LExpr::Var(self.var_of(*l)),
            ExprKind::Ctor { def, variant, args } => {
                let vals = self.exprs(&args.iter().collect::<Vec<_>>(), out);
                if *def == self.std().unit {
                    return LExpr::Unit;
                }
                LExpr::Ctor {
                    def: *def,
                    variant: *variant,
                    args: vals,
                    ty: e.ty.clone(),
                }
            }
            ExprKind::Wrap { def, arg } => LExpr::Wrap {
                def: *def,
                arg: Box::new(self.expr(arg, out)),
            },
            ExprKind::Field { base, def, index } => LExpr::Field {
                base: Box::new(self.expr(base, out)),
                def: *def,
                index: *index,
                ty: base.ty.clone(),
            },
            ExprKind::Call {
                callee,
                generic_args,
                args,
                using,
            } => {
                let all: Vec<&Expr> = args.iter().chain(using.iter()).collect();
                let mut vals = self.exprs(&all, out);
                let handles = vals.split_off(args.len());
                match p.def(*callee).kind.as_fn().map(|f| &f.body) {
                    Some(FnBody::Builtin(id)) => LExpr::Builtin {
                        id: id.clone(),
                        generics: generic_args.clone(),
                        args: vals,
                        ty: e.ty.clone(),
                    },
                    _ => LExpr::Call {
                        def: *callee,
                        generics: generic_args.clone(),
                        args: vals,
                        using: handles,
                    },
                }
            }
            ExprKind::ContextOp {
                handle,
                program,
                op,
                args,
            } => self.context_op(handle, *program, *op, args, out),
            ExprKind::CapCall {
                handle,
                capability,
                op: index,
                args,
            } => self.cap_call(handle, *capability, *index, args, &e.ty, out),
            ExprKind::Check { refined, arg } => self.check(*refined, arg, &e.ty, out),
            ExprKind::Propagate(inner) => {
                let v = self.expr(inner, out);
                let t = self.bind(v, &inner.ty, out);
                let result = self.std().result;
                let error = LExpr::Payload {
                    base: Box::new(t.clone()),
                    def: result,
                    variant: 1,
                    index: 0,
                    ty: inner.ty.clone(),
                };
                let exit = LExpr::Ctor {
                    def: result,
                    variant: 1,
                    args: vec![error],
                    ty: self.ret.clone(),
                };
                push(
                    out,
                    LStmt::If {
                        cond: LExpr::IsVariant {
                            base: Box::new(t.clone()),
                            def: result,
                            variant: 1,
                            ty: inner.ty.clone(),
                        },
                        then: vec![LStmt::Return(exit)],
                        els: Vec::new(),
                    },
                );
                LExpr::Payload {
                    base: Box::new(t),
                    def: result,
                    variant: 0,
                    index: 0,
                    ty: inner.ty.clone(),
                }
            }
            ExprKind::Unary { op: u, operand } => {
                let v = self.expr(operand, out);
                match u {
                    UnaryOp::Not => not(v),
                    UnaryOp::Neg => op(Op::Neg, vec![v], Ty::Int),
                }
            }
            ExprKind::Binary { op: b, lhs, rhs } => self.binary(*b, lhs, rhs, out),
            ExprKind::Parallel {
                binder,
                collection,
                predicate,
            } => {
                let list = self.expr(collection, out);
                let list = self.bind(list, &collection.ty, out);
                let Ty::List(elem) = &e.ty else {
                    unreachable!("a parallel map is a list")
                };
                let acc = self.temp("mapped", e.ty.clone(), true);
                push(
                    out,
                    LStmt::Let {
                        var: acc,
                        value: LExpr::List {
                            elem: (**elem).clone(),
                            items: Vec::new(),
                        },
                    },
                );
                let label = self.label();
                let mut body = Vec::new();
                let v = self.expr(predicate, &mut body);
                push(
                    &mut body,
                    LStmt::Push {
                        list: acc,
                        value: v,
                    },
                );
                push(
                    out,
                    LStmt::ForEach {
                        label,
                        var: self.var_of(*binder),
                        list,
                        body,
                    },
                );
                LExpr::Var(acc)
            }
            ExprKind::Forall {
                binder,
                collection,
                predicate,
            } => {
                let list = self.expr(collection, out);
                let list = self.bind(list, &collection.ty, out);
                let all = self.temp("holds", Ty::Bool, true);
                push(
                    out,
                    LStmt::Let {
                        var: all,
                        value: LExpr::Bool(true),
                    },
                );
                let label = self.label();
                let mut body = Vec::new();
                let v = self.expr(predicate, &mut body);
                push(
                    &mut body,
                    LStmt::If {
                        cond: not(v),
                        then: vec![
                            LStmt::Assign {
                                var: all,
                                value: LExpr::Bool(false),
                            },
                            LStmt::Break { label },
                        ],
                        els: Vec::new(),
                    },
                );
                push(
                    out,
                    LStmt::ForEach {
                        label,
                        var: self.var_of(*binder),
                        list,
                        body,
                    },
                );
                LExpr::Var(all)
            }
            ExprKind::Hole { name, .. } => {
                let fault = self.fault("unresolved_hole", None, name, &self.symbol.clone());
                self.unreachable_value(fault, &e.ty, out)
            }
            ExprKind::List(items) => {
                let vals = self.exprs(&items.iter().collect::<Vec<_>>(), out);
                let Ty::List(elem) = &e.ty else {
                    unreachable!("a list literal is a list")
                };
                LExpr::List {
                    elem: (**elem).clone(),
                    items: vals,
                }
            }
            ExprKind::StructLit {
                def,
                invariant_proven,
                spread,
                fields,
            } => self.struct_lit(
                *def,
                *invariant_proven,
                spread.as_deref(),
                fields,
                &e.ty,
                out,
            ),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let c = self.expr(cond, out);
                let unit = self.is_unit(&e.ty);
                let r = (!unit).then(|| self.temp("r", e.ty.clone(), true));
                if let Some(r) = r {
                    push(out, LStmt::Decl { var: r });
                }
                let mut then = Vec::new();
                let v = self.block(then_block, &mut then);
                self.finish(&mut then, r, v);
                let mut els = Vec::new();
                if let Some(b) = else_block {
                    let v = self.block(b, &mut els);
                    self.finish(&mut els, r, v);
                }
                push(out, LStmt::If { cond: c, then, els });
                r.map_or(LExpr::Unit, LExpr::Var)
            }
            ExprKind::Match { scrutinee, arms } => {
                let s = self.expr(scrutinee, out);
                let s = match s {
                    LExpr::Var(v) => v,
                    other => {
                        let t = self.temp("s", scrutinee.ty.clone(), false);
                        push(
                            out,
                            LStmt::Let {
                                var: t,
                                value: other,
                            },
                        );
                        t
                    }
                };
                let unit = self.is_unit(&e.ty);
                let r = (!unit).then(|| self.temp("r", e.ty.clone(), true));
                if let Some(r) = r {
                    push(out, LStmt::Decl { var: r });
                }
                let mut larms = Vec::new();
                for arm in arms {
                    let pattern = self.pattern(&arm.pattern);
                    let mut body = Vec::new();
                    let v = self.block(&arm.body, &mut body);
                    self.finish(&mut body, r, v);
                    larms.push(LArm { pattern, body });
                }
                push(
                    out,
                    LStmt::Match {
                        scrutinee: s,
                        arms: larms,
                    },
                );
                r.map_or(LExpr::Unit, LExpr::Var)
            }
            ExprKind::Widen { expr, .. } => self.expr(expr, out),
            // Statically established (T39); refinements are erased (semantics 6.1).
            ExprKind::Refine { expr, .. } => self.expr(expr, out),
            ExprKind::Gen(_) | ExprKind::Error => {
                let fault = self.fault(
                    "internal_error",
                    None,
                    "a generator or an error node is not a value",
                    &self.symbol.clone(),
                );
                self.unreachable_value(fault, &e.ty, out)
            }
        }
    }

    /// A value of `ty` that is never produced: the fault, then a placeholder.
    fn unreachable_value(&mut self, fault: Fault, ty: &Ty, out: &mut Vec<LStmt>) -> LExpr {
        let r = self.temp("never", ty.clone(), true);
        push(out, LStmt::Decl { var: r });
        push(out, LStmt::Fault(fault));
        LExpr::Var(r)
    }

    fn is_unit(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Adt(d, _) if *d == self.std().unit)
    }

    /// End a branch: assign its value to the result slot.
    fn finish(&mut self, stmts: &mut Vec<LStmt>, r: Option<Var>, v: LExpr) {
        match r {
            Some(r) => push(stmts, LStmt::Assign { var: r, value: v }),
            None => {
                if !v.is_stable(&self.vars) && !matches!(v, LExpr::Var(_)) {
                    push(stmts, LStmt::Expr(v));
                }
            }
        }
    }

    fn binary(&mut self, b: BinaryOp, lhs: &Expr, rhs: &Expr, out: &mut Vec<LStmt>) -> LExpr {
        if matches!(b, BinaryOp::And | BinaryOp::Or) {
            let l = self.expr(lhs, out);
            let mut buf = Vec::new();
            let r = self.expr(rhs, &mut buf);
            let o = if b == BinaryOp::And { Op::And } else { Op::Or };
            if buf.is_empty() {
                return op(o, vec![l, r], Ty::Bool);
            }
            // The right operand's statements run only when it is needed.
            let t = self.temp("cond", Ty::Bool, true);
            push(out, LStmt::Let { var: t, value: l });
            push(&mut buf, LStmt::Assign { var: t, value: r });
            let cond = if o == Op::And {
                LExpr::Var(t)
            } else {
                not(LExpr::Var(t))
            };
            push(
                out,
                LStmt::If {
                    cond,
                    then: buf,
                    els: Vec::new(),
                },
            );
            return LExpr::Var(t);
        }
        let mut vals = self.exprs(&[lhs, rhs], out);
        let r = vals.pop().expect("two operands");
        let l = vals.pop().expect("two operands");
        let ty = lhs.ty.clone();
        match b {
            BinaryOp::Div | BinaryOp::Rem => LExpr::Div {
                a: Box::new(l),
                b: Box::new(r),
                rem: b == BinaryOp::Rem,
                symbol: self.symbol.clone(),
            },
            BinaryOp::Add => op(Op::Add, vec![l, r], Ty::Int),
            BinaryOp::Sub => op(Op::Sub, vec![l, r], Ty::Int),
            BinaryOp::Mul => op(Op::Mul, vec![l, r], Ty::Int),
            BinaryOp::Lt => op(Op::Lt, vec![l, r], ty),
            BinaryOp::Le => op(Op::Le, vec![l, r], ty),
            BinaryOp::Gt => op(Op::Gt, vec![l, r], ty),
            BinaryOp::Ge => op(Op::Ge, vec![l, r], ty),
            BinaryOp::Eq => op(Op::Eq, vec![l, r], ty),
            BinaryOp::Ne => op(Op::Ne, vec![l, r], ty),
            BinaryOp::Concat => op(Op::Concat, vec![l, r], ty),
            BinaryOp::And | BinaryOp::Or => unreachable!(),
        }
    }

    fn struct_lit(
        &mut self,
        def: DefId,
        invariant_proven: bool,
        spread: Option<&Expr>,
        fields: &[(u32, Expr)],
        ty: &Ty,
        out: &mut Vec<LStmt>,
    ) -> LExpr {
        let p = self.p;
        let s = p
            .def(def)
            .kind
            .as_struct()
            .expect("a struct literal's struct");
        let base = spread.map(|e| {
            let v = self.expr(e, out);
            self.bind(v, &e.ty, out)
        });
        // The initializers run in source order: bind every one that is not stable, so that
        // placing them in declaration order evaluates nothing out of order.
        let exprs: Vec<&Expr> = fields.iter().map(|(_, e)| e).collect();
        let vals = self.exprs(&exprs, out);
        let mut given: Vec<Option<LExpr>> = vec![None; s.fields.len()];
        for ((index, e), v) in fields.iter().zip(vals) {
            let v = self.bind(v, &e.ty, out);
            given[*index as usize] = Some(v);
        }
        let all: Vec<LExpr> = given
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.unwrap_or_else(|| LExpr::Field {
                    base: Box::new(base.clone().expect("a field from the spread")),
                    def,
                    index: i as u32,
                    ty: ty.clone(),
                })
            })
            .collect();
        let value = LExpr::Struct {
            def,
            fields: all,
            ty: ty.clone(),
        };
        let Some(inv) = &s.invariant else {
            return value;
        };
        if invariant_proven {
            return value;
        }
        let t = self.temp("value", ty.clone(), false);
        push(out, LStmt::Let { var: t, value });
        let generics = match ty {
            Ty::Adt(_, args) => args.clone(),
            _ => Vec::new(),
        };
        let fault = self.fault(
            "contract_violation",
            Some("invariant"),
            &inv.predicate.text,
            p.def(def).stable_id.as_str(),
        );
        push(
            out,
            LStmt::If {
                cond: not(LExpr::Pred {
                    def,
                    generics,
                    arg: Box::new(LExpr::Var(t)),
                }),
                then: vec![LStmt::Fault(fault)],
                els: Vec::new(),
            },
        );
        LExpr::Var(t)
    }

    /// `check<R>(x)`: the predicates of R's chain root first; the first that fails gives
    /// `Err(ConstraintError)`, all holding give `Ok(x)`.
    fn check(&mut self, refined: DefId, arg: &Expr, ty: &Ty, out: &mut Vec<LStmt>) -> LExpr {
        let p = self.p;
        let v = self.expr(arg, out);
        let x = self.bind(v, &arg.ty, out);
        let r = self.temp("checked", ty.clone(), true);
        push(out, LStmt::Decl { var: r });
        let label = self.label();
        let mut body = Vec::new();
        let generics = match &arg.ty {
            Ty::Adt(_, args) => args.clone(),
            _ => Vec::new(),
        };
        for def in check_order(p, refined) {
            let Some(rd) = p.def(def).kind.checked_predicate() else {
                continue;
            };
            let failure = LExpr::CheckFailure {
                value: Box::new(x.clone()),
                value_ty: arg.ty.clone(),
                type_name: type_name(p, def),
                predicate: rd.predicate.text.clone(),
                ty: ty.clone(),
            };
            body.push(LStmt::If {
                cond: not(LExpr::Pred {
                    def,
                    generics: if p.def(def).kind.as_struct().is_some() {
                        generics.clone()
                    } else {
                        Vec::new()
                    },
                    arg: Box::new(x.clone()),
                }),
                then: vec![
                    LStmt::Assign {
                        var: r,
                        value: failure,
                    },
                    LStmt::Break { label },
                ],
                els: Vec::new(),
            });
        }
        body.push(LStmt::Assign {
            var: r,
            value: LExpr::Ctor {
                def: self.std().result,
                variant: 0,
                args: vec![x],
                ty: ty.clone(),
            },
        });
        body.push(LStmt::Break { label });
        push(out, LStmt::Loop { label, body });
        LExpr::Var(r)
    }

    /// A capability operation: its `requires` (the caller's obligation), the call, then its
    /// `ensures` with `result` bound (semantics S31).
    fn cap_call(
        &mut self,
        handle: &Expr,
        capability: DefId,
        index: u32,
        args: &[Expr],
        ty: &Ty,
        out: &mut Vec<LStmt>,
    ) -> LExpr {
        let p = self.p;
        let mut all: Vec<&Expr> = vec![handle];
        all.extend(args.iter());
        let mut vals = self.exprs(&all, out);
        let rest = vals.split_off(1);
        let h = vals.pop().expect("a handle");
        let opdef = &p
            .def(capability)
            .kind
            .as_capability()
            .expect("an operation's capability")
            .ops[index as usize];
        let identity = operation_identity(p, capability, &opdef.name);
        if opdef.requires.is_empty() && opdef.ensures.is_empty() {
            // A statement of its own: a host whose operation returns nothing (Go and Java,
            // for an operation returning Unit) calls it as a statement.
            let result = self.temp("result", ty.clone(), true);
            push(
                out,
                LStmt::Let {
                    var: result,
                    value: LExpr::CapCall {
                        handle: Box::new(h),
                        capability,
                        op: index,
                        args: rest,
                        ty: ty.clone(),
                    },
                },
            );
            self.validate_result(result, &opdef.ret, &identity, out);
            return LExpr::Var(result);
        }
        // The operation's own locals, bound to this call's arguments and result.
        let mut map: Vec<Option<Var>> = vec![None; opdef.locals.len()];
        let mut bound = Vec::new();
        for (param, v) in opdef.params.iter().zip(rest) {
            let t = self.temp(&param.name, param.ty.clone(), false);
            push(out, LStmt::Let { var: t, value: v });
            map[param.local.0 as usize] = Some(t);
            bound.push(LExpr::Var(t));
        }
        for l in &opdef.locals {
            if map[l.id.0 as usize].is_none() {
                map[l.id.0 as usize] = Some(self.temp(&l.name, l.ty.clone(), false));
            }
        }
        self.maps.push(map);
        for c in &opdef.requires {
            self.check_clause(c, "requires", &identity, out);
        }
        let result = match opdef.result_local {
            Some(l) => self.var_of(l),
            None => self.temp("result", ty.clone(), true),
        };
        self.vars[result as usize].mutable = true;
        push(
            out,
            LStmt::Let {
                var: result,
                value: LExpr::CapCall {
                    handle: Box::new(h),
                    capability,
                    op: index,
                    args: bound,
                    ty: ty.clone(),
                },
            },
        );
        self.validate_result(result, &opdef.ret, &identity, out);
        for c in &opdef.ensures {
            self.check_clause(c, "ensures", &identity, out);
        }
        self.maps.pop();
        LExpr::Var(result)
    }

    /// Semantics S77: a capability is the host's code, so what an operation returns is
    /// validated against the refinements and invariants of its return type before the
    /// `ensures` see it. A result that fails is an `Err(ForeignError { kind: InvalidResult })`
    /// when the operation returns `Result<T, ForeignError>`, else a `refinement_violation`
    /// blamed on the operation.
    fn validate_result(&mut self, result: Var, ret: &Ty, identity: &str, out: &mut Vec<LStmt>) {
        let p = self.p;
        // A host whose strings may not be Unicode checks the texts too (the printer drops the
        // check where it is all there would be in a host whose strings always are).
        if !crate::needs_validation(p, ret) && !crate::contains_text(p, ret) {
            return;
        }
        let expected = p.display_ty(ret);
        let message = format!("the result of {identity} does not satisfy {expected}");
        let invalid = LExpr::Valid {
            ty: ret.clone(),
            arg: Box::new(LExpr::Var(result)),
        };
        let (result_def, foreign_error) = (self.std().result, self.std().foreign_error);
        let foreign = matches!(ret, Ty::Adt(r, args) if *r == result_def
            && args.len() == 2
            && matches!(&args[1], Ty::Adt(e, a) if *e == foreign_error && a.is_empty()));
        let named = |name: &str| {
            p.defs()
                .find(|d| p.module(d.module).path == ["std", "foreign"] && d.name.display() == name)
                .map(|d| d.id)
        };
        let then = match (foreign, named("ForeignErrorKind"), named("ForeignDetail")) {
            (true, Some(kind), Some(detail)) => {
                let variant = p
                    .def(kind)
                    .kind
                    .as_enum()
                    .and_then(|e| e.variants.iter().position(|v| v.name == "InvalidResult"))
                    .unwrap_or(2) as u32;
                let detail_ty = Ty::Adt(detail, Vec::new());
                let pair = |key: &str, value: String| LExpr::Struct {
                    def: detail,
                    fields: vec![LExpr::Text(key.into()), LExpr::Text(value)],
                    ty: detail_ty.clone(),
                };
                let error = LExpr::Struct {
                    def: foreign_error,
                    fields: vec![
                        LExpr::Ctor {
                            def: kind,
                            variant,
                            args: Vec::new(),
                            ty: Ty::Adt(kind, Vec::new()),
                        },
                        LExpr::Text(message),
                        LExpr::List {
                            elem: detail_ty.clone(),
                            items: vec![
                                pair("operation", identity.to_string()),
                                pair("expected_type", expected),
                            ],
                        },
                    ],
                    ty: Ty::Adt(foreign_error, Vec::new()),
                };
                LStmt::Assign {
                    var: result,
                    value: LExpr::Ctor {
                        def: result_def,
                        variant: 1,
                        args: vec![error],
                        ty: ret.clone(),
                    },
                }
            }
            _ => LStmt::Fault(Fault {
                code: "refinement_violation",
                clause_kind: None,
                clause: String::new(),
                symbol: identity.to_string(),
                message,
            }),
        };
        push(
            out,
            LStmt::If {
                cond: not(invalid),
                then: vec![then],
                els: Vec::new(),
            },
        );
    }

    fn pattern(&mut self, pat: &Pattern) -> LPat {
        match &pat.kind {
            PatternKind::Wildcard => LPat::Wild,
            PatternKind::Int(n) => LPat::Int(n.clone()),
            PatternKind::Text(s) => LPat::Text(s.clone()),
            PatternKind::Bool(b) => LPat::Bool(*b),
            PatternKind::Bind(l) => LPat::Bind(self.var_of(*l)),
            PatternKind::Ctor { def, variant, args } => {
                if *def == self.std().unit {
                    return LPat::Wild;
                }
                LPat::Ctor {
                    def: *def,
                    variant: *variant,
                    args: args.iter().map(|a| self.pattern(a)).collect(),
                    ty: pat.ty.clone(),
                }
            }
            PatternKind::Unwrap { def, arg } => LPat::Unwrap {
                def: *def,
                arg: Box::new(self.pattern(arg)),
            },
        }
    }
}

fn op(op: Op, args: Vec<LExpr>, ty: Ty) -> LExpr {
    LExpr::Op { op, args, ty }
}

fn not(v: LExpr) -> LExpr {
    match v {
        LExpr::Bool(b) => LExpr::Bool(!b),
        LExpr::Op {
            op: Op::Not,
            mut args,
            ..
        } => args.pop().expect("one operand"),
        other => op(Op::Not, vec![other], Ty::Bool),
    }
}
