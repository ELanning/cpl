//! Traversal and span-insensitive comparison of HIR trees.
//!
//! [`Expr::walk`] visits expressions in *body order*: pre-order, children
//! in source order (a `while` visits its condition, invariants, measure,
//! then body; a call its positional then its `using` arguments). The
//! obligation numbering of Section 8.1 (`#div/<n>`, `#refine/<n>`,
//! `#call/<n>`, `#assert/<n>`) counts in this order.

use crate::*;

impl Expr {
    /// Visit `self` and every expression nested in it (including those in
    /// blocks, statements, arms, and `gen.range` bounds), pre-order.
    pub fn walk<F: FnMut(&Expr)>(&self, f: &mut F) {
        f(self);
        match &self.kind {
            ExprKind::Lit(_) | ExprKind::Local(_) | ExprKind::Hole { .. } | ExprKind::Error => {}
            ExprKind::Ctor { args, .. } | ExprKind::List(args) => {
                for a in args {
                    a.walk(f);
                }
            }
            ExprKind::Wrap { arg, .. } | ExprKind::Check { arg, .. } => arg.walk(f),
            ExprKind::Field { base, .. } => base.walk(f),
            ExprKind::Call { args, using, .. } => {
                for a in args.iter().chain(using) {
                    a.walk(f);
                }
            }
            ExprKind::CapCall { handle, args, .. } => {
                handle.walk(f);
                for a in args {
                    a.walk(f);
                }
            }
            ExprKind::Propagate(e) => e.walk(f),
            ExprKind::Unary { operand, .. } => operand.walk(f),
            ExprKind::Binary { lhs, rhs, .. } => {
                lhs.walk(f);
                rhs.walk(f);
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
                collection.walk(f);
                predicate.walk(f);
            }
            ExprKind::StructLit { spread, fields, .. } => {
                if let Some(s) = spread {
                    s.walk(f);
                }
                for (_, e) in fields {
                    e.walk(f);
                }
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                cond.walk(f);
                then_block.walk(f);
                if let Some(b) = else_block {
                    b.walk(f);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                scrutinee.walk(f);
                for arm in arms {
                    arm.body.walk(f);
                }
            }
            ExprKind::Widen { expr, .. } | ExprKind::Refine { expr, .. } => expr.walk(f),
            ExprKind::Gen(g) => g.walk(f),
        }
    }

    /// Equality ignoring every span (and the canonical texts of nested
    /// clauses and assertions): what semantics S55 compares for refinement
    /// classes once grouping parentheses are gone. Types, `DefId`s, and
    /// `LocalId`s must be equal, so two predicates compare equal only when
    /// their locals were numbered the same way (`self` first, then binders
    /// in source order).
    pub fn structurally_eq(&self, other: &Expr) -> bool {
        self.ty == other.ty && kind_eq(&self.kind, &other.kind)
    }
}

impl Block {
    /// Visit every expression of the block: statements in order, then the tail.
    pub fn walk<F: FnMut(&Expr)>(&self, f: &mut F) {
        for s in &self.stmts {
            s.walk(f);
        }
        if let Some(t) = &self.tail {
            t.walk(f);
        }
    }

    /// [`Expr::structurally_eq`] for blocks.
    pub fn structurally_eq(&self, other: &Block) -> bool {
        self.ty == other.ty
            && self.stmts.len() == other.stmts.len()
            && self
                .stmts
                .iter()
                .zip(&other.stmts)
                .all(|(a, b)| a.structurally_eq(b))
            && opt_eq(self.tail.as_deref(), other.tail.as_deref())
    }
}

impl Stmt {
    /// Visit every expression of the statement.
    pub fn walk<F: FnMut(&Expr)>(&self, f: &mut F) {
        match self {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => value.walk(f),
            Stmt::Discard(e) | Stmt::Expr(e) | Stmt::Assert { expr: e, .. } => e.walk(f),
            Stmt::Return(e) => {
                if let Some(e) = e {
                    e.walk(f);
                }
            }
            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
            } => {
                cond.walk(f);
                for c in invariants {
                    c.expr.walk(f);
                }
                if let Some(c) = decreases {
                    c.expr.walk(f);
                }
                body.walk(f);
            }
            Stmt::Concurrent(tasks) => {
                for t in tasks {
                    t.stmt.walk(f);
                }
            }
        }
    }

    /// Visit `self` and every statement nested in it: loop bodies and tasks, and the blocks of
    /// `if` and `match` expressions at any depth.
    pub fn walk_stmts<F: FnMut(&Stmt)>(&self, f: &mut F) {
        fn shallow<F: FnMut(&Stmt)>(s: &Stmt, f: &mut F) {
            f(s);
            match s {
                Stmt::While { body, .. } => {
                    for x in &body.stmts {
                        shallow(x, f);
                    }
                }
                Stmt::Concurrent(tasks) => {
                    for t in tasks {
                        shallow(&t.stmt, f);
                    }
                }
                _ => {}
            }
        }
        shallow(self, f);
        // `walk` reaches every expression, those of loop bodies and tasks included.
        self.walk(&mut |e| match &e.kind {
            ExprKind::If {
                then_block,
                else_block,
                ..
            } => {
                for b in std::iter::once(then_block).chain(else_block) {
                    for x in &b.stmts {
                        shallow(x, f);
                    }
                }
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    for x in &arm.body.stmts {
                        shallow(x, f);
                    }
                }
            }
            _ => {}
        });
    }

    /// [`Expr::structurally_eq`] for statements.
    pub fn structurally_eq(&self, other: &Stmt) -> bool {
        match (self, other) {
            (Stmt::Let { local: a, value: x }, Stmt::Let { local: b, value: y })
            | (Stmt::Assign { local: a, value: x }, Stmt::Assign { local: b, value: y }) => {
                a == b && x.structurally_eq(y)
            }
            (Stmt::Discard(x), Stmt::Discard(y)) | (Stmt::Expr(x), Stmt::Expr(y)) => {
                x.structurally_eq(y)
            }
            (Stmt::Return(x), Stmt::Return(y)) => opt_eq(x.as_ref(), y.as_ref()),
            (Stmt::Assert { expr: x, .. }, Stmt::Assert { expr: y, .. }) => x.structurally_eq(y),
            (
                Stmt::While {
                    cond: c1,
                    invariants: i1,
                    decreases: d1,
                    body: b1,
                },
                Stmt::While {
                    cond: c2,
                    invariants: i2,
                    decreases: d2,
                    body: b2,
                },
            ) => {
                c1.structurally_eq(c2)
                    && i1.len() == i2.len()
                    && i1
                        .iter()
                        .zip(i2)
                        .all(|(a, b)| a.expr.structurally_eq(&b.expr))
                    && opt_eq(d1.as_ref().map(|c| &c.expr), d2.as_ref().map(|c| &c.expr))
                    && b1.structurally_eq(b2)
            }
            (Stmt::Concurrent(a), Stmt::Concurrent(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b)
                        .all(|(x, y)| x.stmt.structurally_eq(&y.stmt))
            }
            _ => false,
        }
    }
}

impl Pattern {
    /// Equality ignoring spans.
    pub fn structurally_eq(&self, other: &Pattern) -> bool {
        self.ty == other.ty
            && match (&self.kind, &other.kind) {
                (
                    PatternKind::Ctor {
                        def: d1,
                        variant: v1,
                        args: a1,
                    },
                    PatternKind::Ctor {
                        def: d2,
                        variant: v2,
                        args: a2,
                    },
                ) => {
                    d1 == d2
                        && v1 == v2
                        && a1.len() == a2.len()
                        && a1.iter().zip(a2).all(|(x, y)| x.structurally_eq(y))
                }
                (
                    PatternKind::Unwrap { def: d1, arg: a1 },
                    PatternKind::Unwrap { def: d2, arg: a2 },
                ) => d1 == d2 && a1.structurally_eq(a2),
                (a, b) => a == b,
            }
    }
}

impl GenExpr {
    /// Visit the `gen.range` bound expressions.
    pub fn walk<F: FnMut(&Expr)>(&self, f: &mut F) {
        match self {
            GenExpr::Int | GenExpr::PositiveInt | GenExpr::Bool | GenExpr::Text => {}
            GenExpr::List(g) | GenExpr::Option(g) => g.walk(f),
            GenExpr::Range(lo, hi) => {
                lo.walk(f);
                hi.walk(f);
            }
        }
    }

    /// Equality ignoring spans.
    pub fn structurally_eq(&self, other: &GenExpr) -> bool {
        match (self, other) {
            (GenExpr::List(a), GenExpr::List(b)) | (GenExpr::Option(a), GenExpr::Option(b)) => {
                a.structurally_eq(b)
            }
            (GenExpr::Range(l1, h1), GenExpr::Range(l2, h2)) => {
                l1.structurally_eq(l2) && h1.structurally_eq(h2)
            }
            // Same-variant List/Option/Range pairs were compared above.
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        }
    }
}

fn opt_eq(a: Option<&Expr>, b: Option<&Expr>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.structurally_eq(y),
        _ => false,
    }
}

fn all_eq(a: &[Expr], b: &[Expr]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.structurally_eq(y))
}

fn kind_eq(a: &ExprKind, b: &ExprKind) -> bool {
    use ExprKind as K;
    match (a, b) {
        (K::Lit(x), K::Lit(y)) => x == y,
        (K::Local(x), K::Local(y)) => x == y,
        (
            K::Ctor {
                def: d1,
                variant: v1,
                args: a1,
            },
            K::Ctor {
                def: d2,
                variant: v2,
                args: a2,
            },
        ) => d1 == d2 && v1 == v2 && all_eq(a1, a2),
        (K::Wrap { def: d1, arg: a1 }, K::Wrap { def: d2, arg: a2 }) => {
            d1 == d2 && a1.structurally_eq(a2)
        }
        (
            K::Field {
                base: b1,
                def: d1,
                index: i1,
            },
            K::Field {
                base: b2,
                def: d2,
                index: i2,
            },
        ) => d1 == d2 && i1 == i2 && b1.structurally_eq(b2),
        (
            K::Call {
                callee: c1,
                generic_args: g1,
                args: a1,
                using: u1,
            },
            K::Call {
                callee: c2,
                generic_args: g2,
                args: a2,
                using: u2,
            },
        ) => c1 == c2 && g1 == g2 && all_eq(a1, a2) && all_eq(u1, u2),
        (
            K::CapCall {
                handle: h1,
                capability: c1,
                op: o1,
                args: a1,
            },
            K::CapCall {
                handle: h2,
                capability: c2,
                op: o2,
                args: a2,
            },
        ) => c1 == c2 && o1 == o2 && h1.structurally_eq(h2) && all_eq(a1, a2),
        (
            K::Check {
                refined: r1,
                arg: a1,
            },
            K::Check {
                refined: r2,
                arg: a2,
            },
        ) => r1 == r2 && a1.structurally_eq(a2),
        (K::Propagate(x), K::Propagate(y)) => x.structurally_eq(y),
        (K::Unary { op: o1, operand: x }, K::Unary { op: o2, operand: y }) => {
            o1 == o2 && x.structurally_eq(y)
        }
        (
            K::Binary {
                op: o1,
                lhs: l1,
                rhs: r1,
            },
            K::Binary {
                op: o2,
                lhs: l2,
                rhs: r2,
            },
        ) => o1 == o2 && l1.structurally_eq(l2) && r1.structurally_eq(r2),
        (
            K::Forall {
                binder: b1,
                collection: c1,
                predicate: p1,
            },
            K::Forall {
                binder: b2,
                collection: c2,
                predicate: p2,
            },
        )
        | (
            K::Parallel {
                binder: b1,
                collection: c1,
                predicate: p1,
            },
            K::Parallel {
                binder: b2,
                collection: c2,
                predicate: p2,
            },
        ) => b1 == b2 && c1.structurally_eq(c2) && p1.structurally_eq(p2),
        (K::Hole { id: i1, name: n1 }, K::Hole { id: i2, name: n2 }) => i1 == i2 && n1 == n2,
        (K::List(x), K::List(y)) => all_eq(x, y),
        (
            K::StructLit {
                def: d1,
                spread: s1,
                fields: f1,
                ..
            },
            K::StructLit {
                def: d2,
                spread: s2,
                fields: f2,
                ..
            },
        ) => {
            d1 == d2
                && opt_eq(s1.as_deref(), s2.as_deref())
                && f1.len() == f2.len()
                && f1
                    .iter()
                    .zip(f2)
                    .all(|((i, x), (j, y))| i == j && x.structurally_eq(y))
        }
        (
            K::If {
                cond: c1,
                then_block: t1,
                else_block: e1,
            },
            K::If {
                cond: c2,
                then_block: t2,
                else_block: e2,
            },
        ) => {
            c1.structurally_eq(c2)
                && t1.structurally_eq(t2)
                && match (e1, e2) {
                    (None, None) => true,
                    (Some(x), Some(y)) => x.structurally_eq(y),
                    _ => false,
                }
        }
        (
            K::Match {
                scrutinee: s1,
                arms: a1,
            },
            K::Match {
                scrutinee: s2,
                arms: a2,
            },
        ) => {
            s1.structurally_eq(s2)
                && a1.len() == a2.len()
                && a1.iter().zip(a2).all(|(x, y)| {
                    x.pattern.structurally_eq(&y.pattern) && x.body.structurally_eq(&y.body)
                })
        }
        (K::Widen { expr: x, from: f1 }, K::Widen { expr: y, from: f2 }) => {
            f1 == f2 && x.structurally_eq(y)
        }
        (
            K::Refine {
                expr: x,
                refined: r1,
                proof: p1,
            },
            K::Refine {
                expr: y,
                refined: r2,
                proof: p2,
            },
        ) => r1 == r2 && p1 == p2 && x.structurally_eq(y),
        (K::Gen(x), K::Gen(y)) => x.structurally_eq(y),
        (K::Error, K::Error) => true,
        _ => false,
    }
}
