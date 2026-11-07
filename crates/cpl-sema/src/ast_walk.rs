//! Walking the *syntax* of a declaration for the effect and contract
//! stages (`spec/semantics.md` 4.4, 5.1, 5.4): every expression and
//! statement of its clauses and bodies, in source order, with the contract
//! clause each lies in.
//!
//! [`crate::bodies::visit_def`] walks the typed HIR, which is what the
//! type-dependent rules need (is this name a handle, which function does
//! this call reach). The rules below are about what the source *writes*,
//! and the HIR does not keep all of it: HIR statements carry no span, a
//! hole in a `for` range leaves no node (the binder's generator is typed
//! from the hole's absence), and an operation call on an unknown operation
//! is an `ExprKind::Error`. So these rules read the AST:
//!
//! * a hole, a `?`, or a statement other than `let` in a contract
//!   expression (5.1, S32): `hole_in_contract`, `propagate_in_contract`,
//!   `statement_in_contract`, spanned at the hole, the `?` expression, and
//!   the statement (S63 (1), (2));
//! * a call of a declaration from its own clauses (S33):
//!   `recursive_contract`, and the self-calls that make a function
//!   recursive (5.4), read like the call graph `Checker::calls` from the
//!   resolution of each callee (`Checker::names`), whether or not the call
//!   could be typed;
//! * a `while` without `decreases` in a `@prove` body (5.4):
//!   `missing_decreases` spanned at the loop (S63 (5));
//! * a `using` parameter that *appears* in no operation call and no
//!   `using` argument (4.4, S29): `unused_capability`.
//!
//! A *contract expression* is the expression of a function's `requires`,
//! `ensures`, or `decreases` clause, of a loop's `invariant` or `decreases`
//! clause, of a refinement's `where` predicate, of a capability
//! operation's `requires` or `ensures` clause, or a property's `for` range
//! (5.1). A function's `effects` clause holds no expression and is not
//! visited.

use crate::checker::Checker;
use cpl_diag::Span;
use cpl_syntax::ast;

/// The contract clause around a node (5.1).
#[derive(Copy, Clone, Debug)]
pub(crate) struct ContractClause<'a> {
    /// The clause from its keyword (S63 (2)): `requires`, `ensures`,
    /// `decreases`, `invariant`, `where`, or `for`.
    pub span: Span,
    /// The clause's expression; its canonical text is the diagnostics'
    /// `clause` field (9.2, `crate::collect::clause_text`).
    pub expr: &'a ast::Expr,
}

/// What a walk calls back. `clause` is the innermost contract clause
/// around the node, `None` in a body.
pub(crate) trait SyntaxVisitor<'a> {
    /// Every expression, pre-order (a call's callee, then its positional
    /// arguments, then its `using` arguments).
    fn expr(&mut self, _e: &'a ast::Expr, _clause: Option<ContractClause<'a>>) {}
    /// Every statement of every block, before its parts.
    fn stmt(&mut self, _s: &'a ast::Statement, _clause: Option<ContractClause<'a>>) {}
}

/// Visit a declaration: a function's clauses then its body; a refinement's
/// predicate; a capability's operation clauses; a test's body; a
/// property's `for` ranges then its body. Other declarations hold no
/// expression.
pub(crate) fn visit_decl<'a>(
    cx: &Checker<'_>,
    decl: &'a ast::Declaration,
    v: &mut impl SyntaxVisitor<'a>,
) {
    match &decl.kind {
        ast::DeclKind::Fn(f) => {
            for clause in &f.clauses {
                match clause {
                    ast::FnClause::Contract(c) => visit_clause(c.span, &c.expr, v),
                    ast::FnClause::Decreases(d) => visit_clause(d.span, &d.expr, v),
                    ast::FnClause::Effects(_) => {}
                }
            }
            if let Some(body) = &f.body {
                visit_block(body, None, v);
            }
        }
        ast::DeclKind::Type(t) => {
            if let ast::TypeDeclKind::Refinement(r) = &t.kind {
                visit_clause(crate::names::where_clause_span(cx, r), &r.predicate, v);
            }
        }
        ast::DeclKind::Capability(c) => {
            for op in &c.operations {
                for clause in &op.clauses {
                    visit_clause(clause.span, &clause.expr, v);
                }
            }
        }
        ast::DeclKind::Test(t) => visit_block(&t.body, None, v),
        ast::DeclKind::Property(p) => {
            for clause in &p.for_clauses {
                visit_clause(clause.span, &clause.range, v);
            }
            visit_block(&p.body, None, v);
        }
        ast::DeclKind::Struct(s) => {
            if let Some(inv) = &s.invariant {
                visit_clause(inv.span, &inv.expr, v);
            }
        }
        ast::DeclKind::Enum(_) => {}
    }
}

fn visit_clause<'a>(span: Span, expr: &'a ast::Expr, v: &mut impl SyntaxVisitor<'a>) {
    visit_expr(expr, Some(ContractClause { span, expr }), v);
}

fn visit_block<'a>(
    b: &'a ast::Block,
    clause: Option<ContractClause<'a>>,
    v: &mut impl SyntaxVisitor<'a>,
) {
    for s in &b.statements {
        v.stmt(s, clause);
        match s {
            ast::Statement::Let(l) => visit_expr(&l.value, clause, v),
            ast::Statement::Var(x) => visit_expr(&x.value, clause, v),
            ast::Statement::Assign(a) => visit_expr(&a.value, clause, v),
            ast::Statement::Context(c) => visit_expr(&c.value, clause, v),
            ast::Statement::Return(r) => {
                if let Some(value) = &r.value {
                    visit_expr(value, clause, v);
                }
            }
            ast::Statement::Assert(a) => visit_expr(&a.condition, clause, v),
            ast::Statement::While(w) => {
                visit_expr(&w.condition, clause, v);
                for c in &w.clauses {
                    match c {
                        ast::WhileClause::Invariant(i) => visit_clause(i.span, &i.expr, v),
                        ast::WhileClause::Decreases(d) => visit_clause(d.span, &d.expr, v),
                    }
                }
                visit_block(&w.body, clause, v);
            }
            ast::Statement::Concurrent(c) => visit_block(&c.body, clause, v),
            ast::Statement::Expr(x) => visit_expr(&x.expr, clause, v),
        }
    }
    if let Some(tail) = &b.tail {
        visit_expr(&tail.expr, clause, v);
    }
}

fn visit_expr<'a>(
    e: &'a ast::Expr,
    clause: Option<ContractClause<'a>>,
    v: &mut impl SyntaxVisitor<'a>,
) {
    use ast::ExprKind as K;
    v.expr(e, clause);
    match &e.kind {
        K::Literal(_) | K::Name(_) | K::Hole(_) => {}
        K::Forall(f) | K::Parallel(f) => {
            visit_expr(&f.collection, clause, v);
            visit_expr(&f.predicate, clause, v);
        }
        K::Binary(b) => {
            visit_expr(&b.lhs, clause, v);
            visit_expr(&b.rhs, clause, v);
        }
        K::Unary(u) => visit_expr(&u.operand, clause, v),
        K::Member(m) => visit_expr(&m.target, clause, v),
        K::Call(c) => {
            visit_expr(&c.callee, clause, v);
            for a in &c.args.positional {
                visit_expr(&a.expr, clause, v);
            }
            for u in &c.args.using {
                visit_expr(&u.expr, clause, v);
            }
        }
        K::Propagate(inner) | K::Paren(inner) => visit_expr(inner, clause, v),
        K::List(l) => {
            for el in &l.elements {
                visit_expr(&el.expr, clause, v);
            }
        }
        K::Struct(s) => {
            if let Some(spread) = &s.entries.spread {
                visit_expr(spread, clause, v);
            }
            for f in &s.entries.fields {
                visit_expr(&f.value, clause, v);
            }
        }
        K::If(i) => visit_if(i, clause, v),
        K::Match(m) => {
            visit_expr(&m.scrutinee, clause, v);
            for arm in &m.arms {
                match &arm.body {
                    ast::ArmBody::Block(b) => visit_block(b, clause, v),
                    ast::ArmBody::Expr(x) => visit_expr(x, clause, v),
                }
            }
        }
    }
}

fn visit_if<'a>(
    i: &'a ast::IfExpr,
    clause: Option<ContractClause<'a>>,
    v: &mut impl SyntaxVisitor<'a>,
) {
    visit_expr(&i.condition, clause, v);
    visit_block(&i.then_block, clause, v);
    match &i.else_branch {
        Some(ast::ElseBranch::Block(b)) => visit_block(b, clause, v),
        Some(ast::ElseBranch::If(nested)) => visit_if(nested, clause, v),
        None => {}
    }
}
