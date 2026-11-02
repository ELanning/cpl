//! [`Hir`]: one-line constructors of typed HIR nodes for hand-built test
//! programs. Every node gets a point span at the start of the module's
//! file; use [`Hir::at`] and set `span` yourself where a test compares
//! locations.

use cpl_diag::{FileId, Span};
use cpl_hir::{
    Arm, Block, Clause, DefId, Expr, ExprKind, GenExpr, HoleId, Lit, LocalId, Pattern, PatternKind,
    RefinementProof, StdDefs, Stmt, Ty,
};
use cpl_syntax::ast::{BinaryOp, UnaryOp};
use num_bigint::BigInt;

/// A factory of HIR nodes in one file (from `ProgramBuilder::hir`).
#[derive(Copy, Clone, Debug)]
pub struct Hir {
    file: FileId,
    std: StdDefs,
}

impl Hir {
    pub fn new(file: FileId, std: StdDefs) -> Hir {
        Hir { file, std }
    }

    /// The file the nodes point into.
    pub fn file(&self) -> FileId {
        self.file
    }

    /// The default span: offset 0 of the file.
    pub fn span(&self) -> Span {
        Span::point(self.file, 0)
    }

    /// A span `start..end` of the file.
    pub fn at(&self, start: u32, end: u32) -> Span {
        Span::new(self.file, start, end)
    }

    fn expr(&self, kind: ExprKind, ty: Ty) -> Expr {
        Expr::new(kind, ty, self.span())
    }

    // ---- types ---------------------------------------------------------------

    pub fn unit_ty(&self) -> Ty {
        Ty::unit(&self.std)
    }

    pub fn option_ty(&self, t: Ty) -> Ty {
        Ty::option(&self.std, t)
    }

    pub fn result_ty(&self, t: Ty, e: Ty) -> Ty {
        Ty::result(&self.std, t, e)
    }

    pub fn constraint_error_ty(&self) -> Ty {
        Ty::Adt(self.std.constraint_error, Vec::new())
    }

    // ---- literals and names ----------------------------------------------------

    pub fn int(&self, n: impl Into<BigInt>) -> Expr {
        self.expr(ExprKind::Lit(Lit::Int(n.into())), Ty::Int)
    }

    pub fn bool(&self, b: bool) -> Expr {
        self.expr(ExprKind::Lit(Lit::Bool(b)), Ty::Bool)
    }

    pub fn text(&self, s: &str) -> Expr {
        self.expr(ExprKind::Lit(Lit::Text(s.to_string())), Ty::Text)
    }

    pub fn local(&self, id: LocalId, ty: Ty) -> Expr {
        self.expr(ExprKind::Local(id), ty)
    }

    // ---- operators ---------------------------------------------------------------

    /// `not e` (type `Bool`) or `-e` (type `Int`).
    pub fn unary(&self, op: UnaryOp, operand: Expr) -> Expr {
        let ty = match op {
            UnaryOp::Not => Ty::Bool,
            UnaryOp::Neg => Ty::Int,
        };
        self.expr(
            ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            ty,
        )
    }

    /// A binary operator; the type is `Int` for arithmetic, the left
    /// operand's for `++`, and `Bool` otherwise.
    pub fn binary(&self, op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        let ty = match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                Ty::Int
            }
            BinaryOp::Concat => lhs.ty.clone(),
            _ => Ty::Bool,
        };
        self.expr(
            ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            ty,
        )
    }

    // ---- calls -------------------------------------------------------------------

    /// A call of a non-generic function (or a member stub, receiver first).
    pub fn call(&self, callee: DefId, args: Vec<Expr>, ret: Ty) -> Expr {
        self.call_generic(callee, Vec::new(), args, ret)
    }

    /// A call with explicit (inferred) type arguments.
    pub fn call_generic(
        &self,
        callee: DefId,
        generic_args: Vec<Ty>,
        args: Vec<Expr>,
        ret: Ty,
    ) -> Expr {
        self.expr(
            ExprKind::Call {
                callee,
                generic_args,
                args,
                using: Vec::new(),
            },
            ret,
        )
    }

    /// A call passing `using` arguments.
    pub fn call_using(&self, callee: DefId, args: Vec<Expr>, using: Vec<Expr>, ret: Ty) -> Expr {
        self.expr(
            ExprKind::Call {
                callee,
                generic_args: Vec::new(),
                args,
                using,
            },
            ret,
        )
    }

    /// `handle.op(args)` on a `using` parameter.
    pub fn cap_call(
        &self,
        handle: Expr,
        capability: DefId,
        op: u32,
        args: Vec<Expr>,
        ret: Ty,
    ) -> Expr {
        self.expr(
            ExprKind::CapCall {
                handle: Box::new(handle),
                capability,
                op,
                args,
            },
            ret,
        )
    }

    /// `check<R>(arg)`; `ty` is `Result<R, ConstraintError>`.
    pub fn check(&self, refined: DefId, arg: Expr, ty: Ty) -> Expr {
        self.expr(
            ExprKind::Check {
                refined,
                arg: Box::new(arg),
            },
            ty,
        )
    }

    // ---- constructors --------------------------------------------------------------

    /// An enum variant by index.
    pub fn ctor(&self, def: DefId, variant: u32, args: Vec<Expr>, ty: Ty) -> Expr {
        self.expr(ExprKind::Ctor { def, variant, args }, ty)
    }

    /// `Unit` (variant 0 of the prelude's `Unit`).
    pub fn unit(&self) -> Expr {
        self.ctor(self.std.unit, 0, Vec::new(), self.unit_ty())
    }

    /// `Some(e)` (variant 0 of `Option`, as `std/prelude.cpl` declares it).
    pub fn some(&self, e: Expr) -> Expr {
        let ty = self.option_ty(e.ty.clone());
        self.ctor(self.std.option, 0, vec![e], ty)
    }

    /// `None` at element type `elem` (variant 1 of `Option`).
    pub fn none(&self, elem: Ty) -> Expr {
        let ty = self.option_ty(elem);
        self.ctor(self.std.option, 1, Vec::new(), ty)
    }

    /// `Ok(e)` (variant 0 of `Result`).
    pub fn ok(&self, e: Expr, err_ty: Ty) -> Expr {
        let ty = self.result_ty(e.ty.clone(), err_ty);
        self.ctor(self.std.result, 0, vec![e], ty)
    }

    /// `Err(e)` (variant 1 of `Result`).
    pub fn err(&self, e: Expr, ok_ty: Ty) -> Expr {
        let ty = self.result_ty(ok_ty, e.ty.clone());
        self.ctor(self.std.result, 1, vec![e], ty)
    }

    /// `PageId(arg)`.
    pub fn wrap(&self, def: DefId, arg: Expr) -> Expr {
        self.expr(
            ExprKind::Wrap {
                def,
                arg: Box::new(arg),
            },
            Ty::Wrapper(def),
        )
    }

    /// `base.field` (field `index` of struct `def`).
    pub fn field(&self, base: Expr, def: DefId, index: u32, ty: Ty) -> Expr {
        self.expr(
            ExprKind::Field {
                base: Box::new(base),
                def,
                index,
            },
            ty,
        )
    }

    /// `[elems]` of element type `elem`.
    pub fn list(&self, elems: Vec<Expr>, elem: Ty) -> Expr {
        self.expr(ExprKind::List(elems), Ty::list(elem))
    }

    /// `Name { ..spread, field: value }` with explicit fields by index.
    pub fn struct_lit(
        &self,
        def: DefId,
        spread: Option<Expr>,
        fields: Vec<(u32, Expr)>,
        ty: Ty,
    ) -> Expr {
        self.expr(
            ExprKind::StructLit {
                invariant_proven: false,
                def,
                spread: spread.map(Box::new),
                fields,
            },
            ty,
        )
    }

    // ---- control -------------------------------------------------------------------

    /// `if cond { then } else { else }`; the type is the `then` block's, or
    /// `Unit` without an `else`.
    pub fn if_(&self, cond: Expr, then_block: Block, else_block: Option<Block>) -> Expr {
        let ty = if else_block.is_some() {
            then_block.ty.clone()
        } else {
            self.unit_ty()
        };
        self.expr(
            ExprKind::If {
                cond: Box::new(cond),
                then_block,
                else_block,
            },
            ty,
        )
    }

    pub fn match_(&self, scrutinee: Expr, arms: Vec<Arm>, ty: Ty) -> Expr {
        self.expr(
            ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            ty,
        )
    }

    /// `forall binder in collection: predicate`.
    pub fn forall(&self, binder: LocalId, collection: Expr, predicate: Expr) -> Expr {
        self.expr(
            ExprKind::Forall {
                binder,
                collection: Box::new(collection),
                predicate: Box::new(predicate),
            },
            Ty::Bool,
        )
    }

    /// `?name` of type `ty`.
    pub fn hole(&self, id: u32, name: &str, ty: Ty) -> Expr {
        self.expr(
            ExprKind::Hole {
                id: HoleId(id),
                name: name.to_string(),
            },
            ty,
        )
    }

    /// `e?`; `ty` is the `Ok` type.
    pub fn propagate(&self, e: Expr, ty: Ty) -> Expr {
        self.expr(ExprKind::Propagate(Box::new(e)), ty)
    }

    /// A refined value used at its base type.
    pub fn widen(&self, e: Expr, from: DefId, ty: Ty) -> Expr {
        self.expr(
            ExprKind::Widen {
                expr: Box::new(e),
                from,
            },
            ty,
        )
    }

    /// A base value statically proved to satisfy `refined` (semantics S16).
    pub fn refine(&self, e: Expr, refined: DefId, ty: Ty) -> Expr {
        self.expr(
            ExprKind::Refine {
                expr: Box::new(e),
                refined,
                proof: RefinementProof::Constant,
            },
            ty,
        )
    }

    /// A generator expression of element type `elem`.
    pub fn gen(&self, g: GenExpr, elem: Ty) -> Expr {
        self.expr(ExprKind::Gen(g), Ty::Gen(Box::new(elem)))
    }

    // ---- blocks, clauses, arms, patterns ----------------------------------------------

    /// A block; its type is the tail's, or `Unit` without a tail.
    pub fn block(&self, stmts: Vec<Stmt>, tail: Option<Expr>) -> Block {
        let ty = tail
            .as_ref()
            .map_or_else(|| self.unit_ty(), |t| t.ty.clone());
        Block {
            stmts,
            tail: tail.map(Box::new),
            ty,
            span: self.span(),
        }
    }

    /// A block whose value is `tail`.
    pub fn value(&self, tail: Expr) -> Block {
        self.block(Vec::new(), Some(tail))
    }

    /// A contract clause (or loop clause) with canonical text `text`.
    pub fn clause(&self, expr: Expr, text: &str) -> Clause {
        Clause {
            expr,
            text: text.to_string(),
            span: self.span(),
        }
    }

    pub fn arm(&self, pattern: Pattern, body: Block) -> Arm {
        Arm {
            pattern,
            body,
            span: self.span(),
        }
    }

    pub fn pattern(&self, kind: PatternKind, ty: Ty) -> Pattern {
        Pattern {
            kind,
            ty,
            span: self.span(),
        }
    }
}
