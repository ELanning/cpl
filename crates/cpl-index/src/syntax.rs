//! Pre-order walks of a declaration's syntax (`cpl_syntax::ast`), for what
//! the typed HIR does not keep (`spec/toolchain.md` 3.3, 3.5): the canonical
//! text of a divisor and the span of an `assert` statement (statements
//! carry no span in the HIR), and the call expressions of the generator
//! calls of a property's `for` clauses, which the typing stage lowers to a
//! span-less `GenExpr` (T18).
//!
//! A declaration is walked in source order: a function's clauses (except
//! `effects`, which holds no expression) then its body; a refinement's
//! predicate; a capability's operation clauses; a property's `for` ranges
//! then its body; a test's body. Every statement is reported before the
//! expressions it holds, and every expression before its operands.

use cpl_diag::Span;
use cpl_syntax::ast::{self, DeclKind, Declaration};

/// Every expression of a declaration's syntax, pre-order.
pub(crate) fn exprs<'a>(decl: &'a Declaration, f: &mut dyn FnMut(&'a ast::Expr)) {
    declaration(decl, &mut |node| {
        if let Node::Expr(e) = node {
            f(e)
        }
    });
}

/// Every statement of a declaration's syntax, pre-order.
pub(crate) fn stmts<'a>(decl: &'a Declaration, f: &mut dyn FnMut(&'a ast::Statement)) {
    declaration(decl, &mut |node| {
        if let Node::Stmt(s) = node {
            f(s)
        }
    });
}

/// Every call expression of a declaration's syntax as `(callee span, call
/// span)`, in source order: `gen.range(0, limit())` is the call whose
/// callee is `gen.range`.
pub(crate) fn calls(decl: &Declaration) -> Vec<(Span, Span)> {
    let mut out = Vec::new();
    exprs(decl, &mut |e| {
        if let ast::ExprKind::Call(c) = &e.kind {
            out.push((c.callee.span, e.span));
        }
    });
    out
}

/// One node of a syntax walk.
#[derive(Copy, Clone)]
enum Node<'a> {
    Expr(&'a ast::Expr),
    Stmt(&'a ast::Statement),
}

fn declaration<'a>(decl: &'a Declaration, f: &mut dyn FnMut(Node<'a>)) {
    match &decl.kind {
        DeclKind::Fn(fd) => {
            for c in &fd.clauses {
                match c {
                    ast::FnClause::Contract(c) => expr(&c.expr, f),
                    ast::FnClause::Decreases(d) => expr(&d.expr, f),
                    ast::FnClause::Effects(_) => {}
                }
            }
            if let Some(b) = &fd.body {
                block(b, f);
            }
        }
        DeclKind::Type(t) => {
            if let ast::TypeDeclKind::Refinement(r) = &t.kind {
                expr(&r.predicate, f);
            }
        }
        DeclKind::Capability(c) => {
            for op in &c.operations {
                for clause in &op.clauses {
                    expr(&clause.expr, f);
                }
            }
        }
        DeclKind::Test(t) => block(&t.body, f),
        DeclKind::Property(p) => {
            for clause in &p.for_clauses {
                expr(&clause.range, f);
            }
            block(&p.body, f);
        }
        DeclKind::Struct(s) => {
            if let Some(invariant) = &s.invariant {
                expr(&invariant.expr, f);
            }
        }
        DeclKind::Enum(_) => {}
    }
}

fn block<'a>(b: &'a ast::Block, f: &mut dyn FnMut(Node<'a>)) {
    for s in &b.statements {
        f(Node::Stmt(s));
        match s {
            ast::Statement::Let(s) => expr(&s.value, f),
            ast::Statement::Var(s) => expr(&s.value, f),
            ast::Statement::Assign(s) => expr(&s.value, f),
            ast::Statement::Return(s) => {
                if let Some(e) = &s.value {
                    expr(e, f);
                }
            }
            ast::Statement::Assert(s) => expr(&s.condition, f),
            ast::Statement::While(w) => {
                expr(&w.condition, f);
                for c in &w.clauses {
                    match c {
                        ast::WhileClause::Invariant(i) => expr(&i.expr, f),
                        ast::WhileClause::Decreases(d) => expr(&d.expr, f),
                    }
                }
                block(&w.body, f);
            }
            ast::Statement::Expr(s) => expr(&s.expr, f),
        }
    }
    if let Some(t) = &b.tail {
        expr(&t.expr, f);
    }
}

fn expr<'a>(e: &'a ast::Expr, f: &mut dyn FnMut(Node<'a>)) {
    f(Node::Expr(e));
    match &e.kind {
        ast::ExprKind::Literal(_) | ast::ExprKind::Name(_) | ast::ExprKind::Hole(_) => {}
        ast::ExprKind::Forall(x) | ast::ExprKind::Parallel(x) => {
            expr(&x.collection, f);
            expr(&x.predicate, f);
        }
        ast::ExprKind::Binary(b) => {
            expr(&b.lhs, f);
            expr(&b.rhs, f);
        }
        ast::ExprKind::Unary(u) => expr(&u.operand, f),
        ast::ExprKind::Member(m) => expr(&m.target, f),
        ast::ExprKind::Call(c) => {
            expr(&c.callee, f);
            for a in &c.args.positional {
                expr(&a.expr, f);
            }
            for a in &c.args.using {
                expr(&a.expr, f);
            }
        }
        ast::ExprKind::Propagate(inner) | ast::ExprKind::Paren(inner) => expr(inner, f),
        ast::ExprKind::List(l) => {
            for el in &l.elements {
                expr(&el.expr, f);
            }
        }
        ast::ExprKind::Struct(s) => {
            if let Some(spread) = &s.entries.spread {
                expr(spread, f);
            }
            for field in &s.entries.fields {
                expr(&field.value, f);
            }
        }
        ast::ExprKind::If(i) => if_expr(i, f),
        ast::ExprKind::Match(m) => {
            expr(&m.scrutinee, f);
            for arm in &m.arms {
                match &arm.body {
                    ast::ArmBody::Block(b) => block(b, f),
                    ast::ArmBody::Expr(e) => expr(e, f),
                }
            }
        }
    }
}

fn if_expr<'a>(i: &'a ast::IfExpr, f: &mut dyn FnMut(Node<'a>)) {
    expr(&i.condition, f);
    block(&i.then_block, f);
    match &i.else_branch {
        Some(ast::ElseBranch::Block(b)) => block(b, f),
        Some(ast::ElseBranch::If(inner)) => if_expr(inner, f),
        None => {}
    }
}
