//! The obligation sites of a function, in the walk order of `cpl-index` (`spec/toolchain.md`
//! 8.1, T62): its `requires`, `ensures`, and `decreases` clauses, then its body, pre-order.

use crate::Site;
use cpl_hir::{Block, Clause, Expr, ExprKind, FnDef, GenExpr, Program, Stmt};
use cpl_syntax::ast::BinaryOp;

/// Every site of `f` whose obligation the prover decides, with the suffix of its obligation id
/// (`ensures/1`, `call/3/requires/1`, ..., numbered as toolchain 8.1 and T62 number them), and
/// whether a loop of its body has no measure.
pub fn of(p: &Program, f: &FnDef) -> (Vec<(Site, String)>, bool) {
    let mut w = Walk {
        p,
        out: Vec::new(),
        unmeasured: false,
        loops: 0,
        calls: 0,
        divisions: 0,
        asserts: 0,
        constructors: 0,
    };
    for c in &f.requires {
        w.expr(&c.expr);
    }
    for c in &f.ensures {
        w.expr(&c.expr);
    }
    if let Some(d) = &f.decreases {
        w.expr(&d.expr);
    }
    if let cpl_hir::FnBody::Block(b) = &f.body {
        w.block(b);
    }
    let mut out: Vec<(Site, String)> = (0..f.ensures.len())
        .map(|i| (Site::Ensures(i), format!("ensures/{}", i + 1)))
        .collect();
    if f.decreases.is_some() {
        out.push((Site::Decreases, "decreases".to_string()));
    }
    out.extend(w.out);
    (out, w.unmeasured)
}

struct Walk<'p> {
    p: &'p Program,
    out: Vec<(Site, String)>,
    unmeasured: bool,
    /// Every site of each kind seen so far, those without an obligation included (T62).
    loops: usize,
    calls: usize,
    divisions: usize,
    asserts: usize,
    constructors: usize,
}

impl Walk<'_> {
    fn clauses(&mut self, cs: &[Clause]) {
        for c in cs {
            self.expr(&c.expr);
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
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => self.expr(value),
            Stmt::Discard(e) | Stmt::Expr(e) => self.expr(e),
            Stmt::Assert { expr, .. } => {
                self.asserts += 1;
                self.out
                    .push((Site::Assert(expr.span), format!("assert/{}", self.asserts)));
                self.expr(expr);
            }
            Stmt::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
            } => {
                self.loops += 1;
                for i in 0..invariants.len() {
                    self.out.push((
                        Site::InvariantInit(cond.span, i),
                        format!("loop/{}/invariant/{}/init", self.loops, i + 1),
                    ));
                    self.out.push((
                        Site::InvariantPreserve(cond.span, i),
                        format!("loop/{}/invariant/{}/preserve", self.loops, i + 1),
                    ));
                }
                match decreases {
                    Some(_) => self.out.push((
                        Site::LoopDecreases(cond.span),
                        format!("loop/{}/decreases", self.loops),
                    )),
                    None => self.unmeasured = true,
                }
                self.expr(cond);
                self.clauses(invariants);
                if let Some(d) = decreases {
                    self.expr(&d.expr);
                }
                self.block(body);
            }
            Stmt::Concurrent(tasks) => {
                for t in tasks {
                    self.stmt(&t.stmt);
                }
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Call {
                callee,
                args,
                using,
                ..
            } => {
                let n = self
                    .p
                    .def(*callee)
                    .kind
                    .as_fn()
                    .map_or(0, |f| f.requires.len());
                self.calls += 1;
                let c = self.calls;
                self.out.extend((0..n).map(|i| {
                    (
                        Site::CallRequires(e.span, i),
                        format!("call/{c}/requires/{}", i + 1),
                    )
                }));
                for a in args.iter().chain(using) {
                    self.expr(a);
                }
            }
            ExprKind::CapCall {
                handle,
                capability,
                op,
                args,
            } => {
                let n = self
                    .p
                    .def(*capability)
                    .kind
                    .as_capability()
                    .and_then(|c| c.ops.get(*op as usize))
                    .map_or(0, |o| o.requires.len());
                self.calls += 1;
                let c = self.calls;
                self.out.extend((0..n).map(|i| {
                    (
                        Site::CallRequires(e.span, i),
                        format!("call/{c}/requires/{}", i + 1),
                    )
                }));
                self.expr(handle);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                if matches!(op, BinaryOp::Div | BinaryOp::Rem) {
                    self.divisions += 1;
                    self.out
                        .push((Site::Division(e.span), format!("div/{}", self.divisions)));
                }
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::StructLit {
                def,
                invariant_proven,
                spread,
                fields,
            } => {
                let invariant = self
                    .p
                    .def(*def)
                    .kind
                    .as_struct()
                    .is_some_and(|s| s.invariant.is_some());
                if invariant {
                    self.constructors += 1;
                    if !invariant_proven {
                        self.out.push((
                            Site::Constructor(e.span),
                            format!("constructor/{}", self.constructors),
                        ));
                    }
                }
                if let Some(s) = spread {
                    self.expr(s);
                }
                for (_, f) in fields {
                    self.expr(f);
                }
            }
            ExprKind::Lit(_) | ExprKind::Local(_) | ExprKind::Hole { .. } | ExprKind::Error => {}
            ExprKind::Ctor { args, .. } | ExprKind::List(args) => {
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Wrap { arg, .. } | ExprKind::Check { arg, .. } => self.expr(arg),
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::Propagate(inner) => self.expr(inner),
            ExprKind::Unary { operand, .. } => self.expr(operand),
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
                    self.block(&arm.body);
                }
            }
            ExprKind::Widen { expr, .. } | ExprKind::Refine { expr, .. } => self.expr(expr),
            ExprKind::Gen(g) => self.gen(g),
        }
    }

    fn gen(&mut self, g: &GenExpr) {
        match g {
            GenExpr::Int | GenExpr::PositiveInt | GenExpr::Bool | GenExpr::Text => {}
            GenExpr::List(inner) | GenExpr::Option(inner) => self.gen(inner),
            GenExpr::Range(lo, hi) => {
                self.expr(lo);
                self.expr(hi);
            }
        }
    }
}
