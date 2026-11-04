//! The binding sites of a declaration (`spec/toolchain.md` 1.7.4, T77;
//! semantics S41; `spec/cabi.md` 3.2, C13).
//!
//! A *binding site* is a point of a body whose fault reports the visible
//! bindings: an `assert` (`assertion_failed`'s `bindings`), a `while` loop
//! (the `actual.bindings` of its `invariant` and `decreases` violations),
//! and a hole (`unresolved_hole`'s `bindings`). [`scope_at`] lists, for each
//! site, the locals the interpreter has in scope there, in the order it
//! brought them into scope, so that `cpl-emit-c` writes the binding-site
//! tables of emitted code from the evaluator's own answer instead of a
//! second derivation (C13); `crates/cpl-eval/tests/m2_hooks.rs` checks the
//! interpreter's environment against it at every site the corpus reaches.

use cpl_diag::Span;
use cpl_hir::{Block, DefId, DefKind, Expr, ExprKind, FnBody, LocalId, Pattern, PatternKind};
use cpl_hir::{Program, Stmt};

/// The binding sites of `def` (semantics S41, `spec/cabi.md` 3.2): for
/// every `assert`, `while` loop, and hole of its body, in source order, the
/// span of the site and the locals in scope there, outermost first, in the
/// order the interpreter brings them into scope: the positional parameters,
/// then the `using` parameters (a property's `for` binders in clause order
/// instead), then every `let`, `var`, pattern, and `forall` binder as its
/// evaluation binds it, a block's binders leaving scope at its end (a
/// `match` arm's pattern binders, in the pattern's left-to-right order,
/// for that arm's block; a `forall` binder for its predicate). The `bindings`
/// of a fault list these locals innermost first.
///
/// The span of a site: an `assert`'s asserted expression (the span of its
/// `assertion_failed`, `spec/cabi.md` B.1 `cplrt_bsite.site`), a `while`
/// loop's condition (the HIR keeps no span of the statement itself; the
/// condition is where the loop reads the clock, toolchain 1.7.1), and a
/// hole's own span. Source order is the order of the sites' start offsets;
/// a site that starts where an enclosing one starts (`assert ?h == x`)
/// follows it.
///
/// Only bodies have sites: a contract clause, a refinement predicate, and a
/// property's `for` range hold no statement, and a hole there is the check
/// error `hole_in_contract` (semantics 5.1), so a checked program has none.
/// A declaration without a body (a builtin stub, a type, a capability) has
/// no site; so has an id that `p` does not define.
pub fn scope_at(p: &Program, def: DefId) -> Vec<(Span, Vec<LocalId>)> {
    let Some(d) = p.defs.get(def.0 as usize) else {
        return Vec::new();
    };
    let mut w = Walker {
        scope: Vec::new(),
        sites: Vec::new(),
    };
    match &d.kind {
        DefKind::Fn(f) => {
            if let FnBody::Block(body) = &f.body {
                w.scope.extend(f.params.iter().map(|param| param.local));
                w.scope.extend(f.using.iter().map(|param| param.local));
                w.block(body);
            }
        }
        DefKind::Test(t) => w.block(&t.body),
        DefKind::Property(pd) => {
            w.scope.extend(pd.binders.iter().map(|b| b.local));
            w.block(&pd.body);
        }
        _ => {}
    }
    // A stable sort: sites with the same start keep the traversal order, in
    // which an enclosing site comes before the sites inside it.
    w.sites.sort_by_key(|(span, _)| span.start);
    w.sites
}

/// A walk of a body that mirrors the interpreter's scope discipline
/// (`Interpreter::bind`, `scope_mark`, `restore_scope`).
struct Walker {
    scope: Vec<LocalId>,
    sites: Vec<(Span, Vec<LocalId>)>,
}

impl Walker {
    fn site(&mut self, span: Span) {
        self.sites.push((span, self.scope.clone()));
    }

    /// A block: its binders leave scope at its end (`eval_block`).
    fn block(&mut self, b: &Block) {
        let mark = self.scope.len();
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(tail) = &b.tail {
            self.expr(tail);
        }
        self.scope.truncate(mark);
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { local, value } => {
                self.expr(value);
                self.scope.push(*local);
            }
            Stmt::Discard(e) | Stmt::Expr(e) | Stmt::Assign { value: e, .. } => self.expr(e),
            Stmt::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Stmt::Assert { expr, .. } => {
                self.site(expr.span);
                self.expr(expr);
            }
            // The clauses are contract expressions (no site, semantics 5.1);
            // the body is a block of its own, entered once per iteration.
            Stmt::While { cond, body, .. } => {
                self.site(cond.span);
                self.expr(cond);
                self.block(body);
            }
            // Tasks see the scope before the block; their bindings appear after it.
            Stmt::Concurrent(tasks) => {
                let mark = self.scope.len();
                let mut bound = Vec::new();
                for task in tasks {
                    self.stmt(&task.stmt);
                    bound.extend(self.scope.drain(mark..));
                }
                self.scope.extend(bound);
            }
        }
    }

    /// The sites inside an expression. Only blocks, `match` arms, and
    /// `forall` bring locals into scope, and each removes them again, so
    /// the scope after an expression is the scope before it.
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lit(_) | ExprKind::Local(_) | ExprKind::Gen(_) | ExprKind::Error => {}
            ExprKind::Hole { .. } => self.site(e.span),
            ExprKind::Ctor { args, .. } | ExprKind::List(args) => self.exprs(args),
            ExprKind::Wrap { arg, .. } | ExprKind::Check { arg, .. } => self.expr(arg),
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::Call { args, using, .. } => {
                self.exprs(args);
                self.exprs(using);
            }
            ExprKind::CapCall { handle, args, .. } => {
                self.expr(handle);
                self.exprs(args);
            }
            ExprKind::Propagate(inner)
            | ExprKind::Unary { operand: inner, .. }
            | ExprKind::Widen { expr: inner, .. }
            | ExprKind::Refine { expr: inner, .. } => self.expr(inner),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Forall {
                binder,
                collection,
                predicate,
            }
            | ExprKind::Parallel {
                binder,
                collection,
                predicate,
            } => {
                self.expr(collection);
                self.scope.push(*binder);
                self.expr(predicate);
                self.scope.pop();
            }
            ExprKind::StructLit { spread, fields, .. } => {
                if let Some(base) = spread {
                    self.expr(base);
                }
                for (_, init) in fields {
                    self.expr(init);
                }
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(cond);
                self.block(then_block);
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    let mark = self.scope.len();
                    pattern_binders(&arm.pattern, &mut self.scope);
                    self.block(&arm.body);
                    self.scope.truncate(mark);
                }
            }
        }
    }

    fn exprs(&mut self, es: &[Expr]) {
        for e in es {
            self.expr(e);
        }
    }
}

/// The binders of a pattern in the order a successful match binds them
/// (the interpreter's `matches`: left to right, depth first).
fn pattern_binders(pattern: &Pattern, out: &mut Vec<LocalId>) {
    match &pattern.kind {
        PatternKind::Bind(local) => out.push(*local),
        PatternKind::Ctor { args, .. } => {
            for arg in args {
                pattern_binders(arg, out);
            }
        }
        PatternKind::Unwrap { arg, .. } => pattern_binders(arg, out),
        PatternKind::Wildcard
        | PatternKind::Int(_)
        | PatternKind::Text(_)
        | PatternKind::Bool(_) => {}
    }
}
