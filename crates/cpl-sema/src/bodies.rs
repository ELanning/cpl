//! Walking the typed bodies for the effect and contract stages
//! (`spec/semantics.md` 4.2-4.5, 5.1; toolchain 3.3): every expression and
//! statement of a definition, in body order, with the contract clause it
//! lies in (if any) and its position in its parent.
//!
//! `cpl_hir::Expr::walk` visits the same expressions without context; the
//! rules of S27 (a handle may appear only as an operation receiver or a
//! `using` argument, never as an operand of `==`/`!=`), of 4.5 (contract
//! expressions are typed with an empty bound), and of 5.1 (no hole, `?`,
//! statement other than `let`, or `using` argument in a contract
//! expression) need both, which [`visit_def`] supplies.
//!
//! The typer's conventions (`crate::types` module docs) that these stages
//! rely on: a handle in any position is `ExprKind::Local` of type
//! `Ty::Capability`; a call whose callee is a handle (`db(x)`) is that
//! `Local` alone; a contract hole is an `ExprKind::Hole` with no report;
//! `var` is `Stmt::Let` of a mutable local.

use cpl_hir::{Block, Clause, Def, DefKind, Expr, ExprKind, FnBody, GenExpr, Stmt};
use cpl_syntax::ast::BinaryOp;

/// Where a visited expression or statement stands.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Site<'h> {
    /// Inside a contract expression (5.1): a `requires`, `ensures`, or
    /// `decreases` clause, a loop `invariant` or `decreases`, a refinement's
    /// `where` predicate, a capability operation's clause, or a property
    /// `for` range.
    pub contract: bool,
    /// The clause of a contract expression (its `span` from the keyword and
    /// canonical `text`, S63 (2), 9.2 `clause`); `None` in bodies and in a
    /// property's `for` range (whose clause has no HIR `Clause`).
    pub clause: Option<&'h Clause>,
    /// The expression's position in its parent (S27).
    pub position: Position,
}

/// The position of an expression in its parent, for the handle rules of
/// S27.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Position {
    /// Any other position: a handle here is `capability_as_value`.
    Value,
    /// The handle of an `ExprKind::CapCall`.
    Receiver,
    /// A `using` argument of an `ExprKind::Call` (in a contract expression,
    /// the typer's `capability_as_value`, 5.1).
    UsingArg,
    /// An operand of `==` or `!=`: a handle here is `capability_equality`.
    Compared,
}

/// What a walk calls back.
pub(crate) trait Visitor<'h> {
    /// Every expression, pre-order.
    fn expr(&mut self, _e: &'h Expr, _site: Site<'h>) {}
    /// Every statement of every block, before its expressions (S32 needs
    /// the ones in contract expressions; `site.position` is `Value`).
    fn stmt(&mut self, _s: &'h Stmt, _site: Site<'h>) {}
}

/// Visit every expression and statement of a definition: a function's
/// clauses then its body, a property's `for` range bounds then its body, a
/// test body, a refinement's predicate, and a capability's operation
/// clauses. Builtin bodies and placeholders have nothing to visit.
pub(crate) fn visit_def<'h>(def: &'h Def, v: &mut impl Visitor<'h>) {
    match &def.kind {
        DefKind::Fn(f) => {
            for c in f.requires.iter().chain(&f.ensures).chain(&f.decreases) {
                visit_clause(c, v);
            }
            if let FnBody::Block(b) = &f.body {
                visit_block(b, BODY, v);
            }
        }
        DefKind::Test(t) => visit_block(&t.body, BODY, v),
        DefKind::Property(p) => {
            for b in &p.binders {
                visit_gen(&b.gen, v);
            }
            visit_block(&p.body, BODY, v);
        }
        DefKind::Refinement(r) => visit_clause(&r.predicate, v),
        DefKind::Struct(s) => {
            if let Some(inv) = &s.invariant {
                visit_clause(&inv.predicate, v);
            }
        }
        DefKind::Capability(c) => {
            for op in &c.ops {
                for cl in op.requires.iter().chain(&op.ensures) {
                    visit_clause(cl, v);
                }
            }
        }
        _ => {}
    }
}

/// The context of a body expression.
const BODY: Ctx<'static> = Ctx {
    contract: false,
    clause: None,
};

/// The context a sub-expression inherits.
#[derive(Copy, Clone)]
struct Ctx<'h> {
    contract: bool,
    clause: Option<&'h Clause>,
}

impl<'h> Ctx<'h> {
    fn site(self, position: Position) -> Site<'h> {
        Site {
            contract: self.contract,
            clause: self.clause,
            position,
        }
    }
}

fn visit_clause<'h>(c: &'h Clause, v: &mut impl Visitor<'h>) {
    let ctx = Ctx {
        contract: true,
        clause: Some(c),
    };
    visit_expr_in(&c.expr, ctx, Position::Value, v);
}

/// The bounds of `gen.range` in a `for` range (contract expressions).
fn visit_gen<'h>(g: &'h GenExpr, v: &mut impl Visitor<'h>) {
    let ctx = Ctx {
        contract: true,
        clause: None,
    };
    match g {
        GenExpr::List(x) | GenExpr::Option(x) => visit_gen(x, v),
        GenExpr::Range(lo, hi) => {
            visit_expr_in(lo, ctx, Position::Value, v);
            visit_expr_in(hi, ctx, Position::Value, v);
        }
        _ => {}
    }
}

fn visit_block<'h>(b: &'h Block, ctx: Ctx<'h>, v: &mut impl Visitor<'h>) {
    for s in &b.stmts {
        visit_stmt(s, ctx, v);
    }
    if let Some(t) = &b.tail {
        visit_expr_in(t, ctx, Position::Value, v);
    }
}

fn visit_stmt<'h>(s: &'h Stmt, ctx: Ctx<'h>, v: &mut impl Visitor<'h>) {
    v.stmt(s, ctx.site(Position::Value));
    match s {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            visit_expr_in(value, ctx, Position::Value, v)
        }
        Stmt::Discard(e) | Stmt::Expr(e) | Stmt::Assert { expr: e, .. } => {
            visit_expr_in(e, ctx, Position::Value, v)
        }
        Stmt::Return(e) => {
            if let Some(e) = e {
                visit_expr_in(e, ctx, Position::Value, v);
            }
        }
        Stmt::While {
            cond,
            invariants,
            decreases,
            body,
        } => {
            visit_expr_in(cond, ctx, Position::Value, v);
            for c in invariants.iter().chain(decreases) {
                visit_clause(c, v);
            }
            visit_block(body, ctx, v);
        }
        Stmt::Concurrent(tasks) => {
            for t in tasks {
                visit_stmt(&t.stmt, ctx, v);
            }
        }
    }
}

pub(crate) fn visit_expression<'h>(e: &'h Expr, v: &mut impl Visitor<'h>) {
    visit_expr_in(e, BODY, Position::Value, v);
}

fn visit_expr_in<'h>(e: &'h Expr, ctx: Ctx<'h>, position: Position, v: &mut impl Visitor<'h>) {
    v.expr(e, ctx.site(position));
    let value = Position::Value;
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Local(_) | ExprKind::Hole { .. } | ExprKind::Error => {}
        ExprKind::Ctor { args, .. } | ExprKind::List(args) => {
            for a in args {
                visit_expr_in(a, ctx, value, v);
            }
        }
        ExprKind::Wrap { arg, .. } | ExprKind::Check { arg, .. } => {
            visit_expr_in(arg, ctx, value, v)
        }
        ExprKind::Field { base, .. } => visit_expr_in(base, ctx, value, v),
        ExprKind::Call { args, using, .. } => {
            for a in args {
                visit_expr_in(a, ctx, value, v);
            }
            for u in using {
                visit_expr_in(u, ctx, Position::UsingArg, v);
            }
        }
        ExprKind::CapCall { handle, args, .. } => {
            visit_expr_in(handle, ctx, Position::Receiver, v);
            for a in args {
                visit_expr_in(a, ctx, value, v);
            }
        }
        ExprKind::Propagate(x)
        | ExprKind::Widen { expr: x, .. }
        | ExprKind::Refine { expr: x, .. } => visit_expr_in(x, ctx, value, v),
        ExprKind::Unary { operand, .. } => visit_expr_in(operand, ctx, value, v),
        ExprKind::Binary { op, lhs, rhs } => {
            let p = match op {
                BinaryOp::Eq | BinaryOp::Ne => Position::Compared,
                _ => value,
            };
            visit_expr_in(lhs, ctx, p, v);
            visit_expr_in(rhs, ctx, p, v);
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
            visit_expr_in(collection, ctx, value, v);
            visit_expr_in(predicate, ctx, value, v);
        }
        ExprKind::StructLit { spread, fields, .. } => {
            if let Some(s) = spread {
                visit_expr_in(s, ctx, value, v);
            }
            for (_, f) in fields {
                visit_expr_in(f, ctx, value, v);
            }
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            visit_expr_in(cond, ctx, value, v);
            visit_block(then_block, ctx, v);
            if let Some(b) = else_block {
                visit_block(b, ctx, v);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            visit_expr_in(scrutinee, ctx, value, v);
            for arm in arms {
                visit_block(&arm.body, ctx, v);
            }
        }
        ExprKind::Gen(g) => visit_gen(g, v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{load_project_from_tree, LoadOptions, VirtualTree};
    use cpl_diag::FileOrigin;
    use cpl_hir::Ty;

    #[derive(Default)]
    struct Collect(
        Vec<(&'static str, Option<String>, Position)>,
        Vec<(bool, Option<String>)>,
    );

    impl<'h> Visitor<'h> for Collect {
        fn expr(&mut self, e: &'h Expr, site: Site<'h>) {
            let what = match &e.kind {
                ExprKind::Local(_) if matches!(e.ty, Ty::Capability(_)) => "handle",
                ExprKind::CapCall { .. } => "capcall",
                ExprKind::Call { .. } => "call",
                ExprKind::Hole { .. } => "hole",
                _ => return,
            };
            assert_eq!(site.contract, site.clause.is_some());
            self.0
                .push((what, site.clause.map(|c| c.text.clone()), site.position));
        }

        fn stmt(&mut self, _s: &'h Stmt, site: Site<'h>) {
            self.1
                .push((site.contract, site.clause.map(|c| c.text.clone())));
        }
    }

    #[test]
    fn sites_and_positions() {
        let source = "module m.v\n\npub capability Store {\n    fn get(key: Text) -> Int\n}\n\nfn read(key: Text, using s: Store) -> Int\n    effects { s.get }\n{\n    s.get(key)\n}\n\nfn f(key: Text, using db: Store) -> Bool\n    requires read(key, using db) > 0\n    ensures if result { let ok = true\n ok } else { true }\n    effects { db.get }\n{\n    var n = db.get(key)\n    while n > 0\n        invariant ?inv\n    {\n        n = n - 1\n    }\n    db == db\n}\n";
        let mut tree = VirtualTree::default();
        tree.files.insert(
            "cpl.toml".into(),
            b"[project]\nname = \"t\"\nedition = \"0.1\"\n".to_vec(),
        );
        tree.files
            .insert("src/m/v.cpl".into(), source.as_bytes().to_vec());
        let options = LoadOptions {
            include_work_files: true,
            ..LoadOptions::default()
        };
        let project =
            load_project_from_tree(&tree, FileOrigin::Virtual, &options).expect("the tree loads");
        let result = crate::check::check(&project);
        let f = result
            .program
            .defs()
            .find(|d| result.program.display_path(d.id) == "m.v.f")
            .expect("f");
        let mut c = Collect::default();
        visit_def(f, &mut c);
        let requires = Some("read(key, using db) > 0".to_string());
        assert_eq!(
            c.0,
            [
                ("call", requires.clone(), Position::Value),
                ("handle", requires, Position::UsingArg),
                ("capcall", None, Position::Value),
                ("handle", None, Position::Receiver),
                ("hole", Some("?inv".to_string()), Position::Value),
                ("handle", None, Position::Compared),
                ("handle", None, Position::Compared),
            ]
        );
        // The `let` in the `ensures` sub-block, then the body's `var` and
        // `while`. The clause text is one line (semantics 9.2 `clause`).
        let ensures = Some("if result { let ok = true ok } else { true }".to_string());
        assert_eq!(c.1[0], (true, ensures));
        assert_eq!(&c.1[1..], [(false, None), (false, None), (false, None)]);
    }
}
