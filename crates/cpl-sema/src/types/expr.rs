//! Expression typing (`spec/semantics.md` 3.6-3.9, S10-S23, S54, S56,
//! S62; toolchain 3.3, 3.5, T18, T19).

use super::infer::{is_poison, subst, var_of};
use super::typer::{HoleState, How, Out, PendingHole, Typer};
use crate::checker::{diag, Stage};
use crate::codes;
use crate::members::{MemberKind, TypeHead};
use crate::resolve::{Ctor, Res, ResOut};
use cpl_diag::Span;
use cpl_hir::{Block, DefId, DefKind, Expr, ExprKind, GenExpr, Lit, ParamDef, RefKind, Ty};
use cpl_syntax::ast::{self, BinaryOp};
use num_bigint::BigInt;
use serde_json::json;

impl<'t, 'c, 'a> Typer<'t, 'c, 'a> {
    /// Type `e`, with the expected type `exp` when the position supplies
    /// one (S56). The caller applies `A ≤ E` unless the result is `done`.
    pub fn expr(&mut self, e: &ast::Expr, exp: Option<&Ty>) -> Out {
        let gen_ok = std::mem::take(&mut self.gen_ok);
        use ast::ExprKind as K;
        match &e.kind {
            K::Literal(l) => Out::new(literal(e.span, l)),
            K::Name(n) => self.name(e, n, exp, gen_ok),
            K::Hole(n) => self.hole(e.span, n, exp),
            K::Forall(f) | K::Parallel(f) => Out::new(self.forall(e, f, exp)),
            K::Binary(b) => Out::new(self.binary(e, b)),
            K::Unary(u) => {
                let (want, ty) = match u.op {
                    ast::UnaryOp::Not => (Ty::Bool, Ty::Bool),
                    ast::UnaryOp::Neg => (Ty::Int, Ty::Int),
                };
                let operand = self.check(&u.operand, &want);
                // S62: the operator form of a failed operand is erroneous.
                let ty = if self.last_coerce_failed {
                    Ty::Error
                } else {
                    ty
                };
                Out::new(Expr::new(
                    ExprKind::Unary {
                        op: u.op,
                        operand: Box::new(operand),
                    },
                    ty,
                    e.span,
                ))
            }
            K::Member(m) => self.member(e, m, exp, gen_ok),
            K::Call(c) => self.call(e, c, exp, gen_ok),
            K::Propagate(inner) => Out::new(self.propagate(e, inner)),
            K::Paren(inner) => {
                self.gen_ok = gen_ok;
                self.expr(inner, exp)
            }
            K::List(l) => Out::new(self.list(e, l, exp)),
            K::Struct(s) => Out::new(self.struct_lit(e, s, exp)),
            K::If(i) => self.if_expr(i, e.span, exp),
            K::Match(m) => self.match_expr(e, m, exp),
        }
    }

    // ---- names -------------------------------------------------------------------

    fn name(&mut self, e: &ast::Expr, n: &ast::Name, exp: Option<&Ty>, gen_ok: bool) -> Out {
        let res = self.tcx.cx.names.exprs.get(&e.span).cloned();
        match res {
            Some(Res::Local(id)) => {
                Out::new(Expr::new(ExprKind::Local(id), self.local_ty(id), e.span))
            }
            Some(Res::Fn(d)) => Out::new(self.module_property(e.span, d, gen_ok)),
            Some(Res::Ctor(c)) => Out::new(self.nullary(e.span, c, exp)),
            Some(Res::DeferredCtor { .. }) => match self.resolve_ctor(n, exp) {
                Some(c) => Out::new(self.nullary(e.span, c, exp)),
                None => Out::new(self.error_expr(e.span)),
            },
            _ => Out::new(self.error_expr(e.span)),
        }
    }

    /// S10 for an unqualified constructor whose resolution waited for the
    /// expected type: step 1 against `strip(E)`, then the lexical steps.
    pub fn resolve_ctor(&mut self, n: &ast::Name, exp: Option<&Ty>) -> Option<Ctor> {
        let expected = exp.map(|t| self.zonk(t));
        let poisoned = expected
            .as_ref()
            .is_some_and(|t| is_poison(&self.resolve(t)));
        let known = expected.filter(|t| var_of(&self.resolve(t)).is_none() && !poisoned);
        let mut out = ResOut::default();
        let res = self.r.resolve_ctor(n, known.as_ref(), &mut out);
        // With an erroneous expected type the lexical outcome is not the
        // author's mistake (S62).
        if !poisoned {
            for d in out.diags {
                self.emit(Stage::Resolve, d);
            }
        }
        match res {
            Res::Ctor(c) => Some(c),
            _ => None,
        }
    }

    /// A constructor used without arguments: a payload-less variant,
    /// `None`, or `Unit` (3.8).
    fn nullary(&mut self, span: Span, c: Ctor, exp: Option<&Ty>) -> Expr {
        let qname = c.qualified_name(self.program);
        match c {
            Ctor::Variant { def, index } => {
                let Some(en) = self.program.def(def).kind.as_enum() else {
                    return self.error_expr(span);
                };
                let payload = en
                    .variants
                    .get(index as usize)
                    .map_or(0, |v| v.payload.len());
                self.add_ref(def, RefKind::Ctor, span);
                if payload > 0 {
                    self.arity_mismatch(span, &qname, payload, 0);
                    return self.error_expr(span);
                }
                let args: Vec<Ty> = (0..en.generics.len()).map(|_| self.fresh(span)).collect();
                let ty = Ty::Adt(def, args);
                if let Some(x) = exp {
                    let s = self.strip(x);
                    self.vars.unify(&s, &ty);
                }
                self.created(span, &ty);
                Expr::new(
                    ExprKind::Ctor {
                        def,
                        variant: index,
                        args: Vec::new(),
                    },
                    ty,
                    span,
                )
            }
            Ctor::Wrapper(def) => {
                self.add_ref(def, RefKind::Ctor, span);
                self.arity_mismatch(span, &qname, 1, 0);
                self.error_expr(span)
            }
        }
    }

    // ---- holes (3.8, S56, S64) -------------------------------------------------------

    fn hole(&mut self, span: Span, n: &ast::Name, exp: Option<&Ty>) -> Out {
        let id = self.tcx.hole_id();
        let (ty, state) = if self.contract {
            (exp.cloned().unwrap_or(Ty::Error), HoleState::Contract)
        } else {
            match exp {
                Some(t) => (t.clone(), HoleState::Pending),
                None => {
                    self.hole_type_unknown(&n.text, span);
                    (Ty::Error, HoleState::Unknown)
                }
            }
        };
        let invariants: Vec<String> = self.whiles.iter().rev().flatten().cloned().collect();
        self.holes.push(PendingHole {
            name: n.text.clone(),
            span,
            ty: ty.clone(),
            frame: self.frame(),
            state,
            invariants,
        });
        Out::done(Expr::new(
            ExprKind::Hole {
                id,
                name: n.text.clone(),
            },
            ty,
            span,
        ))
    }

    // ---- forall ------------------------------------------------------------------

    fn forall(&mut self, e: &ast::Expr, f: &ast::ForallExpr, exp: Option<&Ty>) -> Expr {
        let parallel = matches!(e.kind, ast::ExprKind::Parallel(_));
        let collection = self.infer(&f.collection);
        let ct = self.strip(&collection.ty);
        let elem = match ct {
            Ty::List(t) => *t,
            t if is_poison(&t) => Ty::Error,
            t if var_of(&t).is_some() => {
                let el = self.fresh(f.collection.span);
                self.vars.unify(&t, &Ty::list(el.clone()));
                el
            }
            other => {
                let actual = self.render(&other);
                self.error(
                    diag(
                        if parallel {
                            codes::PARALLEL_RANGE_NOT_LIST
                        } else {
                            codes::FORALL_RANGE_NOT_LIST
                        },
                        Some(f.collection.span),
                        format!(
                            "`{}` ranges over a `List`, found `{actual}`",
                            if parallel { "parallel for" } else { "forall" }
                        ),
                    )
                    .with_data("actual", json!(actual)),
                );
                Ty::Error
            }
        };
        let collection = self.widen_to_root(collection);
        if let Some(&binder) = self.tcx.cx.names.binders.get(&f.binder.span) {
            self.set_local_ty(binder, elem);
            let predicate = if parallel {
                match exp.map(|t| self.strip(t)) {
                    Some(Ty::List(ty)) => self.check(&f.predicate, &ty),
                    _ => self.infer(&f.predicate),
                }
            } else {
                self.check(&f.predicate, &Ty::Bool)
            };
            if parallel {
                let ty = Ty::list(predicate.ty.clone());
                return Expr::new(
                    ExprKind::Parallel {
                        binder,
                        collection: Box::new(collection),
                        predicate: Box::new(predicate),
                    },
                    ty,
                    e.span,
                );
            }
            Expr::new(
                ExprKind::Forall {
                    binder,
                    collection: Box::new(collection),
                    predicate: Box::new(predicate),
                },
                Ty::Bool,
                e.span,
            )
        } else {
            self.check(&f.predicate, &Ty::Bool);
            self.error_expr(e.span)
        }
    }

    // ---- operators (3.6, 3.8, S17, S18, S21) ------------------------------------------

    fn binary(&mut self, e: &ast::Expr, b: &ast::BinaryExpr) -> Expr {
        use BinaryOp::*;
        // S62: an operator form whose operand failed its fixed type is
        // erroneous (`"a" + "b"` in a `-> Text` function is one mistake).
        let (lhs, rhs, ty) = match b.op {
            Add | Sub | Mul | Div | Rem => {
                let (l, r, failed) = self.fixed_operands(b, &Ty::Int);
                (l, r, if failed { Ty::Error } else { Ty::Int })
            }
            And | Or => {
                let (l, r, failed) = self.fixed_operands(b, &Ty::Bool);
                (l, r, if failed { Ty::Error } else { Ty::Bool })
            }
            Lt | Le | Gt | Ge => {
                let mut reported = false;
                let l = self.ordering_operand(&b.lhs, &mut reported);
                let r = self.ordering_operand(&b.rhs, &mut reported);
                (l, r, if reported { Ty::Error } else { Ty::Bool })
            }
            Eq | Ne => {
                let (l, r) = self.equality(b);
                (l, r, Ty::Bool)
            }
            Concat => return self.concat(e, b),
        };
        Expr::new(
            ExprKind::Binary {
                op: b.op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            ty,
            e.span,
        )
    }

    /// Both operands `⇐ T`. One operator form reports at most one operand
    /// mismatch, the first (`"a" + "b"` is one mistake, `+` on `Text`;
    /// semantics S62: the second diagnostic would share the first's fix).
    fn fixed_operands(&mut self, b: &ast::BinaryExpr, want: &Ty) -> (Expr, Expr, bool) {
        let lhs = self.check(&b.lhs, want);
        let failed = self.last_coerce_failed;
        let rhs = if failed {
            self.check_quiet_top(&b.rhs, want)
        } else {
            self.check(&b.rhs, want)
        };
        let failed = failed || self.last_coerce_failed;
        (lhs, rhs, failed)
    }

    /// Check mode without reporting a failure of the final `≤` test.
    pub fn check_quiet_top(&mut self, e: &ast::Expr, want: &Ty) -> Expr {
        let out = self.expr(e, Some(want));
        if out.done {
            self.last_coerce_failed = false;
            return out.e;
        }
        let before = self.diags.len();
        let x = self.coerce(out.e, e, want, How::Normal);
        if self.last_coerce_failed {
            self.diags.truncate(before);
        }
        x
    }

    /// An operand of `<`, `<=`, `>`, `>=`: `⇐ Int`, and an operand whose
    /// `strip` type is not `Int` is `comparison_unsupported` (S18), once
    /// per comparison.
    fn ordering_operand(&mut self, x: &ast::Expr, reported: &mut bool) -> Expr {
        let out = self.expr(x, Some(&Ty::Int));
        if out.done {
            return out.e;
        }
        let s = self.strip(&out.e.ty);
        if is_poison(&s) {
            return out.e;
        }
        if self.vars.unify(&s, &Ty::Int) {
            return self.coerce(out.e, x, &Ty::Int, How::Normal);
        }
        if !*reported {
            *reported = true;
            let actual = self.render(&out.e.ty);
            self.error(
                diag(
                    codes::COMPARISON_UNSUPPORTED,
                    Some(out.e.span),
                    format!("ordering is defined only on `Int`, found `{actual}`"),
                )
                .with_data("actual", json!(actual)),
            );
        }
        out.e
    }

    /// `==` and `!=` (3.6, S17): the left operand is inferred; with a
    /// resolved left type the right operand is checked against its
    /// erasure, the final test being erased identity; otherwise both
    /// erased types are unified. An erroneous operand unifies with the
    /// other operand's type (S62): it binds that type's unknowns, so
    /// `[] == ?right` is `hole_type_unknown` alone.
    fn equality(&mut self, b: &ast::BinaryExpr) -> (Expr, Expr) {
        let lhs = self.infer(&b.lhs);
        let lt = self.zonk(&lhs.ty);
        if is_poison(&lt) {
            // An erroneous left operand, or a capability (the effect
            // stage's `capability_equality`, S27), accepts every right
            // operand: it is checked against the error type (S62).
            let rhs = self.accept_any(&b.rhs);
            return (lhs, rhs);
        }
        let rhs = if !self.vars.has_unbound(&lt) {
            let el = self.erase(&lt);
            let out = self.expr(&b.rhs, Some(&el));
            if !out.done {
                let rt = self.zonk(&out.e.ty);
                if !is_poison(&rt) {
                    let er = self.erase(&rt);
                    if !self.vars.unify(&er, &el) {
                        self.equality_mismatch(&out.e, &lt, &rt);
                    }
                }
            }
            out.e
        } else {
            let r = self.infer(&b.rhs);
            let rt = self.zonk(&r.ty);
            if is_poison(&rt) {
                self.vars.bind_errors(&lt);
            } else {
                let (el, er) = (self.erase(&lt), self.erase(&rt));
                if !self.vars.unify(&el, &er) {
                    self.equality_mismatch(&r, &lt, &rt);
                }
            }
            r
        };
        (lhs, rhs)
    }

    fn equality_mismatch(&mut self, rhs: &Expr, lt: &Ty, rt: &Ty) {
        let (expected, actual) = self.render_pair(lt, rt);
        self.error(
            diag(
                codes::TYPE_MISMATCH,
                Some(rhs.span),
                format!("`==` compares values of one type: the left operand is `{expected}`, this one is `{actual}`"),
            )
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual)),
        );
        self.vars.bind_errors(lt);
        self.vars.bind_errors(rt);
    }

    /// `++` (S21): the left operand's `strip` type is a `List<T>`, `Text`,
    /// or `Bytes`, and the right operand checks against it. Beside an
    /// erroneous left operand (an error, or the `type_mismatch` of a left
    /// operand that is no sequence) the right operand accepts every type
    /// (S62).
    fn concat(&mut self, e: &ast::Expr, b: &ast::BinaryExpr) -> Expr {
        let lhs = self.infer(&b.lhs);
        let s = self.strip(&lhs.ty);
        let (lhs, rhs, ty) = match s {
            Ty::List(_) | Ty::Text | Ty::Bytes => {
                let lhs = self.widen_to_root(lhs);
                let rhs = self.check(&b.rhs, &s);
                (lhs, rhs, s)
            }
            t if is_poison(&t) => {
                let rhs = self.accept_any(&b.rhs);
                (lhs, rhs, Ty::Error)
            }
            t if var_of(&t).is_some() => {
                let rhs = self.infer(&b.rhs);
                let rs = self.strip(&rhs.ty);
                self.vars.unify(&t, &rs);
                (lhs, rhs, rs)
            }
            other => {
                let actual = self.render(&other);
                self.error(
                    diag(
                        codes::TYPE_MISMATCH,
                        Some(lhs.span),
                        format!("`++` concatenates a `List`, `Text`, or `Bytes`, found `{actual}`"),
                    )
                    .with_data("expected", json!("List<T>, Text, or Bytes"))
                    .with_data("actual", json!(actual)),
                );
                let rhs = self.accept_any(&b.rhs);
                (lhs, rhs, Ty::Error)
            }
        };
        Expr::new(
            ExprKind::Binary {
                op: BinaryOp::Concat,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            ty,
            e.span,
        )
    }

    // ---- members (3.9, T19) ----------------------------------------------------------

    fn member(
        &mut self,
        e: &ast::Expr,
        m: &ast::MemberExpr,
        exp: Option<&Ty>,
        gen_ok: bool,
    ) -> Out {
        if let Some(res) = self.tcx.cx.names.exprs.get(&e.span).cloned() {
            // A chain resolved through a module or a type (2.4).
            return Out::new(match res {
                Res::Fn(d) => self.module_property(e.span, d, gen_ok),
                Res::Ctor(c) => self.nullary(e.span, c, exp),
                _ => self.error_expr(e.span),
            });
        }
        let recv = self.receiver(&m.target);
        Out::new(self.value_member(e.span, recv, &m.member))
    }

    /// A receiver (an infer-only position, S56) whose head type must be
    /// known.
    fn receiver(&mut self, target: &ast::Expr) -> Expr {
        let r = self.infer(target);
        let t = self.resolve(&r.ty);
        if var_of(&t).is_some() {
            self.require_resolved(&t);
        }
        r
    }

    /// `e.m` without a call: a struct field (`Field`), a property (a
    /// `Call` of its stub, T19), or `method_not_called` / `unknown_member`.
    fn value_member(&mut self, span: Span, recv: Expr, name: &ast::Name) -> Expr {
        let rt = self.zonk(&recv.ty);
        let s = self.strip(&rt);
        match &s {
            Ty::Error => return self.error_expr(span),
            Ty::Capability(cap) => {
                let op = self
                    .program
                    .def(*cap)
                    .kind
                    .as_capability()
                    .and_then(|c| c.ops.iter().find(|o| o.name == name.text))
                    .map(|o| o.ret.clone());
                match op {
                    Some(ret) => {
                        self.method_not_called(name, &rt);
                        return Expr::new(ExprKind::Error, ret, span);
                    }
                    None => {
                        self.unknown_member(name, &rt);
                        return self.error_expr(span);
                    }
                }
            }
            Ty::Adt(def, args) => {
                if let DefKind::Struct(sd) = &self.program.def(*def).kind {
                    if let Some(index) = sd.fields.iter().position(|f| f.name == name.text) {
                        let ty = subst(&sd.fields[index].ty, args);
                        let base = self.widen_to_root(recv);
                        self.add_ref(*def, RefKind::FieldAccess, name.span);
                        return Expr::new(
                            ExprKind::Field {
                                base: Box::new(base),
                                def: *def,
                                index: index as u32,
                            },
                            ty,
                            span,
                        );
                    }
                }
            }
            _ => {}
        }
        let member =
            TypeHead::of(&s, &self.std).and_then(|h| self.tcx.cx.members.lookup(h, &name.text));
        match member {
            Some(member) => {
                let inst = self.instantiate_member(member.def, &recv, &s, span, None);
                let (tys, ret) = (inst.tys, inst.ret);
                match member.kind {
                    MemberKind::Property => {
                        let recv = self.widen_to_root(recv);
                        self.add_ref(member.def, RefKind::Call, name.span);
                        self.created(span, &ret);
                        Expr::new(
                            ExprKind::Call {
                                callee: member.def,
                                generic_args: tys,
                                args: vec![recv],
                                using: Vec::new(),
                            },
                            ret,
                            span,
                        )
                    }
                    MemberKind::Method => {
                        self.method_not_called(name, &rt);
                        self.vars.bind_errors(&ret);
                        Expr::new(ExprKind::Error, ret, span)
                    }
                }
            }
            None => {
                self.unknown_member(name, &rt);
                self.error_expr(span)
            }
        }
    }

    /// Instantiate a member stub at the receiver `recv`, whose stripped
    /// type is `s`: fresh variables (or the explicit arguments, 3.7 step 1)
    /// for its type parameters, then the receiver as the stub's first
    /// argument (T19: `xs.first()` is `Call { callee: first, args: [xs] }`),
    /// so 3.7 step 2 checks it against the instantiated receiver parameter.
    /// Lookup is by the receiver's head type, so only explicit arguments
    /// can make that check fail: `xs.first<Text>()` on a `List<Int>` is a
    /// `type_mismatch` at the receiver (S63 (1): the operand that failed),
    /// never a silent `Option<Text>`; the call keeps its result type, as a
    /// call with a mismatched argument does.
    fn instantiate_member(
        &mut self,
        stub: DefId,
        recv: &Expr,
        s: &Ty,
        span: Span,
        explicit: Option<&ast::GenericArgs>,
    ) -> MemberInst {
        let Some(fd) = self.program.def(stub).kind.as_fn() else {
            return MemberInst {
                tys: Vec::new(),
                ret: Ty::Error,
                params: Vec::new(),
                count_ok: true,
            };
        };
        let (tys, count_ok) = self.instantiation(stub, fd.generics.len(), explicit, span);
        if let Some(p) = fd.params.first() {
            let want = subst(&p.ty, &tys);
            if !self.vars.unify(&want, s) {
                let rt = self.zonk(&recv.ty);
                let (expected, actual) = self.render_pair(&want, &rt);
                self.error(
                    diag(
                        codes::TYPE_MISMATCH,
                        Some(recv.span),
                        format!("the explicit type arguments make this member take a `{expected}` receiver, found `{actual}`"),
                    )
                    .with_data("expected", json!(expected))
                    .with_data("actual", json!(actual)),
                );
            }
        }
        let ret = subst(&fd.ret, &tys);
        let params = fd
            .params
            .iter()
            .skip(1)
            .map(|p| subst(&p.ty, &tys))
            .collect();
        MemberInst {
            tys,
            ret,
            params,
            count_ok,
        }
    }

    /// The instantiation of `n` type parameters (3.7 step 1): the explicit
    /// arguments when their count is right; otherwise fresh variables and
    /// `false` after `type_argument_count_mismatch`. The caller then makes
    /// the form erroneous once its arguments are typed (S62: the call
    /// carries a diagnostic, so the unknowns that the right arguments would
    /// have fixed are not also `cannot_infer_type`).
    fn instantiation(
        &mut self,
        callee: DefId,
        n: usize,
        explicit: Option<&ast::GenericArgs>,
        span: Span,
    ) -> (Vec<Ty>, bool) {
        if let Some(g) = explicit {
            let given: Vec<Ty> = g
                .args
                .iter()
                .map(|t| {
                    self.tcx
                        .cx
                        .names
                        .types
                        .get(&t.span)
                        .cloned()
                        .unwrap_or(Ty::Error)
                })
                .collect();
            if given.len() == n {
                return (given, true);
            }
            let name = self.program.display_path(callee);
            self.count_mismatch(g.span, &name, n, given.len());
            return ((0..n).map(|_| self.fresh(span)).collect(), false);
        }
        ((0..n).map(|_| self.fresh(span)).collect(), true)
    }

    // ---- calls (3.7, 3.8) ------------------------------------------------------------

    fn call(&mut self, e: &ast::Expr, c: &ast::CallExpr, exp: Option<&Ty>, gen_ok: bool) -> Out {
        let names = &self.tcx.cx.names;
        match &c.callee.kind {
            ast::ExprKind::Name(n) => {
                let res = names.exprs.get(&c.callee.span).cloned();
                self.call_res(e, c, res, Some(n), exp, gen_ok)
            }
            ast::ExprKind::Member(m) => match names.exprs.get(&c.callee.span).cloned() {
                Some(res) => self.call_res(e, c, Some(res), None, exp, gen_ok),
                None => Out::new(self.method_call(e, c, m)),
            },
            _ => {
                let callee = self.infer(&c.callee);
                if !callee.ty.is_error() {
                    let text = self.text(&c.callee);
                    self.not_callable(
                        c.callee.span,
                        &text,
                        "an expression is not a function",
                        None,
                    );
                }
                self.args_infer(c);
                Out::new(self.error_expr(e.span))
            }
        }
    }

    fn call_res(
        &mut self,
        e: &ast::Expr,
        c: &ast::CallExpr,
        res: Option<Res>,
        name: Option<&ast::Name>,
        exp: Option<&Ty>,
        gen_ok: bool,
    ) -> Out {
        match res {
            Some(Res::Fn(d)) => Out::new(self.fn_call(e, c, d, gen_ok)),
            Some(Res::Check) => Out::new(self.check_call(e, c)),
            Some(Res::Ctor(ctor)) => Out::new(self.ctor_app(e, c, ctor, exp)),
            Some(Res::DeferredCtor { .. }) => {
                let resolved = name.and_then(|n| self.resolve_ctor(n, exp));
                match resolved {
                    Some(ctor) => Out::new(self.ctor_app(e, c, ctor, exp)),
                    None => {
                        self.args_infer(c);
                        Out::new(self.error_expr(e.span))
                    }
                }
            }
            Some(Res::Local(id)) => Out::new(self.local_callee(e, c, id)),
            Some(Res::Module(path)) => {
                let text = path.join(".");
                self.not_callable(
                    c.callee.span,
                    &text,
                    &format!("`{text}` is a module, not a function"),
                    None,
                );
                self.args_infer(c);
                Out::new(self.error_expr(e.span))
            }
            _ => {
                self.args_infer(c);
                Out::new(self.error_expr(e.span))
            }
        }
    }

    /// A callee that is a local binding: `not_callable` with the hidden
    /// item in the note (S8); a `using` parameter as callee (`db(x)`) is
    /// the effect stage's `capability_as_value`, so the call is the
    /// handle's `Local` alone.
    fn local_callee(&mut self, e: &ast::Expr, c: &ast::CallExpr, id: cpl_hir::LocalId) -> Expr {
        let ty = self.local_ty(id);
        self.args_infer(c);
        if matches!(ty, Ty::Capability(_)) {
            return Expr::new(ExprKind::Local(id), ty, c.callee.span);
        }
        if !ty.is_error() {
            let name = self
                .locals
                .get(id.0 as usize)
                .map_or(String::new(), |l| l.name.clone());
            let hidden = self.tcx.cx.names.hidden.get(&c.callee.span).copied();
            let note = hidden.map(|d| {
                format!(
                    "the local `{name}` hides `{}`",
                    self.program.display_path(d)
                )
            });
            self.not_callable(
                c.callee.span,
                &name,
                &format!("`{name}` is a local binding, not a function"),
                note,
            );
        }
        self.error_expr(e.span)
    }

    /// Type the arguments of a call whose callee failed (an unresolved
    /// name, `not_callable`, `unknown_member`, `property_called`, ...), for
    /// their own diagnostics. The erroneous callee "may be called with any
    /// arguments" (S62), so each positional argument is checked against the
    /// error type ([`Typer::accept_any`]) and each `using` argument is
    /// still held to the shape rule of S28, a condition on its position.
    pub fn args_infer(&mut self, c: &ast::CallExpr) {
        for a in &c.args.positional {
            self.accept_any(&a.expr);
        }
        for u in &c.args.using {
            self.using_arg(&u.expr, None);
        }
    }

    /// Check `e` against the error type: the position belongs to a form
    /// that already failed (an argument of a failed callee or beyond the
    /// arity, an entry of a broken struct literal, an operand beside an
    /// erroneous one), and the error type accepts every type (S62). The
    /// expression's own mistakes are still reported, but the variables it
    /// creates are bound to the error type instead of surviving the
    /// statement as `cannot_infer_type`, and a hole in it is reported with
    /// that expected type (S64) rather than as `hole_type_unknown`: the fix
    /// of the failed form is what would give it a type.
    pub fn accept_any(&mut self, e: &ast::Expr) -> Expr {
        self.check(e, &Ty::Error)
    }

    /// Type something without reporting (a `using` argument that is not a
    /// bare name is the effect stage's diagnostic, S28).
    pub fn quiet<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let (d, h, v) = (self.diags.len(), self.holes.len(), self.vars.info.len());
        let r = f(self);
        self.diags.truncate(d);
        self.holes.truncate(h);
        for i in v..self.vars.info.len() {
            if self.vars.info[i].binding.is_none() {
                self.vars.bind_error(i as u32);
            }
        }
        r
    }

    /// A call of a function (3.7 steps 1-4), a generator function (S14,
    /// T18), or a module-level property used as a call (`property_called`,
    /// stdlib S40).
    fn fn_call(&mut self, e: &ast::Expr, c: &ast::CallExpr, d: DefId, gen_ok: bool) -> Expr {
        let Some(fd) = self.program.def(d).kind.as_fn() else {
            self.args_infer(c);
            return self.error_expr(e.span);
        };
        if self.tcx.cx.decls[d.0 as usize].annots.module_property {
            let name = self.program.def(d).name.display();
            let module = self
                .program
                .module(self.program.def(d).module)
                .path
                .last()
                .cloned()
                .unwrap_or_default();
            let shown = if module == "gen" {
                format!("gen.{name}")
            } else {
                name.clone()
            };
            let ty = self.render(&fd.ret);
            self.error(
                diag(
                    codes::PROPERTY_CALLED,
                    Some(e.span),
                    format!("`{name}` is a property; write `{shown}`"),
                )
                .with_data("name", json!(name))
                .with_data("type", json!(ty)),
            );
            self.args_infer(c);
            // S62: the expression keeps the property's own type.
            return self.module_property(e.span, d, gen_ok);
        }
        let qname = self.program.display_path(d);
        self.add_ref(d, RefKind::Call, c.callee.span);
        let (tys, count_ok) =
            self.instantiation(d, fd.generics.len(), c.generic_args.as_ref(), e.span);
        let params: Vec<Ty> = fd.params.iter().map(|p| subst(&p.ty, &tys)).collect();
        let arity_ok = c.args.positional.len() == params.len();
        if !arity_ok {
            self.arity_mismatch(e.span, &qname, params.len(), c.args.positional.len());
        }
        let gen_id = self.gen_builtin(d);
        let mut args = Vec::new();
        for (i, a) in c.args.positional.iter().enumerate() {
            match params.get(i) {
                Some(p) => {
                    self.gen_ok = gen_id.is_some();
                    args.push(self.check(&a.expr, p));
                }
                None => args.push(self.accept_any(&a.expr)),
            }
        }
        let using = self.using_args(e.span, c, &qname, &fd.using, &tys);
        let mut ret = subst(&fd.ret, &tys);
        self.created(e.span, &ret);
        // S62: a call with `arity_mismatch` or `type_argument_count_mismatch`
        // is erroneous once its arguments are typed.
        let ok = arity_ok && count_ok;
        if !ok {
            self.vars.bind_errors(&ret);
            ret = Ty::Error;
            for t in &tys {
                self.vars.bind_errors(t);
            }
        }
        if let Some(id) = gen_id {
            return match gen_node(&id, &args) {
                Some(g) if ok => self.gen_expr(e.span, g, ret, gen_ok),
                _ => self.error_expr(e.span),
            };
        }
        Expr::new(
            ExprKind::Call {
                callee: d,
                generic_args: tys,
                args,
                using,
            },
            ret,
            e.span,
        )
    }

    /// The `@builtin` id of a `std.gen` function (`gen_int`, `gen_list`, ...).
    fn gen_builtin(&self, d: DefId) -> Option<String> {
        let module = self.program.def(d).module;
        if self.program.module(module).path != ["std", "gen"] {
            return None;
        }
        self.tcx.cx.decls[d.0 as usize]
            .annots
            .builtin
            .as_ref()
            .map(|(id, _)| id.clone())
            .filter(|id| id.starts_with("gen_"))
    }

    /// A module-level property read (`gen.int`, S54): a generator in a
    /// `for` range or `std.gen` argument, else `generator_outside_for`.
    fn module_property(&mut self, span: Span, d: DefId, gen_ok: bool) -> Expr {
        let ret = self
            .program
            .def(d)
            .kind
            .as_fn()
            .map_or(Ty::Error, |f| f.ret.clone());
        self.add_ref(d, RefKind::Call, span);
        if let Some(id) = self.gen_builtin(d) {
            return match gen_node(&id, &[]) {
                Some(g) => self.gen_expr(span, g, ret, gen_ok),
                None => self.error_expr(span),
            };
        }
        Expr::new(
            ExprKind::Call {
                callee: d,
                generic_args: Vec::new(),
                args: Vec::new(),
                using: Vec::new(),
            },
            ret,
            span,
        )
    }

    /// A generator expression (S14, T18): `ExprKind::Gen` where a
    /// generator may stand, else `generator_outside_for` and the error
    /// type (a condition on the position, reported even beside other
    /// errors, S62).
    fn gen_expr(&mut self, span: Span, g: GenExpr, ty: Ty, gen_ok: bool) -> Expr {
        if gen_ok {
            return Expr::new(ExprKind::Gen(g), ty, span);
        }
        let actual = self.render(&ty);
        self.error(
            diag(
                codes::GENERATOR_OUTSIDE_FOR,
                Some(span),
                "generator values may only appear in a property's for clause",
            )
            .with_data("actual", json!(actual)),
        );
        self.error_expr(span)
    }

    /// `using` arguments (S28): their count, and for a bare name of a
    /// handle the identity of its capability type with the parameter's.
    fn using_args(
        &mut self,
        span: Span,
        c: &ast::CallExpr,
        callee: &str,
        params: &[ParamDef],
        tys: &[Ty],
    ) -> Vec<Expr> {
        let (expected, actual) = (params.len(), c.args.using.len());
        if expected != actual {
            self.error(
                diag(
                    codes::USING_ARITY_MISMATCH,
                    Some(span),
                    format!(
                        "`{callee}` takes {expected} `using` argument{}, found {actual}",
                        plural(expected)
                    ),
                )
                .with_data("callee", json!(callee))
                .with_data("expected", json!(expected))
                .with_data("actual", json!(actual)),
            );
        }
        let mut out = Vec::new();
        for (j, u) in c.args.using.iter().enumerate() {
            let param = params.get(j).map(|p| subst(&p.ty, tys));
            out.push(self.using_arg(&u.expr, param));
        }
        out
    }

    /// One `using` argument (S28): a bare name of a `using` parameter of
    /// the enclosing function whose capability type is the parameter's
    /// (`type_mismatch` otherwise). Anything else is
    /// `using_argument_not_capability`, reported here because only the
    /// syntax tells a bare name from another expression (the HIR keeps the
    /// argument: a `Local` for a name, the quietly typed expression
    /// otherwise, `Error` for a name that did not resolve). In a contract
    /// expression every `using` argument is `capability_as_value` alone
    /// ([`Typer::contract_using_arg`], 5.1).
    fn using_arg(&mut self, x: &ast::Expr, param: Option<Ty>) -> Expr {
        if self.contract {
            return self.contract_using_arg(x);
        }
        if let ast::ExprKind::Name(n) = &x.kind {
            if !n.is_upper() {
                return match self.tcx.cx.names.exprs.get(&x.span).cloned() {
                    Some(Res::Local(id)) => {
                        let lt = self.local_ty(id);
                        let is_using = self
                            .locals
                            .get(id.0 as usize)
                            .is_some_and(|l| l.kind == cpl_hir::BindingKind::Using);
                        if !is_using {
                            self.not_a_handle(x, &lt, false);
                        } else if let (Ty::Capability(a), Some(Ty::Capability(b))) = (&lt, &param) {
                            if a != b {
                                let p = param.clone().unwrap_or(Ty::Error);
                                let (expected, actual) = self.render_pair(&p, &lt);
                                self.error(
                                    diag(
                                        codes::TYPE_MISMATCH,
                                        Some(x.span),
                                        format!("this `using` argument is a `{actual}`; the parameter is a `{expected}`"),
                                    )
                                    .with_data("expected", json!(expected))
                                    .with_data("actual", json!(actual)),
                                );
                            }
                        }
                        Expr::new(ExprKind::Local(id), lt, x.span)
                    }
                    _ => self.error_expr(x.span),
                };
            }
        }
        let e = self.quiet(|t| t.infer(x));
        let ty = e.ty.clone();
        // A handle written in another form than its bare name (`(db)`, or
        // `db(key)`, which is the handle's `Local`) is not poison: it is a
        // genuine handle in the wrong shape (S28). The effect stage builds
        // S28's σ from bare names only.
        let handle = match e.kind {
            ExprKind::Local(id) if matches!(ty, Ty::Capability(_)) => self
                .locals
                .get(id.0 as usize)
                .filter(|l| l.kind == cpl_hir::BindingKind::Using)
                .map(|l| l.name.clone()),
            _ => None,
        };
        if let Some(name) = handle {
            let text = self.text(x);
            let shown = self.render(&ty);
            self.emit(
                Stage::Effect,
                diag(
                    codes::USING_ARGUMENT_NOT_CAPABILITY,
                    Some(x.span),
                    format!("a `using` argument is the bare name of a `using` parameter: write `using {name}`, not `using {text}`"),
                )
                .with_data("name", json!(text))
                .with_data("type", json!(shown)),
            );
        } else {
            // Not a bare name: S28's shape rule is a condition on the
            // position, which an error inside the argument does not suppress
            // (S62's last rule: `using ?h`, `using key.nope`, `using
            // undefined()` are each this error, the argument's own
            // diagnostics being cascades of it, since its fix, a bare name,
            // removes them). A hole in the argument gets no `hole` beside it
            // (S64, as for `hole_in_contract`: the position admits only a
            // bare name, so it may never stay unfinished); its type is
            // unknown and renders as `?`.
            self.not_a_handle(x, &ty, true);
        }
        e
    }

    /// A `using` argument inside a contract expression (5.1: "no `using`
    /// argument (there is no handle to pass); in a contract this is the
    /// only report for a `using` argument, whatever its form"):
    /// `capability_as_value` at the argument (S63 (1)), whatever it is (a
    /// bare handle, `(db)`, `db(key)`, a non-handle, a hole), and none of
    /// S28's `using_argument_not_capability` and `type_mismatch`, over
    /// which the contract rule takes precedence as `propagate_in_contract`
    /// does over `propagate_outside_function`. The argument is typed
    /// quietly, as a rejected argument is outside contracts (S62), and the
    /// contract stage reports no `hole_in_contract` for a hole in it: this
    /// diagnostic is that hole's one report (S64), as S28's is outside
    /// contracts. `name` is the handle's name when the argument is a
    /// handle (through parentheses or a call of it), else the argument's
    /// text; `type` is its type (the capability for a handle; `?` when
    /// erroneous, S69). Reported in the effect stage, the code's category.
    fn contract_using_arg(&mut self, x: &ast::Expr) -> Expr {
        let e = self.quiet(|t| t.infer(x));
        let handle = match e.kind {
            ExprKind::Local(id) if matches!(e.ty, Ty::Capability(_)) => self
                .locals
                .get(id.0 as usize)
                .filter(|l| l.kind == cpl_hir::BindingKind::Using)
                .map(|l| l.name.clone()),
            _ => None,
        };
        let shown = self.render(&e.ty);
        let (name, message) = match handle {
            Some(name) => {
                let message = format!(
                    "a contract expression cannot pass the handle `{name}`: contracts are pure and have no handle to pass"
                );
                (name, message)
            }
            None => {
                let text = self.text(x);
                let message = format!(
                    "a contract expression cannot pass a `using` argument (`{text}`): contracts are pure and have no handle to pass"
                );
                (text, message)
            }
        };
        let d = diag(codes::CAPABILITY_AS_VALUE, Some(x.span), message)
            .with_data("name", json!(name))
            .with_data("type", json!(shown));
        let d = crate::checker::blame(d, self.tcx.cx, self.def, Some(cpl_diag::Facet::Contract));
        self.emit(Stage::Effect, d);
        e
    }

    /// `using_argument_not_capability` (S28) for an argument of type `t`.
    /// A bare name (`not_bare` false) of a poison type is exempt: an
    /// erroneous binding, or a handle that a signature error left in a
    /// positional parameter, is already explained by that error (S62).
    fn not_a_handle(&mut self, x: &ast::Expr, t: &Ty, not_bare: bool) {
        let t = self.zonk(t);
        if is_poison(&t) && !not_bare {
            return;
        }
        let text = self.text(x);
        let ty = self.render(&t);
        self.emit(
            Stage::Effect,
            diag(
                codes::USING_ARGUMENT_NOT_CAPABILITY,
                Some(x.span),
                format!("a `using` argument names a `using` parameter of the enclosing function; `{text}` is not one"),
            )
            .with_data("name", json!(text))
            .with_data("type", json!(ty)),
        );
    }

    /// `check<R>(e)` (3.7, stdlib 1.4).
    fn check_call(&mut self, e: &ast::Expr, c: &ast::CallExpr) -> Expr {
        let refined: Option<(Ty, DefId, Span)> = match &c.generic_args {
            None => {
                // The call fails, so every argument accepts every type
                // (S62); the first one's own type is the payload's `type`.
                let mut shown = String::new();
                for (i, a) in c.args.positional.iter().enumerate() {
                    let x = self.accept_any(&a.expr);
                    if i == 0 {
                        shown = self.render(&x.ty);
                    }
                }
                self.error(
                    diag(
                        codes::CHECK_REQUIRES_TYPE_ARGUMENT,
                        Some(e.span),
                        "`check` needs an explicit refinement type argument: write `check<R>(value)`",
                    )
                    .with_data("type", json!(shown)),
                );
                return self.error_expr(e.span);
            }
            Some(g) if g.args.len() != 1 => {
                self.count_mismatch(g.span, "check", 1, g.args.len());
                None
            }
            Some(g) => {
                let t = self
                    .tcx
                    .cx
                    .names
                    .types
                    .get(&g.args[0].span)
                    .cloned()
                    .unwrap_or(Ty::Error);
                match t {
                    Ty::Refined(_, def) => Some((t, def, g.args[0].span)),
                    Ty::Error => None,
                    other => {
                        let shown = self.render(&other);
                        let message = match &other {
                            Ty::Wrapper(_) => format!(
                                "{shown} is a nominal wrapper; construct it with {shown}(...)"
                            ),
                            Ty::Int | Ty::Bool | Ty::Text | Ty::Bytes => {
                                format!(
                                    "check requires a refinement type; {shown} has no predicate"
                                )
                            }
                            _ => format!(
                                "check requires a refinement type; {shown} is not a refinement"
                            ),
                        };
                        self.error(
                            diag(codes::CHECK_REQUIRES_REFINEMENT, Some(e.span), message)
                                .with_data("type", json!(shown)),
                        );
                        None
                    }
                }
            }
        };
        let arity_ok = c.args.positional.len() == 1;
        if !arity_ok {
            self.arity_mismatch(e.span, "check", 1, c.args.positional.len());
        }
        if !c.args.using.is_empty() {
            self.using_args(e.span, c, "check", &[], &[]);
        }
        match refined {
            Some((rt, def, tspan)) if arity_ok => {
                let base = self.strip(&rt);
                let arg = self.check(&c.args.positional[0].expr, &base);
                self.add_ref(def, RefKind::Check, tspan);
                let ce = Ty::Adt(self.std.constraint_error, Vec::new());
                Expr::new(
                    ExprKind::Check {
                        refined: def,
                        arg: Box::new(arg),
                    },
                    Ty::result(&self.std, rt, ce),
                    e.span,
                )
            }
            _ => {
                for a in &c.args.positional {
                    self.accept_any(&a.expr);
                }
                self.error_expr(e.span)
            }
        }
    }

    /// A constructor application `C(e1, ..., en)` (3.8, S19: the expected
    /// type flows in first).
    fn ctor_app(&mut self, e: &ast::Expr, c: &ast::CallExpr, ctor: Ctor, exp: Option<&Ty>) -> Expr {
        let qname = ctor.qualified_name(self.program);
        let callee_span = c.callee.span;
        match ctor {
            Ctor::Variant { def, index } => {
                let Some(en) = self.program.def(def).kind.as_enum() else {
                    self.args_infer(c);
                    return self.error_expr(e.span);
                };
                let (tys, count_ok) =
                    self.instantiation(def, en.generics.len(), c.generic_args.as_ref(), e.span);
                let ty = Ty::Adt(def, tys.clone());
                if let Some(x) = exp {
                    let s = self.strip(x);
                    self.vars.unify(&s, &ty);
                }
                let payload: Vec<Ty> = en
                    .variants
                    .get(index as usize)
                    .map(|v| v.payload.iter().map(|p| subst(p, &tys)).collect())
                    .unwrap_or_default();
                let arity_ok = c.args.positional.len() == payload.len();
                if !arity_ok {
                    self.arity_mismatch(e.span, &qname, payload.len(), c.args.positional.len());
                }
                let mut args = Vec::new();
                for (i, a) in c.args.positional.iter().enumerate() {
                    match payload.get(i) {
                        Some(p) => args.push(self.check(&a.expr, p)),
                        None => args.push(self.accept_any(&a.expr)),
                    }
                }
                if !c.args.using.is_empty() {
                    self.using_args(e.span, c, &qname, &[], &[]);
                }
                self.add_ref(def, RefKind::Ctor, callee_span);
                self.created(e.span, &ty);
                // S62: erroneous after `arity_mismatch` or
                // `type_argument_count_mismatch`.
                if !arity_ok || !count_ok {
                    self.vars.bind_errors(&ty);
                    return self.error_expr(e.span);
                }
                Expr::new(
                    ExprKind::Ctor {
                        def,
                        variant: index,
                        args,
                    },
                    ty,
                    e.span,
                )
            }
            Ctor::Wrapper(def) => {
                // A wrapper has no type parameters (3.8): explicit arguments
                // are `type_argument_count_mismatch`, and the application is
                // then erroneous (S62).
                let count_ok = c.generic_args.is_none();
                if let Some(g) = &c.generic_args {
                    self.count_mismatch(g.span, &qname, 0, g.args.len());
                }
                let inner = self
                    .program
                    .def(def)
                    .kind
                    .as_wrapper()
                    .map_or(Ty::Error, |w| w.inner.clone());
                let arity_ok = c.args.positional.len() == 1;
                if !arity_ok {
                    self.arity_mismatch(e.span, &qname, 1, c.args.positional.len());
                }
                let mut args = Vec::new();
                for (i, a) in c.args.positional.iter().enumerate() {
                    if i == 0 {
                        args.push(self.check(&a.expr, &inner));
                    } else {
                        args.push(self.accept_any(&a.expr));
                    }
                }
                if !c.args.using.is_empty() {
                    self.using_args(e.span, c, &qname, &[], &[]);
                }
                self.add_ref(def, RefKind::Ctor, callee_span);
                if !arity_ok || !count_ok {
                    return self.error_expr(e.span);
                }
                Expr::new(
                    ExprKind::Wrap {
                        def,
                        arg: Box::new(args.remove(0)),
                    },
                    Ty::Wrapper(def),
                    e.span,
                )
            }
        }
    }

    /// A method or operation call on a value receiver (3.8, 3.9): a
    /// member stub (`Call` with the receiver first, T19) or a capability
    /// operation (`CapCall`).
    fn method_call(&mut self, e: &ast::Expr, c: &ast::CallExpr, m: &ast::MemberExpr) -> Expr {
        let recv = self.receiver(&m.target);
        let rt = self.zonk(&recv.ty);
        let s = self.strip(&rt);
        let name = &m.member;
        match &s {
            Ty::Error => {
                self.args_infer(c);
                return self.error_expr(e.span);
            }
            Ty::Capability(cap) => return self.op_call(e, c, recv, *cap, name, &rt),
            Ty::Adt(def, _) => {
                if let DefKind::Struct(sd) = &self.program.def(*def).kind {
                    if sd.fields.iter().any(|f| f.name == name.text) {
                        let ty = self.render(&rt);
                        self.not_callable(
                            name.span,
                            &name.text,
                            &format!("`{}` is a field of `{ty}`, not a method", name.text),
                            Some(format!("read it without parentheses: `.{}`", name.text)),
                        );
                        self.args_infer(c);
                        return self.error_expr(e.span);
                    }
                }
            }
            _ => {}
        }
        let member =
            TypeHead::of(&s, &self.std).and_then(|h| self.tcx.cx.members.lookup(h, &name.text));
        let Some(member) = member else {
            self.unknown_member(name, &rt);
            self.args_infer(c);
            return self.error_expr(e.span);
        };
        let MemberInst {
            tys,
            ret,
            params,
            count_ok,
        } = self.instantiate_member(member.def, &recv, &s, e.span, c.generic_args.as_ref());
        if member.kind == MemberKind::Property {
            let ty = self.render(&rt);
            self.error(
                diag(
                    codes::PROPERTY_CALLED,
                    Some(e.span),
                    format!(
                        "`{}` is a property; write `x.{}` without parentheses",
                        name.text, name.text
                    ),
                )
                .with_data("name", json!(name.text))
                .with_data("type", json!(ty)),
            );
            self.args_infer(c);
            self.vars.bind_errors(&ret);
            return Expr::new(ExprKind::Error, ret, e.span);
        }
        let qname = self.program.display_path(member.def);
        let arity_ok = c.args.positional.len() == params.len();
        if !arity_ok {
            self.arity_mismatch(e.span, &qname, params.len(), c.args.positional.len());
        }
        let mut args = vec![self.widen_to_root(recv)];
        for (i, a) in c.args.positional.iter().enumerate() {
            match params.get(i) {
                Some(p) => args.push(self.check(&a.expr, p)),
                None => args.push(self.accept_any(&a.expr)),
            }
        }
        if !c.args.using.is_empty() {
            self.using_args(e.span, c, &qname, &[], &[]);
        }
        self.add_ref(member.def, RefKind::Call, name.span);
        self.created(e.span, &ret);
        // S62: erroneous after `arity_mismatch` or
        // `type_argument_count_mismatch`.
        let ret = if arity_ok && count_ok {
            ret
        } else {
            self.vars.bind_errors(&ret);
            for t in &tys {
                self.vars.bind_errors(t);
            }
            Ty::Error
        };
        Expr::new(
            ExprKind::Call {
                callee: member.def,
                generic_args: tys,
                args,
                using: Vec::new(),
            },
            ret,
            e.span,
        )
    }

    /// `c.op(args)` on a handle (3.8): a `CapCall`.
    fn op_call(
        &mut self,
        e: &ast::Expr,
        c: &ast::CallExpr,
        recv: Expr,
        cap: DefId,
        name: &ast::Name,
        rt: &Ty,
    ) -> Expr {
        let found = self.program.def(cap).kind.as_capability().and_then(|cd| {
            cd.ops
                .iter()
                .position(|o| o.name == name.text)
                .map(|i| (i, cd.ops[i].clone()))
        });
        let Some((index, op)) = found else {
            self.unknown_member(name, rt);
            self.args_infer(c);
            return self.error_expr(e.span);
        };
        let qname = format!("{}.{}", self.program.display_path(cap), op.name);
        let arity_ok = c.args.positional.len() == op.params.len();
        if !arity_ok {
            self.arity_mismatch(e.span, &qname, op.params.len(), c.args.positional.len());
        }
        let mut args = Vec::new();
        for (i, a) in c.args.positional.iter().enumerate() {
            match op.params.get(i) {
                Some(p) => args.push(self.check(&a.expr, &p.ty)),
                None => args.push(self.accept_any(&a.expr)),
            }
        }
        if !c.args.using.is_empty() {
            self.using_args(e.span, c, &qname, &[], &[]);
        }
        self.add_ref(cap, RefKind::CapabilityOp, name.span);
        Expr::new(
            ExprKind::CapCall {
                handle: Box::new(recv),
                capability: cap,
                op: index as u32,
                args,
            },
            if arity_ok { op.ret.clone() } else { Ty::Error },
            e.span,
        )
    }

    // ---- `?` (3.8) ---------------------------------------------------------------------

    fn propagate(&mut self, e: &ast::Expr, inner: &ast::Expr) -> Expr {
        let operand = self.infer(inner);
        let t = self.strip(&operand.ty);
        let (ok, err) = match &t {
            Ty::Adt(d, args) if *d == self.std.result && args.len() == 2 => {
                (args[0].clone(), args[1].clone())
            }
            t if is_poison(t) => return self.wrap_propagate(e.span, operand, Ty::Error),
            t if var_of(t).is_some() => {
                let (ok, err) = (self.fresh(inner.span), self.fresh(inner.span));
                let r = Ty::result(&self.std, ok.clone(), err.clone());
                self.vars.unify(t, &r);
                (ok, err)
            }
            other => {
                let actual = self.render(other);
                self.error(
                    diag(
                        codes::PROPAGATE_NOT_RESULT,
                        Some(e.span),
                        format!("`?` is defined on `Result` only, found `{actual}`"),
                    )
                    .with_data("expected", json!("Result<T, E>"))
                    .with_data("actual", json!(actual)),
                );
                return self.wrap_propagate(e.span, operand, Ty::Error);
            }
        };
        // Outside a function body (a test, a property, a contract) the
        // position itself is the error (resolve and contract stages).
        if self.owner == super::typer::Owner::Fn && !self.contract {
            let ret = self.strip(&self.ret.clone());
            match &ret {
                Ty::Adt(d, rargs) if *d == self.std.result && rargs.len() == 2 => {
                    if !self.vars.unify(&err, &rargs[1]) {
                        let (expected, actual) = self.render_pair(&rargs[1], &err);
                        self.error(
                            diag(
                                codes::PROPAGATE_ERROR_MISMATCH,
                                Some(e.span),
                                format!("`?` propagates a `{actual}` error, but this function's error type is `{expected}`; `?` performs no conversion"),
                            )
                            .with_data("expected", json!(expected))
                            .with_data("actual", json!(actual)),
                        );
                    } else if let Ty::Refined(_, rdef) = self.resolve(&self.ret) {
                        self.propagate_at_refinement(e.span, rdef, &ret);
                    }
                }
                t if is_poison(t) => {}
                _ => {
                    let actual = self.render(&self.ret.clone());
                    self.error(
                        diag(
                            codes::PROPAGATE_RETURN_MISMATCH,
                            Some(e.span),
                            format!("`?` returns the error from the enclosing function, which returns `{actual}`, not a `Result`"),
                        )
                        .with_data("expected", json!("Result<T, E>"))
                        .with_data("actual", json!(actual)),
                    );
                }
            }
        }
        self.wrap_propagate(e.span, operand, ok)
    }

    /// S73 [D-251]: `?` in a function whose declared return type `R` (the
    /// refinement `rdef`) strips to a `Result` (`root`) is
    /// `cannot_prove_refinement` at the `?`. The propagated `Err(x)` is an
    /// exit value, an implicit `return Err(x)` (5.2 step 5), which 3.4 rule
    /// 3 accepts at `R` only through the proof rule of 3.5, and a propagated
    /// value is never a constant expression (3.5 step 4). `expected` is `R`,
    /// `actual` its root type `strip(R)`.
    ///
    /// Readings. (1) D-251 applies "with `strip(R)` a `Result`, so the
    /// premise of the `e?` row holds", and that premise includes `E1 ≡ E2`:
    /// a `?` whose error type differs from the function's is
    /// `propagate_error_mismatch` alone. (2) "The success path is
    /// unaffected": the `?` keeps its type `T`, as it does after the row's
    /// other diagnostics, rather than taking the error type of S62, so the
    /// tail or `return` producing the function's value is still checked
    /// against `R` (a tail `Ok(r?)` is a second `cannot_prove_refinement`,
    /// of its own: it is no constant expression whatever the `?` does).
    fn propagate_at_refinement(&mut self, span: Span, rdef: DefId, root: &Ty) {
        let (expected, actual) = self.render_pair(&self.ret, root);
        let name = self.program.def(rdef).name.display();
        self.error(
            diag(
                codes::CANNOT_PROVE_REFINEMENT,
                Some(span),
                format!("`?` would return the propagated `Err` at the refinement `{expected}` (of `{actual}`) without proving it"),
            )
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual))
            .with_note(
                format!("a propagated `Err` leaves the function at `{expected}` without a proof; match on the operand and return `check<{name}>(...)` of the value instead"),
                None,
            ),
        );
    }

    fn wrap_propagate(&self, span: Span, operand: Expr, ty: Ty) -> Expr {
        Expr::new(ExprKind::Propagate(Box::new(operand)), ty, span)
    }

    // ---- literal forms (S19, S20) ------------------------------------------------------

    fn list(&mut self, e: &ast::Expr, l: &ast::ListLiteral, exp: Option<&Ty>) -> Expr {
        let elem_expected = match exp {
            Some(x) => match self.strip(x) {
                Ty::List(t) => Some(*t),
                // The error type unifies with `List<?α>`, binding `?α` to
                // itself (S62): every element accepts every type.
                t if is_poison(&t) => Some(Ty::Error),
                t if var_of(&t).is_some() => {
                    let el = self.fresh(e.span);
                    self.vars.unify(&t, &Ty::list(el.clone()));
                    Some(el)
                }
                _ => None,
            },
            None => None,
        };
        let mut items = Vec::new();
        let elem = match elem_expected {
            Some(t) => {
                for el in &l.elements {
                    items.push(self.check(&el.expr, &t));
                }
                t
            }
            None => match l.elements.split_first() {
                None => self.fresh(e.span),
                Some((first, rest)) => {
                    let first = self.infer(&first.expr);
                    let t = first.ty.clone();
                    items.push(first);
                    for el in rest {
                        items.push(self.check(&el.expr, &t));
                    }
                    t
                }
            },
        };
        let ty = Ty::list(elem);
        self.created(e.span, &ty);
        Expr::new(ExprKind::List(items), ty, e.span)
    }

    fn struct_lit(&mut self, e: &ast::Expr, s: &ast::StructLiteral, exp: Option<&Ty>) -> Expr {
        let res = self.tcx.cx.names.struct_paths.get(&s.path.span).cloned();
        let sd = match res {
            Some(Res::Struct(def)) => self.program.def(def).kind.as_struct().map(|sd| (def, sd)),
            _ => None,
        };
        let Some((def, sd)) = sd else {
            // Not a struct (`not_a_constructor`) or unresolved: the literal
            // is erroneous and its entries accept every type (S62).
            if let Some(b) = &s.entries.spread {
                self.accept_any(b);
            }
            for f in &s.entries.fields {
                self.accept_any(&f.value);
            }
            return self.error_expr(e.span);
        };
        let tys: Vec<Ty> = sd.generics.iter().map(|_| self.fresh(e.span)).collect();
        let ty = Ty::Adt(def, tys.clone());
        if let Some(x) = exp {
            let st = self.strip(x);
            self.vars.unify(&st, &ty);
        }
        let spread = s
            .entries
            .spread
            .as_ref()
            .map(|b| Box::new(self.check(b, &ty)));
        let mut fields: Vec<(u32, Expr)> = Vec::new();
        for f in &s.entries.fields {
            match sd.fields.iter().position(|x| x.name == f.name.text) {
                Some(i) => {
                    let ft = subst(&sd.fields[i].ty, &tys);
                    let v = self.check(&f.value, &ft);
                    if !fields.iter().any(|(j, _)| *j == i as u32) {
                        fields.push((i as u32, v));
                    }
                }
                None => {
                    // `unknown_field` (the resolution walk): no field type.
                    self.accept_any(&f.value);
                }
            }
        }
        self.created(e.span, &ty);
        // `missing_field` (the resolution walk, spanning the literal): the
        // literal carries a diagnostic, so it has the error type, and the
        // type arguments that only the missing fields could fix are not also
        // `cannot_infer_type` (S62; adding the fields is the one fix).
        let missing = s.entries.spread.is_none()
            && sd
                .fields
                .iter()
                .any(|f| !s.entries.fields.iter().any(|x| x.name.text == f.name));
        let ty = if missing {
            self.vars.bind_errors(&ty);
            Ty::Error
        } else {
            ty
        };
        let mut value = Expr::new(
            ExprKind::StructLit {
                def,
                invariant_proven: false,
                spread,
                fields,
            },
            ty,
            e.span,
        );
        if let Some(invariant) = sd.invariant.as_ref().filter(|_| value.ty != Ty::Error) {
            if let Some(predicate) = self.tcx.predicate(def).filter(|p| p.ok) {
                match crate::consteval::eval_constructor(
                    self.program,
                    &value,
                    &predicate.expr,
                    predicate.self_local,
                ) {
                    Ok(true) => {
                        if let ExprKind::StructLit {
                            invariant_proven, ..
                        } = &mut value.kind
                        {
                            *invariant_proven = true;
                        }
                    }
                    Ok(false) => self.emit(
                        Stage::Contract,
                        diag(
                            codes::CONSTRUCTOR_INVARIANT_FALSE,
                            Some(e.span),
                            format!(
                                "construction of `{}` violates its invariant",
                                self.program.display_path(def)
                            ),
                        )
                        .with_data("type", json!(self.program.display_path(def)))
                        .with_data("clause", json!(invariant.predicate.text)),
                    ),
                    Err(crate::consteval::Fail::Fault(code)) => self.emit(
                        Stage::Contract,
                        diag(
                            codes::CONSTANT_EVALUATION_FAULT,
                            Some(e.span),
                            format!("evaluating this constructor invariant raises `{code}`"),
                        )
                        .with_data("fault", json!(code))
                        .with_data("clause", json!(invariant.predicate.text)),
                    ),
                    Err(crate::consteval::Fail::Unsupported(_)) => {} // Unproved checks remain mandatory.
                }
            }
        }
        value
    }

    // ---- if (3.8, S19, S20, S24) ---------------------------------------------------------

    pub fn if_expr(&mut self, i: &ast::IfExpr, span: Span, exp: Option<&Ty>) -> Out {
        let cond = self.check(&i.condition, &Ty::Bool);
        let Some(eb) = &i.else_branch else {
            // `if c { A }`: `A ⇐ Unit`, type `Unit`.
            let unit = self.unit();
            let then = self.block(&i.then_block, Some(&unit));
            let ty = self.elseless(span, then.tail_failed, exp);
            return Out {
                e: Expr::new(
                    ExprKind::If {
                        cond: Box::new(cond),
                        then_block: then.block,
                        else_block: None,
                    },
                    ty,
                    span,
                ),
                done: true,
                div: false,
            };
        };
        let (then, els, div, ty) = match exp {
            Some(x) => {
                let then = self.block(&i.then_block, Some(x));
                let (els, ediv) = self.else_branch(eb, Some(x));
                let div = then.div && ediv;
                (then.block, els, div, x.clone())
            }
            None => {
                let then = self.block(&i.then_block, None);
                let (els, ediv) = if then.div {
                    self.else_branch(eb, None)
                } else {
                    let t = then.block.ty.clone();
                    self.else_branch(eb, Some(&t))
                };
                let div = then.div && ediv;
                let ty = if div {
                    Ty::Error
                } else if then.div {
                    els.ty.clone()
                } else {
                    then.block.ty.clone()
                };
                (then.block, els, div, ty)
            }
        };
        Out {
            e: Expr::new(
                ExprKind::If {
                    cond: Box::new(cond),
                    then_block: then,
                    else_block: Some(els),
                },
                ty,
                span,
            ),
            done: exp.is_some() || div,
            div,
        }
    }

    /// The type of an `if` without `else` (3.8): `Unit`; a branch tail that
    /// is not `Unit` was reported there (S68), and an expected type that
    /// rejects `Unit` is reported at the `if` with the note.
    fn elseless(&mut self, span: Span, tail_failed: bool, exp: Option<&Ty>) -> Ty {
        let unit = self.unit();
        let note = "an `if` without `else` has type `Unit`";
        let rejects = |t: &mut Self, x: &Ty| -> bool {
            let z = t.zonk(x);
            !(is_poison(&z) || t.vars.unify(&z, &unit))
        };
        if tail_failed {
            if let Some(x) = exp {
                if rejects(self, x) {
                    if let Some((_, d)) = self.diags.last_mut() {
                        d.notes.push(cpl_diag::Note::new(note));
                    }
                }
            }
            return Ty::Error;
        }
        if let Some(x) = exp {
            if rejects(self, x) {
                let expected = self.render(x);
                self.error(
                    diag(
                        codes::TYPE_MISMATCH,
                        Some(span),
                        format!("expected `{expected}`, found `Unit`"),
                    )
                    .with_data("expected", json!(expected))
                    .with_data("actual", json!("Unit"))
                    .with_note(note, None),
                );
                return Ty::Error;
            }
        }
        unit
    }

    /// An `else` branch: a block, or a further `if` (an `else if` chain,
    /// desugared into a block whose tail is the nested `if`, toolchain 3.3).
    fn else_branch(&mut self, eb: &ast::ElseBranch, exp: Option<&Ty>) -> (Block, bool) {
        match eb {
            ast::ElseBranch::Block(b) => {
                let o = self.block(b, exp);
                (o.block, o.div)
            }
            ast::ElseBranch::If(nested) => {
                let out = self.if_expr(nested, nested.span, exp);
                let ty = out.e.ty.clone();
                let span = out.e.span;
                (
                    Block {
                        stmts: Vec::new(),
                        tail: Some(Box::new(out.e)),
                        ty,
                        span,
                    },
                    out.div,
                )
            }
        }
    }

    // ---- generators (S14, 3.14, T18) -----------------------------------------------------

    /// A property `for` range: a `Gen<T>` expression lowered to a
    /// `GenExpr`, and the binder's type `T`.
    pub fn for_range(&mut self, range: &ast::Expr) -> (GenExpr, Ty) {
        self.gen_ok = true;
        let x = self.infer(range);
        let t = self.zonk(&x.ty);
        match (&x.kind, self.resolve(&t)) {
            (ExprKind::Gen(g), Ty::Gen(elem)) => (g.clone(), *elem),
            (_, t) if is_poison(&t) || matches!(t, Ty::Gen(_)) => (GenExpr::Int, Ty::Error),
            (_, other) => {
                let actual = self.render(&other);
                self.error(
                    diag(
                        codes::FOR_RANGE_NOT_GENERATOR,
                        Some(range.span),
                        format!(
                            "a `for` range must be a generator such as `gen.int`, found `{actual}`"
                        ),
                    )
                    .with_data("actual", json!(actual)),
                );
                (GenExpr::Int, Ty::Error)
            }
        }
    }

    // ---- diagnostics ---------------------------------------------------------------------

    pub fn arity_mismatch(&mut self, span: Span, callee: &str, expected: usize, actual: usize) {
        self.error(
            diag(
                codes::ARITY_MISMATCH,
                Some(span),
                format!(
                    "`{callee}` takes {expected} argument{}, found {actual}",
                    plural(expected)
                ),
            )
            .with_data("callee", json!(callee))
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual)),
        );
    }

    fn count_mismatch(&mut self, span: Span, callee: &str, expected: usize, actual: usize) {
        self.error(
            diag(
                codes::TYPE_ARGUMENT_COUNT_MISMATCH,
                Some(span),
                format!(
                    "`{callee}` takes {expected} type argument{}, found {actual}",
                    plural(expected)
                ),
            )
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual)),
        );
    }

    fn not_callable(&mut self, span: Span, name: &str, message: &str, note: Option<String>) {
        let mut d = diag(codes::NOT_CALLABLE, Some(span), message).with_data("name", json!(name));
        if let Some(n) = note {
            d = d.with_note(n, None);
        }
        self.error(d);
    }

    fn unknown_member(&mut self, name: &ast::Name, t: &Ty) {
        let ty = self.render(t);
        self.error(
            diag(
                codes::UNKNOWN_MEMBER,
                Some(name.span),
                format!("`{ty}` has no member `{}`", name.text),
            )
            .with_data("name", json!(name.text))
            .with_data("type", json!(ty)),
        );
    }

    fn method_not_called(&mut self, name: &ast::Name, t: &Ty) {
        let ty = self.render(t);
        self.error(
            diag(
                codes::METHOD_NOT_CALLED,
                Some(name.span),
                format!("`{}` is a method; write `x.{}(...)`", name.text, name.text),
            )
            .with_data("name", json!(name.text))
            .with_data("type", json!(ty)),
        );
    }
}

/// A member stub instantiated at a receiver (`Typer::instantiate_member`).
struct MemberInst {
    /// The instantiation of the stub's type parameters.
    tys: Vec<Ty>,
    /// The result type `R[subst]`.
    ret: Ty,
    /// The parameter types after the receiver, `[subst]`.
    params: Vec<Ty>,
    /// The explicit type arguments had the right count (3.7 step 1).
    count_ok: bool,
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn literal(span: Span, l: &ast::Literal) -> Expr {
    match &l.kind {
        ast::LiteralKind::Int(digits) => Expr::new(
            ExprKind::Lit(Lit::Int(digits.parse::<BigInt>().unwrap_or_default())),
            Ty::Int,
            span,
        ),
        ast::LiteralKind::Text(t) => Expr::new(ExprKind::Lit(Lit::Text(t.clone())), Ty::Text, span),
        ast::LiteralKind::Bool(b) => Expr::new(ExprKind::Lit(Lit::Bool(*b)), Ty::Bool, span),
    }
}

/// The `GenExpr` of a `std.gen` builtin with typed arguments (T18).
fn gen_node(id: &str, args: &[Expr]) -> Option<GenExpr> {
    let inner = |i: usize| match args.get(i).map(|a| &a.kind) {
        Some(ExprKind::Gen(g)) => Some(Box::new(g.clone())),
        _ => None,
    };
    Some(match id {
        "gen_int" => GenExpr::Int,
        "gen_positive_int" => GenExpr::PositiveInt,
        "gen_bool" => GenExpr::Bool,
        "gen_text" => GenExpr::Text,
        "gen_list" => GenExpr::List(inner(0)?),
        "gen_option" => GenExpr::Option(inner(0)?),
        "gen_range" => GenExpr::Range(
            Box::new(args.first()?.clone()),
            Box::new(args.get(1)?.clone()),
        ),
        _ => return None,
    })
}
