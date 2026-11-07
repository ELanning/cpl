//! Body order over a definition's typed HIR, statements included
//! (`spec/toolchain.md` 8.1 numbers obligations "in body order").
//!
//! The order is `cpl_hir`'s `Expr::walk` (pre-order, children in source
//! order; a `while` visits its condition, invariants, measure, then body;
//! a call its positional then its `using` arguments), with every statement
//! reported before the expressions it holds, so that loops and `assert`
//! statements are numbered in the same pass as calls and divisions.
//!
//! A definition is walked in its canonical order ([`definition`]): a
//! function's `requires`, `ensures`, and `decreases` clauses (the contract
//! facet's order, Section 5.2), then its body; a refinement's predicate; a
//! capability's operations, each with its `requires` then `ensures`
//! clauses; a property's `for` ranges, then its body; a test's body.

use cpl_sema::hir::{Block, Clause, Def, DefKind, Expr, ExprKind, FnBody, GenExpr, Stmt};

/// What a walk reports. Both methods default to doing nothing.
pub(crate) trait Visitor<'h> {
    fn expr(&mut self, _expr: &'h Expr) {}
    fn stmt(&mut self, _stmt: &'h Stmt) {}
}

/// Every clause and body of `def`, in its canonical order (module docs).
pub(crate) fn definition<'h>(def: &'h Def, v: &mut impl Visitor<'h>) {
    match &def.kind {
        DefKind::Fn(f) => {
            clauses(&f.requires, v);
            clauses(&f.ensures, v);
            if let Some(d) = &f.decreases {
                expr(&d.expr, v);
            }
            if let FnBody::Block(b) = &f.body {
                block(b, v);
            }
        }
        DefKind::Refinement(r) => expr(&r.predicate.expr, v),
        DefKind::Struct(s) => {
            if let Some(r) = &s.invariant {
                expr(&r.predicate.expr, v);
            }
        }
        DefKind::Capability(c) => {
            for op in &c.ops {
                clauses(&op.requires, v);
                clauses(&op.ensures, v);
            }
        }
        DefKind::Test(t) => block(&t.body, v),
        DefKind::Property(p) => {
            for binder in &p.binders {
                gen(&binder.gen, v);
            }
            block(&p.body, v);
        }
        DefKind::Wrapper(_) | DefKind::Enum(_) | DefKind::BuiltinType(_) => {}
    }
}

fn clauses<'h>(cs: &'h [Clause], v: &mut impl Visitor<'h>) {
    for c in cs {
        expr(&c.expr, v);
    }
}

/// The statements of a block in order, then its tail.
pub(crate) fn block<'h>(b: &'h Block, v: &mut impl Visitor<'h>) {
    for s in &b.stmts {
        stmt(s, v);
    }
    if let Some(t) = &b.tail {
        expr(t, v);
    }
}

fn stmt<'h>(s: &'h Stmt, v: &mut impl Visitor<'h>) {
    v.stmt(s);
    match s {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } | Stmt::Context { value, .. } => {
            expr(value, v)
        }
        Stmt::Discard(e) | Stmt::Expr(e) | Stmt::Assert { expr: e, .. } => expr(e, v),
        Stmt::Return(e) => {
            if let Some(e) = e {
                expr(e, v);
            }
        }
        Stmt::While {
            cond,
            invariants,
            decreases,
            body,
        } => {
            expr(cond, v);
            clauses(invariants, v);
            if let Some(d) = decreases {
                expr(&d.expr, v);
            }
            block(body, v);
        }
        Stmt::Concurrent(tasks) => {
            for t in tasks {
                stmt(&t.stmt, v);
            }
        }
    }
}

/// `e` and every expression nested in it, pre-order.
pub(crate) fn expr<'h>(e: &'h Expr, v: &mut impl Visitor<'h>) {
    v.expr(e);
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Local(_) | ExprKind::Hole { .. } | ExprKind::Error => {}
        ExprKind::Ctor { args, .. } | ExprKind::List(args) => {
            for a in args {
                expr(a, v);
            }
        }
        ExprKind::Wrap { arg, .. } | ExprKind::Check { arg, .. } => expr(arg, v),
        ExprKind::Field { base, .. } => expr(base, v),
        ExprKind::Call { args, using, .. } => {
            for a in args.iter().chain(using) {
                expr(a, v);
            }
        }
        ExprKind::CapCall { handle, args, .. } | ExprKind::ContextOp { handle, args, .. } => {
            expr(handle, v);
            for a in args {
                expr(a, v);
            }
        }
        ExprKind::Propagate(inner) => expr(inner, v),
        ExprKind::Unary { operand, .. } => expr(operand, v),
        ExprKind::Binary { lhs, rhs, .. } => {
            expr(lhs, v);
            expr(rhs, v);
        }
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
            expr(collection, v);
            expr(predicate, v);
        }
        ExprKind::StructLit { spread, fields, .. } => {
            if let Some(s) = spread {
                expr(s, v);
            }
            for (_, f) in fields {
                expr(f, v);
            }
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            expr(cond, v);
            block(then_block, v);
            if let Some(b) = else_block {
                block(b, v);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            expr(scrutinee, v);
            for arm in arms {
                block(&arm.body, v);
            }
        }
        ExprKind::Widen { expr: inner, .. } | ExprKind::Refine { expr: inner, .. } => {
            expr(inner, v)
        }
        ExprKind::Gen(g) => gen(g, v),
    }
}

/// The `gen.range` bounds of a generator expression.
fn gen<'h>(g: &'h GenExpr, v: &mut impl Visitor<'h>) {
    match g {
        GenExpr::Int | GenExpr::PositiveInt | GenExpr::Bool | GenExpr::Text => {}
        GenExpr::List(inner) | GenExpr::Option(inner) => gen(inner, v),
        GenExpr::Range(lo, hi) => {
            expr(lo, v);
            expr(hi, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpl_diag::Span;
    use cpl_project::{load_project, LoadOptions};
    use std::path::Path;

    /// Expression spans in walk order.
    #[derive(Default)]
    struct Spans(Vec<Span>);

    impl<'h> Visitor<'h> for Spans {
        fn expr(&mut self, e: &'h Expr) {
            self.0.push(e.span);
        }
    }

    /// The walk visits expressions in `cpl_hir`'s body order (the order its
    /// `Expr::walk` defines for the obligation numbering of Section 8.1),
    /// over every definition of the corpus project and of std.
    #[test]
    fn expressions_come_in_the_hirs_body_order() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/reader");
        let project = load_project(&root, &LoadOptions::default()).expect("loads");
        let program = cpl_sema::check(&project).program;
        let mut compared = 0;
        for def in program.defs() {
            let mut ours = Spans::default();
            definition(def, &mut ours);
            let mut theirs = Vec::new();
            let mut push = |e: &Expr| theirs.push(e.span);
            match &def.kind {
                DefKind::Fn(f) => {
                    for c in f.requires.iter().chain(&f.ensures).chain(&f.decreases) {
                        c.expr.walk(&mut push);
                    }
                    if let FnBody::Block(b) = &f.body {
                        b.walk(&mut push);
                    }
                }
                DefKind::Refinement(r) => r.predicate.expr.walk(&mut push),
                DefKind::Capability(c) => {
                    for op in &c.ops {
                        for clause in op.requires.iter().chain(&op.ensures) {
                            clause.expr.walk(&mut push);
                        }
                    }
                }
                DefKind::Test(t) => t.body.walk(&mut push),
                DefKind::Property(p) => {
                    for b in &p.binders {
                        b.gen.walk(&mut push);
                    }
                    p.body.walk(&mut push);
                }
                _ => {}
            }
            compared += theirs.len();
            assert_eq!(ours.0, theirs, "{}", program.display_path(def.id));
        }
        assert!(compared > 100, "{compared} expressions compared");
    }
}
